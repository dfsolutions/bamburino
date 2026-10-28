// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP/JSON-RPC client for the in-process MCP server of the patched
//! Bambu Studio (streamable HTTP, protocol `2025-11-25`).
//!
//! Wire format (from the driver audit):
//! - `POST <url>` with `Authorization: Bearer <token>`, `Content-Type:
//!   application/json`, `Accept: application/json, text/event-stream` and
//!   `MCP-Protocol-Version: 2025-11-25` (the driver rejects non-`initialize`
//!   requests that lack it with `-32022`);
//! - the driver keeps no session: the `initialize` handshake is a client-side
//!   formality done on the first call and repeated once after a protocol
//!   version error;
//! - a tool failure is a normal result with `isError: true`, except bad
//!   arguments, which are a JSON-RPC error `-32602`; a queued mutation that
//!   exceeds the driver's 30 s queue is an HTTP 504.
//!
//! The token is never part of any error, `Debug` output or log line.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use reqwest::{StatusCode, Url, header};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use super::{
    AbsoluteTransform, ActivePresets, AmsInventory, AmsTray, AmsUnit, AppState, DeviceSummary,
    DriverCapabilities, DriverClient, DriverError, DriverJob, DriverJobError, DriverTool,
    EffectiveSetting, ExportedPlate, FilamentAssignment, Instance, JobStatus, ModelObject, Plate,
    PlateContents, PlateDeleted, PlateList, SettingSource, SettingsTarget, SliceState,
    SliceSummary,
};
use crate::build_info::BAMBURINO_VERSION;

const PROTOCOL_VERSION: &str = "2025-11-25";

/// Connection settings. `url` must be a loopback `http` URL.
#[derive(Clone)]
pub struct HttpDriverConfig {
    pub url: reqwest::Url,
    pub token: Option<String>,
    pub read_timeout: Duration,
    /// Must exceed the driver's own 30 s mutation queue (35 s by default in
    /// the service configuration).
    pub mutation_timeout: Duration,
}

impl fmt::Debug for HttpDriverConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpDriverConfig")
            .field("url", &self.url.as_str())
            .field("token", &redacted(&self.token))
            .field("read_timeout", &self.read_timeout)
            .field("mutation_timeout", &self.mutation_timeout)
            .finish()
    }
}

fn redacted(token: &Option<String>) -> &'static str {
    if token.is_some() {
        "<redacted>"
    } else {
        "<none>"
    }
}

/// The token in use and, when it is file-backed, the file stamp it came from.
struct TokenState {
    value: Option<String>,
    stamp: Option<(std::time::SystemTime, u64)>,
}

pub struct HttpDriverClient {
    http: reqwest::Client,
    url: Url,
    token: std::sync::Mutex<TokenState>,
    token_file: Option<std::path::PathBuf>,
    read_timeout: Duration,
    mutation_timeout: Duration,
    /// `true` once the `initialize` handshake succeeded; the lock also
    /// serializes concurrent first calls.
    initialized: Mutex<bool>,
    next_id: AtomicU64,
}

impl fmt::Debug for HttpDriverClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpDriverClient")
            .field("url", &self.url.as_str())
            .field("token", &redacted(&self.current_token()))
            .field("read_timeout", &self.read_timeout)
            .field("mutation_timeout", &self.mutation_timeout)
            .finish_non_exhaustive()
    }
}

/// A failed request; `reinit` asks the caller to redo the handshake once.
struct CallError {
    err: DriverError,
    reinit: bool,
}

impl From<DriverError> for CallError {
    fn from(err: DriverError) -> Self {
        Self { err, reinit: false }
    }
}

fn proto(message: impl Into<String>) -> DriverError {
    DriverError::Protocol {
        message: message.into(),
    }
}

impl HttpDriverClient {
    /// Rejects URLs that are not plain `http` to a loopback host.
    pub fn new(cfg: HttpDriverConfig) -> Result<Self, DriverError> {
        if cfg.url.scheme() != "http" {
            return Err(proto("driver URL must use http on loopback"));
        }
        let loopback = cfg.url.host_str().is_some_and(|h| {
            let h = h.trim_start_matches('[').trim_end_matches(']');
            h.eq_ignore_ascii_case("localhost")
                || h.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        });
        if !loopback {
            return Err(proto("driver URL must point to a loopback address"));
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            // Loopback connects are instant; Windows takes ~2 s to report a
            // refused one, so cap it and report `Unreachable`.
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| proto(format!("cannot build HTTP client: {}", e.without_url())))?;
        Ok(Self {
            http,
            url: cfg.url,
            token: std::sync::Mutex::new(TokenState {
                value: cfg.token,
                stamp: None,
            }),
            token_file: None,
            read_timeout: cfg.read_timeout,
            mutation_timeout: cfg.mutation_timeout,
            initialized: Mutex::new(false),
            next_id: AtomicU64::new(1),
        })
    }

    /// Makes the token follow `path`: whenever the file's modification time
    /// or size changes it is re-read (trimmed; empty means no token), so a new
    /// token takes effect without restarting the process. The token given in
    /// the config is used until the file is first seen to differ.
    pub fn with_token_file(mut self, path: std::path::PathBuf) -> Self {
        self.token_file = Some(path);
        self
    }

    /// The token to use now (re-read from the token file if it changed).
    fn current_token(&self) -> Option<String> {
        let mut state = self.token.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(path) = &self.token_file {
            let stamp = std::fs::metadata(path)
                .and_then(|m| Ok((m.modified()?, m.len())))
                .ok();
            if stamp != state.stamp {
                state.stamp = stamp;
                state.value = stamp.and_then(|_| {
                    let text = std::fs::read_to_string(path).ok()?;
                    let t = text.trim();
                    (!t.is_empty()).then(|| t.to_owned())
                });
            }
        }
        state.value.clone()
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// One HTTP exchange. Returns the parsed JSON-RPC response (`Null` for an
    /// empty body such as the `202` of a notification).
    async fn post(&self, body: &Value, timeout: Duration) -> Result<Value, CallError> {
        let token = self.current_token().ok_or(DriverError::Unauthorized)?;
        let after_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let map_send = |e: reqwest::Error| -> DriverError {
            // A failed or timed-out connect means nobody is listening.
            if e.is_connect() {
                DriverError::Unreachable
            } else if e.is_timeout() {
                DriverError::Timeout { after_ms }
            } else {
                proto(format!("HTTP request failed: {}", e.without_url()))
            }
        };
        let response = self
            .http
            .post(self.url.clone())
            .timeout(timeout)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .body(body.to_string())
            .send()
            .await
            .map_err(map_send)?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(DriverError::Unauthorized.into());
        }
        if status == StatusCode::GATEWAY_TIMEOUT {
            return Err(DriverError::Timeout { after_ms }.into());
        }
        let is_sse = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let text = response.text().await.map_err(map_send)?;
        let payload = if is_sse {
            first_sse_data(&text)
        } else {
            text.trim().to_string()
        };
        let parsed: Option<Value> = if payload.is_empty() {
            None
        } else {
            serde_json::from_str(&payload).ok()
        };
        if let Some(error) = parsed.as_ref().and_then(|v| v.get("error")) {
            return Err(rpc_error(error));
        }
        if !status.is_success() {
            return Err(proto(format!("HTTP {}", status.as_u16())).into());
        }
        match parsed {
            Some(v) => Ok(v),
            None if payload.is_empty() => Ok(Value::Null),
            None => Err(proto("response is not valid JSON").into()),
        }
    }

    async fn ensure_initialized(&self) -> Result<(), DriverError> {
        let mut done = self.initialized.lock().await;
        if *done {
            return Ok(());
        }
        let init = json!({
            "jsonrpc": "2.0",
            "id": self.next_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "bamburino", "version": BAMBURINO_VERSION},
            },
        });
        self.post(&init, self.read_timeout)
            .await
            .map_err(|c| c.err)?;
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        self.post(&note, self.read_timeout)
            .await
            .map_err(|c| c.err)?;
        *done = true;
        Ok(())
    }

    /// `tools/call` with the lazy handshake and one re-initialization after a
    /// protocol error. Returns the tool's structured result.
    async fn call(&self, tool: DriverTool, arguments: Value) -> Result<Value, DriverError> {
        if self.current_token().is_none() {
            return Err(DriverError::Unauthorized);
        }
        let timeout = if tool.is_mutation() {
            self.mutation_timeout
        } else {
            self.read_timeout
        };
        let mut retried = false;
        loop {
            self.ensure_initialized().await?;
            let body = json!({
                "jsonrpc": "2.0",
                "id": self.next_id(),
                "method": "tools/call",
                "params": {"name": tool.name(), "arguments": arguments},
            });
            match self.post(&body, timeout).await {
                Ok(response) => return tool_result(&response),
                Err(c) if c.reinit && !retried => {
                    retried = true;
                    *self.initialized.lock().await = false;
                }
                Err(c) => return Err(c.err),
            }
        }
    }
}

/// Maps a JSON-RPC `error` object.
fn rpc_error(error: &Value) -> CallError {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    match code {
        -32602 => DriverError::InvalidParams { message }.into(),
        -32022 | -32020 => CallError {
            err: proto(format!("JSON-RPC {code}: {message}")),
            reinit: true,
        },
        _ => proto(format!("JSON-RPC {code}: {message}")).into(),
    }
}

/// Payload of the first `data:` event of a `text/event-stream` body.
fn first_sse_data(body: &str) -> String {
    let mut data: Vec<&str> = Vec::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest));
        } else if line.trim().is_empty() && !data.is_empty() {
            break;
        }
    }
    data.join("\n")
}

/// Extracts the tool payload from a JSON-RPC response.
fn tool_result(response: &Value) -> Result<Value, DriverError> {
    let result = response
        .get("result")
        .filter(|r| r.is_object())
        .ok_or_else(|| proto("response has no result"))?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(tool_error(result));
    }
    if let Some(structured) = result.get("structuredContent").filter(|v| !v.is_null()) {
        return Ok(structured.clone());
    }
    let text =
        first_text(result).ok_or_else(|| proto("result has no structuredContent or text"))?;
    serde_json::from_str(text).map_err(|_| proto("result text is not JSON"))
}

fn first_text(result: &Value) -> Option<&str> {
    result
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|c| c.get("text").and_then(Value::as_str))
}

/// Maps an `isError: true` result. The code is searched in
/// `structuredContent.error.code`, `structuredContent.code`, then as a
/// `"<CODE>:"` prefix of the text.
fn tool_error(result: &Value) -> DriverError {
    let text = first_text(result).unwrap_or("");
    let structured = result
        .get("structuredContent")
        .filter(|v| v.is_object())
        .cloned()
        .or_else(|| {
            serde_json::from_str::<Value>(text)
                .ok()
                .filter(Value::is_object)
        });
    let pick = |path: &[&[&str]]| -> Option<String> {
        let s = structured.as_ref()?;
        path.iter().find_map(|p| {
            p.iter()
                .try_fold(s, |v, k| v.get(*k))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
    };
    let mut code = pick(&[&["error", "code"], &["code"]]);
    let mut message = pick(&[&["error", "message"], &["message"]]);
    if code.is_none()
        && let Some((prefix, rest)) = text.split_once(':')
        && !prefix.is_empty()
        && prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        code = Some(prefix.to_string());
        message.get_or_insert_with(|| rest.trim().to_string());
    }
    let message = message
        .or_else(|| (!text.is_empty()).then(|| text.to_string()))
        .unwrap_or_else(|| "driver reported an error".to_string());
    match code.as_deref() {
        Some("BUSY") => DriverError::Busy { message },
        Some("STALE_REFERENCE") => DriverError::StaleReference { message },
        Some("INVALID_PARAMS") => DriverError::InvalidParams { message },
        Some(other) => DriverError::Native {
            code: other.to_string(),
            message,
        },
        // Uncoded refusals of the driver's call gate (GUI_App.cpp): another
        // call is running or the editor is rebuilding.
        None if text.contains("Another MCP operation is active")
            || text.contains("The editor is rebuilding") =>
        {
            DriverError::Busy { message }
        }
        None => DriverError::Native {
            code: "UNKNOWN".to_string(),
            message,
        },
    }
}

// ---- response parsing helpers ---------------------------------------------

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value, DriverError> {
    v.get(key)
        .filter(|f| !f.is_null())
        .ok_or_else(|| proto(format!("missing field `{key}`")))
}

fn str_of(v: &Value, key: &str) -> Result<String, DriverError> {
    field(v, key)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| proto(format!("field `{key}` is not a string")))
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn bool_of(v: &Value, key: &str) -> Result<bool, DriverError> {
    field(v, key)?
        .as_bool()
        .ok_or_else(|| proto(format!("field `{key}` is not a boolean")))
}

fn u32_of(v: &Value, key: &str) -> Result<u32, DriverError> {
    field(v, key)?
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| proto(format!("field `{key}` is not an unsigned integer")))
}

fn array_of<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>, DriverError> {
    field(v, key)?
        .as_array()
        .ok_or_else(|| proto(format!("field `{key}` is not an array")))
}

fn vec3_of(v: &Value, key: &str) -> Result<[f64; 3], DriverError> {
    let bad = || proto(format!("field `{key}` is not a 3-number array"));
    let a = array_of(v, key)?;
    if a.len() != 3 {
        return Err(bad());
    }
    let mut out = [0.0; 3];
    for (slot, n) in out.iter_mut().zip(a) {
        *slot = n.as_f64().ok_or_else(bad)?;
    }
    Ok(out)
}

fn parse_app_state(v: &Value) -> Result<AppState, DriverError> {
    Ok(AppState {
        session_id: str_of(v, "sessionId")?,
        app_version: str_of(v, "appVersion")?,
        project_name: str_of(v, "projectName")?,
        project_path: str_of(v, "projectPath")?,
        dirty: bool_of(v, "dirty")?,
        presets_dirty: bool_of(v, "presetsDirty")?,
        ui_job_running: bool_of(v, "uiJobRunning")?,
        slicing_running: bool_of(v, "slicingRunning")?,
        modal_open: bool_of(v, "modalOpen")?,
        object_count: u32_of(v, "objectCount")?,
        plate_count: u32_of(v, "plateCount")?,
        selected_plate_index: u32_of(v, "selectedPlateIndex")?,
        // Optional field added by driver patch 0120; string or number.
        undo_snapshot: match v.get("undoSnapshot") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(n @ Value::Number(_)) => Some(n.to_string()),
            _ => None,
        },
    })
}

/// Plate state: `{plates[], selectedIndex, plateRevision}`. `settings_override`
/// nests it under a `plates` key.
fn parse_plate_list(v: &Value) -> Result<PlateList, DriverError> {
    if v.get("plateRevision").is_none() && v.get("plates").is_some_and(Value::is_object) {
        return parse_plate_list(&v["plates"]);
    }
    let plates = array_of(v, "plates")?
        .iter()
        .map(|p| {
            Ok(Plate {
                index: u32_of(p, "index")?,
                name: str_of(p, "name")?,
                locked: bool_of(p, "locked")?,
            })
        })
        .collect::<Result<Vec<_>, DriverError>>()?;
    Ok(PlateList {
        plates,
        selected_index: u32_of(v, "selectedIndex")?,
        plate_revision: str_of(v, "plateRevision")?,
    })
}

fn utf8_path<'a>(path: &'a std::path::Path, what: &str) -> Result<&'a str, DriverError> {
    path.to_str().ok_or_else(|| DriverError::InvalidParams {
        message: format!("{what} path is not valid UTF-8"),
    })
}

fn parse_ams_inventory(v: &Value) -> Result<AmsInventory, DriverError> {
    let units = array_of(v, "units")?
        .iter()
        .map(|u| {
            let trays = array_of(u, "slots")?
                .iter()
                .map(|t| {
                    Ok(AmsTray {
                        slot_id: str_of(t, "slotId")?,
                        exists: bool_of(t, "exists")?,
                        filament_type: opt_str(t, "filamentType").unwrap_or_default(),
                        color: opt_str(t, "color").unwrap_or_default(),
                        remaining_percent: t
                            .get("remainingPercent")
                            .and_then(Value::as_i64)
                            .and_then(|n| i32::try_from(n).ok()),
                    })
                })
                .collect::<Result<Vec<_>, DriverError>>()?;
            Ok(AmsUnit {
                ams_id: str_of(u, "amsId")?,
                trays,
            })
        })
        .collect::<Result<Vec<_>, DriverError>>()?;
    Ok(AmsInventory { units })
}

fn parse_object(o: &Value) -> Result<ModelObject, DriverError> {
    let instances = array_of(o, "instances")?
        .iter()
        .map(|i| {
            Ok(Instance {
                id: str_of(i, "id")?,
                index: u32_of(i, "index")?,
                position_mm: vec3_of(i, "position")?,
                rotation_deg: vec3_of(i, "rotation")?,
                scale: vec3_of(i, "scale")?,
                plate_index: field(i, "plateIndex")?
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(|| proto("field `plateIndex` is not an integer"))?,
            })
        })
        .collect::<Result<Vec<_>, DriverError>>()?;
    Ok(ModelObject {
        id: str_of(o, "id")?,
        index: u32_of(o, "index")?,
        name: str_of(o, "name")?,
        dimensions_mm: vec3_of(o, "dimensions")?,
        instances,
    })
}

fn parse_model(v: &Value) -> Result<Vec<ModelObject>, DriverError> {
    array_of(v, "objects")?.iter().map(parse_object).collect()
}

fn parse_job_status(s: &str) -> JobStatus {
    match s {
        "running" => JobStatus::Running,
        "cancelling" => JobStatus::Cancelling,
        "succeeded" => JobStatus::Succeeded,
        "failed" => JobStatus::Failed,
        "cancelled" => JobStatus::Cancelled,
        "stale" => JobStatus::Stale,
        other => JobStatus::Unknown(other.to_string()),
    }
}

fn parse_job(v: &Value) -> Result<DriverJob, DriverError> {
    let error = match v.get("error") {
        Some(e) if e.is_object() => Some(DriverJobError {
            code: opt_str(e, "code").unwrap_or_default(),
            message: opt_str(e, "message").unwrap_or_default(),
        }),
        // UI jobs (arrange) report a plain string.
        Some(Value::String(m)) if !m.is_empty() => Some(DriverJobError {
            code: "NATIVE_ERROR".to_string(),
            message: m.clone(),
        }),
        _ => None,
    };
    let result = v.get("result").filter(|r| r.is_object());
    Ok(DriverJob {
        id: str_of(v, "jobId")?,
        plate_index: u32_of(v, "plateIndex")?,
        status: parse_job_status(&str_of(v, "status")?),
        progress_percent: v
            .get("progressPercent")
            .and_then(Value::as_u64)
            .map(|p| u8::try_from(p.min(100)).unwrap_or(100)),
        message: opt_str(v, "message").filter(|m| !m.is_empty()),
        error,
        slice_result_id: result.and_then(|r| opt_str(r, "sliceResultId")),
        reused: result
            .and_then(|r| r.get("reused"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Parses the driver's duration string (`1d 2h 3m 4s`, any subset) to seconds.
/// `None` for `N/A` or anything else unrecognised.
fn parse_dhms(s: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut tokens = 0;
    for token in s.split_whitespace() {
        let unit = token.chars().last()?;
        let factor = match unit {
            'd' => 86_400.0,
            'h' => 3_600.0,
            'm' => 60.0,
            's' => 1.0,
            _ => return None,
        };
        let n: f64 = token[..token.len() - 1].parse().ok()?;
        if !n.is_finite() || n < 0.0 {
            return None;
        }
        total += n * factor;
        tokens += 1;
    }
    (tokens > 0).then_some(total)
}

fn parse_slice_state(v: &Value) -> Result<SliceState, DriverError> {
    let estimated = v
        .get("statistics")
        .and_then(|s| s.get("estimatedNormalTime"))
        .and_then(|t| match t {
            Value::String(s) => parse_dhms(s),
            Value::Number(n) => n.as_f64(),
            _ => None,
        });
    Ok(SliceState {
        running: bool_of(v, "busy")?,
        valid: bool_of(v, "valid")?,
        estimated_time_s: estimated,
        slice_result_id: opt_str(v, "sliceResultId"),
        plate_index: v
            .get("plateIndex")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
    })
}

fn target_args(target: &SettingsTarget, args: &mut Map<String, Value>) {
    match target {
        SettingsTarget::Project => {}
        SettingsTarget::Plate {
            index,
            plate_revision,
        } => {
            args.insert("plateIndex".into(), json!(index));
            args.insert("expectedPlateRevision".into(), json!(plate_revision));
        }
        SettingsTarget::Object { object_id } => {
            args.insert("objectId".into(), json!(object_id));
        }
    }
}

fn parse_setting_source(s: &str) -> Result<SettingSource, DriverError> {
    match s {
        "global" => Ok(SettingSource::Project),
        "plate" => Ok(SettingSource::Plate),
        "object" => Ok(SettingSource::Object),
        "volume" => Ok(SettingSource::Volume),
        other => Err(proto(format!("unknown setting source `{other}`"))),
    }
}

async fn preset_selected(
    client: &HttpDriverClient,
    kind: &str,
) -> Result<(Option<String>, Vec<String>), DriverError> {
    let v = client
        .call(DriverTool::PresetList, json!({ "kind": kind }))
        .await?;
    let selected = opt_str(&v, "selected").filter(|s| !s.is_empty());
    let slots = v
        .get("slots")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok((selected, slots))
}

#[async_trait::async_trait]
impl DriverClient for HttpDriverClient {
    fn token_configured(&self) -> bool {
        self.current_token().is_some()
    }

    async fn app_state(&self) -> Result<AppState, DriverError> {
        parse_app_state(&self.call(DriverTool::AppGetState, json!({})).await?)
    }

    async fn capabilities(&self) -> Result<DriverCapabilities, DriverError> {
        let raw = self.call(DriverTool::AppGetCapabilities, json!({})).await?;
        Ok(DriverCapabilities {
            app_version: str_of(&raw, "appVersion")?,
            build_suffix: opt_str(&raw, "buildSuffix"),
            patch_id: opt_str(&raw, "patchId"),
            raw,
        })
    }

    async fn plate_list(&self) -> Result<PlateList, DriverError> {
        parse_plate_list(&self.call(DriverTool::PlateList, json!({})).await?)
    }

    async fn plate_select(
        &self,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<PlateList, DriverError> {
        let args = json!({"plateIndex": plate_index, "expectedPlateRevision": plate_revision});
        parse_plate_list(&self.call(DriverTool::PlateSelect, args).await?)
    }

    async fn model_list(&self) -> Result<Vec<ModelObject>, DriverError> {
        parse_model(&self.call(DriverTool::ModelList, json!({})).await?)
    }

    async fn active_presets(&self) -> Result<ActivePresets, DriverError> {
        let (printer, _) = preset_selected(self, "printer").await?;
        let (process, _) = preset_selected(self, "process").await?;
        let (_, filaments) = preset_selected(self, "filament").await?;
        Ok(ActivePresets {
            printer,
            process,
            filaments,
        })
    }

    async fn writable_keys(&self) -> Result<BTreeSet<String>, DriverError> {
        let v = self.call(DriverTool::SettingsGet, json!({})).await?;
        Ok(array_of(&v, "writableKeys")?
            .iter()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect())
    }

    async fn settings_effective(
        &self,
        target: &SettingsTarget,
        keys: Option<&[String]>,
    ) -> Result<Vec<EffectiveSetting>, DriverError> {
        let mut args = Map::new();
        if let Some(keys) = keys {
            args.insert("keys".into(), json!(keys));
        }
        target_args(target, &mut args);
        let v = self
            .call(DriverTool::SettingsEffective, Value::Object(args))
            .await?;
        let values = field(&v, "values")?
            .as_object()
            .ok_or_else(|| proto("field `values` is not an object"))?;
        values
            .iter()
            .map(|(key, entry)| {
                let value = match field(entry, "value")? {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                Ok(EffectiveSetting {
                    key: key.clone(),
                    value,
                    source: parse_setting_source(&str_of(entry, "source")?)?,
                })
            })
            .collect()
    }

    async fn settings_update(&self, values: &BTreeMap<String, String>) -> Result<(), DriverError> {
        self.call(DriverTool::SettingsUpdate, json!({ "values": values }))
            .await
            .map(|_| ())
    }

    async fn settings_override(
        &self,
        target: &SettingsTarget,
        values: &BTreeMap<String, String>,
    ) -> Result<PlateList, DriverError> {
        let mut args = Map::new();
        let scope = match target {
            SettingsTarget::Project => {
                return Err(DriverError::InvalidParams {
                    message: "settings_override needs a plate or object target".into(),
                });
            }
            SettingsTarget::Plate { .. } => "plate",
            SettingsTarget::Object { .. } => "object",
        };
        args.insert("scope".into(), json!(scope));
        args.insert("values".into(), json!(values));
        target_args(target, &mut args);
        parse_plate_list(
            &self
                .call(DriverTool::SettingsOverride, Value::Object(args))
                .await?,
        )
    }

    async fn object_transform(
        &self,
        object_id: &str,
        instance_id: &str,
        t: &AbsoluteTransform,
    ) -> Result<ModelObject, DriverError> {
        let args = json!({
            "objectId": object_id,
            "instanceId": instance_id,
            "position": t.position_mm,
            "rotation": t.rotation_deg,
            "scale": t.scale,
        });
        let v = self.call(DriverTool::ObjectTransform, args).await?;
        parse_model(&v)?
            .into_iter()
            .find(|o| o.id == object_id)
            .ok_or_else(|| proto("transformed object missing from the returned model"))
    }

    async fn slice_start(
        &self,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<DriverJob, DriverError> {
        let args = json!({"plateIndex": plate_index, "expectedPlateRevision": plate_revision});
        parse_job(&self.call(DriverTool::SliceStart, args).await?)
    }

    async fn job_get(&self, job_id: &str) -> Result<DriverJob, DriverError> {
        parse_job(
            &self
                .call(DriverTool::JobGet, json!({"jobId": job_id}))
                .await?,
        )
    }

    async fn job_cancel(&self, job_id: &str) -> Result<DriverJob, DriverError> {
        parse_job(
            &self
                .call(DriverTool::JobCancel, json!({"jobId": job_id}))
                .await?,
        )
    }

    async fn slice_state(&self) -> Result<SliceState, DriverError> {
        parse_slice_state(&self.call(DriverTool::SliceGetState, json!({})).await?)
    }

    async fn preview_summary(
        &self,
        plate_index: u32,
        plate_revision: &str,
        slice_result_id: &str,
    ) -> Result<SliceSummary, DriverError> {
        let args = json!({
            "plateIndex": plate_index,
            "expectedPlateRevision": plate_revision,
            "sliceResultId": slice_result_id,
        });
        let raw = self.call(DriverTool::PreviewGetSummary, args).await?;
        let warnings = raw
            .get("warnings")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|w| match w {
                        Value::String(s) => Some(s.clone()),
                        other => opt_str(other, "message"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // The driver exposes no print time, filament usage or support volume
        // here (print time comes from `slice_state`).
        Ok(SliceSummary {
            print_time_s: None,
            filaments: Vec::new(),
            support_volume_mm3: None,
            warnings,
            raw,
        })
    }

    async fn export_gcode(
        &self,
        plate_index: u32,
        plate_revision: &str,
        slice_result_id: &str,
        path: &std::path::Path,
    ) -> Result<DriverJob, DriverError> {
        let path = path.to_str().ok_or_else(|| DriverError::InvalidParams {
            message: "export path is not valid UTF-8".to_string(),
        })?;
        let args = json!({
            "plateIndex": plate_index,
            "expectedPlateRevision": plate_revision,
            "sliceResultId": slice_result_id,
            "path": path,
            "overwrite": false,
        });
        parse_job(&self.call(DriverTool::ExportGcode, args).await?)
    }

    async fn plate_create(&self) -> Result<PlateList, DriverError> {
        parse_plate_list(&self.call(DriverTool::PlateCreate, json!({})).await?)
    }

    async fn plate_rename(
        &self,
        plate_index: u32,
        plate_revision: &str,
        name: &str,
    ) -> Result<PlateList, DriverError> {
        let args = json!({
            "plateIndex": plate_index,
            "expectedPlateRevision": plate_revision,
            "name": name,
        });
        parse_plate_list(&self.call(DriverTool::PlateRename, args).await?)
    }

    async fn plate_delete(
        &self,
        plate_index: u32,
        plate_revision: &str,
        contents: PlateContents,
        target_plate_index: Option<u32>,
    ) -> Result<PlateDeleted, DriverError> {
        let mut args = Map::new();
        args.insert("plateIndex".into(), json!(plate_index));
        args.insert("expectedPlateRevision".into(), json!(plate_revision));
        args.insert("contents".into(), json!(contents.wire()));
        if let Some(t) = target_plate_index {
            args.insert("targetPlateIndex".into(), json!(t));
        }
        let v = self
            .call(DriverTool::PlateDelete, Value::Object(args))
            .await?;
        Ok(PlateDeleted {
            plates: parse_plate_list(&v)?,
            moved_instances: u32_of(&v, "movedInstanceCount")?,
            deleted_objects: u32_of(&v, "deletedObjectCount")?,
            destination_plate_index: v
                .get("destinationPlateIndex")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
        })
    }

    async fn plate_move_instance(
        &self,
        object_id: &str,
        instance_id: &str,
        plate_index: u32,
        plate_revision: &str,
    ) -> Result<PlateList, DriverError> {
        let args = json!({
            "objectId": object_id,
            "instanceId": instance_id,
            "plateIndex": plate_index,
            "expectedPlateRevision": plate_revision,
        });
        parse_plate_list(&self.call(DriverTool::PlateMoveInstance, args).await?)
    }

    async fn filament_assign(
        &self,
        object_id: &str,
        filament_index: u32,
    ) -> Result<FilamentAssignment, DriverError> {
        let args = json!({"objectId": object_id, "filamentIndex": filament_index});
        let v = self.call(DriverTool::FilamentAssign, args).await?;
        Ok(FilamentAssignment {
            object_id: str_of(field(&v, "target")?, "objectId")?,
            filament_index: u32_of(&v, "filamentIndex")?,
            preset: str_of(&v, "preset")?,
        })
    }

    async fn arrange_start(&self) -> Result<DriverJob, DriverError> {
        parse_job(&self.call(DriverTool::ArrangeStart, json!({})).await?)
    }

    async fn device_list(&self) -> Result<Vec<DeviceSummary>, DriverError> {
        let v = self.call(DriverTool::DeviceList, json!({})).await?;
        array_of(&v, "devices")?
            .iter()
            .map(|d| {
                Ok(DeviceSummary {
                    device_id: str_of(d, "deviceId")?,
                    name: opt_str(d, "name").unwrap_or_default(),
                    selected: d.get("selected").and_then(Value::as_bool).unwrap_or(false),
                    online: d.get("online").and_then(Value::as_bool).unwrap_or(false),
                    connected: d.get("connected").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect()
    }

    async fn ams_inventory(&self, device_id: &str) -> Result<AmsInventory, DriverError> {
        let v = self
            .call(DriverTool::AmsInventory, json!({"deviceId": device_id}))
            .await?;
        parse_ams_inventory(&v)
    }

    async fn project_save(
        &self,
        path: &std::path::Path,
        overwrite: bool,
    ) -> Result<(), DriverError> {
        let path = utf8_path(path, "project")?;
        self.call(
            DriverTool::ProjectSave,
            json!({"path": path, "overwrite": overwrite}),
        )
        .await
        .map(|_| ())
    }

    async fn export_sliced_3mf(
        &self,
        plate_index: u32,
        plate_revision: &str,
        slice_result_id: &str,
        path: &std::path::Path,
        overwrite: bool,
    ) -> Result<ExportedPlate, DriverError> {
        let path = utf8_path(path, "export")?;
        let args = json!({
            "plateIndex": plate_index,
            "expectedPlateRevision": plate_revision,
            "sliceResultId": slice_result_id,
            "path": path,
            "overwrite": overwrite,
        });
        let v = self.call(DriverTool::ExportSliced3mf, args).await?;
        Ok(ExportedPlate {
            slice_result_id: str_of(&v, "sliceResultId")?,
            plate_index: u32_of(&v, "plateIndex")?,
            path: str_of(&v, "path")?,
            bytes: field(&v, "bytes")?
                .as_u64()
                .ok_or_else(|| proto("field `bytes` is not an unsigned integer"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dhms_parsing() {
        assert_eq!(parse_dhms("1h 23m 45s"), Some(5025.0));
        assert_eq!(parse_dhms("2d 1h"), Some(176_400.0));
        assert_eq!(parse_dhms("45s"), Some(45.0));
        assert_eq!(parse_dhms("N/A"), None);
        assert_eq!(parse_dhms(""), None);
        assert_eq!(parse_dhms("12"), None);
    }

    #[test]
    fn sse_first_event() {
        assert_eq!(
            first_sse_data("event: m\ndata: {\"a\":1}\n\ndata: {\"b\":2}\n\n"),
            "{\"a\":1}"
        );
    }
}
