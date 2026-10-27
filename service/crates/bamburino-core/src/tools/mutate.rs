// SPDX-License-Identifier: AGPL-3.0-or-later
//! The mutation pipeline shared by every tool that changes Studio, and
//! `studio_set_settings`.
//!
//! Pipeline of [`Tools::guarded_mutation`] (serialized by a mutex):
//!
//! 1. Studio is ready (writes also need the pinned build);
//! 2. no modal dialog, UI job or slice is running, else `BUSY` without any
//!    write call;
//! 3. `refresh` the revision and compare it with `expected_revision`, else
//!    `REVISION_CONFLICT` with the current state;
//! 4. run the mutation;
//! 5. `refresh` immediately, so that the mutation is not mistaken for a user
//!    edit later (see the "Race window" contract in `revision`);
//! 6. when the driver reports `undo_snapshot`, it must have advanced by one
//!    snapshot per successful mutating driver call, else `REVISION_CONFLICT`
//!    (something else edited the project while the mutation ran).

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};

use super::args::{Change, SetSettingsArgs};
use super::{Tools, driver_error};
use crate::driver::map::OVERRIDE_KEYS;
use crate::driver::{
    AbsoluteTransform, ActivePresets, AppState, DriverCapabilities, DriverClient, DriverError,
    DriverJob, EffectiveSetting, ModelObject, PlateList, SettingsTarget, SliceState, SliceSummary,
};
use crate::errors::{ToolError, ToolErrorCode};
use crate::validate::{Scope, validate_change};

/// Maximum number of changes of one `studio_set_settings` call.
const MAX_CHANGES: usize = 100;

/// Driver decorator that counts the successful calls that take an undo
/// snapshot, so the pipeline knows how far `undo_snapshot` must advance.
struct CountingDriver {
    inner: Arc<dyn DriverClient>,
    snapshots: AtomicU32,
}

impl CountingDriver {
    fn bump<T>(&self, r: Result<T, DriverError>) -> Result<T, DriverError> {
        if r.is_ok() {
            self.snapshots.fetch_add(1, Ordering::SeqCst);
        }
        r
    }
}

#[async_trait::async_trait]
impl DriverClient for CountingDriver {
    async fn app_state(&self) -> Result<AppState, DriverError> {
        self.inner.app_state().await
    }
    async fn capabilities(&self) -> Result<DriverCapabilities, DriverError> {
        self.inner.capabilities().await
    }
    async fn plate_list(&self) -> Result<PlateList, DriverError> {
        self.inner.plate_list().await
    }
    async fn plate_select(&self, i: u32, r: &str) -> Result<PlateList, DriverError> {
        self.bump(self.inner.plate_select(i, r).await)
    }
    async fn model_list(&self) -> Result<Vec<ModelObject>, DriverError> {
        self.inner.model_list().await
    }
    async fn active_presets(&self) -> Result<ActivePresets, DriverError> {
        self.inner.active_presets().await
    }
    async fn writable_keys(&self) -> Result<BTreeSet<String>, DriverError> {
        self.inner.writable_keys().await
    }
    async fn settings_effective(
        &self,
        target: &SettingsTarget,
        keys: Option<&[String]>,
    ) -> Result<Vec<EffectiveSetting>, DriverError> {
        self.inner.settings_effective(target, keys).await
    }
    async fn settings_update(&self, values: &BTreeMap<String, String>) -> Result<(), DriverError> {
        self.bump(self.inner.settings_update(values).await)
    }
    async fn settings_override(
        &self,
        target: &SettingsTarget,
        values: &BTreeMap<String, String>,
    ) -> Result<PlateList, DriverError> {
        self.bump(self.inner.settings_override(target, values).await)
    }
    async fn object_transform(
        &self,
        o: &str,
        i: &str,
        t: &AbsoluteTransform,
    ) -> Result<ModelObject, DriverError> {
        self.bump(self.inner.object_transform(o, i, t).await)
    }
    async fn slice_start(&self, p: u32, r: &str) -> Result<DriverJob, DriverError> {
        self.inner.slice_start(p, r).await
    }
    async fn job_get(&self, id: &str) -> Result<DriverJob, DriverError> {
        self.inner.job_get(id).await
    }
    async fn job_cancel(&self, id: &str) -> Result<DriverJob, DriverError> {
        self.inner.job_cancel(id).await
    }
    async fn slice_state(&self) -> Result<SliceState, DriverError> {
        self.inner.slice_state().await
    }
    async fn preview_summary(&self, p: u32, r: &str, s: &str) -> Result<SliceSummary, DriverError> {
        self.inner.preview_summary(p, r, s).await
    }
}

/// Maps a failure of a driver call that reads. A stale reference means the
/// user edited the project in the meantime.
pub(crate) fn read_error(e: DriverError) -> ToolError {
    match e {
        DriverError::StaleReference { message } => {
            ToolError::new(ToolErrorCode::RevisionConflict, message)
                .with_details(json!({ "reason": "stale_reference" }))
        }
        other => driver_error(other),
    }
}

/// Maps a failure of a driver call that writes. A timeout leaves the outcome
/// unknown: the write may still complete in Studio.
pub(crate) fn write_error(e: DriverError) -> ToolError {
    match e {
        DriverError::Timeout { after_ms } => ToolError::new(
            ToolErrorCode::Busy,
            format!(
                "Bambu Studio did not answer within {after_ms} ms; the change may or may not have been applied. Read the state again before retrying."
            ),
        )
        .with_details(json!({ "reason": "driver_timeout", "outcome": "unknown" })),
        DriverError::Busy { message } => ToolError::new(ToolErrorCode::Busy, message)
            .with_details(json!({ "reason": "driver_busy" })),
        other => read_error(other),
    }
}

/// True when `after` is `before` plus `n` snapshots. Ids are opaque; when
/// both are numbers the exact distance is checked, otherwise only that the id
/// changed.
fn advanced_by(before: &str, after: &str, n: u32) -> bool {
    match (before.parse::<u64>(), after.parse::<u64>()) {
        (Ok(b), Ok(a)) => a == b + u64::from(n),
        _ => before != after,
    }
}

fn busy_reason(app: &AppState) -> Option<&'static str> {
    if app.modal_open {
        Some("modal_open")
    } else if app.ui_job_running {
        Some("ui_job_running")
    } else if app.slicing_running {
        Some("slicing_running")
    } else {
        None
    }
}

impl Tools {
    /// Runs `apply` under the mutation pipeline (module docs) and returns its
    /// result with the revision issued right after the mutation.
    pub(crate) async fn guarded_mutation<F, Fut, T>(
        &self,
        expected_revision: u64,
        apply: F,
    ) -> Result<(T, u64), ToolError>
    where
        F: FnOnce(Arc<dyn DriverClient>) -> Fut,
        Fut: Future<Output = Result<T, ToolError>>,
    {
        self.revision.touch_client();
        let _serial = self.write_lock.lock().await;
        let app = self
            .probe
            .require_ready(true, self.cfg.allow_version_mismatch)
            .await?;
        if let Some(reason) = busy_reason(&app) {
            return Err(ToolError::new(
                ToolErrorCode::Busy,
                format!("Bambu Studio is busy ({reason}); nothing was changed."),
            )
            .with_details(json!({ "reason": reason })));
        }
        let current = self
            .revision
            .refresh(self.driver.as_ref())
            .await
            .map_err(read_error)?;
        if current != expected_revision {
            return Err(self
                .conflict(
                    "revision_mismatch",
                    expected_revision,
                    current,
                    "The project changed since `expected_revision`; nothing was written. Review `details.state` and retry with `details.current`.",
                    json!({}),
                )
                .await);
        }
        let before = self
            .driver
            .app_state()
            .await
            .map_err(read_error)?
            .undo_snapshot;

        let counting = Arc::new(CountingDriver {
            inner: self.driver.clone(),
            snapshots: AtomicU32::new(0),
        });
        let value = apply(counting.clone()).await?;

        // Immediately after the mutation: see the race-window contract.
        let unverified = |e: DriverError| {
            let mut err = write_error(e);
            err.details["outcome"] = json!("applied_unverified");
            err
        };
        let revision = self
            .revision
            .refresh(self.driver.as_ref())
            .await
            .map_err(unverified)?;
        let after = self
            .driver
            .app_state()
            .await
            .map_err(unverified)?
            .undo_snapshot;
        let n = counting.snapshots.load(Ordering::SeqCst);
        let ok = match (&before, &after) {
            (Some(b), Some(a)) => advanced_by(b, a, n),
            (Some(_), None) => n == 0,
            // No snapshot id before the call: nothing to compare with.
            (None, _) => true,
        };
        if !ok {
            return Err(self
                .conflict(
                    "undo_snapshot_mismatch",
                    expected_revision,
                    revision,
                    "The change was applied, but the undo history moved by more than the change itself: the project was edited while the change ran. Review `details.state`.",
                    json!({ "undo_snapshot": { "before": before, "after": after, "expected_advance": n } }),
                )
                .await);
        }
        Ok((value, revision))
    }

    async fn conflict(
        &self,
        reason: &str,
        expected: u64,
        current: u64,
        message: &str,
        extra: Value,
    ) -> ToolError {
        let state = self.studio_get_state().await.ok();
        let current = state
            .as_ref()
            .and_then(|s| s["revision"].as_u64())
            .unwrap_or(current);
        let mut details = json!({
            "reason": reason,
            "expected": expected,
            "current": current,
            "state": state,
        });
        if let (Some(d), Some(x)) = (details.as_object_mut(), extra.as_object()) {
            d.extend(x.clone());
        }
        ToolError::new(ToolErrorCode::RevisionConflict, message).with_details(details)
    }

    /// Changes project, plate or object settings: validates everything first,
    /// then writes one driver call per target.
    pub async fn studio_set_settings(&self, a: SetSettingsArgs) -> Result<Value, ToolError> {
        self.revision.touch_client();
        let plan = build_plan(self, &a.changes)?;
        let dry_run = a.dry_run.unwrap_or(false);
        let (from, revision) = self
            .guarded_mutation(a.expected_revision, |drv| {
                self.apply_plan(drv, &plan, dry_run)
            })
            .await?;

        let applied: Vec<Value>;
        let to: Vec<String>;
        if dry_run {
            applied = Vec::new();
            to = plan.items.iter().map(|i| i.native.clone()).collect();
        } else {
            let now = self.read_all(&self.driver, &plan).await.map_err(|mut e| {
                e.details["outcome"] = json!("applied_unverified");
                e.details["revision"] = json!(revision);
                e
            })?;
            to = now;
            applied = plan
                .items
                .iter()
                .zip(&to)
                .map(|(i, v)| i.applied_json(v))
                .collect();
        }
        let diff: Vec<Value> = plan
            .items
            .iter()
            .zip(from.iter().zip(&to))
            .map(|(i, (f, t))| {
                json!({
                    "key": i.key,
                    "scope": i.scope,
                    "target": i.target,
                    "from": f,
                    "to": t,
                    "changed": f != t,
                })
            })
            .collect();
        Ok(json!({
            "applied": applied,
            "diff": diff,
            "revision": revision,
            "dry_run": dry_run,
        }))
    }

    /// Steps 3b..4 of the set-settings flow: writability, the `from` values,
    /// then one write per target. Returns the `from` values aligned to
    /// `plan.items`.
    async fn apply_plan(
        &self,
        drv: Arc<dyn DriverClient>,
        plan: &Plan,
        dry_run: bool,
    ) -> Result<Vec<String>, ToolError> {
        // Writability, against the driver's own list for the project scope.
        let project_keys: BTreeSet<String> = match drv.writable_keys().await {
            Ok(k) => k,
            Err(_) => crate::driver::map::WRITABLE_KEYS
                .iter()
                .map(|k| k.to_string())
                .collect(),
        };
        let mut errors: Vec<(ToolErrorCode, Value)> = Vec::new();
        let mut keys: Vec<&str> = Vec::new();
        for item in &plan.items {
            let writable = match item.scope {
                Scope::Project => project_keys.contains(&item.key),
                _ => OVERRIDE_KEYS.contains(&item.key.as_str()),
            };
            if !writable {
                keys.push(&item.key);
                errors.push((
                    ToolErrorCode::NotWritableByDriver,
                    json!({
                        "index": item.index,
                        "key": item.key,
                        "code": ToolErrorCode::NotWritableByDriver,
                        "message": format!("`{}` cannot be written by the driver at scope `{}`", item.key, scope_name(item.scope)),
                    }),
                ));
            }
        }
        if !errors.is_empty() {
            let mut e = batch_error(errors, plan.items.len());
            e.details["keys"] = json!(keys);
            return Err(e);
        }

        // Current values, read before any write (also checks that every
        // plate and object exists).
        let from = self.read_all(&drv, plan).await?;
        if dry_run {
            return Ok(from);
        }

        for (g, group) in plan.groups.iter().enumerate() {
            let values: BTreeMap<String, String> = plan
                .items
                .iter()
                .filter(|i| i.group == g)
                .map(|i| (i.key.clone(), i.native.clone()))
                .collect();
            if let Err(e) = write_group(drv.as_ref(), group, &values).await {
                let err = write_error(e);
                if g == 0 {
                    return Err(err);
                }
                return Err(self.partial(&drv, plan, g, err).await);
            }
        }
        Ok(from)
    }

    /// Builds the `PARTIALLY_APPLIED` error after group `failed` failed.
    async fn partial(
        &self,
        drv: &Arc<dyn DriverClient>,
        plan: &Plan,
        failed: usize,
        cause: ToolError,
    ) -> ToolError {
        // Absorb the applied part into the revision straight away.
        let revision = self.revision.refresh(self.driver.as_ref()).await.ok();
        let mut applied = Vec::new();
        for g in 0..failed {
            let keys = plan.group_keys(g);
            let now = read_group(drv.as_ref(), plan, g, &keys).await.ok();
            for item in plan.items.iter().filter(|i| i.group == g) {
                let v = now.as_ref().and_then(|m| m.get(&item.key));
                applied.push(match v {
                    Some(v) => item.applied_json(v),
                    None => json!({
                        "key": item.key, "scope": item.scope, "target": item.target,
                        "requested": item.requested, "applied": null,
                    }),
                });
            }
        }
        let not_attempted: Vec<&str> = (failed + 1..plan.groups.len())
            .flat_map(|g| plan.items.iter().filter(move |i| i.group == g))
            .map(|i| i.key.as_str())
            .collect();
        ToolError::new(
            ToolErrorCode::PartiallyApplied,
            "Some changes were applied and a later one failed. Ctrl+Z in Bambu Studio undoes the applied part.",
        )
        .with_details(json!({
            "applied": applied,
            "failed": [{
                "keys": plan.group_keys(failed),
                "target": plan.items.iter().find(|i| i.group == failed).map(|i| i.target.clone()),
                "error": { "code": cause.code, "message": cause.message, "details": cause.details },
            }],
            "not_attempted": not_attempted,
            "revision": revision,
        }))
    }

    /// Effective value of every item, aligned to `plan.items`.
    async fn read_all(
        &self,
        drv: &Arc<dyn DriverClient>,
        plan: &Plan,
    ) -> Result<Vec<String>, ToolError> {
        let mut by_group = Vec::with_capacity(plan.groups.len());
        for g in 0..plan.groups.len() {
            by_group.push(read_group(drv.as_ref(), plan, g, &plan.group_keys(g)).await?);
        }
        plan.items
            .iter()
            .map(|i| {
                by_group[i.group].get(&i.key).cloned().ok_or_else(|| {
                    ToolError::new(
                        ToolErrorCode::DriverError,
                        format!("the driver did not report `{}`", i.key),
                    )
                })
            })
            .collect()
    }
}

fn scope_name(s: Scope) -> &'static str {
    match s {
        Scope::Project => "project",
        Scope::Plate => "plate",
        Scope::Object => "object",
    }
}

/// Where one driver write goes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Group {
    Project,
    Plate(u32),
    Object(String),
}

struct Item {
    /// Position in the request, for error reports.
    index: usize,
    key: String,
    scope: Scope,
    target: Value,
    requested: Value,
    native: String,
    group: usize,
}

impl Item {
    fn applied_json(&self, applied: &str) -> Value {
        json!({
            "key": self.key,
            "scope": self.scope,
            "target": self.target,
            "requested": self.requested,
            "applied": applied,
        })
    }
}

struct Plan {
    items: Vec<Item>,
    groups: Vec<Group>,
}

impl Plan {
    fn group_keys(&self, g: usize) -> Vec<String> {
        self.items
            .iter()
            .filter(|i| i.group == g)
            .map(|i| i.key.clone())
            .collect()
    }
}

fn entry(index: usize, key: &str, e: &ToolError) -> (ToolErrorCode, Value) {
    (
        e.code,
        json!({
            "index": index,
            "key": key,
            "code": e.code,
            "message": e.message,
            "details": e.details,
        }),
    )
}

/// The whole batch is rejected: the code is the common one of the entries,
/// `INVALID_ARGUMENT` when they differ.
fn batch_error(errors: Vec<(ToolErrorCode, Value)>, total: usize) -> ToolError {
    let codes: BTreeSet<String> = errors.iter().map(|(c, _)| format!("{c:?}")).collect();
    let code = if codes.len() == 1 {
        errors[0].0
    } else {
        ToolErrorCode::InvalidArgument
    };
    let n = errors.len();
    let list: Vec<Value> = errors.into_iter().map(|(_, v)| v).collect();
    ToolError::new(
        code,
        format!("{n} of {total} changes rejected; nothing was written."),
    )
    .with_details(json!({ "errors": list }))
}

/// Validates every change and groups them by write target. Nothing touches
/// the driver here.
fn build_plan(tools: &Tools, changes: &[Change]) -> Result<Plan, ToolError> {
    if changes.is_empty() || changes.len() > MAX_CHANGES {
        return Err(ToolError::new(
            ToolErrorCode::InvalidArgument,
            format!("`changes` must contain 1 to {MAX_CHANGES} entries"),
        ));
    }
    let mut errors = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    for (index, c) in changes.iter().enumerate() {
        let scope = c.scope.unwrap_or(Scope::Project);
        let target = c.target.as_ref();
        let (group, target_json) = match scope {
            Scope::Project => (Group::Project, Value::Null),
            Scope::Plate => match target.and_then(|t| t.plate_id) {
                Some(id) => (Group::Plate(id), json!({ "plate_id": id })),
                None => {
                    let e = ToolError::new(
                        ToolErrorCode::InvalidArgument,
                        "scope plate requires target.plate_id",
                    );
                    errors.push(entry(index, &c.key, &e));
                    continue;
                }
            },
            Scope::Object => match target.and_then(|t| t.object_id.clone()) {
                Some(id) => (Group::Object(id.clone()), json!({ "object_id": id })),
                None => {
                    let e = ToolError::new(
                        ToolErrorCode::InvalidArgument,
                        "scope object requires target.object_id",
                    );
                    errors.push(entry(index, &c.key, &e));
                    continue;
                }
            },
        };
        let native = match validate_change(tools.catalog, &c.key, &c.value, scope) {
            Ok(n) => n.0,
            Err(e) => {
                errors.push(entry(index, &c.key, &e));
                continue;
            }
        };
        let g = match groups.iter().position(|x| *x == group) {
            Some(g) => g,
            None => {
                groups.push(group);
                groups.len() - 1
            }
        };
        if items.iter().any(|i| i.group == g && i.key == c.key) {
            let e = ToolError::new(
                ToolErrorCode::InvalidArgument,
                format!("`{}` appears more than once for the same target", c.key),
            );
            errors.push(entry(index, &c.key, &e));
            continue;
        }
        items.push(Item {
            index,
            key: c.key.clone(),
            scope,
            target: target_json,
            requested: c.value.clone(),
            native,
            group: g,
        });
    }
    if !errors.is_empty() {
        return Err(batch_error(errors, changes.len()));
    }
    Ok(Plan { items, groups })
}

/// Effective values of `keys` at the target of group `g`. Plate revisions go
/// stale on every write, so the plate list is read fresh each time.
async fn read_group(
    drv: &dyn DriverClient,
    plan: &Plan,
    g: usize,
    keys: &[String],
) -> Result<BTreeMap<String, String>, ToolError> {
    let target = settings_target(drv, &plan.groups[g])
        .await
        .map_err(|e| match e {
            TargetError::Tool(t) => t,
            TargetError::Driver(d) => read_error(d),
        })?;
    let eff = drv
        .settings_effective(&target, Some(keys))
        .await
        .map_err(read_error)?;
    Ok(eff.into_iter().map(|s| (s.key, s.value)).collect())
}

enum TargetError {
    Tool(ToolError),
    Driver(DriverError),
}

async fn settings_target(
    drv: &dyn DriverClient,
    group: &Group,
) -> Result<SettingsTarget, TargetError> {
    match group {
        Group::Project => Ok(SettingsTarget::Project),
        Group::Plate(id) => {
            let plates = drv.plate_list().await.map_err(TargetError::Driver)?;
            if !plates.plates.iter().any(|p| p.index == *id) {
                return Err(TargetError::Tool(ToolError::new(
                    ToolErrorCode::PlateNotFound,
                    format!("plate {id} does not exist"),
                )));
            }
            Ok(SettingsTarget::Plate {
                index: *id,
                plate_revision: plates.plate_revision,
            })
        }
        Group::Object(id) => {
            let models = drv.model_list().await.map_err(TargetError::Driver)?;
            if !models.iter().any(|m| m.id == *id) {
                return Err(TargetError::Tool(ToolError::new(
                    ToolErrorCode::ObjectNotFound,
                    format!("object {id} does not exist"),
                )));
            }
            Ok(SettingsTarget::Object {
                object_id: id.clone(),
            })
        }
    }
}

async fn write_group(
    drv: &dyn DriverClient,
    group: &Group,
    values: &BTreeMap<String, String>,
) -> Result<(), DriverError> {
    match group {
        Group::Project => drv.settings_update(values).await,
        Group::Plate(index) => {
            let plates = drv.plate_list().await?;
            let target = SettingsTarget::Plate {
                index: *index,
                plate_revision: plates.plate_revision,
            };
            drv.settings_override(&target, values).await.map(|_| ())
        }
        Group::Object(id) => {
            let target = SettingsTarget::Object {
                object_id: id.clone(),
            };
            drv.settings_override(&target, values).await.map(|_| ())
        }
    }
}
