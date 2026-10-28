// SPDX-License-Identifier: AGPL-3.0-or-later
//! MCP server over real HTTP on an ephemeral loopback port, MockDriver behind.

use std::sync::Arc;
use std::time::Duration;

use bamburino::server::{ServerError, serve_on};
use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::tools::{ToolConfig, Tools};
use serde_json::{Value, json};
use tokio::sync::oneshot;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Running {
    port: u16,
    http: reqwest::Client,
    mock: Arc<MockDriver>,
    tools: Arc<Tools>,
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
            token_present: true,
        },
        std::env::temp_dir().join("bamburino-server-tests-unused"),
    ))
}

async fn start_with(mock: Arc<MockDriver>, studio_running: bool, poll: Duration) -> Running {
    let tools = make_tools(&mock, studio_running, poll);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(serve_on(
        listener,
        TOKEN.to_owned(),
        tools.clone(),
        async move {
            let _ = rx.await;
        },
    ));
    Running {
        port,
        http: reqwest::Client::new(),
        mock,
        tools,
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

#[tokio::test]
async fn binds_only_loopback() {
    let s = start().await;
    // Any non-loopback IPv4 address of this machine must refuse the port.
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let local = sock
        .connect("192.0.2.1:9")
        .ok()
        .and_then(|()| sock.local_addr().ok())
        .map(|a| a.ip());
    match local {
        Some(ip) if !ip.is_loopback() && !ip.is_unspecified() => {
            let r = tokio::time::timeout(
                Duration::from_secs(3),
                tokio::net::TcpStream::connect((ip, s.port)),
            )
            .await;
            assert!(!matches!(r, Ok(Ok(_))), "reachable on {ip}");
        }
        _ => eprintln!("binds_only_loopback: no non-loopback IPv4 address, skipped"),
    }
    s.shutdown().await;
}

#[tokio::test]
async fn shutdown_finishes_in_flight_request_and_stops_poller() {
    let mock = Arc::new(MockDriver::new());
    let s = start_with(mock.clone(), true, Duration::from_millis(40)).await;
    // Make the poller active.
    let _ = s.call("studio_get_state", json!({})).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(s.tools.revision_state().current() > 0);

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
    tokio::time::sleep(Duration::from_millis(150)).await;
    let port = s.port;
    let mock2 = s.mock.clone();
    s.shutdown().await;
    let reply = parse_reply(&inflight.await.unwrap());
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    // No new connections.
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
    );
    // Poller is gone: the driver sees no more calls.
    let before = mock2.calls().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mock2.calls().len(), before);
}
