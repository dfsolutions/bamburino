// SPDX-License-Identifier: AGPL-3.0-or-later
//! Helpers shared by the plate, filament and export tools.

use std::future::Future;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::driver::{
    DriverClient, DriverError, Instance, ModelObject, Plate, PlateContents, PlateList,
};
use crate::errors::{ToolError, ToolErrorCode};

use super::Tools;
use super::args::{
    MoveToPlateArgs, PlateCreateArgs, PlateDeleteArgs, PlateDeleteContents, PlateRenameArgs,
};
use super::mutate::{read_error, write_error};

/// Runs a plate write with a fresh plate revision. The revision is read
/// inside this call (a plate revision goes stale at the first snapshot, so
/// every mutation needs its own read). There is deliberately NO retry: this
/// runs inside the write lock, so a `StaleReference` can only mean the user
/// edited Studio between the read and the call, and retrying would overwrite
/// that edit silently. The failure goes through `plate_write_error` (the
/// `write_error` mapping plus the plate refusals), which yields
/// the 409 `stale_reference` conflict with nothing written.
pub(crate) async fn with_plate_revision<T, F, Fut>(
    drv: &Arc<dyn DriverClient>,
    call: F,
) -> Result<T, ToolError>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, DriverError>>,
{
    let list = drv.plate_list().await.map_err(read_error)?;
    call(list.plate_revision).await.map_err(plate_write_error)
}

/// Finds the instance a tool refers to. `instance_id` None means the object's
/// only instance; an object with several instances requires it.
pub(crate) fn resolve_instance<'a>(
    models: &'a [ModelObject],
    object_id: &str,
    instance_id: Option<&str>,
) -> Result<&'a Instance, ToolError> {
    let not_found = |m: String| ToolError::new(ToolErrorCode::ObjectNotFound, m);
    let obj = models
        .iter()
        .find(|o| o.id == object_id)
        .ok_or_else(|| not_found(format!("object {object_id} not found")))?;
    match instance_id {
        Some(id) => obj
            .instances
            .iter()
            .find(|i| i.id == id)
            .ok_or_else(|| not_found(format!("instance {id} not found on object {object_id}"))),
        None => match obj.instances.as_slice() {
            [only] => Ok(only),
            [] => Err(not_found(format!("object {object_id} has no instances"))
                .with_details(json!({ "reason": "no_instances" }))),
            many => Err(ToolError::new(
                ToolErrorCode::InstanceRequired,
                format!(
                    "object {object_id} has {} instances; pass instance_id",
                    many.len()
                ),
            )
            .with_details(json!({
                "instances": many.iter().map(|i| i.id.clone()).collect::<Vec<_>>()
            }))),
        },
    }
}

/// Maps a driver refusal of a locked plate to `PLATE_LOCKED`. The marker is the
/// stable part of the driver's wording ("Unlock every plate containing the
/// object first", "Unlock the plate before deleting it", "Unlock both plates
/// ..."); matched by substring so a wording tweak around it is fine.
pub(crate) fn locked_plate_error(e: DriverError) -> ToolError {
    if let DriverError::InvalidParams { message } = &e
        && message.contains("Unlock")
    {
        return ToolError::new(
            ToolErrorCode::PlateLocked,
            format!("{message}. Unlock the plate in Bambu Studio and retry; nothing was changed."),
        );
    }
    write_error(e)
}

/// Backstop for the plate tools: the service pre-checks everything, so these
/// only fire when Studio changed in the gap after the pre-check reads.
fn plate_write_error(e: DriverError) -> ToolError {
    if let DriverError::InvalidParams { message } = &e {
        let m = message.as_str();
        let mapped = if m.contains("Unlock") {
            return locked_plate_error(e);
        } else if m.contains("last plate") {
            Some(ToolErrorCode::LastPlate)
        } else if m.contains("contains instances") || m.contains("unlocked and empty") {
            Some(ToolErrorCode::PlateNotEmpty)
        } else if m.contains("out of range") || m.contains("outside current state") {
            Some(ToolErrorCode::PlateNotFound)
        } else {
            None
        };
        if let Some(code) = mapped {
            return ToolError::new(code, message.clone());
        }
    }
    write_error(e)
}

fn plate_not_found(list: &PlateList, index: u32) -> ToolError {
    ToolError::new(
        ToolErrorCode::PlateNotFound,
        format!(
            "plate {index} does not exist; the project has {} plate(s)",
            list.plates.len()
        ),
    )
    .with_details(json!({ "plate_count": list.plates.len() }))
}

fn plate_locked(index: u32) -> ToolError {
    ToolError::new(
        ToolErrorCode::PlateLocked,
        format!(
            "plate {index} is locked; unlock it in Bambu Studio and retry. Nothing was changed."
        ),
    )
    .with_details(json!({ "plate_index": index }))
}

fn check_plate(list: &PlateList, index: u32) -> Result<&Plate, ToolError> {
    list.plates
        .iter()
        .find(|p| p.index == index)
        .ok_or_else(|| plate_not_found(list, index))
}

fn plates_json(list: &PlateList) -> Vec<Value> {
    list.plates
        .iter()
        .map(|p| {
            json!({
                "plate_index": p.index,
                "name": p.name,
                "locked": p.locked,
                "selected": p.index == list.selected_index,
            })
        })
        .collect()
}

fn non_empty_name(name: &str) -> Result<(), ToolError> {
    if name.trim().is_empty() {
        return Err(ToolError::new(
            ToolErrorCode::InvalidArgument,
            "name must not be empty",
        ));
    }
    Ok(())
}

/// (object_id, instance_id) pairs on a plate.
fn instances_on(models: &[ModelObject], plate: u32) -> Vec<(String, String)> {
    models
        .iter()
        .flat_map(|o| {
            o.instances
                .iter()
                .filter(move |i| i.plate_index == plate as i32)
                .map(move |i| (o.id.clone(), i.id.clone()))
        })
        .collect()
}

fn invalid_arg(message: &str, reason: &str) -> ToolError {
    ToolError::new(ToolErrorCode::InvalidArgument, message.to_string())
        .with_details(json!({ "reason": reason }))
}

impl Tools {
    /// Appends an empty plate, optionally naming it.
    pub async fn studio_plate_create(&self, a: PlateCreateArgs) -> Result<Value, ToolError> {
        if let Some(n) = &a.name {
            non_empty_name(n)?;
        }
        let name = a.name.clone();
        let ((index, final_name, name_error), revision) = self
            .guarded_mutation(a.expected_revision, |drv| async move {
                drv.plate_create().await.map_err(plate_write_error)?;
                // The new plate is the last one; confirm it from the driver.
                let list = drv.plate_list().await.map_err(read_error)?;
                let last = list.plates.last().ok_or_else(|| {
                    ToolError::new(
                        ToolErrorCode::DriverError,
                        "the driver reports no plates after plate_create",
                    )
                })?;
                let (index, listed) = (last.index, last.name.clone());
                let Some(name) = name else {
                    return Ok((index, listed, None));
                };
                // The plate exists now: a rename failure must not hide it.
                let renamed = with_plate_revision(&drv, |rev| {
                    let (drv, name) = (drv.clone(), name.clone());
                    async move { drv.plate_rename(index, &rev, &name).await }
                })
                .await;
                Ok(match renamed {
                    Ok(_) => (index, name, None),
                    Err(e) => (index, listed, Some(e.message)),
                })
            })
            .await?;
        let mut out = json!({
            "plate_index": index,
            "name": final_name,
            "name_applied": name_error.is_none(),
            "revision": revision,
        });
        if let Some(msg) = name_error {
            out["name_error"] = json!(msg);
        }
        Ok(out)
    }

    pub async fn studio_plate_rename(&self, a: PlateRenameArgs) -> Result<Value, ToolError> {
        non_empty_name(&a.name)?;
        let a = &a;
        let (_, revision) = self
            .guarded_mutation(a.expected_revision, |drv| async move {
                let list = drv.plate_list().await.map_err(read_error)?;
                check_plate(&list, a.plate_index)?;
                with_plate_revision(&drv, |rev| {
                    let drv = drv.clone();
                    async move { drv.plate_rename(a.plate_index, &rev, &a.name).await }
                })
                .await
            })
            .await?;
        Ok(json!({ "plate_index": a.plate_index, "name": a.name, "revision": revision }))
    }

    pub async fn studio_plate_delete(&self, a: PlateDeleteArgs) -> Result<Value, ToolError> {
        let a = &a;
        let ((contents, done), revision) = self
            .guarded_mutation(a.expected_revision, |drv| async move {
                let list = drv.plate_list().await.map_err(read_error)?;
                let source = check_plate(&list, a.plate_index)?;
                if list.plates.len() == 1 {
                    return Err(ToolError::new(
                        ToolErrorCode::LastPlate,
                        "the project's only plate cannot be deleted",
                    ));
                }
                if source.locked {
                    return Err(plate_locked(a.plate_index));
                }
                if a.contents != Some(PlateDeleteContents::Move) && a.target_plate_index.is_some()
                {
                    return Err(invalid_arg(
                        "target_plate_index is only valid with contents \"move\"",
                        "target_without_move",
                    ));
                }
                let models = drv.model_list().await.map_err(read_error)?;
                let here = instances_on(&models, a.plate_index);
                let contents = match a.contents {
                    None => {
                        if !here.is_empty() {
                            let items: Vec<Value> = here
                                .iter()
                                .map(|(o, i)| json!({ "object_id": o, "instance_id": i }))
                                .collect();
                            return Err(ToolError::new(
                                ToolErrorCode::PlateNotEmpty,
                                format!(
                                    "plate {} holds {} instance(s); pass contents \"move\" (with target_plate_index) or \"delete\"",
                                    a.plate_index,
                                    here.len()
                                ),
                            )
                            .with_details(json!({ "instances": items })));
                        }
                        PlateContents::RequireEmpty
                    }
                    Some(PlateDeleteContents::Move) => {
                        let target = a.target_plate_index.ok_or_else(|| {
                            invalid_arg(
                                "contents \"move\" requires target_plate_index",
                                "target_required",
                            )
                        })?;
                        if target == a.plate_index {
                            return Err(invalid_arg(
                                "target_plate_index must differ from plate_index",
                                "target_is_source",
                            ));
                        }
                        if check_plate(&list, target)?.locked {
                            return Err(plate_locked(target));
                        }
                        if !instances_on(&models, target).is_empty() {
                            return Err(ToolError::new(
                                ToolErrorCode::PlateNotEmpty,
                                format!("target plate {target} is not empty"),
                            )
                            .with_details(json!({
                                "reason": "target_not_empty",
                                "plate_index": target,
                            })));
                        }
                        PlateContents::Move
                    }
                    Some(PlateDeleteContents::Delete) => {
                        let spans: Vec<&str> = models
                            .iter()
                            .filter(|o| {
                                let on = o
                                    .instances
                                    .iter()
                                    .filter(|i| i.plate_index == a.plate_index as i32)
                                    .count();
                                on > 0 && on < o.instances.len()
                            })
                            .map(|o| o.id.as_str())
                            .collect();
                        if !spans.is_empty() {
                            return Err(ToolError::new(
                                ToolErrorCode::InvalidArgument,
                                "an object on this plate also has instances on other plates; move those instances first or use contents \"move\"",
                            )
                            .with_details(json!({
                                "reason": "object_spans_plates",
                                "object_ids": spans,
                            })));
                        }
                        PlateContents::Delete
                    }
                };
                let target = a.target_plate_index;
                let done = with_plate_revision(&drv, |rev| {
                    let drv = drv.clone();
                    async move {
                        drv.plate_delete(a.plate_index, &rev, contents, target)
                            .await
                    }
                })
                .await?;
                Ok((contents, done))
            })
            .await?;
        let label = match contents {
            PlateContents::RequireEmpty => "none",
            PlateContents::Move => "move",
            PlateContents::Delete => "delete",
        };
        Ok(json!({
            "deleted_plate_index": a.plate_index,
            "contents": label,
            "moved_instances": done.moved_instances,
            "deleted_objects": done.deleted_objects,
            "destination_plate_index": done.destination_plate_index,
            "plates": plates_json(&done.plates),
            "revision": revision,
        }))
    }

    /// Moves one instance to another plate.
    pub async fn studio_move_to_plate(&self, a: MoveToPlateArgs) -> Result<Value, ToolError> {
        let a = &a;
        let ((instance_id, from, moved), revision) = self
            .guarded_mutation(a.expected_revision, |drv| async move {
                let models = drv.model_list().await.map_err(read_error)?;
                let inst = resolve_instance(&models, &a.object_id, a.instance_id.as_deref())?;
                let list = drv.plate_list().await.map_err(read_error)?;
                let target = check_plate(&list, a.plate_index)?;
                let from = inst.plate_index;
                let id = inst.id.clone();
                if from == a.plate_index as i32 {
                    // No driver call: no snapshot would follow.
                    return Ok((id, from, false));
                }
                if let Some(src) = list.plates.iter().find(|p| p.index as i32 == from)
                    && src.locked
                {
                    return Err(plate_locked(src.index));
                }
                if target.locked {
                    return Err(plate_locked(a.plate_index));
                }
                with_plate_revision(&drv, |rev| {
                    let (drv, id) = (drv.clone(), id.clone());
                    async move {
                        drv.plate_move_instance(&a.object_id, &id, a.plate_index, &rev)
                            .await
                    }
                })
                .await?;
                Ok((id, from, true))
            })
            .await?;
        Ok(json!({
            "object_id": a.object_id,
            "instance_id": instance_id,
            "from_plate_index": from,
            "plate_index": a.plate_index,
            "moved": moved,
            "revision": revision,
        }))
    }
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use crate::driver::mock::MockDriver;

    fn inst(id: &str) -> Instance {
        Instance {
            id: id.into(),
            index: 0,
            position_mm: [0.0; 3],
            rotation_deg: [0.0; 3],
            scale: [1.0; 3],
            plate_index: 0,
        }
    }

    fn obj(instances: Vec<Instance>) -> ModelObject {
        ModelObject {
            id: "o1".into(),
            index: 0,
            name: "o".into(),
            dimensions_mm: [1.0; 3],
            instances,
        }
    }

    #[tokio::test]
    async fn with_plate_revision_does_not_retry_a_stale_revision() {
        let mock = Arc::new(MockDriver::new());
        let drv: Arc<dyn DriverClient> = mock.clone();
        let (d, m) = (drv.clone(), mock.clone());
        let err = with_plate_revision(&drv, |rev| {
            let (d, m) = (d.clone(), m.clone());
            async move {
                // Test seam: a Studio edit between the helper's read and the
                // driver call (setup helpers stale every plate revision).
                m.add_object("late", "Late", 0);
                d.plate_rename(0, &rev, "x").await
            }
        })
        .await
        .unwrap_err();
        assert_eq!(err.code, ToolErrorCode::RevisionConflict);
        assert_eq!(err.details["reason"], "stale_reference");
        let calls = mock.calls();
        assert_eq!(calls.iter().filter(|c| *c == "plate_list").count(), 1);
        let after = drv.plate_list().await.unwrap();
        assert_ne!(after.plates[0].name, "x");
    }

    #[test]
    fn resolve_instance_defaults_only_for_single_instance_objects() {
        let one = [obj(vec![inst("a")])];
        assert_eq!(resolve_instance(&one, "o1", None).unwrap().id, "a");
        let two = [obj(vec![inst("a"), inst("b")])];
        let e = resolve_instance(&two, "o1", None).unwrap_err();
        assert_eq!(e.code, ToolErrorCode::InstanceRequired);
        assert_eq!(e.details["instances"].as_array().unwrap().len(), 2);
        assert_eq!(resolve_instance(&two, "o1", Some("b")).unwrap().id, "b");
        let e = resolve_instance(&two, "o1", Some("nope")).unwrap_err();
        assert_eq!(e.code, ToolErrorCode::ObjectNotFound);
        let e = resolve_instance(&two, "zzz", None).unwrap_err();
        assert_eq!(e.code, ToolErrorCode::ObjectNotFound);
        let empty = [obj(vec![])];
        let e = resolve_instance(&empty, "o1", None).unwrap_err();
        assert_eq!(e.code, ToolErrorCode::ObjectNotFound);
        assert_eq!(e.details["reason"], "no_instances");
    }
}
