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
//!   (app state, active presets, models, plates, effective project settings)
//!   detects changes. The variant is chosen per read, by the field's presence.
//!
//! Race window: [`RevisionState::refresh`] observes the state at one instant.
//! A manual edit made between the `refresh` that precedes a mutation and the
//! moment the mutation is applied is not seen, so the agent may overwrite it.
//! The polling task narrows the window to `every` (1-2 s) while clients are
//! connected but cannot close it. Under variant (b) an edit that leaves every
//! hashed value identical (for example an edit immediately reverted by hand
//! between two reads) is invisible as well. After an own mutation the caller
//! must `refresh` so that the revision reflects the mutation and is not
//! mistaken for a user edit later.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::Instant;

use crate::driver::{
    ActivePresets, AppState, DriverClient, DriverError, EffectiveSetting, ModelObject, PlateList,
    SettingSource, SettingsTarget,
};

/// Default polling period.
pub const DEFAULT_POLL_EVERY: Duration = Duration::from_millis(1500);
/// Default time without client activity after which polling stops.
pub const DEFAULT_IDLE_AFTER: Duration = Duration::from_secs(60);

/// Revision counter plus the fingerprint it was last bumped for.
pub struct RevisionState {
    /// `(revision, last fingerprint)`; revision 0 means never observed.
    inner: Mutex<(u64, Option<[u8; 32]>)>,
    /// Milliseconds since `origin` of the last client request, -1 if none.
    last_client_activity: AtomicI64,
    origin: Instant,
}

impl Default for RevisionState {
    fn default() -> Self {
        Self::new()
    }
}

impl RevisionState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new((0, None)),
            last_client_activity: AtomicI64::new(-1),
            origin: Instant::now(),
        }
    }

    pub fn current(&self) -> u64 {
        self.lock().0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (u64, Option<[u8; 32]>)> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads the driver state, fingerprints it and bumps the revision if the
    /// fingerprint changed. The first successful call yields revision 1.
    pub async fn refresh(&self, driver: &dyn DriverClient) -> Result<u64, DriverError> {
        let app = driver.app_state().await?;
        let presets = driver.active_presets().await?;
        let models = driver.model_list().await?;
        let plates = driver.plate_list().await?;
        let settings = driver
            .settings_effective(&SettingsTarget::Project, None)
            .await?;
        let fp = fingerprint(&app, &presets, &models, &plates, &settings);
        let mut g = self.lock();
        if g.1 != Some(fp) {
            g.0 += 1;
            g.1 = Some(fp);
        }
        Ok(g.0)
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

    /// Polls the driver every `every` while a client was active within the
    /// last `idle_after`; sleeps without calling the driver otherwise. Driver
    /// errors are ignored (the next tick retries). Abort the handle to stop.
    pub fn spawn_poller(
        self: Arc<Self>,
        driver: Arc<dyn DriverClient>,
        every: Duration,
        idle_after: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                if self.client_active_within(idle_after) {
                    let _ = self.refresh(driver.as_ref()).await;
                }
            }
        })
    }
}

fn round6(x: f64) -> i64 {
    (x * 1e6).round() as i64
}

fn round3(v: [f64; 3]) -> Value {
    json!([round6(v[0]), round6(v[1]), round6(v[2])])
}

/// SHA-256 of the canonical JSON (object keys sorted) of the state, without
/// the volatile fields (`session_id`, `ui_job_running`, `slicing_running`,
/// `modal_open`, `plate_revision`).
pub fn fingerprint(
    app: &AppState,
    presets: &ActivePresets,
    models: &[ModelObject],
    plates: &PlateList,
    settings: &[EffectiveSetting],
) -> [u8; 32] {
    let models: Vec<Value> = models
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
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
    let settings: Vec<Value> = settings
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
        "settings": settings,
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
