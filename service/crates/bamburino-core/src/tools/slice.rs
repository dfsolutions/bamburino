// SPDX-License-Identifier: AGPL-3.0-or-later
//! Slicing tools: start, poll, cancel and summarize a slice job.
//!
//! * `studio_slice` is not guarded by `expected_revision` (starting a slice
//!   does not change the project) but needs a ready Studio and no modal, UI
//!   job or slice running. The driver slices the selected plate only, so a
//!   different plate is selected first, through the mutation pipeline.
//! * The print time of the summary comes from `slice_get_state`; the driver's
//!   `preview_get_summary` has no grams or meters. Those are read from the
//!   header of the G-code, exported to a temp file in `Tools::export_dir` that
//!   is always deleted afterwards (and leftovers older than 60 s are swept).
//!
//! # Export directory contract (Tasks 13/16)
//!
//! Bambu Studio (interactive user) writes the file, the service (possibly
//! `LocalService`, whose `%TEMP%` the user cannot reach) reads and deletes it.
//! Production value: `%ProgramData%\Bamburino\tmp`, created by the installer
//! with an ACL that gives the interactive user write and the service
//! read/delete. The service creates the directory at startup when it is
//! missing (`runtime::prepare` in the `bamburino` crate), but it does not set
//! that ACL: the installer does. This module never creates it.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::args::{JobArgs, SliceArgs, SummaryArgs};
use super::mutate::{busy_reason, read_error, write_error};
use super::{Tools, driver_error};
use crate::driver::{DriverClient, DriverError, DriverJob, JobStatus};
use crate::errors::{ToolError, ToolErrorCode};

const STALE_MESSAGE: &str =
    "The slice result was invalidated by a later change to the project; slice again.";
const NO_RESULT_MESSAGE: &str =
    "There is no current slice result: slice the plate first (studio_slice).";
/// How much of the exported G-code is scanned for the header.
const HEADER_SCAN_LIMIT: u64 = 4 * 1024 * 1024;
/// Polls of an export job that is not terminal yet (the driver copies the
/// file synchronously, so this is a safety net).
const EXPORT_POLLS: u32 = 20;
const EXPORT_POLL_EVERY: Duration = Duration::from_millis(100);
/// Leftover exports older than this are deleted before a new export.
const STALE_EXPORT_AGE: Duration = Duration::from_secs(60);

fn invalid(msg: &str) -> ToolError {
    ToolError::new(ToolErrorCode::InvalidArgument, msg)
}

/// A job id the driver does not know (or from another Studio session).
fn job_error(e: DriverError, job_id: &str) -> ToolError {
    match e {
        DriverError::StaleReference { .. } => ToolError::new(
            ToolErrorCode::JobNotFound,
            format!(
                "job `{job_id}` is unknown, expired, or belongs to another Bambu Studio session"
            ),
        ),
        other => read_error(other),
    }
}

pub(super) fn status_name(s: &JobStatus) -> &'static str {
    match s {
        // Cancellation is requested but not finished: still running.
        JobStatus::Running | JobStatus::Cancelling => "running",
        JobStatus::Succeeded => "succeeded",
        // A stale job is terminal and its result is unusable.
        JobStatus::Failed | JobStatus::Stale => "failed",
        JobStatus::Cancelled => "cancelled",
        JobStatus::Unknown(_) => "unknown",
    }
}

fn job_json(job: &DriverJob, plate_id: u32) -> Value {
    let (error, message) = match &job.status {
        JobStatus::Stale => (
            json!({"code": "STALE_RESULT", "message": STALE_MESSAGE}),
            Some(STALE_MESSAGE.to_string()),
        ),
        JobStatus::Cancelling => (
            Value::Null,
            job.message
                .clone()
                .or_else(|| Some("cancellation requested".to_string())),
        ),
        _ => (
            job.error.as_ref().map_or(
                Value::Null,
                |e| json!({"code": e.code, "message": e.message}),
            ),
            job.message.clone(),
        ),
    };
    let mut v = json!({
        "job_id": job.id,
        "plate_id": plate_id,
        "status": status_name(&job.status),
        "progress_percent": job.progress_percent,
        "message": message,
        "error": error,
        "slice_result_id": job.slice_result_id,
    });
    if let JobStatus::Unknown(raw) = &job.status {
        v["details"] = json!({"raw_status": raw});
    }
    v
}

/// Filament totals of the Bambu G-code header.
///
/// The three `total filament ...` lines list ONLY the filaments the plate
/// uses (a `std::map<filament_id, double>` in `GCodeProcessor.cpp`), in id
/// order, so their position is not the filament slot. The slots come from the
/// header line `; filament: 1,3` (`GCode.cpp`, 1-based ids of the used
/// filaments, `print.get_slice_used_filaments`).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct HeaderUsage {
    length_mm: Option<Vec<Option<f64>>>,
    volume_cm3: Option<Vec<Option<f64>>>,
    weight_g: Option<Vec<Option<f64>>>,
    /// 1-based slots of the used filaments (`; filament: ...`).
    slots: Option<Vec<u32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FilamentRow {
    /// 0-based filament slot (Studio shows `index + 1`).
    pub index: u32,
    pub grams: Option<f64>,
    pub meters: Option<f64>,
    pub volume_cm3: Option<f64>,
}

fn values_after(rest: &str) -> Option<Vec<Option<f64>>> {
    let (_, list) = rest.split_once(':')?;
    if list.trim().is_empty() {
        return Some(Vec::new());
    }
    Some(
        list.split(',')
            .map(|v| v.trim().parse::<f64>().ok().filter(|n| n.is_finite()))
            .collect(),
    )
}

fn slots_after(rest: &str) -> Option<Vec<u32>> {
    let list = rest.trim_start().strip_prefix(':')?.trim();
    if list.is_empty() {
        return Some(Vec::new());
    }
    list.split(',')
        .map(|v| v.trim().parse::<u32>().ok().filter(|n| *n >= 1))
        .collect()
}

impl HeaderUsage {
    /// Reads one G-code line; only the first occurrence of each tag counts.
    pub(crate) fn feed(&mut self, line: &str) {
        let Some(body) = line.trim_start().strip_prefix(';') else {
            return;
        };
        let body = body.trim_start();
        for (tag, slot) in [
            ("total filament length [mm]", &mut self.length_mm),
            ("total filament volume [cm^3]", &mut self.volume_cm3),
            ("total filament weight [g]", &mut self.weight_g),
        ] {
            if slot.is_none()
                && let Some(rest) = body.strip_prefix(tag)
            {
                *slot = values_after(rest);
            }
        }
        if self.slots.is_none()
            && let Some(rest) = body.strip_prefix("filament")
            && rest.trim_start().starts_with(':')
        {
            self.slots = slots_after(rest);
        }
    }

    pub(crate) fn complete(&self) -> bool {
        self.length_mm.is_some()
            && self.volume_cm3.is_some()
            && self.weight_g.is_some()
            && self.slots.is_some()
    }

    #[cfg(test)]
    pub(crate) fn parse(text: &str) -> Self {
        let mut u = Self::default();
        for line in text.lines() {
            u.feed(line);
        }
        u
    }

    /// One row per used filament. `Err` carries the reason the usage cannot
    /// be attributed to filament slots reliably; nothing is guessed.
    pub(crate) fn rows(&self) -> Result<Vec<FilamentRow>, String> {
        let lists = [&self.length_mm, &self.volume_cm3, &self.weight_g];
        let present: Vec<&Vec<Option<f64>>> = lists.iter().filter_map(|l| l.as_ref()).collect();
        if present.is_empty() {
            return Err("The exported G-code header has no filament totals (total filament length/volume/weight); grams and meters are unavailable.".to_string());
        }
        let n = present[0].len();
        if present.iter().any(|l| l.len() != n) {
            return Err("The exported G-code header lists a different number of filaments in its length, volume and weight totals; usage is not attributed to avoid mixing them up.".to_string());
        }
        let Some(slots) = &self.slots else {
            return Err("The exported G-code header has no `; filament:` line with the used filament slots, so the totals cannot be attributed to slots.".to_string());
        };
        if slots.len() != n {
            return Err(format!(
                "The exported G-code header lists {} used filament slot(s) but {n} total(s); usage is not attributed to avoid mixing them up.",
                slots.len()
            ));
        }
        if n == 0 {
            return Err("The exported G-code header lists no used filament.".to_string());
        }
        // The totals are keyed by filament id in ascending order.
        let at = |l: &Option<Vec<Option<f64>>>, i: usize| {
            l.as_ref().and_then(|v| v.get(i).copied().flatten())
        };
        let mut sorted_slots: Vec<u32> = slots.clone();
        sorted_slots.sort_unstable();
        Ok(sorted_slots
            .into_iter()
            .enumerate()
            .map(|(i, slot)| FilamentRow {
                index: slot - 1,
                grams: at(&self.weight_g, i),
                meters: at(&self.length_mm, i).map(|mm| mm / 1000.0),
                volume_cm3: at(&self.volume_cm3, i),
            })
            .collect())
    }
}

fn round4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

/// A temp file that is deleted when dropped, whatever happens.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn temp_gcode_path(dir: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    dir.join(format!("summary-{}-{n}.gcode", std::process::id()))
}

/// Deletes `summary-*.gcode` files older than [`STALE_EXPORT_AGE`]: the
/// driver copies the file synchronously (up to 10 s), so one can land after
/// the call that asked for it was dropped (client timeout or cancel) and its
/// guard already ran.
fn sweep_stale_exports(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !(name.starts_with("summary-") && name.ends_with(".gcode")) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_EXPORT_AGE);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Scans the head of the file for the filament totals.
fn scan_header(path: &Path) -> std::io::Result<HeaderUsage> {
    let mut reader = BufReader::new(std::fs::File::open(path)?.take(HEADER_SCAN_LIMIT));
    let mut usage = HeaderUsage::default();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 || usage.complete() {
            break;
        }
        usage.feed(&String::from_utf8_lossy(&buf));
    }
    Ok(usage)
}

/// Polls `job_get` until the job is terminal or `timeout` has passed and
/// returns the last job seen. A timeout is not an error: the caller sees a
/// non-terminal status. The wait can overshoot `timeout` by up to one `poll`
/// interval. With a zero timeout the job is returned unpolled.
pub(crate) async fn wait_job(
    drv: &dyn DriverClient,
    first: DriverJob,
    poll: Duration,
    timeout: Duration,
) -> Result<DriverJob, DriverError> {
    let started = std::time::Instant::now();
    let mut job = first;
    while !job.status.is_terminal() && started.elapsed() < timeout {
        tokio::time::sleep(poll).await;
        job = drv.job_get(&job.id).await?;
    }
    Ok(job)
}

impl Tools {
    /// Starts slicing a plate (default: the selected one). Returns the job.
    pub async fn studio_slice(&self, a: SliceArgs) -> Result<Value, ToolError> {
        self.revision.touch_client();
        let app = self
            .probe
            .require_ready(true, self.cfg.allow_version_mismatch)
            .await?;
        if let Some(reason) = busy_reason(&app) {
            return Err(ToolError::new(
                ToolErrorCode::Busy,
                format!("Bambu Studio is busy ({reason}); slicing was not started."),
            )
            .with_details(json!({ "reason": reason })));
        }
        let plates = self.driver.plate_list().await.map_err(read_error)?;
        let target = a.plate_id.unwrap_or(plates.selected_index);
        if !plates.plates.iter().any(|p| p.index == target) {
            return Err(ToolError::new(
                ToolErrorCode::PlateNotFound,
                format!("plate {target} does not exist"),
            ));
        }
        let mut known_revision = None;
        if target == plates.selected_index {
            known_revision = Some(plates.plate_revision);
        } else {
            // The driver slices the selected plate only. The selection is
            // visible to the user and takes an undo snapshot, so it goes
            // through the mutation pipeline.
            let current = self
                .revision
                .refresh(self.driver.as_ref())
                .await
                .map_err(read_error)?;
            let revision = plates.plate_revision;
            let (selected, _) = self
                .guarded_mutation(current, |drv| async move {
                    drv.plate_select(target, &revision)
                        .await
                        .map_err(write_error)
                })
                .await?;
            if selected.selected_index != target {
                return Err(ToolError::new(
                    ToolErrorCode::DriverError,
                    format!("the driver did not select plate {target}"),
                ));
            }
        }

        let _serial = self.write_lock.lock().await;
        let mut retried = false;
        let job = loop {
            let revision = match known_revision.take() {
                Some(r) => r,
                None => {
                    self.driver
                        .plate_list()
                        .await
                        .map_err(read_error)?
                        .plate_revision
                }
            };
            match self.driver.slice_start(target, &revision).await {
                Ok(job) => break job,
                // The plate revision went stale: one retry with a fresh one.
                Err(DriverError::StaleReference { .. }) if !retried => retried = true,
                Err(DriverError::InvalidParams { message })
                    if message.contains("selected plate") =>
                {
                    return Err(ToolError::new(
                        ToolErrorCode::RevisionConflict,
                        "The plate selection changed in Bambu Studio before slicing started; nothing was sliced. Retry.",
                    )
                    .with_details(json!({ "reason": "plate_selection_changed", "driver": message })));
                }
                Err(e) => return Err(write_error(e)),
            }
        };
        {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            if jobs.len() >= 1024 {
                jobs.clear();
            }
            jobs.insert(job.id.clone(), target);
        }
        Ok(json!({
            "job_id": job.id,
            "plate_id": target,
            "status": status_name(&job.status),
        }))
    }

    fn plate_of_job(&self, job: &DriverJob) -> u32 {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&job.id)
            .copied()
            .unwrap_or(job.plate_index)
    }

    /// One poll of a slice or arrange job (`studio_arrange` job ids work too).
    /// Every `export_gcode` (see the summary) creates a
    /// driver job too and the driver keeps only the last 64 jobs, so an old slice
    /// job may become `JOB_NOT_FOUND` after many summaries.
    pub async fn studio_slice_status(&self, a: JobArgs) -> Result<Value, ToolError> {
        self.revision.touch_client();
        self.probe
            .require_ready(false, self.cfg.allow_version_mismatch)
            .await?;
        let job = self
            .driver
            .job_get(&a.job_id)
            .await
            .map_err(|e| job_error(e, &a.job_id))?;
        Ok(job_json(&job, self.plate_of_job(&job)))
    }

    /// Asks the driver to cancel a job (slice or arrange; `studio_arrange` job
    /// ids work too). The driver treats this as a read, so it works while a
    /// dialog is open.
    pub async fn studio_cancel_slice(&self, a: JobArgs) -> Result<Value, ToolError> {
        self.revision.touch_client();
        self.probe
            .require_ready(false, self.cfg.allow_version_mismatch)
            .await?;
        let job = self
            .driver
            .job_cancel(&a.job_id)
            .await
            .map_err(|e| job_error(e, &a.job_id))?;
        // A job that already finished (succeeded, failed, stale) was not
        // cancelled by this call.
        let requested = matches!(
            job.status,
            JobStatus::Running | JobStatus::Cancelling | JobStatus::Cancelled
        );
        let mut v = json!({
            "job_id": job.id,
            "status": status_name(&job.status),
            "cancel_requested": requested,
        });
        if !requested {
            v["message"] = json!("the job had already finished; nothing was cancelled");
        }
        Ok(v)
    }

    /// Print time, filament usage and warnings of the current slice result.
    pub async fn studio_get_slice_summary(&self, a: SummaryArgs) -> Result<Value, ToolError> {
        let (app, revision) = self.ready_with_revision().await?;
        let slice = self.driver.slice_state().await.map_err(driver_error)?;
        let result_id = match (&slice.slice_result_id, slice.valid) {
            (Some(id), true) => id.clone(),
            _ => return Err(invalid(NO_RESULT_MESSAGE)),
        };
        let plate = slice.plate_index.unwrap_or(app.selected_plate_index);
        if let Some(wanted) = a.plate_id
            && wanted != plate
        {
            return Err(invalid(&format!(
                "plate {wanted} has no current slice result (the current one is for plate {plate}): slice plate {wanted} first"
            )));
        }

        let mut retried = false;
        let summary = loop {
            let plates = self.driver.plate_list().await.map_err(read_error)?;
            match self
                .driver
                .preview_summary(plate, &plates.plate_revision, &result_id)
                .await
            {
                Ok(s) => break s,
                Err(DriverError::StaleReference { .. }) if !retried => retried = true,
                Err(DriverError::StaleReference { .. }) => {
                    return Err(invalid(
                        "The slice result is no longer current (the project changed): slice again.",
                    ));
                }
                Err(e) => return Err(read_error(e)),
            }
        };

        let (rows, note) = match self.filament_usage(plate, &result_id).await {
            Ok(rows) => (rows, None),
            Err(why) => (Vec::new(), Some(why)),
        };
        let filaments: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "index": r.index,
                    "grams": r.grams,
                    "meters": r.meters.map(round4),
                    "volume_cm3": r.volume_cm3,
                })
            })
            .collect();
        Ok(json!({
            "plate_id": plate,
            "print_time_s": slice.estimated_time_s,
            "filaments": filaments,
            "support_volume_mm3": summary.support_volume_mm3,
            "warnings": summary.warnings,
            "note": note,
            "revision": revision,
        }))
    }

    /// Grams, meters and volume per filament, read from the header of the
    /// exported G-code. `Err` carries the reason it is unavailable.
    async fn filament_usage(
        &self,
        plate: u32,
        result_id: &str,
    ) -> Result<Vec<FilamentRow>, String> {
        // No edit may land between the plate revision and the export.
        let _serial = self.write_lock.lock().await;
        let dir = self.export_dir.as_path();
        if !dir.is_dir() {
            return Err(format!(
                "the G-code export directory {} does not exist; filament usage is unavailable",
                dir.display()
            ));
        }
        sweep_stale_exports(dir);
        let path = temp_gcode_path(dir);
        let _cleanup = TempFile(path.clone());
        let revision = self
            .driver
            .plate_list()
            .await
            .map_err(|e| format!("G-code export skipped ({e}); filament usage is unavailable"))?
            .plate_revision;
        let mut job = self
            .driver
            .export_gcode(plate, &revision, result_id, &path)
            .await
            .map_err(|e| format!("G-code export failed ({e}); filament usage is unavailable"))?;
        let mut polls = 0;
        while !job.status.is_terminal() && polls < EXPORT_POLLS {
            polls += 1;
            tokio::time::sleep(EXPORT_POLL_EVERY).await;
            job = self.driver.job_get(&job.id).await.map_err(|e| {
                format!("G-code export failed ({e}); filament usage is unavailable")
            })?;
        }
        if job.status != JobStatus::Succeeded {
            let why = job
                .error
                .map(|e| e.message)
                .or(job.message)
                .unwrap_or_else(|| status_name(&job.status).to_string());
            return Err(format!(
                "G-code export did not complete ({why}); filament usage is unavailable"
            ));
        }
        let scan_path = path.clone();
        let usage = tokio::task::spawn_blocking(move || scan_header(&scan_path))
            .await
            .map_err(|e| format!("G-code header scan failed ({e}); filament usage is unavailable"))?
            .map_err(|e| {
                format!("G-code header could not be read ({e}); filament usage is unavailable")
            })?;
        usage.rows()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/bambu_header.gcode");

    #[test]
    fn parses_the_bambu_header() {
        let rows = HeaderUsage::parse(FIXTURE).rows().unwrap();
        assert_eq!(rows.len(), 2);
        // `; filament: 1,3` -> slots 0 and 2 (0-based).
        assert_eq!(rows[0].index, 0);
        assert_eq!(rows[0].grams, Some(4.28));
        assert!((rows[0].meters.unwrap() - 1.43451).abs() < 1e-9);
        assert_eq!(rows[0].volume_cm3, Some(3.45));
        assert_eq!(rows[1].index, 2);
        assert_eq!(rows[1].grams, Some(2.42));
        assert!((rows[1].meters.unwrap() - 0.81207).abs() < 1e-9);
        assert_eq!(rows[1].volume_cm3, Some(1.95));
    }

    #[test]
    fn filament_line_is_not_confused_with_density_or_diameter() {
        let u =
            HeaderUsage::parse("; filament_density: 1.24,1.24\n; filament_diameter: 1.75,1.75\n");
        assert_eq!(u.slots, None);
    }

    #[test]
    fn missing_tags_are_an_error() {
        assert!(
            HeaderUsage::parse("; HEADER_BLOCK_START\nG1 X1\n")
                .rows()
                .is_err()
        );
    }

    #[test]
    fn missing_filament_line_is_an_error() {
        let u = HeaderUsage::parse("; total filament weight [g] : 1.00\n");
        assert!(u.rows().unwrap_err().contains("`; filament:`"));
    }

    #[test]
    fn length_mismatch_between_lists_is_an_error() {
        let u = HeaderUsage::parse(
            "; total filament length [mm] : 1.00,2.00\n; total filament weight [g] : 1.00\n; filament: 1,2\n",
        );
        assert!(u.rows().unwrap_err().contains("different number"));
    }

    #[test]
    fn slot_count_mismatch_is_an_error() {
        let u = HeaderUsage::parse("; total filament weight [g] : 1.00,2.00\n; filament: 1\n");
        assert!(u.rows().is_err());
    }

    #[test]
    fn empty_value_lists_are_no_rows() {
        let u = HeaderUsage::parse(
            "; total filament length [mm] : \n; total filament volume [cm^3] : \n; total filament weight [g] : \n; filament: \n",
        );
        assert_eq!(u.length_mm, Some(Vec::new()));
        assert!(u.rows().unwrap_err().contains("no used filament"));
    }

    #[test]
    fn slots_are_sorted_like_the_map_they_come_from() {
        let u = HeaderUsage::parse("; total filament weight [g] : 1.00,2.00\n; filament: 4,2\n");
        let rows = u.rows().unwrap();
        assert_eq!((rows[0].index, rows[0].grams), (1, Some(1.0)));
        assert_eq!((rows[1].index, rows[1].grams), (3, Some(2.0)));
    }

    #[test]
    fn garbage_values_are_null() {
        let u = HeaderUsage::parse(
            ";total filament length [mm] : 100.00\n; total filament weight [g] : abc\n; filament: 2\n",
        );
        let rows = u.rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index, 1);
        assert_eq!(rows[0].meters, Some(0.1));
        assert_eq!(rows[0].grams, None);
        assert_eq!(rows[0].volume_cm3, None);
    }

    #[test]
    fn only_the_first_occurrence_counts() {
        let u = HeaderUsage::parse(
            "; total filament weight [g] : 1.00\n; total filament weight [g] : 9.00\n; filament: 1\n",
        );
        assert_eq!(u.rows().unwrap()[0].grams, Some(1.0));
    }

    #[test]
    fn unknown_raw_status_is_surfaced() {
        let job = DriverJob {
            id: "j".to_string(),
            plate_index: 2,
            status: JobStatus::Unknown("queued".to_string()),
            progress_percent: None,
            message: None,
            error: None,
            slice_result_id: None,
            reused: false,
        };
        let v = job_json(&job, 2);
        assert_eq!(v["status"], "unknown");
        assert_eq!(v["details"]["raw_status"], "queued");
    }

    #[test]
    fn stale_job_is_a_terminal_failure_with_a_clear_message() {
        let job = DriverJob {
            id: "j".to_string(),
            plate_index: 0,
            status: JobStatus::Stale,
            progress_percent: None,
            message: None,
            error: None,
            slice_result_id: None,
            reused: false,
        };
        let v = job_json(&job, 0);
        assert_eq!(v["status"], "failed");
        assert_eq!(v["error"]["code"], "STALE_RESULT");
        assert!(v["message"].as_str().unwrap().contains("slice again"));
    }
}
