// SPDX-License-Identifier: AGPL-3.0-or-later
//! `studio_list_filaments`: project filament slots (1-based) and, when a
//! printer with an AMS is reachable, its trays. Colours are parsed from the
//! driver strings and are `null` when the format is not recognised.

use serde_json::{Value, json};

use super::args::{AssignFilamentArgs, ListFilamentsArgs};
use super::mutate::{read_error, write_error};
use super::{Tools, driver_error};
use crate::driver::{DriverError, SettingsTarget};
use crate::errors::{ToolError, ToolErrorCode};

fn hex_rgb(s: &str) -> Option<String> {
    if s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", s.to_ascii_uppercase()))
    } else {
        None
    }
}

/// Splits a `filament_colour` list (`;`-separated, optionally quoted) into
/// `"#RRGGBB"` upper-case values; an empty or unrecognised segment is `None`.
pub fn parse_project_colours(raw: &str) -> Vec<Option<String>> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    raw.split(';')
        .map(|seg| {
            let t = seg.trim().trim_matches('"').trim();
            hex_rgb(t.strip_prefix('#')?)
        })
        .collect()
}

/// AMS colour (`RRGGBBAA`, `RRGGBB` or with a leading `#`) to `"#RRGGBB"`.
/// Alpha `00` (empty tray), empty and non-hex input give `None`.
pub fn normalize_ams_colour(raw: &str) -> Option<String> {
    let t = raw.trim().trim_start_matches('#');
    if !t.is_ascii() {
        return None;
    }
    match t.len() {
        6 => hex_rgb(t),
        8 if t[6..].eq_ignore_ascii_case("00") => None,
        8 if t[6..].chars().all(|c| c.is_ascii_hexdigit()) => hex_rgb(&t[..6]),
        _ => None,
    }
}

fn split_list(raw: &str) -> Vec<String> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    raw.split(';')
        .map(|s| s.trim().trim_matches('"').trim().to_string())
        .collect()
}

/// Stable part of the driver's refusal for an object on a locked plate
/// (`McpTools.cpp`, `filament_assign`: "Unlock every plate containing the
/// object first"); matched by substring so a wording tweak around it is fine.
const LOCKED_PLATE_MARKER: &str = "Unlock every plate";

fn assign_error(e: DriverError) -> ToolError {
    if let DriverError::InvalidParams { message } = &e
        && message.contains(LOCKED_PLATE_MARKER)
    {
        return ToolError::new(
            ToolErrorCode::PlateLocked,
            format!("{message}. Unlock the plate in Bambu Studio and retry; nothing was changed."),
        );
    }
    write_error(e)
}

/// Extruder (1-based slot) of an object as the driver reports it.
async fn extruder_slot(
    drv: &dyn crate::driver::DriverClient,
    object_id: &str,
) -> Result<u32, DriverError> {
    let keys = vec!["extruder".to_string()];
    let eff = drv
        .settings_effective(
            &SettingsTarget::Object {
                object_id: object_id.to_string(),
            },
            Some(&keys),
        )
        .await?;
    eff.iter()
        .find(|s| s.key == "extruder")
        .and_then(|s| s.value.trim().parse::<u32>().ok())
        .ok_or_else(|| DriverError::Protocol {
            message: "the driver did not report `extruder`".into(),
        })
}

type AmsRead = (Option<String>, Value, Value, Option<String>);

impl Tools {
    /// Assigns a project filament slot (1-based) to an object.
    pub async fn studio_assign_filament(&self, a: AssignFilamentArgs) -> Result<Value, ToolError> {
        let a = &a;
        let ((previous, preset), applied, revision) = self
            .guarded_mutation_with(
                a.expected_revision,
                false,
                |drv| async move {
                    let models = drv.model_list().await.map_err(read_error)?;
                    if !models.iter().any(|m| m.id == a.object_id) {
                        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
                        return Err(ToolError::new(
                            ToolErrorCode::ObjectNotFound,
                            format!("object {} does not exist", a.object_id),
                        )
                        .with_details(json!({ "object_ids": ids })));
                    }
                    let n = drv
                        .active_presets()
                        .await
                        .map_err(read_error)?
                        .filaments
                        .len();
                    if a.filament_slot == 0 || a.filament_slot as usize > n {
                        return Err(ToolError::new(
                            ToolErrorCode::InvalidArgument,
                            format!(
                                "filament_slot {} is outside 1..={n}; list the slots with studio_list_filaments",
                                a.filament_slot
                            ),
                        )
                        .with_details(json!({ "filament_slots": n })));
                    }
                    let previous = extruder_slot(drv.as_ref(), &a.object_id)
                        .await
                        .map_err(read_error)?;
                    let done = drv
                        .filament_assign(&a.object_id, a.filament_slot - 1)
                        .await
                        .map_err(assign_error)?;
                    Ok((previous, done.preset))
                },
                || async {
                    extruder_slot(self.driver.as_ref(), &a.object_id)
                        .await
                        .map_err(|e| {
                            let mut err = read_error(e);
                            err.details["outcome"] = json!("applied_unverified");
                            err
                        })
                },
            )
            .await?;
        if applied != a.filament_slot {
            return Err(ToolError::new(
                ToolErrorCode::DriverError,
                format!(
                    "the driver accepted the assignment but reports slot {applied}, not {}; the project revision has moved, so read `studio_get_state` again before retrying",
                    a.filament_slot
                ),
            )
            .with_details(json!({ "outcome": "applied_unverified", "applied_slot": applied })));
        }
        Ok(json!({
            "object_id": a.object_id,
            "filament_slot": applied,
            "previous_slot": previous,
            "preset": preset,
            "revision": revision,
        }))
    }

    pub async fn studio_list_filaments(&self, a: ListFilamentsArgs) -> Result<Value, ToolError> {
        let (_app, revision) = self.ready_with_revision().await?;
        let presets = self.driver.active_presets().await.map_err(driver_error)?;
        let keys = vec!["filament_colour".to_string(), "filament_type".to_string()];
        let eff = self
            .driver
            .settings_effective(&SettingsTarget::Project, Some(&keys))
            .await
            .map_err(driver_error)?;
        let value_of = |k: &str| {
            eff.iter()
                .find(|s| s.key == k)
                .map(|s| s.value.as_str())
                .unwrap_or("")
        };
        let colours = parse_project_colours(value_of("filament_colour"));
        let types = split_list(value_of("filament_type"));
        let project: Vec<Value> = presets
            .filaments
            .iter()
            .enumerate()
            .map(|(i, preset)| {
                json!({
                    "slot": i + 1,
                    "preset": preset,
                    "material": types.get(i).filter(|t| !t.is_empty()),
                    "color_hex": colours.get(i).cloned().flatten(),
                })
            })
            .collect();

        let (device_id, ams, reason, ams_error) = self.read_ams(a.device_id).await;
        let mut out = json!({
            "project": project,
            "ams": ams,
            "ams_unavailable_reason": reason,
            "device_id": device_id,
            "revision": revision,
        });
        if let Some(msg) = ams_error {
            out["ams_error"] = json!(msg);
        }
        Ok(out)
    }

    /// Never fails: returns (device id, ams, reason, driver message).
    async fn read_ams(&self, requested: Option<String>) -> AmsRead {
        let failed = |id: Option<String>, e: DriverError| -> AmsRead {
            (
                id,
                Value::Null,
                json!("ams_read_failed"),
                Some(e.to_string()),
            )
        };
        let none = |id: Option<String>, r: &str| -> AmsRead { (id, Value::Null, json!(r), None) };
        let id = match requested {
            Some(id) => id,
            None => {
                let devices = match self.driver.device_list().await {
                    Ok(d) => d,
                    Err(e) => return failed(None, e),
                };
                let mut selected = devices.iter().filter(|d| d.selected);
                match (selected.next(), selected.next()) {
                    (Some(d), None) => {
                        if !d.online || !d.connected {
                            return none(Some(d.device_id.clone()), "printer_offline");
                        }
                        d.device_id.clone()
                    }
                    _ => return none(None, "no_printer_selected"),
                }
            }
        };
        let inv = match self.driver.ams_inventory(&id).await {
            Ok(i) => i,
            Err(e) => return failed(Some(id), e),
        };
        let mut trays = Vec::new();
        for u in &inv.units {
            for t in u.trays.iter().filter(|t| t.exists) {
                trays.push(json!({
                    "unit": u.ams_id,
                    "tray": t.slot_id,
                    "material": t.filament_type,
                    "color_hex": normalize_ams_colour(&t.color),
                    "remaining": t.remaining_percent.filter(|r| *r >= 0),
                }));
            }
        }
        if trays.is_empty() {
            return none(Some(id), "no_ams");
        }
        (Some(id), Value::Array(trays), Value::Null, None)
    }
}
