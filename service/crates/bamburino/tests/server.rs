// SPDX-License-Identifier: AGPL-3.0-or-later
//! MCP server over real HTTP on an ephemeral loopback port, MockDriver behind.

use std::sync::Arc;
use std::time::Duration;

use bamburino::config::Config;
use bamburino::server::{ServerError, serve, serve_on};
use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::tools::{ToolConfig, Tools};
use serde_json::{Value, json};
use tokio::sync::oneshot;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Running {
    port: u16,
    http: reqwest::Client,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), ServerError>>,
}

fn make_tools(mock: &Arc<MockDriver>, studio_running: bool, poll: Duration) -> Arc<Tools> {
    Arc::new(Tools::new(
        mock.clone(),
        Arc::new(FixedProcessProbe(studio_running)),
        ToolConfig {
            allow_version_mismatch: false,
            poll_every: poll,
            idle_after: Duration::from_secs(60),
        },
        std::env::temp_dir().join("bamburino-server-tests-unused"),
    ))
}

async fn start_with(mock: Arc<MockDriver>, studio_running: bool, poll: Duration) -> Running {
    let tools = make_tools(&mock, studio_running, poll);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(serve_on(listener, TOKEN.to_owned(), tools, async move {
        let _ = rx.await;
    }));
    Running {
        port,
        http: reqwest::Client::new(),
        stop: Some(tx),
        task,
    }
}

async fn start() -> Running {
    start_with(Arc::new(MockDriver::new()), true, Duration::from_secs(60)).await
}

impl Running {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        self.http
            .post(self.url("/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(body)
    }

    fn authed(&self, body: &Value) -> reqwest::RequestBuilder {
        self.post(body)
            .header("Authorization", format!("Bearer {TOKEN}"))
    }

    /// One JSON-RPC request; the reply may be plain JSON or one SSE event.
    async fn rpc(&self, id: u64, method: &str, params: Value) -> Value {
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let resp = self.authed(&body).send().await.unwrap();
        assert_eq!(resp.status(), 200, "{method}");
        let text = resp.text().await.unwrap();
        parse_reply(&text)
    }

    async fn call(&self, name: &str, args: Value) -> Value {
        self.rpc(2, "tools/call", json!({"name": name, "arguments": args}))
            .await
    }

    async fn shutdown(mut self) {
        let _ = self.stop.take().unwrap().send(());
        tokio::time::timeout(Duration::from_secs(15), self.task)
            .await
            .expect("server stops")
            .unwrap()
            .unwrap();
    }
}

fn parse_reply(text: &str) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return v;
    }
    text.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .find(|v| v.get("result").is_some() || v.get("error").is_some())
        .unwrap_or_else(|| panic!("no JSON-RPC reply in: {text}"))
}

fn init_params() -> Value {
    json!({"protocolVersion": "2025-03-26", "capabilities": {},
           "clientInfo": {"name": "test", "version": "1"}})
}

#[tokio::test]
async fn lists_exactly_twelve_tools_with_exact_names() {
    let s = start().await;
    let init = s.rpc(1, "initialize", init_params()).await;
    assert!(init["result"]["serverInfo"].is_object(), "{init}");
    let list = s.rpc(2, "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, Tools::NAMES);
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert!(!t["description"].as_str().unwrap().is_empty());
    }
    let set = tools
        .iter()
        .find(|t| t["name"] == "studio_set_settings")
        .unwrap();
    assert!(set["inputSchema"]["properties"]["expected_revision"].is_object());
    assert!(
        set["description"]
            .as_str()
            .unwrap()
            .contains("expected_revision")
    );
    s.shutdown().await;
}

#[tokio::test]
async fn missing_token_is_401() {
    let s = start().await;
    let r = s
        .post(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    s.shutdown().await;
}

#[tokio::test]
async fn wrong_token_is_401() {
    let s = start().await;
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    for bad in [
        "Bearer nope".to_owned(),
        format!("Bearer {TOKEN}x"),
        format!("Basic {TOKEN}"),
        TOKEN.to_owned(),
    ] {
        let r = s
            .post(&body)
            .header("Authorization", bad)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
    }
    s.shutdown().await;
}

#[tokio::test]
async fn foreign_host_is_403() {
    let s = start().await;
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    let r = s
        .authed(&body)
        .header("Host", "evil.example:27190")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // Right name, wrong port.
    let r = s
        .authed(&body)
        .header("Host", "localhost:1")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    s.shutdown().await;
}

#[tokio::test]
async fn foreign_origin_is_403_and_localhost_is_allowed() {
    let s = start().await;
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    let r = s
        .authed(&body)
        .header("Origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    for ok in [
        format!("http://127.0.0.1:{}", s.port),
        format!("http://localhost:{}", s.port),
    ] {
        let r = s.authed(&body).header("Origin", ok).send().await.unwrap();
        assert_eq!(r.status(), 200);
    }
    let r = s
        .authed(&body)
        .header("Host", format!("localhost:{}", s.port))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    s.shutdown().await;
}

#[tokio::test]
async fn healthz_needs_no_token_and_leaks_nothing() {
    let s = start().await;
    let r = s.http.get(s.url("/healthz")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "ok");
    let r = s
        .http
        .get(s.url("/healthz"))
        .header("Host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    s.shutdown().await;
}

#[tokio::test]
async fn capabilities_and_state_calls_work() {
    let s = start().await;
    let r = s.call("bamburino_capabilities", json!({})).await;
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(
        r["result"]["structuredContent"]["driver"]["status"],
        "ready"
    );
    let r = s.call("studio_get_state", json!({})).await;
    assert!(r["result"]["structuredContent"]["revision"].is_u64(), "{r}");
    s.shutdown().await;
}

#[tokio::test]
async fn set_settings_demo_batch_is_reread() {
    let s = start().await;
    let state = s.call("studio_get_state", json!({})).await;
    let rev = state["result"]["structuredContent"]["revision"]
        .as_u64()
        .unwrap();
    let changes = json!([
        {"key": "sparse_infill_density", "value": 35},
        {"key": "sparse_infill_pattern", "value": "gyroid"},
        {"key": "wall_loops", "value": 4},
        {"key": "layer_height", "value": 0.2},
    ]);
    let r = s
        .call(
            "studio_set_settings",
            json!({"changes": changes, "expected_revision": rev}),
        )
        .await;
    assert_eq!(r["result"]["isError"], false, "{r}");
    let got = s
        .call("studio_get_settings", json!({"scope": "project"}))
        .await;
    let settings = got["result"]["structuredContent"].to_string();
    for needle in ["35%", "gyroid"] {
        assert!(settings.contains(needle), "{needle} missing in {settings}");
    }
    s.shutdown().await;
}

#[tokio::test]
async fn unknown_key_is_structured_error_with_candidates() {
    let s = start().await;
    let state = s.call("studio_get_state", json!({})).await;
    let rev = state["result"]["structuredContent"]["revision"]
        .as_u64()
        .unwrap();
    let r = s
        .call(
            "studio_set_settings",
            json!({"changes": [{"key": "sparse_infill_densty", "value": 35}],
                   "expected_revision": rev}),
        )
        .await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    let err = &r["result"]["structuredContent"]["error"];
    assert_eq!(err["code"], "UNKNOWN_KEY", "{r}");
    assert_eq!(err["http_status"], 422);
    assert!(
        err["details"].to_string().contains("sparse_infill_density"),
        "{err}"
    );
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(text).unwrap(),
        r["result"]["structuredContent"]
    );
    s.shutdown().await;
}

#[tokio::test]
async fn tool_error_has_structured_content_when_studio_closed() {
    let mock = Arc::new(MockDriver::new());
    mock.set_unreachable(true);
    let s = start_with(mock, false, Duration::from_secs(60)).await;
    let r = s.call("studio_get_state", json!({})).await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert_eq!(
        r["result"]["structuredContent"]["error"]["code"],
        "STUDIO_NOT_RUNNING"
    );
    s.shutdown().await;
}

#[tokio::test]
async fn bad_arguments_are_invalid_argument_tool_errors() {
    let s = start().await;
    let r = s
        .call(
            "studio_set_settings",
            json!({"changes": "no", "expected_revision": 1}),
        )
        .await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert_eq!(
        r["result"]["structuredContent"]["error"]["code"],
        "INVALID_ARGUMENT"
    );
    let r = s.call("no_such_tool", json!({})).await;
    assert!(r["error"].is_object(), "{r}");
    s.shutdown().await;
}

#[tokio::test]
async fn catalog_resource_is_listed_and_readable() {
    let s = start().await;
    let list = s.rpc(1, "resources/list", json!({})).await;
    let uris: Vec<&str> = list["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert!(uris.contains(&"bambustudio://catalog"), "{uris:?}");
    for uri in ["bambustudio://catalog", "bamburino://catalog"] {
        let r = s.rpc(2, "resources/read", json!({"uri": uri})).await;
        let c = &r["result"]["contents"][0];
        assert_eq!(c["mimeType"], "application/json", "{r}");
        let text = c["text"].as_str().unwrap();
        assert_eq!(
            text.as_bytes(),
            bamburino_core::catalog::Catalog::raw_json()
        );
    }
    let r = s
        .rpc(3, "resources/read", json!({"uri": "bambustudio://nope"}))
        .await;
    assert!(r["error"].is_object(), "{r}");
    s.shutdown().await;
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A non-loopback IPv4 address of this host, if there is one.
fn local_lan_ip() -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (ip.is_ipv4() && !ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

async fn refused(addr: std::net::SocketAddr) -> bool {
    !matches!(
        tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(addr)).await,
        Ok(Ok(_))
    )
}

#[tokio::test]
async fn serve_binds_only_127_0_0_1() {
    let mock = Arc::new(MockDriver::new());
    let tools = make_tools(&mock, true, Duration::from_secs(60));
    let port = free_port();
    let cfg = Config {
        listen_port: port,
        ..Config::default()
    };
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        serve(&cfg, TOKEN.to_owned(), tools, async move {
            let _ = rx.await;
        })
        .await
    });
    let loopback = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let http = reqwest::Client::new();
    let mut up = false;
    for _ in 0..100 {
        if let Ok(r) = http.get(format!("http://{loopback}/healthz")).send().await {
            assert_eq!(r.text().await.unwrap(), "ok");
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(up, "serve never came up on {loopback}");
    // Not reachable via IPv6 loopback nor via a LAN address of this host.
    assert!(refused(std::net::SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], port))).await);
    match local_lan_ip() {
        Some(ip) => assert!(refused(std::net::SocketAddr::new(ip, port)).await, "{ip}"),
        None => eprintln!("serve_binds_only_127_0_0_1: no non-loopback IPv4, LAN check skipped"),
    }
    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn serve_on_refuses_other_loopback_addresses() {
    let mock = Arc::new(MockDriver::new());
    let tools = make_tools(&mock, true, Duration::from_secs(60));
    if let Ok(l) = tokio::net::TcpListener::bind("127.0.0.2:0").await {
        let r = serve_on(l, TOKEN.to_owned(), tools.clone(), std::future::pending()).await;
        assert!(matches!(r, Err(ServerError::NotLoopback(_))));
    }
    if let Ok(l) = tokio::net::TcpListener::bind("[::1]:0").await {
        let r = serve_on(l, TOKEN.to_owned(), tools, std::future::pending()).await;
        assert!(matches!(r, Err(ServerError::NotLoopback(_))));
    }
}

#[tokio::test]
async fn shutdown_finishes_in_flight_request() {
    let mock = Arc::new(MockDriver::new());
    let s = start_with(mock.clone(), true, Duration::from_secs(60)).await;
    let before = mock.calls().len();
    mock.delay_next(Duration::from_millis(600));
    let url = s.url("/mcp");
    let http = s.http.clone();
    let inflight = tokio::spawn(async move {
        http.post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {TOKEN}"))
            .json(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call",
                "params":{"name":"studio_list_models","arguments":{}}}))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    });
    // The first driver call of the request is recorded, then delayed: the
    // request is demonstrably in flight.
    for _ in 0..200 {
        if mock.calls().len() > before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        mock.calls().len() > before,
        "request never reached the driver"
    );
    let port = s.port;
    s.shutdown().await;
    let reply = parse_reply(&inflight.await.unwrap());
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn shutdown_stops_poller() {
    let mock = Arc::new(MockDriver::new());
    let s = start_with(mock.clone(), true, Duration::from_millis(40)).await;
    let _ = s.call("studio_get_state", json!({})).await; // client active
    tokio::time::sleep(Duration::from_millis(200)).await;
    let polling = mock.calls().len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(mock.calls().len() > polling, "poller never polled");
    s.shutdown().await;
    let before = mock.calls().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mock.calls().len(), before);
}

#[tokio::test]
async fn shutdown_is_bounded_when_the_poller_is_stuck_in_a_driver_read() {
    let mock = Arc::new(MockDriver::new());
    let s = start_with(mock.clone(), true, Duration::from_millis(40)).await;
    let _ = s.call("studio_get_state", json!({})).await; // client active
    let before = mock.calls().len();
    mock.delay_next(Duration::from_secs(60)); // the next driver read is a poll
    for _ in 0..200 {
        if mock.calls().len() > before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(mock.calls().len() > before, "poller never read");
    let t = std::time::Instant::now();
    s.shutdown().await;
    let took = t.elapsed();
    assert!(took >= Duration::from_secs(1), "{took:?}"); // waited for the poller
    assert!(
        took < bamburino::server::SHUTDOWN_GRACE + bamburino::server::POLLER_GRACE,
        "{took:?}"
    );
}

#[tokio::test]
async fn no_arg_tools_reject_arguments() {
    let s = start().await;
    let r = s.call("studio_get_state", json!({"x": 1})).await;
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert_eq!(
        r["result"]["structuredContent"]["error"]["code"],
        "INVALID_ARGUMENT"
    );
    s.shutdown().await;
}

#[tokio::test]
async fn input_schemas_match_the_args_types() {
    use bamburino_core::tools::{
        GetSettingsArgs, JobArgs, SearchArgs, SetSettingsArgs, SliceArgs, SummaryArgs,
        SupportsArgs, TransformArgs,
    };
    fn schema<T: schemars::JsonSchema>() -> Value {
        serde_json::to_value(schemars::schema_for!(T)).unwrap()
    }
    let s = start().await;
    let list = s.rpc(1, "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    let expected: [(&str, Option<Value>); 12] = [
        ("bamburino_capabilities", None),
        ("studio_get_state", None),
        ("studio_get_settings", Some(schema::<GetSettingsArgs>())),
        ("studio_set_settings", Some(schema::<SetSettingsArgs>())),
        ("studio_list_models", None),
        ("studio_transform_model", Some(schema::<TransformArgs>())),
        ("studio_set_supports", Some(schema::<SupportsArgs>())),
        ("studio_slice", Some(schema::<SliceArgs>())),
        ("studio_slice_status", Some(schema::<JobArgs>())),
        ("studio_cancel_slice", Some(schema::<JobArgs>())),
        ("studio_get_slice_summary", Some(schema::<SummaryArgs>())),
        ("catalog_search", Some(schema::<SearchArgs>())),
    ];
    for (name, want) in expected {
        let t = tools.iter().find(|t| t["name"] == name).unwrap();
        let got = &t["inputSchema"];
        match want {
            Some(mut w) => {
                // The `$schema` and root `title` markers are dropped; everything else is equal.
                let mut g = got.clone();
                for k in ["$schema", "title"] {
                    w.as_object_mut().unwrap().remove(k);
                    g.as_object_mut().unwrap().remove(k);
                }
                assert_eq!(g, w, "{name}");
            }
            None => {
                assert_eq!(got["type"], "object", "{name}");
                assert!(
                    got.get("properties")
                        .is_none_or(|p| p.as_object().is_some_and(|o| o.is_empty())),
                    "{name}: {got}"
                );
            }
        }
    }
    s.shutdown().await;
}

#[tokio::test]
async fn host_and_origin_regressions() {
    let s = start().await;
    let body = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    let port = s.port;
    for host in [
        format!("127.0.0.1.evil:{port}"),
        "127.0.0.1".to_owned(),
        "localhost".to_owned(),
        format!("localhost.evil:{port}"),
        format!("127.0.0.1:{port}.evil"),
    ] {
        let r = s.authed(&body).header("Host", &host).send().await.unwrap();
        assert_eq!(r.status(), 403, "Host {host}");
    }
    for origin in [
        "null".to_owned(),
        format!("http://127.0.0.1.evil:{port}"),
        "http://127.0.0.1".to_owned(),
        format!("https://127.0.0.1:{port}"),
        String::new(),
    ] {
        let r = s
            .authed(&body)
            .header("Origin", &origin)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "Origin {origin:?}");
    }
    s.shutdown().await;
}
