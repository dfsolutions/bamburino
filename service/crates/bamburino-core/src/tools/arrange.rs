// SPDX-License-Identifier: AGPL-3.0-or-later
//! `studio_arrange`: select a plate, run the native arrange job and report
//! what moved.
//!
//! The driver's arrange job works on the whole project (`ArrangeJob.cpp`
//! 785-794, default `prepare_all`), so instances may leave their plate. The
//! result says `scope: "project"` and lists those moves separately. Whether
//! the driver can arrange a single plate is unconfirmed (acceptance row P5).
//! Side effect: the call selects `plate_index` (one undo snapshot, visible in
//! Studio) and leaves it selected; the result reports the previous selection.
//! The driver arranges the whole project whichever plate is selected.
//! All positions come from `model_list` before and after the job.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::Tools;
use super::args::ArrangeArgs;
use super::mutate::{read_error, write_error};
use super::plates::{check_plate, with_plate_revision};
use super::slice::{status_name, wait_job};
use crate::driver::{DriverJob, JobStatus, ModelObject};
use crate::errors::{ToolError, ToolErrorCode};

/// `(object_id, instance_id)` -> `(position_mm, plate_index)`.
type Placement = HashMap<(String, String), ([f64; 3], i32)>;

fn placements(models: &[ModelObject]) -> Placement {
    models
        .iter()
        .flat_map(|o| {
            o.instances
                .iter()
                .map(|i| ((o.id.clone(), i.id.clone()), (i.position_mm, i.plate_index)))
        })
        .collect()
}

/// Instances that changed position / plate between two listings, in the
/// order of the `after` listing.
fn diff(before: &Placement, after: &[ModelObject]) -> (Vec<Value>, Vec<Value>) {
    let (mut moved, mut between) = (Vec::new(), Vec::new());
    for o in after {
        for i in &o.instances {
            let Some((from, from_plate)) = before.get(&(o.id.clone(), i.id.clone())) else {
                continue;
            };
            if *from != i.position_mm {
                moved.push(json!({
                    "object_id": o.id,
                    "instance_id": i.id,
                    "plate_index": i.plate_index,
                    "from_mm": from,
                    "to_mm": i.position_mm,
                }));
            }
            if *from_plate != i.plate_index {
                between.push(json!({
                    "object_id": o.id,
                    "instance_id": i.id,
                    "from_plate_index": from_plate,
                    "to_plate_index": i.plate_index,
                }));
            }
        }
    }
    (moved, between)
}

struct Outcome {
    job: DriverJob,
    /// Plate selected before the call.
    previous_plate: u32,
    /// A `job_get` failure while waiting; the job itself keeps running.
    poll_error: Option<ToolError>,
    moved: Vec<Value>,
    between: Vec<Value>,
}

impl Tools {
    /// Arranges the project's objects on the plates (native arrange job).
    pub async fn studio_arrange(&self, a: ArrangeArgs) -> Result<Value, ToolError> {
        let target = a.plate_index;
        let (poll, timeout) = (self.job_poll_every, self.job_timeout);
        let (out, revision) = self
            .guarded_mutation(a.expected_revision, |drv| async move {
                let list = drv.plate_list().await.map_err(read_error)?;
                check_plate(&list, target)?;
                let previous_plate = list.selected_index;
                if list.selected_index != target {
                    with_plate_revision(&drv, |rev| {
                        let drv = drv.clone();
                        async move { drv.plate_select(target, &rev).await }
                    })
                    .await?;
                }
                let before = placements(&drv.model_list().await.map_err(read_error)?);
                let selected = previous_plate != target;
                let first = match drv.arrange_start().await {
                    Ok(job) => job,
                    Err(e) => {
                        let mut err = write_error(e);
                        err.details["selection_changed"] = json!(selected);
                        if selected {
                            // plate_select succeeded and took an undo snapshot: the
                            // project revision moved although no arrange ran.
                            err.details["snapshot_taken"] = json!(true);
                            if let Ok(rev) = self.revision.refresh(drv.as_ref()).await {
                                err.details["revision"] = json!(rev);
                            }
                            err.message = format!(
                                "{} Plate {target} was selected before the failure (one undo snapshot was taken, the revision moved); no arrange ran. Read studio_get_state again.",
                                err.message
                            );
                        }
                        return Err(err);
                    }
                };
                let waited = wait_job(drv.as_ref(), first.clone(), poll, timeout).await;
                let (job, poll_error) = match waited {
                    Ok(job) => (job, None),
                    // The job runs in Studio whatever the poll did: keep its
                    // id, and still refresh/verify like the success path.
                    Err(e) => (first, Some(write_error(e))),
                };
                let after = drv.model_list().await.map_err(read_error)?;
                let (moved, between) = diff(&before, &after);
                // A failed job is returned as a value, not an error, so the
                // pipeline still refreshes the revision and checks the undo
                // snapshots (arrange_start took one either way).
                Ok(Outcome {
                    job,
                    previous_plate,
                    poll_error,
                    moved,
                    between,
                })
            })
            .await?;
        let Outcome {
            job,
            previous_plate,
            poll_error,
            moved,
            between,
        } = out;
        {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.len() >= 1024 {
                jobs.clear();
            }
            jobs.insert(job.id.clone(), target);
        }
        if let Some(mut e) = poll_error {
            e.details["job_id"] = json!(job.id);
            e.details["outcome"] = json!("job_running_unobserved");
            e.details["revision"] = json!(revision);
            e.message = format!(
                "{} The arrange job {} was started and is probably still running in Bambu Studio; poll it with studio_slice_status or cancel it with studio_cancel_slice, then read studio_get_state again.",
                e.message, job.id
            );
            return Err(e);
        }
        if matches!(
            job.status,
            JobStatus::Failed | JobStatus::Cancelled | JobStatus::Stale
        ) {
            let driver_error = job
                .error
                .as_ref()
                .map(|e| e.message.clone())
                .or_else(|| job.message.clone());
            return Err(ToolError::new(
                ToolErrorCode::DriverError,
                format!(
                    "the arrange job {} ended {}; check the project with studio_list_models before retrying",
                    job.id,
                    status_name(&job.status),
                ),
            )
            .with_details(json!({
                "job_id": job.id,
                "status": status_name(&job.status),
                "driver_error": driver_error,
                "revision": revision,
            })));
        }
        let mut warnings = Vec::new();
        if !between.is_empty() {
            warnings.push(format!(
                "Arrange works on the whole project and moved {} instance(s) to a different plate; see moved_between_plates.",
                between.len()
            ));
        }
        let running = job.status != JobStatus::Succeeded;
        if running {
            warnings.push(format!(
                "The arrange job {} is still running (status {}); poll it with studio_slice_status or cancel it with studio_cancel_slice. The moves listed may be incomplete, and the revision will change when the job finishes: read studio_get_state again before the next change (changes are refused as BUSY until then).",
                job.id,
                status_name(&job.status)
            ));
        }
        Ok(json!({
            "plate_index": target,
            "job_id": job.id,
            "status": if running { status_name(&job.status) } else { "succeeded" },
            "outcome": if running { json!("timeout_job_running") } else { Value::Null },
            "scope": "project",
            "selected_plate_index": target,
            "previous_plate_index": previous_plate,
            "selected_plate_changed": previous_plate != target,
            "moved": moved,
            "moved_between_plates": between,
            "warning": if warnings.is_empty() { Value::Null } else { json!(warnings.join(" ")) },
            "revision": revision,
        }))
    }
}
