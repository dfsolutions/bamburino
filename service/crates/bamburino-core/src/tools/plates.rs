// SPDX-License-Identifier: AGPL-3.0-or-later
//! Helpers shared by the plate, filament and export tools.

// TODO(Task 6): remove when the plate tools use these helpers.
#![allow(dead_code)]

use std::future::Future;
use std::sync::Arc;

use serde_json::json;

use crate::driver::{DriverClient, DriverError, Instance, ModelObject};
use crate::errors::{ToolError, ToolErrorCode};

use super::mutate::{read_error, write_error};

/// Runs a plate write with a fresh plate revision. The revision is read
/// inside this call (a plate revision goes stale at the first snapshot, so
/// every mutation needs its own read). There is deliberately NO retry: this
/// runs inside the write lock, so a `StaleReference` can only mean the user
/// edited Studio between the read and the call, and retrying would overwrite
/// that edit silently. The failure goes through `write_error`, which yields
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
    call(list.plate_revision).await.map_err(write_error)
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

    fn stale() -> DriverError {
        DriverError::StaleReference {
            message: "stale".into(),
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
