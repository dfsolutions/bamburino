// SPDX-License-Identifier: AGPL-3.0-or-later
//! File hand-off tools: `studio_save_project` and `studio_export_plate`.
//!
//! These are the only tools that take a user path, and the first production
//! callers of the path allowlist. Order of the checks, identical in both:
//!
//! 1. suffix, case-insensitive (`.3mf` / `.gcode.3mf`), else
//!    `INVALID_ARGUMENT`;
//! 2. [`PathGuard::check_destination`] (no guard installed = nothing is
//!    allowed), else `PATH_NOT_ALLOWED` with `details.reason` = the
//!    `PathError` variant in snake_case and no path in the message;
//! 3. an existing file without `overwrite: true` is `FILE_EXISTS`.
//!
//! Steps 1 to 3 make NO driver call (not even `app_get_state`). Then, under
//! the write lock: readiness, `BUSY` when a dialog, UI job or slice is
//! running, the driver write (with `overwrite` passed through verbatim,
//! default false), a revision refresh, and the file facts (read back while
//! still holding the lock, so the hash describes this call's file).
//!
//! The service may run as `LocalService` and be unable to inspect or read
//! the owner's folders (see `pathguard`). The driver runs as the user and
//! re-validates the directory and the overwrite rule itself; its refusals are
//! mapped to the same codes. `sha256` is computed by reading the file back;
//! when the service cannot read it, `sha256` is null with
//! `hash_unavailable_reason`, never an error (the file was written). The
//! file is streamed in chunks and capped at 2 GiB: above it `sha256` is null
//! with a reason starting `too_large` (`bytes` is still reported).

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Tools;
use super::args::{ExportPlateArgs, SaveProjectArgs};
use super::mutate::{busy_reason, read_error, write_error};
use crate::driver::DriverError;
use crate::errors::{ToolError, ToolErrorCode};
use crate::pathguard::PathError;

const PROJECT_SUFFIX: &str = ".3mf";
const SLICED_SUFFIX: &str = ".gcode.3mf";

/// Which file a destination is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    /// `studio_save_project`: `.3mf`, not `.gcode.3mf`.
    Project,
    /// `studio_export_plate`: `.gcode.3mf`.
    SlicedPlate,
}

fn ends_with_ignore_case(s: &str, suffix: &str) -> bool {
    s.len() >= suffix.len()
        && s.is_char_boundary(s.len() - suffix.len())
        && s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn invalid_suffix(message: &str, reason: &str) -> ToolError {
    ToolError::new(ToolErrorCode::InvalidArgument, message)
        .with_details(json!({ "reason": reason }))
}

fn check_suffix(path: &str, kind: FileKind) -> Result<(), ToolError> {
    let sliced = ends_with_ignore_case(path, SLICED_SUFFIX);
    match kind {
        FileKind::SlicedPlate if !sliced => Err(invalid_suffix(
            "the export path must end with .gcode.3mf",
            "wrong_suffix",
        )),
        FileKind::Project if sliced => Err(invalid_suffix(
            "a .gcode.3mf name is reserved for sliced plates (studio_export_plate); save the project as .3mf",
            "wrong_suffix",
        )),
        FileKind::Project if !ends_with_ignore_case(path, PROJECT_SUFFIX) => Err(invalid_suffix(
            "the project path must end with .3mf",
            "wrong_suffix",
        )),
        _ => Ok(()),
    }
}

/// `PathError` variant name in snake_case (`details.reason`).
fn path_error_reason(e: PathError) -> &'static str {
    match e {
        PathError::Relative => "relative",
        PathError::Traversal => "traversal",
        PathError::Unc => "unc",
        PathError::DevicePath => "device_path",
        PathError::ReservedName => "reserved_name",
        PathError::AlternateDataStream => "alternate_data_stream",
        PathError::TrailingDotOrSpace => "trailing_dot_or_space",
        PathError::ReparsePoint => "reparse_point",
        PathError::OutsideRoots => "outside_roots",
        PathError::NotFound => "not_found",
    }
}

/// `PATH_NOT_ALLOWED`; the message carries no path.
fn path_not_allowed(e: PathError) -> ToolError {
    let message = match e {
        PathError::NotFound => {
            "the destination folder does not exist (folders are not created); choose an existing folder inside the allowed directories".to_string()
        }
        PathError::OutsideRoots => {
            "the path is outside the directories allowed in config.toml (allowed_dirs, set by `bamburino config init`); nothing was written".to_string()
        }
        other => format!("{other}; nothing was written"),
    };
    ToolError::new(ToolErrorCode::PathNotAllowed, message)
        .with_details(json!({ "reason": path_error_reason(e) }))
}

fn file_exists() -> ToolError {
    ToolError::new(
        ToolErrorCode::FileExists,
        "the destination file already exists; nothing was written. Pass overwrite: true to replace it, or choose another name.",
    )
    .with_details(json!({ "reason": "destination_exists" }))
}

/// `.3MF` (any case) becomes `.3mf`: the driver compares the project
/// extension case-sensitively. NTFS is case-insensitive, so it is the same
/// file.
fn lowercase_project_suffix(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    let cut = s.len() - PROJECT_SUFFIX.len();
    PathBuf::from(format!("{}{PROJECT_SUFFIX}", &s[..cut]))
}

/// Maps a refusal of a driver file write. The driver runs as the user and
/// re-checks what the service may not have been able to see.
fn file_write_error(e: DriverError) -> ToolError {
    if let DriverError::InvalidParams { message } = &e {
        if message.contains("Destination exists") {
            return file_exists();
        }
        if message.contains("directory does not exist") {
            return path_not_allowed(PathError::NotFound);
        }
    }
    write_error(e)
}

/// A failure after the file was written: the error says so.
fn after_write(mut err: ToolError) -> ToolError {
    err.message = format!(
        "the file was written, but reading the new state failed: {}",
        err.message
    );
    match err.details.as_object_mut() {
        Some(d) => {
            d.insert("outcome".to_string(), json!("written"));
        }
        None => err.details = json!({ "outcome": "written" }),
    }
    err
}

/// Size and SHA-256 of a written file, read back by the service.
struct FileFacts {
    bytes: Option<u64>,
    sha256: Option<String>,
    unavailable: Option<String>,
}

/// Largest file the service hashes (2 GiB). A written 3MF is far smaller;
/// above this, `sha256` is null with a `too_large` reason and the size is
/// still reported.
const MAX_HASH_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Outcome of [`hash_file`].
#[derive(Debug, PartialEq, Eq)]
enum Hashed {
    Done {
        bytes: u64,
        sha256: String,
    },
    /// The file is above the cap; `bytes` is what is known of its size.
    TooLarge {
        bytes: u64,
    },
}

/// Streams the file in 64 KiB chunks (never reads it whole) and stops with
/// [`Hashed::TooLarge`] as soon as it is above `cap` bytes.
fn hash_file(path: &Path, cap: u64) -> std::io::Result<Hashed> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len > cap {
        return Ok(Hashed::TooLarge { bytes: len });
    }
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > cap {
            // Grew while being read.
            return Ok(Hashed::TooLarge { bytes: total });
        }
        hasher.update(&buf[..n]);
    }
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(Hashed::Done {
        bytes: total,
        sha256,
    })
}

async fn file_facts(path: PathBuf) -> FileFacts {
    file_facts_capped(path, MAX_HASH_BYTES).await
}

async fn file_facts_capped(path: PathBuf, cap: u64) -> FileFacts {
    let read = tokio::task::spawn_blocking(move || {
        hash_file(&path, cap).map_err(|e| (e, std::fs::metadata(&path).map(|m| m.len()).ok()))
    })
    .await;
    match read {
        Ok(Ok(Hashed::Done { bytes, sha256 })) => FileFacts {
            bytes: Some(bytes),
            sha256: Some(sha256),
            unavailable: None,
        },
        Ok(Ok(Hashed::TooLarge { bytes })) => FileFacts {
            bytes: Some(bytes),
            sha256: None,
            unavailable: Some(format!(
                "too_large: the file is above the {cap}-byte limit the service hashes; sha256 was not computed"
            )),
        },
        Ok(Err((e, bytes))) => FileFacts {
            bytes,
            sha256: None,
            unavailable: Some(format!(
                "the service could not read the written file back ({e}); Bambu Studio wrote it with the user's rights"
            )),
        },
        Err(e) => FileFacts {
            bytes: None,
            sha256: None,
            unavailable: Some(format!("the hash task failed ({e})")),
        },
    }
}

fn not_sliced(plate_index: u32, selected_plate_index: u32) -> ToolError {
    ToolError::new(
        ToolErrorCode::NotSliced,
        format!(
            "plate {plate_index} has no current slice result (selected plate: {selected_plate_index}); nothing was exported"
        ),
    )
    .with_details(json!({
        "plate_index": plate_index,
        "selected_plate_index": selected_plate_index,
        "hint": format!("run studio_slice with plate_id {plate_index}, then studio_export_plate"),
    }))
}

impl Tools {
    /// Steps 1 to 3 of the module docs. No driver call.
    fn destination(
        &self,
        raw: &str,
        kind: FileKind,
        overwrite: bool,
    ) -> Result<PathBuf, ToolError> {
        check_suffix(raw, kind)?;
        let guard = self
            .path_guard()
            .ok_or_else(|| path_not_allowed(PathError::OutsideRoots))?;
        let path = guard.check_destination(raw).map_err(path_not_allowed)?;
        // `check_destination` refused links; anything else that exists counts.
        // An entry this account cannot inspect is left to the driver.
        if !overwrite && std::fs::symlink_metadata(&path).is_ok() {
            return Err(file_exists());
        }
        Ok(path)
    }

    /// BUSY when Studio has a dialog, a UI job or a slice running.
    fn busy_error(reason: &str) -> ToolError {
        ToolError::new(
            ToolErrorCode::Busy,
            format!("Bambu Studio is busy ({reason}); nothing was written."),
        )
        .with_details(json!({ "reason": reason }))
    }

    /// Saves the project (native `.3mf`, not sliced) to a path inside the
    /// allowed directories. A `.gcode.3mf` name is refused (INVALID_ARGUMENT,
    /// reserved for `studio_export_plate`). Returns `{path, bytes, sha256,
    /// hash_unavailable_reason?, revision}`; the revision is the one after
    /// the save (saving changes the project path and the dirty flag).
    pub async fn studio_save_project(&self, a: SaveProjectArgs) -> Result<Value, ToolError> {
        self.revision.touch_client();
        let overwrite = a.overwrite.unwrap_or(false);
        let path =
            lowercase_project_suffix(self.destination(&a.path, FileKind::Project, overwrite)?);

        let (revision, facts) = {
            let _serial = self.write_lock.lock().await;
            let app = self
                .probe
                .require_ready(true, self.cfg.allow_version_mismatch)
                .await?;
            if let Some(reason) = busy_reason(&app) {
                return Err(Self::busy_error(reason));
            }
            self.driver
                .project_save(&path, overwrite)
                .await
                .map_err(file_write_error)?;
            let revision = self
                .revision
                .refresh(self.driver.as_ref())
                .await
                .map_err(|e| after_write(read_error(e)))?;
            // Still under the lock: no other file tool can replace the file
            // between the write and the read-back.
            (revision, file_facts(path.clone()).await)
        };

        let mut out = json!({
            "path": path.to_string_lossy(),
            "bytes": facts.bytes,
            "sha256": facts.sha256,
            "revision": revision,
        });
        if let Some(why) = facts.unavailable {
            out["hash_unavailable_reason"] = json!(why);
        }
        Ok(out)
    }

    /// Exports the current slice of a plate as `.gcode.3mf` to a path inside
    /// the allowed directories. The plate must be the selected one and carry
    /// a current, completed slice (`NOT_SLICED` otherwise, with a hint); no
    /// automatic select or re-slice. A plate index beyond the project's plates
    /// is `PLATE_NOT_FOUND`. Returns `{path, plate_index,
    /// slice_result_id, bytes, sha256, hash_unavailable_reason?, revision}`;
    /// `bytes` is the driver's count.
    pub async fn studio_export_plate(&self, a: ExportPlateArgs) -> Result<Value, ToolError> {
        self.revision.touch_client();
        let overwrite = a.overwrite.unwrap_or(false);
        let plate = a.plate_index;
        let path = self.destination(&a.path, FileKind::SlicedPlate, overwrite)?;

        let (exported, revision, facts) = {
            let _serial = self.write_lock.lock().await;
            let app = self
                .probe
                .require_ready(true, self.cfg.allow_version_mismatch)
                .await?;
            if let Some(reason) = busy_reason(&app) {
                return Err(Self::busy_error(reason));
            }
            if plate >= app.plate_count {
                return Err(ToolError::new(
                    ToolErrorCode::PlateNotFound,
                    format!(
                        "plate {plate} does not exist; the project has {} plate(s)",
                        app.plate_count
                    ),
                )
                .with_details(json!({ "plate_index": plate, "plate_count": app.plate_count })));
            }
            let selected = app.selected_plate_index;
            let slice = self.driver.slice_state().await.map_err(read_error)?;
            let result_id = match slice.slice_result_id {
                Some(id)
                    if slice.valid
                        && selected == plate
                        && slice.plate_index.unwrap_or(selected) == plate =>
                {
                    id
                }
                _ => return Err(not_sliced(plate, selected)),
            };
            let plates = self.driver.plate_list().await.map_err(read_error)?;
            let exported = self
                .driver
                .export_sliced_3mf(plate, &plates.plate_revision, &result_id, &path, overwrite)
                .await
                .map_err(|e| match e {
                    // The slice went stale (an edit) or the selection moved
                    // after the checks above: still "not sliced".
                    DriverError::StaleReference { .. } => not_sliced(plate, selected),
                    DriverError::InvalidParams { message }
                        if message.contains("selected plate") =>
                    {
                        not_sliced(plate, selected)
                    }
                    other => file_write_error(other),
                })?;
            let revision = self
                .revision
                .refresh(self.driver.as_ref())
                .await
                .map_err(|e| after_write(read_error(e)))?;
            // Read back under the lock (see the save).
            let facts = file_facts(path.clone()).await;
            (exported, revision, facts)
        };

        let mut out = json!({
            "path": path.to_string_lossy(),
            "plate_index": plate,
            "slice_result_id": exported.slice_result_id,
            "bytes": exported.bytes,
            "sha256": facts.sha256,
            "revision": revision,
        });
        if let Some(why) = facts.unavailable {
            out["hash_unavailable_reason"] = json!(why);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, data: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("bamburino-files-{}-{name}", std::process::id()));
        std::fs::write(&p, data).unwrap();
        p
    }

    #[test]
    fn hash_file_streams_across_chunks_and_matches_a_known_digest() {
        // 200 000 bytes = three full 64 KiB chunks and a partial one.
        let data = vec![b'a'; 200_000];
        let p = temp_file("stream.bin", &data);
        let want: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let got = hash_file(&p, MAX_HASH_BYTES).unwrap();
        std::fs::remove_file(&p).ok();
        assert_eq!(
            got,
            Hashed::Done {
                bytes: 200_000,
                sha256: want
            }
        );
        let empty = temp_file("empty.bin", b"");
        let got = hash_file(&empty, MAX_HASH_BYTES).unwrap();
        std::fs::remove_file(&empty).ok();
        assert_eq!(
            got,
            Hashed::Done {
                bytes: 0,
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into()
            }
        );
    }

    #[test]
    fn hash_file_above_the_cap_is_too_large_and_at_the_cap_is_hashed() {
        let p = temp_file("cap.bin", &[7u8; 1000]);
        assert_eq!(
            hash_file(&p, 999).unwrap(),
            Hashed::TooLarge { bytes: 1000 }
        );
        assert!(matches!(
            hash_file(&p, 1000).unwrap(),
            Hashed::Done { bytes: 1000, .. }
        ));
        std::fs::remove_file(&p).ok();
    }

    #[tokio::test]
    async fn file_facts_over_the_cap_reports_bytes_and_a_too_large_reason() {
        let p = temp_file("facts.bin", &[1u8; 5000]);
        let f = file_facts_capped(p.clone(), 4096).await;
        std::fs::remove_file(&p).ok();
        assert_eq!(f.bytes, Some(5000));
        assert_eq!(f.sha256, None);
        assert!(f.unavailable.unwrap().starts_with("too_large:"));
    }

    #[test]
    fn suffix_check_is_case_insensitive() {
        assert!(check_suffix(r"C:\a\X.GCODE.3MF", FileKind::SlicedPlate).is_ok());
        assert!(check_suffix(r"C:\a\x.gcode", FileKind::SlicedPlate).is_err());
        assert!(check_suffix(r"C:\a\x.3mf", FileKind::SlicedPlate).is_err());
        assert!(check_suffix(r"C:\a\A.3MF", FileKind::Project).is_ok());
        assert!(check_suffix(r"C:\a\a.gcode.3mf", FileKind::Project).is_err());
        assert!(check_suffix(r"C:\a\a.stl", FileKind::Project).is_err());
        // Multi-byte characters before the suffix do not split a char.
        assert!(check_suffix("C:\\a\\è.3mf", FileKind::Project).is_ok());
        assert!(check_suffix("è", FileKind::Project).is_err());
    }

    #[test]
    fn project_suffix_is_lowercased_only_at_the_end() {
        assert_eq!(
            lowercase_project_suffix(PathBuf::from(r"C:\Docs.3MF\A.3MF")),
            PathBuf::from(r"C:\Docs.3MF\A.3mf")
        );
    }

    #[test]
    fn every_path_error_has_a_snake_case_reason() {
        for (e, want) in [
            (PathError::Relative, "relative"),
            (PathError::Traversal, "traversal"),
            (PathError::Unc, "unc"),
            (PathError::DevicePath, "device_path"),
            (PathError::ReservedName, "reserved_name"),
            (PathError::AlternateDataStream, "alternate_data_stream"),
            (PathError::TrailingDotOrSpace, "trailing_dot_or_space"),
            (PathError::ReparsePoint, "reparse_point"),
            (PathError::OutsideRoots, "outside_roots"),
            (PathError::NotFound, "not_found"),
        ] {
            let err = path_not_allowed(e);
            assert_eq!(err.code, ToolErrorCode::PathNotAllowed);
            assert_eq!(err.details["reason"], want);
        }
    }
}
