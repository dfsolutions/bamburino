// SPDX-License-Identifier: AGPL-3.0-or-later
//! Global revision of the project state (design 13.2), used for optimistic
//! concurrency between the agent and the user's manual edits in Bambu Studio.
//!
//! The revision is a counter bumped whenever a fingerprint of the state read
//! from the driver changes. Two signals feed the fingerprint:
//!
//! * variant (a): when the driver reports `undo_snapshot` (plan A patch 0120,
//!   the id of the active undo snapshot) it is hashed in and is the primary
//!   signal: every snapshotting user edit, undo and redo changes it;
//! * variant (b): when it is `None`, only the content hash of the state
//!   detects changes. The variant is chosen per read, by the field's presence.
//!
//! The hashed content is: app state (including `dirty`, `presets_dirty` and
//! `project_path` on purpose, so a Ctrl+S bumps the revision), active presets,
//! models (id, name, dimensions, instance transforms rounded to 1e-6), plates
//! without `plate_revision`, the effective project settings, and the
//! effective settings of every object and every plate (so that a manual
//! per-object or per-plate override is seen even without `undo_snapshot`).
//! Cost: one `refresh` makes a fixed number of reads plus one
//! `settings_effective` call per object and per plate, so it scales linearly
//! with the number of objects and plates.
//!
//! # Revision values
//!
//! A revision is `(process epoch in ms << 20) | sequence`, the sequence being
//! strictly increasing inside the process. Restarting the service therefore
//! never reuses revisions of a previous run, and a stale `expected_revision`
//! from before the restart cannot match. [`RevisionState::current`] is 0
//! before the first successful `refresh`.
//!
//! # Race window (contract for the mutation pipeline, Task 10)
//!
//! [`RevisionState::refresh`] observes the state at one instant. There are
//! two gaps:
//!
//! 1. A manual edit made between the `refresh` that precedes a mutation and
//!    the moment the mutation is applied is not seen, so the agent may
//!    overwrite it. The poller narrows this gap to `every` while clients are
//!    connected but cannot close it. Under variant (b) an edit that leaves
//!    every hashed value identical is invisible as well.
//! 2. A user edit landing between the agent's own mutation and the
//!    post-mutation `refresh` is absorbed into the agent's bump. Accepted for
//!    the MVP.
//!
//! Therefore the mutation pipeline MUST call `refresh` immediately after the
//! mutation returns, so that the new revision reflects the mutation and is
//! not mistaken for a user edit later. When `undo_snapshot` is available
//! (patch 0120), it MUST also check that the snapshot id advanced by exactly
//! one snapshot between the pre- and post-mutation reads, and report a
//! conflict otherwise.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::driver::{
    ActivePresets, AppState, DriverClient, DriverError, EffectiveSetting, ModelObject, PlateList,
    SettingSource, SettingsTarget,
};

/// Default polling period.
pub const DEFAULT_POLL_EVERY: Duration = Duration::from_millis(1500);
/// Default time without client activity after which polling stops.
pub const DEFAULT_IDLE_AFTER: Duration = Duration::from_secs(60);

const SEQ_BITS: u32 = 20;
const SEQ_MASK: u64 = (1 << SEQ_BITS) - 1;

/// Effective settings of one object or plate, keyed `object:<id>` or
/// `plate:<index>`.
pub type ScopedSettings = (String, Vec<EffectiveSetting>);

/// Revision counter plus the fingerprint it was last bumped for.
pub struct RevisionState {
    /// `(sequence, last fingerprint)`; sequence 0 means never observed.
    inner: Mutex<(u64, Option<[u8; 32]>)>,
    /// Serializes whole refreshes (reads + compare + bump).
    refresh_lock: tokio::sync::Mutex<()>,
    epoch_ms: u64,
    /// Milliseconds since `origin` of the last client request, -1 if none.
    last_client_activity: AtomicI64,
    origin: Instant,
    poll_errors: AtomicU32,
    last_poll_error: Mutex<Option<String>>,
    stop: Notify,
}

impl Default for RevisionState {
    fn default() -> Self {
        Self::new()
    }
}

impl RevisionState {
    /// Seeds the revision epoch with the current wall-clock time.
    pub fn new() -> Self {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self::with_epoch(ms)
    }

    /// Like [`Self::new`] with an explicit epoch (milliseconds).
    pub fn with_epoch(epoch_ms: u64) -> Self {
        Self {
            inner: Mutex::new((0, None)),
            refresh_lock: tokio::sync::Mutex::new(()),
            epoch_ms,
            last_client_activity: AtomicI64::new(-1),
            origin: Instant::now(),
            poll_errors: AtomicU32::new(0),
            last_poll_error: Mutex::new(None),
            stop: Notify::new(),
        }
    }

    /// Current revision, 0 if the state was never observed.
    pub fn current(&self) -> u64 {
        self.value(self.lock().0)
    }

    fn value(&self, seq: u64) -> u64 {
        if seq == 0 {
            0
        } else {
            (self.epoch_ms << SEQ_BITS) | (seq & SEQ_MASK)
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (u64, Option<[u8; 32]>)> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads the driver state, fingerprints it and bumps the revision if the
    /// fingerprint changed. The first successful call yields the first
    /// revision of this process. Calls are serialized.
    pub async fn refresh(&self, driver: &dyn DriverClient) -> Result<u64, DriverError> {
        let _serial = self.refresh_lock.lock().await;
        let mut attempt = 0;
        let fp = loop {
            match read_fingerprint(driver).await {
                Ok(fp) => break fp,
                // A plate revision went stale between the plate read and the
                // per-plate settings read (user edit in between): re-read.
                Err(DriverError::StaleReference { .. }) if attempt < 2 => attempt += 1,
                Err(e) => return Err(e),
            }
        };
        let mut g = self.lock();
        if g.1 != Some(fp) {
            g.0 += 1;
            g.1 = Some(fp);
        }
        Ok(self.value(g.0))
    }

    /// Records client activity; called on every MCP request.
    pub fn touch_client(&self) {
        let ms = self.origin.elapsed().as_millis() as i64;
        self.last_client_activity.store(ms, Ordering::Relaxed);
    }

    fn client_active_within(&self, idle_after: Duration) -> bool {
        let last = self.last_client_activity.load(Ordering::Relaxed);
        if last < 0 {
            return false;
        }
        let now = self.origin.elapsed().as_millis() as i64;
        now - last <= idle_after.as_millis() as i64
    }

    /// Number of consecutive failed polls (0 after any success).
    pub fn poll_errors(&self) -> u32 {
        self.poll_errors.load(Ordering::Relaxed)
    }

    /// Message of the last failed poll, cleared by the next success.
    pub fn last_poll_error(&self) -> Option<String> {
        self.last_poll_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Asks the poller to exit (alternative to aborting its handle). Safe to
    /// call before the poller starts waiting.
    pub fn stop_poller(&self) {
        self.stop.notify_one();
    }

    fn record_poll(&self, result: Result<u64, DriverError>) {
        let mut last = self
            .last_poll_error
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(_) => {
                self.poll_errors.store(0, Ordering::Relaxed);
                *last = None;
            }
            Err(e) => {
                let n = self.poll_errors.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::warn!(poll_errors = n, error = %e, "revision poll failed");
                *last = Some(e.to_string());
            }
        }
    }

    /// Polls the driver every `every` while a client was active within the
    /// last `idle_after`; sleeps without calling the driver otherwise. Driver
    /// errors are counted (see [`Self::poll_errors`]) and the next tick
    /// retries. Stops on [`Self::stop_poller`] or when the handle is aborted.
    pub fn spawn_poller(
        self: Arc<Self>,
        driver: Arc<dyn DriverClient>,
        every: Duration,
        idle_after: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = self.stop.notified() => return,
                    _ = tokio::time::sleep(every) => {}
                }
                if self.client_active_within(idle_after) {
                    let result = self.refresh(driver.as_ref()).await;
                    self.record_poll(result);
                }
            }
        })
    }
}

async fn read_fingerprint(driver: &dyn DriverClient) -> Result<[u8; 32], DriverError> {
    let app = driver.app_state().await?;
    let presets = driver.active_presets().await?;
    let models = driver.model_list().await?;
    let plates = driver.plate_list().await?;
    let settings = driver
        .settings_effective(&SettingsTarget::Project, None)
        .await?;
    let mut scoped: Vec<ScopedSettings> = Vec::new();
    for m in &models {
        let target = SettingsTarget::Object {
            object_id: m.id.clone(),
        };
        let s = driver.settings_effective(&target, None).await?;
        scoped.push((format!("object:{}", m.id), s));
    }
    for p in &plates.plates {
        let target = SettingsTarget::Plate {
            index: p.index,
            plate_revision: plates.plate_revision.clone(),
        };
        let s = driver.settings_effective(&target, None).await?;
        scoped.push((format!("plate:{}", p.index), s));
    }
    Ok(fingerprint(
        &app, &presets, &models, &plates, &settings, &scoped,
    ))
}

fn settings_json(settings: &[EffectiveSetting]) -> Vec<Value> {
    settings
        .iter()
        .map(|s| {
            let source = match s.source {
                SettingSource::Project => "project",
                SettingSource::Plate => "plate",
                SettingSource::Object => "object",
                SettingSource::Volume => "volume",
            };
            json!({"key": s.key, "value": s.value, "source": source})
        })
        .collect()
}

fn round6(x: f64) -> i64 {
    (x * 1e6).round() as i64
}

fn round3(v: [f64; 3]) -> Value {
    json!([round6(v[0]), round6(v[1]), round6(v[2])])
}

/// SHA-256 of the canonical JSON (object keys sorted) of the state, without
/// the volatile fields (`session_id`, `ui_job_running`, `slicing_running`,
/// `modal_open`, `plate_revision`). `scoped_settings` carries the effective
/// settings of each object and plate.
pub fn fingerprint(
    app: &AppState,
    presets: &ActivePresets,
    models: &[ModelObject],
    plates: &PlateList,
    settings: &[EffectiveSetting],
    scoped_settings: &[ScopedSettings],
) -> [u8; 32] {
    let models: Vec<Value> = models
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "name": m.name,
                "dimensions": round3(m.dimensions_mm),
                "instances": m.instances.iter().map(|i| json!({
                    "id": i.id,
                    "position": round3(i.position_mm),
                    "rotation": round3(i.rotation_deg),
                    "scale": round3(i.scale),
                    "plate": i.plate_index,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let scoped: Vec<Value> = scoped_settings
        .iter()
        .map(|(scope, s)| json!({"scope": scope, "settings": settings_json(s)}))
        .collect();
    let doc = json!({
        "app": {
            "app_version": app.app_version,
            "project_name": app.project_name,
            "project_path": app.project_path,
            "dirty": app.dirty,
            "presets_dirty": app.presets_dirty,
            "object_count": app.object_count,
            "plate_count": app.plate_count,
            "selected_plate_index": app.selected_plate_index,
            "undo_snapshot": app.undo_snapshot,
        },
        "presets": {
            "printer": presets.printer,
            "process": presets.process,
            "filaments": presets.filaments,
        },
        "models": models,
        "plates": {
            "selected_index": plates.selected_index,
            "plates": plates.plates.iter().map(|p| json!({
                "index": p.index, "name": p.name, "locked": p.locked,
            })).collect::<Vec<_>>(),
        },
        "settings": settings_json(settings),
        "scoped_settings": scoped,
    });
    let mut out = String::new();
    canonical(&doc, &mut out);
    Sha256::digest(out.as_bytes()).into()
}

/// Writes `v` as JSON with object keys in sorted order, independent of the
/// map implementation behind `serde_json::Value`.
fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical(&m[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}
