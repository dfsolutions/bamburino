// SPDX-License-Identifier: AGPL-3.0-or-later
//! The MCP server: Streamable HTTP on loopback (rmcp + axum), bearer
//! authentication, Host/Origin checks, the 12 tools and the catalog resource.
//!
//! Public API: [`serve`] (binds `127.0.0.1:<listen_port>`), [`serve_on`]
//! (an already bound loopback listener, for tests) and [`router`].
//!
//! The client token is never logged and never part of any error or `Debug`
//! output: the types that hold it do not implement `Debug`.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bamburino_core::catalog::Catalog;
use bamburino_core::errors::{ToolError, ToolErrorCode};
use bamburino_core::tools::{
    GetSettingsArgs, JobArgs, SearchArgs, SetSettingsArgs, SliceArgs, SummaryArgs, SupportsArgs,
    Tools, TransformArgs,
};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use subtle::ConstantTimeEq;

/// Time in-flight requests get to finish after the shutdown signal.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Resource URI of the catalog (the design name).
pub const CATALOG_URI: &str = "bambustudio://catalog";
/// Same content under the service-named URI.
pub const CATALOG_URI_ALIAS: &str = "bamburino://catalog";

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("cannot bind {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error("the listener is not on a loopback address: {0}")]
    NotLoopback(SocketAddr),
    #[error("server error: {0}")]
    Serve(std::io::Error),
}

// ---------------------------------------------------------------- handler

/// Tools without arguments.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Clone)]
struct Mcp {
    tools: Arc<Tools>,
}

fn tool_def<T: JsonSchema + 'static>(
    name: &'static str,
    desc: &'static str,
    read_only: bool,
) -> Tool {
    Tool::new(name, desc, serde_json::Map::new())
        .with_input_schema::<T>()
        .with_annotations(ToolAnnotations::new().read_only(read_only))
}

fn tool_defs() -> Vec<Tool> {
    let defs = vec![
        tool_def::<NoArgs>(
            "bamburino_capabilities",
            "Reads the Bamburino, catalog and Bambu Studio driver status; works with Studio closed.",
            true,
        ),
        tool_def::<NoArgs>(
            "studio_get_state",
            "Reads the open project (plates, models, presets, slice state) and the `revision` that every mutation needs as `expected_revision`.",
            true,
        ),
        tool_def::<GetSettingsArgs>(
            "studio_get_settings",
            "Reads the effective settings of the project, a plate or an object.",
            true,
        ),
        tool_def::<SetSettingsArgs>(
            "studio_set_settings",
            "Modifies settings by key (validated against the catalog, dry_run available); requires `expected_revision` from studio_get_state.",
            false,
        ),
        tool_def::<NoArgs>(
            "studio_list_models",
            "Reads the objects and instances of the project.",
            true,
        ),
        tool_def::<TransformArgs>(
            "studio_transform_model",
            "Modifies an object instance by rotation, translation or scale; requires `expected_revision` from studio_get_state.",
            false,
        ),
        tool_def::<SupportsArgs>(
            "studio_set_supports",
            "Modifies the support settings of the project, a plate or an object; requires `expected_revision` from studio_get_state.",
            false,
        ),
        tool_def::<SliceArgs>(
            "studio_slice",
            "Starts slicing a plate in Studio (does not modify the project) and returns a job id.",
            false,
        ),
        tool_def::<JobArgs>(
            "studio_slice_status",
            "Reads the progress of a slice job started with studio_slice.",
            true,
        ),
        tool_def::<JobArgs>(
            "studio_cancel_slice",
            "Cancels a running slice job started with studio_slice.",
            false,
        ),
        tool_def::<SummaryArgs>(
            "studio_get_slice_summary",
            "Reads the summary (time, filament, layers) of the current slice result of a plate.",
            true,
        ),
        tool_def::<SearchArgs>(
            "catalog_search",
            "Reads the settings catalog: finds setting keys by words of key, label or description.",
            true,
        ),
    ];
    debug_assert!(
        defs.iter()
            .map(|t| t.name.as_ref())
            .eq(Tools::NAMES.iter().copied())
    );
    defs
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| {
        ToolError::new(
            ToolErrorCode::InvalidArgument,
            format!("invalid arguments: {e}"),
        )
    })
}

/// `None`: no such tool.
async fn dispatch(tools: &Tools, name: &str, args: Value) -> Option<Result<Value, ToolError>> {
    Some(match name {
        "bamburino_capabilities" => Ok(tools.bamburino_capabilities().await),
        "studio_get_state" => tools.studio_get_state().await,
        "studio_get_settings" => match parse(args) {
            Ok(a) => tools.studio_get_settings(a).await,
            Err(e) => Err(e),
        },
        "studio_set_settings" => match parse(args) {
            Ok(a) => tools.studio_set_settings(a).await,
            Err(e) => Err(e),
        },
        "studio_list_models" => tools.studio_list_models().await,
        "studio_transform_model" => match parse(args) {
            Ok(a) => tools.studio_transform_model(a).await,
            Err(e) => Err(e),
        },
        "studio_set_supports" => match parse(args) {
            Ok(a) => tools.studio_set_supports(a).await,
            Err(e) => Err(e),
        },
        "studio_slice" => match parse(args) {
            Ok(a) => tools.studio_slice(a).await,
            Err(e) => Err(e),
        },
        "studio_slice_status" => match parse(args) {
            Ok(a) => tools.studio_slice_status(a).await,
            Err(e) => Err(e),
        },
        "studio_cancel_slice" => match parse(args) {
            Ok(a) => tools.studio_cancel_slice(a).await,
            Err(e) => Err(e),
        },
        "studio_get_slice_summary" => match parse(args) {
            Ok(a) => tools.studio_get_slice_summary(a).await,
            Err(e) => Err(e),
        },
        "catalog_search" => match parse(args) {
            Ok(a) => Ok(tools.catalog_search(a).await),
            Err(e) => Err(e),
        },
        _ => return None,
    })
}

/// `structuredContent` must be an object.
fn as_object(v: Value) -> Value {
    if v.is_object() {
        v
    } else {
        serde_json::json!({ "result": v })
    }
}

impl ServerHandler for Mcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(
            "bamburino",
            bamburino_core::build_info::BAMBURINO_VERSION,
        ))
        .with_instructions(
            "Controls a running Bambu Studio. Read studio_get_state first: every mutation \
             needs its `revision` as `expected_revision`. Find setting keys with catalog_search.",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tool_defs()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = Value::Object(request.arguments.unwrap_or_default());
        match dispatch(&self.tools, &request.name, args).await {
            None => Err(ErrorData::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            )),
            Some(Ok(v)) => Ok(CallToolResult::structured(as_object(v)).into()),
            Some(Err(e)) => Ok(CallToolResult::structured_error(e.to_json()).into()),
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let size = Catalog::raw_json().len() as u64;
        let res = |uri: &str| {
            let mut r = Resource::new(uri, "catalog").with_mime_type("application/json");
            r.description = Some(
                "Bambu Studio settings catalog: keys, types, enums, ranges and scopes.".to_owned(),
            );
            r.with_size(size)
        };
        Ok(ListResourcesResult::with_all_items(vec![
            res(CATALOG_URI),
            res(CATALOG_URI_ALIAS),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        if request.uri != CATALOG_URI && request.uri != CATALOG_URI_ALIAS {
            return Err(ErrorData::resource_not_found("unknown resource", None));
        }
        let text = std::str::from_utf8(Catalog::raw_json())
            .map_err(|_| ErrorData::internal_error("catalog is not UTF-8", None))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, request.uri).with_mime_type("application/json"),
        ])
        .into())
    }
}

// ------------------------------------------------------------- middleware

/// Holds the token: deliberately not `Debug`.
#[derive(Clone)]
struct Guard {
    hosts: Arc<[String; 2]>,
    origins: Arc<[String; 2]>,
    token: Arc<Vec<u8>>,
    tools: Arc<Tools>,
}

fn status(code: StatusCode) -> Response {
    code.into_response()
}

fn header_str(h: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// Host and Origin allow-lists (DNS rebinding and browser cross-origin).
async fn host_origin(State(g): State<Guard>, req: Request, next: Next) -> Response {
    let h = req.headers();
    let host_ok = header_str(h, header::HOST)
        .is_some_and(|v| g.hosts.iter().any(|a| a.eq_ignore_ascii_case(v)));
    if !host_ok {
        tracing::warn!("request rejected: Host not allowed");
        return status(StatusCode::FORBIDDEN);
    }
    if let Some(origin) = h.get(header::ORIGIN) {
        let ok = origin
            .to_str()
            .ok()
            .is_some_and(|v| g.origins.iter().any(|a| a.eq_ignore_ascii_case(v)));
        if !ok {
            tracing::warn!("request rejected: Origin not allowed");
            return status(StatusCode::FORBIDDEN);
        }
    }
    next.run(req).await
}

/// Bearer token, constant-time comparison; marks client activity.
async fn bearer(State(g): State<Guard>, req: Request, next: Next) -> Response {
    let presented = header_str(req.headers(), header::AUTHORIZATION).and_then(|v| {
        let (scheme, rest) = v.split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then_some(rest.trim())
    });
    let ok = presented.is_some_and(|t| bool::from(t.as_bytes().ct_eq(g.token.as_slice())));
    if !ok {
        tracing::warn!("request rejected: missing or invalid bearer token");
        let mut r = status(StatusCode::UNAUTHORIZED);
        r.headers_mut()
            .insert(header::WWW_AUTHENTICATE, "Bearer".parse().expect("static"));
        return r;
    }
    g.tools.revision_state().touch_client();
    next.run(req).await
}

// ----------------------------------------------------------------- server

/// The router and a closure that cancels the rmcp service (ends its sessions).
fn build(port: u16, client_token: String, tools: Arc<Tools>) -> (Router, impl Fn() + Send + Sync) {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true);
    let ct = config.cancellation_token.clone();
    let handler_tools = tools.clone();
    let service: StreamableHttpService<Mcp, LocalSessionManager> = StreamableHttpService::new(
        move || {
            Ok(Mcp {
                tools: handler_tools.clone(),
            })
        },
        Arc::default(),
        config,
    );
    let guard = Guard {
        hosts: Arc::new([format!("127.0.0.1:{port}"), format!("localhost:{port}")]),
        origins: Arc::new([
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ]),
        token: Arc::new(client_token.into_bytes()),
        tools,
    };
    let mcp = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(guard.clone(), bearer));
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(mcp)
        .layer(middleware::from_fn_with_state(guard, host_origin));
    (app, move || ct.cancel())
}

/// Router for `port` (the port the clients connect to).
pub fn router(port: u16, client_token: String, tools: Arc<Tools>) -> Router {
    build(port, client_token, tools).0
}

/// Binds `127.0.0.1:<cfg.listen_port>` and serves until `shutdown` completes.
pub async fn serve(
    cfg: &crate::config::Config,
    client_token: String,
    tools: Arc<Tools>,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), ServerError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], cfg.listen_port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|source| ServerError::Bind { addr, source })?;
    serve_on(listener, client_token, tools, shutdown).await
}

/// Serves on an already bound loopback listener. On `shutdown`: stops
/// accepting, lets in-flight requests finish for at most [`SHUTDOWN_GRACE`],
/// stops the revision poller and ends the MCP service.
pub async fn serve_on(
    listener: tokio::net::TcpListener,
    client_token: String,
    tools: Arc<Tools>,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), ServerError> {
    let addr = listener.local_addr().map_err(ServerError::Serve)?;
    if !addr.ip().is_loopback() {
        return Err(ServerError::NotLoopback(addr));
    }
    let (app, cancel_mcp) = build(addr.port(), client_token, tools.clone());
    let poller = tools.start_poller();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let mut srv = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stop_rx.await;
            })
            .await
    });
    tracing::info!(%addr, "MCP server listening");

    tokio::pin!(shutdown);
    let early = tokio::select! {
        () = &mut shutdown => None,
        r = &mut srv => Some(r),
    };
    tools.revision_state().stop_poller();
    let result = match early {
        Some(r) => flatten(r),
        None => {
            tracing::info!("MCP server shutting down");
            let _ = stop_tx.send(());
            match tokio::time::timeout(SHUTDOWN_GRACE, &mut srv).await {
                Ok(r) => flatten(r),
                Err(_) => {
                    tracing::warn!("in-flight requests did not finish in time; dropped");
                    srv.abort();
                    Ok(())
                }
            }
        }
    };
    cancel_mcp();
    let _ = poller.await;
    result
}

fn flatten(r: Result<std::io::Result<()>, tokio::task::JoinError>) -> Result<(), ServerError> {
    match r {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(ServerError::Serve(e)),
        Err(e) => Err(ServerError::Serve(std::io::Error::other(e.to_string()))),
    }
}
