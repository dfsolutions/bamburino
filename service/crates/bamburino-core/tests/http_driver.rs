// SPDX-License-Identifier: AGPL-3.0-or-later
//! `HttpDriverClient` against a local wiremock server.

use std::time::Duration;

use bamburino_core::driver::{
    AbsoluteTransform, DriverClient, DriverError, HttpDriverClient, HttpDriverConfig, JobStatus,
    SettingSource, SettingsTarget,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "t0k";

fn config(server: &MockServer) -> HttpDriverConfig {
    HttpDriverConfig {
        url: format!("{}/mcp", server.uri()).parse().unwrap(),
        token: Some(TOKEN.to_string()),
        read_timeout: Duration::from_secs(5),
        mutation_timeout: Duration::from_secs(5),
    }
}

fn rpc(result: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result}))
}

fn tool_ok(structured: Value) -> ResponseTemplate {
    rpc(json!({
        "content":[{"type":"text","text":structured.to_string()}],
        "structuredContent": structured
    }))
}

async fn mount_handshake(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(json!({"method":"initialize"})))
        .respond_with(rpc(json!({
            "protocolVersion":"2025-11-25","capabilities":{"tools":{}},
            "serverInfo":{"name":"BambuStudio","version":"1.0"}
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(
            json!({"method":"notifications/initialized"}),
        ))
        .respond_with(ResponseTemplate::new(202))
        .mount(server)
        .await;
}

async fn mount_tool(server: &MockServer, name: &str, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(body_partial_json(
            json!({"method":"tools/call","params":{"name":name}}),
        ))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn received_methods(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| {
            let v: Value = serde_json::from_slice(&r.body).unwrap();
            v["method"].as_str().unwrap().to_string()
        })
        .collect()
}

fn app_state_json() -> Value {
    json!({
        "sessionId":"s","appVersion":"02.08.04.57","projectName":"p","projectPath":"",
        "dirty":false,"presetsDirty":false,"uiJobRunning":false,"slicingRunning":false,
        "modalOpen":true,"objectCount":1,"plateCount":2,"selectedPlateIndex":0,
        "platerView":"prepare"
    })
}

fn one_value() -> std::collections::BTreeMap<String, String> {
    [("layer_height".to_string(), "0.2".to_string())].into()
}

#[tokio::test]
async fn sends_bearer_and_initializes_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("Authorization", "Bearer t0k"))
        .and(header("MCP-Protocol-Version", "2025-11-25"))
        .and(body_partial_json(json!({"method":"initialize"})))
        .respond_with(rpc(
            json!({"protocolVersion":"2025-11-25","capabilities":{}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"method":"notifications/initialized"}),
        ))
        .and(header("Authorization", "Bearer t0k"))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(header("Authorization", "Bearer t0k"))
        .and(body_partial_json(
            json!({"method":"tools/call","params":{"name":"app_get_state","arguments":{}}}),
        ))
        .respond_with(tool_ok(app_state_json()))
        .expect(2)
        .mount(&server)
        .await;

    let client = HttpDriverClient::new(config(&server)).unwrap();
    client.app_state().await.unwrap();
    client.app_state().await.unwrap();
    let requests = server.received_requests().await.unwrap();
    for r in &requests {
        let accept = r.headers.get("accept").unwrap().to_str().unwrap();
        assert!(accept.contains("application/json") && accept.contains("text/event-stream"));
        assert_eq!(r.headers.get("content-type").unwrap(), "application/json");
    }
    let methods = received_methods(&server).await;
    assert_eq!(
        methods,
        [
            "initialize",
            "notifications/initialized",
            "tools/call",
            "tools/call"
        ]
    );
}

#[tokio::test]
async fn maps_401_to_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Local bearer credential required"}
        })))
        .mount(&server)
        .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    assert_eq!(
        client.app_state().await.unwrap_err(),
        DriverError::Unauthorized
    );
}

#[tokio::test]
async fn missing_token_makes_no_call() {
    let server = MockServer::start().await;
    let mut cfg = config(&server);
    cfg.token = None;
    let client = HttpDriverClient::new(cfg).unwrap();
    assert_eq!(
        client.app_state().await.unwrap_err(),
        DriverError::Unauthorized
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn maps_connection_refused_to_unreachable() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let cfg = HttpDriverConfig {
        url: format!("http://127.0.0.1:{port}/mcp").parse().unwrap(),
        token: Some(TOKEN.into()),
        read_timeout: Duration::from_secs(10),
        mutation_timeout: Duration::from_secs(10),
    };
    let client = HttpDriverClient::new(cfg).unwrap();
    assert_eq!(
        client.app_state().await.unwrap_err(),
        DriverError::Unreachable
    );
}

#[tokio::test]
async fn maps_slow_response_to_timeout() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "app_get_state",
        tool_ok(app_state_json()).set_delay(Duration::from_millis(300)),
    )
    .await;
    let mut cfg = config(&server);
    cfg.read_timeout = Duration::from_millis(100);
    let client = HttpDriverClient::new(cfg).unwrap();
    // The handshake is fast; the tool call is the slow one.
    let err = client.app_state().await.unwrap_err();
    assert!(matches!(err, DriverError::Timeout { .. }), "{err:?}");
}

#[tokio::test]
async fn mutations_use_the_mutation_timeout() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "settings_update",
        tool_ok(json!({"updated":{}})).set_delay(Duration::from_millis(300)),
    )
    .await;
    let mut cfg = config(&server);
    cfg.read_timeout = Duration::from_millis(100);
    cfg.mutation_timeout = Duration::from_secs(5);
    let client = HttpDriverClient::new(cfg).unwrap();
    client.settings_update(&one_value()).await.unwrap();
}

#[tokio::test]
async fn maps_gateway_timeout_to_timeout() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "settings_update",
        ResponseTemplate::new(504).set_body_json(json!({
            "jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"Request timed out"}
        })),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let err = client.settings_update(&one_value()).await.unwrap_err();
    assert!(matches!(err, DriverError::Timeout { .. }), "{err:?}");
}

#[tokio::test]
async fn maps_busy_tool_error() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "settings_update",
        rpc(json!({
            "content":[{"type":"text","text":"modal"}],
            "structuredContent":{"error":{"code":"BUSY","message":"modal"}},
            "isError":true
        })),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    assert_eq!(
        client.settings_update(&one_value()).await.unwrap_err(),
        DriverError::Busy {
            message: "modal".into()
        }
    );
}

#[tokio::test]
async fn maps_flat_and_text_prefix_error_codes() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "slice_start",
        rpc(json!({
            "content":[{"type":"text","text":"{\"code\":\"STALE_REFERENCE\",\"message\":\"old\"}"}],
            "structuredContent":{"code":"STALE_REFERENCE","message":"old"},
            "isError":true
        })),
    )
    .await;
    mount_tool(
        &server,
        "plate_select",
        rpc(json!({
            "content":[{"type":"text","text":"VALIDATION_FAILED: no printable instances"}],
            "isError":true
        })),
    )
    .await;
    mount_tool(
        &server,
        "job_get",
        rpc(json!({"content":[{"type":"text","text":"Another MCP operation is active"}],"isError":true})),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    assert_eq!(
        client.slice_start(0, "r").await.unwrap_err(),
        DriverError::StaleReference {
            message: "old".into()
        }
    );
    assert_eq!(
        client.plate_select(0, "r").await.unwrap_err(),
        DriverError::Native {
            code: "VALIDATION_FAILED".into(),
            message: "no printable instances".into()
        }
    );
    assert!(matches!(
        client.job_get("j").await.unwrap_err(),
        DriverError::Busy { .. }
    ));
}

#[tokio::test]
async fn maps_jsonrpc_invalid_params() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "settings_effective",
        ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"Unknown setting: x"}
        })),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    assert_eq!(
        client
            .settings_effective(&SettingsTarget::Project, None)
            .await
            .unwrap_err(),
        DriverError::InvalidParams {
            message: "Unknown setting: x".into()
        }
    );
}

#[tokio::test]
async fn reinitializes_once_after_protocol_version_error() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    // First tool call: -32022, then success.
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method":"tools/call"})))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "jsonrpc":"2.0","id":1,"error":{"code":-32022,"message":"Unsupported protocol version"}
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_tool(&server, "app_get_state", tool_ok(app_state_json())).await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    client.app_state().await.unwrap();
    let methods = received_methods(&server).await;
    assert_eq!(
        methods,
        [
            "initialize",
            "notifications/initialized",
            "tools/call",
            "initialize",
            "notifications/initialized",
            "tools/call"
        ]
    );
}

#[tokio::test]
async fn parses_app_state_fields() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(&server, "app_get_state", tool_ok(app_state_json())).await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let s = client.app_state().await.unwrap();
    assert!(s.modal_open);
    assert!(!s.dirty);
    assert_eq!(s.plate_count, 2);
    assert_eq!(s.app_version, "02.08.04.57");
    assert_eq!(s.undo_snapshot, None);
}

#[tokio::test]
async fn parses_capabilities_identity() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "app_get_capabilities",
        tool_ok(json!({
            "appVersion":"02.08.04.57","buildSuffix":"bamburino.abcdef012345","patchId":"abcdef012345"
        })),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let c = client.capabilities().await.unwrap();
    assert_eq!(c.app_version, "02.08.04.57");
    assert_eq!(c.build_suffix.as_deref(), Some("bamburino.abcdef012345"));
    assert_eq!(c.patch_id.as_deref(), Some("abcdef012345"));

    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "app_get_capabilities",
        tool_ok(json!({"appVersion":"02.08.04.57"})),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let c = client.capabilities().await.unwrap();
    assert_eq!(c.patch_id, None);
    assert_eq!(c.build_suffix, None);
}

#[tokio::test]
async fn parses_event_stream_response() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    let payload = json!({"jsonrpc":"2.0","id":1,"result":{"structuredContent":app_state_json()}});
    mount_tool(
        &server,
        "app_get_state",
        ResponseTemplate::new(200).set_body_raw(
            format!("event: message\ndata: {payload}\n\n"),
            "text/event-stream",
        ),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    assert!(client.app_state().await.unwrap().modal_open);
}

#[tokio::test]
async fn falls_back_to_content_text_json() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "app_get_state",
        rpc(json!({"content":[{"type":"text","text":app_state_json().to_string()}]})),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    assert!(client.app_state().await.unwrap().modal_open);
}

fn cfg_for(url: &str, token: Option<&str>) -> HttpDriverConfig {
    HttpDriverConfig {
        url: url.parse().unwrap(),
        token: token.map(str::to_string),
        read_timeout: Duration::from_secs(1),
        mutation_timeout: Duration::from_secs(1),
    }
}

#[test]
fn rejects_non_loopback_url() {
    let bad = cfg_for("http://192.168.1.10:27183/mcp", Some(TOKEN));
    assert!(matches!(
        HttpDriverClient::new(bad),
        Err(DriverError::Protocol { .. })
    ));
    for ok in [
        "http://127.0.0.1:27183/mcp",
        "http://localhost:27183/mcp",
        "http://[::1]:27183/mcp",
    ] {
        assert!(HttpDriverClient::new(cfg_for(ok, None)).is_ok(), "{ok}");
    }
}

#[test]
fn token_never_in_debug_output() {
    let cfg = cfg_for("http://127.0.0.1:27183/mcp", Some("s3cr3t-token-value"));
    let dbg_cfg = format!("{cfg:?}");
    let client = HttpDriverClient::new(cfg).unwrap();
    let dbg_client = format!("{client:?}");
    assert!(!dbg_cfg.contains("s3cr3t"));
    assert!(!dbg_client.contains("s3cr3t"));
}

#[tokio::test]
async fn token_never_in_error_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let err = client.app_state().await.unwrap_err();
    assert!(!format!("{err} {err:?}").contains(TOKEN));
}

#[tokio::test]
async fn plate_calls_send_camel_case_arguments_and_parse_plate_state() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    let plates = json!({
        "plates":[{"index":0,"name":"A","locked":false,"selected":true,"printable":true},
                  {"index":1,"name":"B","locked":true,"selected":false,"printable":true}],
        "selectedIndex":1,"plateRevision":"sess:7"
    });
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"params":{"name":"plate_select",
            "arguments":{"plateIndex":1,"expectedPlateRevision":"sess:6"}}})))
        .respond_with(tool_ok(plates.clone()))
        .expect(1)
        .mount(&server)
        .await;
    // settings_override nests the plate state under `plates`.
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"params":{"name":"settings_override",
            "arguments":{"scope":"plate","plateIndex":1,"expectedPlateRevision":"sess:7",
                         "values":{"layer_height":"0.1"}}}}),
        ))
        .respond_with(tool_ok(
            json!({"scope":"plate","updated":{},"model":{"objects":[]},"plates":plates}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let list = client.plate_select(1, "sess:6").await.unwrap();
    assert_eq!(list.plate_revision, "sess:7");
    assert_eq!(list.selected_index, 1);
    assert_eq!(list.plates.len(), 2);
    assert!(list.plates[1].locked);
    let values = [("layer_height".to_string(), "0.1".to_string())].into();
    let after = client
        .settings_override(
            &SettingsTarget::Plate {
                index: 1,
                plate_revision: "sess:7".into(),
            },
            &values,
        )
        .await
        .unwrap();
    assert_eq!(after.plate_revision, "sess:7");
}

#[tokio::test]
async fn settings_effective_maps_sources_and_keys() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"params":{"name":"settings_effective",
            "arguments":{"keys":["layer_height","wall_loops"],"objectId":"s:5"}}}),
        ))
        .respond_with(tool_ok(json!({"scope":"object","values":{
            "layer_height":{"value":"0.2","source":"global"},
            "wall_loops":{"value":"3","source":"object"}
        }})))
        .mount(&server)
        .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let keys = vec!["layer_height".to_string(), "wall_loops".to_string()];
    let got = client
        .settings_effective(
            &SettingsTarget::Object {
                object_id: "s:5".into(),
            },
            Some(&keys),
        )
        .await
        .unwrap();
    let get = |k: &str| got.iter().find(|e| e.key == k).unwrap();
    assert_eq!(get("layer_height").source, SettingSource::Project);
    assert_eq!(get("layer_height").value, "0.2");
    assert_eq!(get("wall_loops").source, SettingSource::Object);
}

#[tokio::test]
async fn jobs_and_slice_state() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "slice_start",
        tool_ok(json!({
            "jobId":"j1","kind":"slice","plateIndex":0,"status":"succeeded","progressPercent":100,
            "message":"","error":null,"result":{"sliceResultId":"r1","reused":true}
        })),
    )
    .await;
    mount_tool(
        &server,
        "job_get",
        tool_ok(
            json!({"jobId":"j1","plateIndex":0,"status":"cancelling","progressPercent":40,
                       "message":"","error":null,"result":null}),
        ),
    )
    .await;
    mount_tool(
        &server,
        "job_cancel",
        tool_ok(json!({"jobId":"j1","plateIndex":0,"status":"failed","progressPercent":40,
                       "message":"x","error":{"code":"NATIVE_ERROR","message":"bad"},"result":null})),
    )
    .await;
    mount_tool(
        &server,
        "slice_get_state",
        tool_ok(
            json!({"plateIndex":0,"busy":false,"valid":true,"sliceResultId":"r1",
                       "statistics":{"estimatedNormalTime":"1h 23m 45s"}}),
        ),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let j = client.slice_start(0, "r").await.unwrap();
    assert_eq!(j.status, JobStatus::Succeeded);
    assert!(j.reused);
    assert_eq!(j.slice_result_id.as_deref(), Some("r1"));
    assert_eq!(j.progress_percent, Some(100));
    assert_eq!(j.message, None);
    assert_eq!(
        client.job_get("j1").await.unwrap().status,
        JobStatus::Cancelling
    );
    let c = client.job_cancel("j1").await.unwrap();
    assert_eq!(c.status, JobStatus::Failed);
    assert_eq!(c.error.unwrap().code, "NATIVE_ERROR");
    let s = client.slice_state().await.unwrap();
    assert!(!s.running && s.valid);
    assert_eq!(s.estimated_time_s, Some(5025.0));
    assert_eq!(s.slice_result_id.as_deref(), Some("r1"));
}

#[tokio::test]
async fn unknown_job_status_and_unparsable_time() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    mount_tool(
        &server,
        "job_get",
        tool_ok(
            json!({"jobId":"j1","plateIndex":0,"status":"weird","progressPercent":null,
                       "message":null,"error":null,"result":null}),
        ),
    )
    .await;
    mount_tool(
        &server,
        "slice_get_state",
        tool_ok(
            json!({"plateIndex":0,"busy":true,"valid":false,"sliceResultId":null,
                       "statistics":{"estimatedNormalTime":"N/A"}}),
        ),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    assert_eq!(
        client.job_get("j1").await.unwrap().status,
        JobStatus::Unknown("weird".into())
    );
    let s = client.slice_state().await.unwrap();
    assert_eq!(s.estimated_time_s, None);
    assert!(s.running);
}

#[tokio::test]
async fn model_list_and_object_transform() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    let model = json!({"objects":[
        {"index":0,"id":"s:1","name":"cube","dimensions":[20.0,20.0,20.0],"volumes":[],
         "instances":[{"index":0,"id":"s:2","position":[1.0,2.0,3.0],"rotation":[0.0,0.0,90.0],
                       "scale":[1.0,1.0,1.0],"plateIndex":0}]},
        {"index":1,"id":"s:3","name":"other","dimensions":[1.0,2.0,3.0],"volumes":[],"instances":[]}
    ]});
    mount_tool(&server, "model_list", tool_ok(model.clone())).await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({"params":{"name":"object_transform","arguments":{
            "objectId":"s:1","instanceId":"s:2","position":[1.0,2.0,3.0],
            "rotation":[0.0,0.0,90.0],"scale":[1.0,1.0,1.0]}}}),
        ))
        .respond_with(tool_ok(model))
        .mount(&server)
        .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let list = client.model_list().await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].dimensions_mm, [20.0, 20.0, 20.0]);
    assert_eq!(list[0].instances[0].rotation_deg, [0.0, 0.0, 90.0]);
    assert_eq!(list[0].instances[0].plate_index, 0);
    let t = AbsoluteTransform {
        position_mm: [1.0, 2.0, 3.0],
        rotation_deg: [0.0, 0.0, 90.0],
        scale: [1.0, 1.0, 1.0],
    };
    let obj = client.object_transform("s:1", "s:2", &t).await.unwrap();
    assert_eq!(obj.id, "s:1");
    let err = client.object_transform("s:9", "s:2", &t).await.unwrap_err();
    assert!(matches!(err, DriverError::Protocol { .. }));
}

#[tokio::test]
async fn presets_writable_keys_and_preview() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    for (kind, selected, slots) in [
        ("printer", "P1S 0.4", json!([])),
        ("process", "0.20mm", json!([])),
        ("filament", "PLA", json!(["PLA", "PETG"])),
    ] {
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"params":{"name":"preset_list","arguments":{"kind":kind}}}),
            ))
            .respond_with(tool_ok(
                json!({"kind":kind,"selected":selected,"slots":slots,"profiles":[]}),
            ))
            .mount(&server)
            .await;
    }
    mount_tool(
        &server,
        "settings_get",
        tool_ok(
            json!({"scope":"process","values":{},"writableKeys":["layer_height","wall_loops"]}),
        ),
    )
    .await;
    mount_tool(
        &server,
        "preview_get_summary",
        tool_ok(json!({"moveCount":10,"filamentCount":1,
                       "warnings":[{"level":"warning","code":"W","message":"careful"}]})),
    )
    .await;
    let client = HttpDriverClient::new(config(&server)).unwrap();
    let p = client.active_presets().await.unwrap();
    assert_eq!(p.printer.as_deref(), Some("P1S 0.4"));
    assert_eq!(p.process.as_deref(), Some("0.20mm"));
    assert_eq!(p.filaments, ["PLA", "PETG"]);
    let keys = client.writable_keys().await.unwrap();
    assert!(keys.contains("layer_height") && keys.len() == 2);
    let s = client.preview_summary(0, "r", "res").await.unwrap();
    assert_eq!(s.print_time_s, None);
    assert!(s.filaments.is_empty());
    assert_eq!(s.warnings, ["careful"]);
    assert_eq!(s.raw["moveCount"], 10);
}
