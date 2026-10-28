// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read-only tools.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::args::{GetSettingsArgs, SEARCH_DEFAULT_LIMIT, SEARCH_MAX_LIMIT, SearchArgs};
use super::{TargetRef, Tools, driver_error};
use crate::build_info;
use crate::catalog::OptionDef;
use crate::detect::{StudioStatus, build_identity, version_match};
use crate::driver::map::{OVERRIDE_KEYS, WRITABLE_KEYS};
use crate::driver::{AppState, EffectiveSetting, ModelObject, SettingSource, SettingsTarget};
use crate::errors::{ToolError, ToolErrorCode};
use crate::validate::Scope;

fn models_json(models: &[ModelObject]) -> Vec<Value> {
    models
        .iter()
        .map(|m| {
            let instances: Vec<Value> = m
                .instances
                .iter()
                .map(|i| {
                    json!({
                        "instance_id": i.id,
                        "plate_id": i.plate_index,
                        "position_mm": i.position_mm,
                        "rotation_deg": i.rotation_deg,
                        "scale": i.scale,
                    })
                })
                .collect();
            json!({"object_id": m.id, "name": m.name, "instances": instances})
        })
        .collect()
}

fn origin(s: SettingSource) -> &'static str {
    match s {
        SettingSource::Project => "project",
        SettingSource::Plate => "plate",
        SettingSource::Object => "object",
        SettingSource::Volume => "volume",
    }
}

fn setting_json(s: &EffectiveSetting, def: Option<&OptionDef>, writable: bool) -> Value {
    let enum_values = def
        .map(|d| {
            if d.serialized_enum_values.is_empty() {
                &d.enum_values
            } else {
                &d.serialized_enum_values
            }
        })
        .filter(|v| !v.is_empty());
    json!({
        "key": s.key,
        "value": s.value,
        "type": def.map(|d| d.kind.as_str()),
        "units": def.and_then(|d| d.units.clone()),
        "enum": enum_values,
        "min": def.and_then(|d| d.min),
        "max": def.and_then(|d| d.max),
        "origin": origin(s.source),
        "writable_by_driver": writable,
    })
}

fn invalid(msg: &str) -> ToolError {
    ToolError::new(ToolErrorCode::InvalidArgument, msg)
}

impl Tools {
    /// Never fails: works with Studio closed.
    pub async fn bamburino_capabilities(&self) -> Value {
        self.revision.touch_client();
        let mut driver = match self.probe.status().await {
            StudioStatus::NotRunning => json!({"status": "not_running"}),
            StudioStatus::DriverOff => json!({"status": "driver_off"}),
            StudioStatus::Unauthorized => json!({"status": "unauthorized"}),
            StudioStatus::TokenMissing => json!({"status": "token_missing"}),
            StudioStatus::ProcessUnknown => json!({
                "status": "driver_error",
                "message": "the driver is unreachable and the process list could not be inspected",
            }),
            StudioStatus::DriverFault {
                code: ToolErrorCode::Busy,
                message,
                ..
            } => json!({"status": "busy", "message": message}),
            StudioStatus::DriverFault { message, .. } => {
                json!({"status": "driver_error", "message": message})
            }
            StudioStatus::Ready { app_version, .. } => {
                let mut v = json!({"status": "ready", "app_version": app_version});
                if let Ok(caps) = self.driver.capabilities().await {
                    let id = build_identity(&caps);
                    v["build_suffix"] = json!(caps.build_suffix);
                    v["patch_id"] = json!(id.patch_id);
                    v["version_match"] = json!(version_match(&id));
                }
                v
            }
        };
        driver["expected_studio_version"] = json!(build_info::STUDIO_VERSION);
        driver["expected_patch_id"] = json!(build_info::EXPECTED_PATCH_ID);
        json!({
            "bamburino_version": build_info::BAMBURINO_VERSION,
            "catalog": {
                "studio_version": self.catalog.studio_version,
                "source_revision": self.catalog.source_revision,
                "keys": self.catalog.options.len(),
            },
            "driver": driver,
            "tools": Self::NAMES,
            "physical_tools": false,
        })
    }

    /// Requires a ready Studio, then refreshes the revision. The revision is
    /// refreshed before the data is read: data read later than the revision
    /// can only make a following write conflict, never slip through.
    pub(super) async fn ready_with_revision(&self) -> Result<(AppState, u64), ToolError> {
        self.revision.touch_client();
        let app = self
            .probe
            .require_ready(false, self.cfg.allow_version_mismatch)
            .await?;
        let revision = self
            .revision
            .refresh(self.driver.as_ref())
            .await
            .map_err(driver_error)?;
        Ok((app, revision))
    }

    pub async fn studio_get_state(&self) -> Result<Value, ToolError> {
        let (app, revision) = self.ready_with_revision().await?;
        let plates = self.driver.plate_list().await.map_err(driver_error)?;
        let models = self.driver.model_list().await.map_err(driver_error)?;
        let presets = self.driver.active_presets().await.map_err(driver_error)?;
        let slice = self.driver.slice_state().await.map_err(driver_error)?;
        let plates_json: Vec<Value> = plates
            .plates
            .iter()
            .map(|p| {
                json!({
                    "plate_id": p.index,
                    "name": p.name,
                    "selected": p.index == plates.selected_index,
                    "locked": p.locked,
                })
            })
            .collect();
        Ok(json!({
            // The running Studio instance (brief 5): `session_id` changes when
            // Studio restarts.
            "instance": {
                "session_id": app.session_id,
                "app_version": app.app_version,
            },
            "project": {
                "name": app.project_name,
                "path": app.project_path,
                "dirty": app.dirty,
                "presets_dirty": app.presets_dirty,
            },
            "plates": plates_json,
            "models": models_json(&models),
            "presets": {
                "printer": presets.printer,
                "process": presets.process,
                "filaments": presets.filaments,
            },
            "slice": {
                "running": slice.running,
                "valid": slice.valid,
                "slice_result_id": slice.slice_result_id,
            },
            "busy": {
                "modal_open": app.modal_open,
                "ui_job_running": app.ui_job_running,
            },
            "revision": revision,
        }))
    }

    /// `origin` "project" is the global project config (presets plus unsaved
    /// changes); "plate", "object" and "volume" are overrides.
    pub async fn studio_get_settings(&self, a: GetSettingsArgs) -> Result<Value, ToolError> {
        // Argument errors do not depend on Studio: report them first.
        let target = a.target.unwrap_or(TargetRef {
            plate_id: None,
            object_id: None,
        });
        match a.scope {
            Scope::Plate if target.plate_id.is_none() => {
                return Err(invalid("scope plate requires target.plate_id"));
            }
            Scope::Object if target.object_id.is_none() => {
                return Err(invalid("scope object requires target.object_id"));
            }
            _ => {}
        }
        let (_, revision) = self.ready_with_revision().await?;
        let overridable =
            || -> BTreeSet<String> { OVERRIDE_KEYS.iter().map(|k| k.to_string()).collect() };
        let (driver_target, target_json, writable) = match (a.scope, target) {
            (
                Scope::Plate,
                TargetRef {
                    plate_id: Some(id), ..
                },
            ) => {
                let plates = self.driver.plate_list().await.map_err(driver_error)?;
                if !plates.plates.iter().any(|p| p.index == id) {
                    return Err(ToolError::new(
                        ToolErrorCode::PlateNotFound,
                        format!("plate {id} does not exist"),
                    ));
                }
                (
                    SettingsTarget::Plate {
                        index: id,
                        plate_revision: plates.plate_revision,
                    },
                    json!({"plate_id": id}),
                    overridable(),
                )
            }
            (
                Scope::Object,
                TargetRef {
                    object_id: Some(id),
                    ..
                },
            ) => {
                let models = self.driver.model_list().await.map_err(driver_error)?;
                if !models.iter().any(|m| m.id == id) {
                    return Err(ToolError::new(
                        ToolErrorCode::ObjectNotFound,
                        format!("object {id} does not exist"),
                    ));
                }
                (
                    SettingsTarget::Object {
                        object_id: id.clone(),
                    },
                    json!({"object_id": id}),
                    overridable(),
                )
            }
            _ => (
                SettingsTarget::Project,
                Value::Null,
                self.driver.writable_keys().await.map_err(driver_error)?,
            ),
        };
        let effective = self
            .driver
            .settings_effective(&driver_target, None)
            .await
            .map_err(driver_error)?;
        let settings: Vec<Value> = effective
            .iter()
            .map(|s| setting_json(s, self.catalog.get(&s.key), writable.contains(&s.key)))
            .collect();
        Ok(json!({
            "scope": a.scope,
            "target": target_json,
            "revision": revision,
            "settings": settings,
        }))
    }

    pub async fn studio_list_models(&self) -> Result<Value, ToolError> {
        let (_, revision) = self.ready_with_revision().await?;
        let models = self.driver.model_list().await.map_err(driver_error)?;
        Ok(json!({"models": models_json(&models), "revision": revision}))
    }

    /// Does not need Studio. `writable_by_driver` follows the driver's static
    /// whitelist for project settings.
    pub async fn catalog_search(&self, a: SearchArgs) -> Value {
        self.revision.touch_client();
        let limit = a
            .limit
            .unwrap_or(SEARCH_DEFAULT_LIMIT)
            .clamp(1, SEARCH_MAX_LIMIT);
        let hits: Vec<Value> = self
            .catalog
            .search(&a.query, limit as usize)
            .into_iter()
            .map(|h| {
                json!({
                    "key": h.key,
                    "label": h.label,
                    "kind": h.kind,
                    "units": h.units,
                    "scopes": h.scopes,
                    "score": h.score,
                    "writable_by_driver": WRITABLE_KEYS.contains(&h.key.as_str()),
                })
            })
            .collect();
        json!({"query": a.query, "hits": hits})
    }
}
