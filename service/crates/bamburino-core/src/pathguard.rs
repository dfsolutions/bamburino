// SPDX-License-Identifier: AGPL-3.0-or-later
//! Path allowlist (design section 8): a candidate path is accepted only if it
//! is an absolute, plain, drive-letter path strictly inside one of the allowed
//! roots, with no traversal, UNC or device syntax, no alternate data stream,
//! no reserved name, no trailing dot or space and no symlink or junction on
//! the way.
//!
//! The rules are rewritten from `Workspace.cs` and `BoundaryTests.cs` in
//! `Ding-Ding-Projects/BambuStudio` (AGPL-3.0, see `NOTICE.md`).
//!
//! Error values carry no path: callers pass paths through `redact` when they
//! log them.
//!
//! Root containment compares names exactly except for ASCII letter case: a
//! non-ASCII letter must be spelled as in `config.toml` (`Città` is not
//! `CITTÀ`). This can only reject a path NTFS would accept, never match a
//! different folder; agents should use the root exactly as configured.
//!
//! # Best-effort link inspection
//!
//! The rules are lexical (always enforced, never skipped) plus a walk of the
//! file system that refuses symlinks and junctions. The service may run as
//! `LocalService`, which usually cannot list the owner's profile folders.
//! Where the walk of a ROOT or of a DESTINATION hits "access denied" it stops
//! inspecting, the caller logs a warning, and the lexically valid path is
//! accepted: the driver, which runs as the user, validates the directory
//! itself (it must exist) and the write happens with the user's rights. Such
//! a root is kept and listed by [`PathGuard::unverified_roots`]. Every other
//! failure (a missing component, a link, a file used as a directory) still
//! refuses, and [`PathGuard::check`] (existing entries) stays strict.

use std::path::{Path, PathBuf};

/// Why a path (or a configured root) was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// Not an absolute `X:\...` path (also: empty, control characters).
    #[error("the path is not an absolute drive path")]
    Relative,
    /// Contains a `..` component.
    #[error("the path contains a `..` component")]
    Traversal,
    /// `\\server\share` or `\\?\UNC\...`.
    #[error("network (UNC) paths are not allowed")]
    Unc,
    /// `\\.\...`, `\\?\GLOBALROOT`, `\??\...` and other device namespaces.
    #[error("device paths are not allowed")]
    DevicePath,
    /// `CON`, `NUL`, `COM1`... (with any extension) or a wildcard/reserved
    /// character in a component.
    #[error("the path contains a reserved device name or character")]
    ReservedName,
    /// `file.txt:stream`.
    #[error("alternate data streams are not allowed")]
    AlternateDataStream,
    /// A component ends with `.` or a space (Windows would silently strip it).
    #[error("a path component ends with a dot or a space")]
    TrailingDotOrSpace,
    /// A symlink or junction (mount point) in the path.
    #[error("symlinks and junctions are not allowed in the path")]
    ReparsePoint,
    /// Not strictly inside any allowed root (or no root configured).
    #[error("the path is outside the allowed directories")]
    OutsideRoots,
    /// A component does not exist (for a root: also "is not a directory").
    #[error("the path does not exist")]
    NotFound,
}

/// The allowlist of directories.
#[derive(Debug, Clone)]
pub struct PathGuard {
    roots: Vec<PathBuf>,
    /// Roots accepted on lexical grounds only (access denied while walking).
    unverified: Vec<PathBuf>,
}

impl PathGuard {
    /// Never fails: every invalid root (relative, missing, not a directory,
    /// UNC or device syntax, symlink/junction on the way) is excluded and
    /// returned with the reason, for the caller to log as a warning. A guard
    /// with no root rejects everything with `OutsideRoots`.
    pub fn new(roots: Vec<PathBuf>) -> (PathGuard, Vec<(PathBuf, PathError)>) {
        let mut valid = Vec::new();
        let mut unverified = Vec::new();
        let mut excluded = Vec::new();
        for root in roots {
            match validate_root(&root) {
                Ok(w) => {
                    if !w.verified {
                        unverified.push(w.path.clone());
                    }
                    valid.push(w.path);
                }
                Err(e) => excluded.push((root, e)),
            }
        }
        (
            PathGuard {
                roots: valid,
                unverified,
            },
            excluded,
        )
    }

    /// Accepted roots this process could not fully inspect (access denied,
    /// typically the owner's profile seen from `LocalService`). They are
    /// included in [`Self::roots`]; the caller logs them as a warning.
    pub fn unverified_roots(&self) -> &[PathBuf] {
        &self.unverified
    }

    /// The accepted roots (normalized).
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Accepts `candidate` only if it names an existing entry strictly inside
    /// an allowed root; returns it normalized (`X:\a\b`, no `\\?\` prefix).
    pub fn check(&self, candidate: &str) -> Result<PathBuf, PathError> {
        let parsed = parse(candidate)?;
        if !self.is_inside(&parsed) {
            return Err(PathError::OutsideRoots);
        }
        walk(&parsed, Inspect::Strict).map(|w| w.path)
    }

    /// For a file that may not exist yet (a write destination). Same lexical
    /// rules as [`Self::check`]; the file must be strictly inside an allowed
    /// root (the root itself is refused), the PARENT must exist and be a
    /// plain directory with no symlink or junction on the way, and an
    /// existing target must not be a symlink or junction. Returns the
    /// normalized path (`X:\a\b.3mf`, no `\\?\` prefix).
    ///
    /// Best effort (module docs): where this process gets "access denied"
    /// the inspection stops with a warning and the lexically valid path is
    /// returned; the driver then validates the directory as the user.
    pub fn check_destination(&self, candidate: &str) -> Result<PathBuf, PathError> {
        let parsed = parse(candidate)?;
        if !self.is_inside(&parsed) {
            return Err(PathError::OutsideRoots);
        }
        let Some((name, parent_comps)) = parsed.comps.split_last() else {
            // Unreachable: strictly inside a root means one component at least.
            return Err(PathError::OutsideRoots);
        };
        let parent = Parsed {
            drive: parsed.drive,
            comps: parent_comps.to_vec(),
        };
        let walked = walk(&parent, Inspect::BestEffort)?;
        let mut target = walked.path.to_string_lossy().into_owned();
        if !target.ends_with('\\') {
            target.push('\\');
        }
        target.push_str(name);
        if !walked.verified {
            tracing::warn!(
                "pathguard: this account cannot inspect the destination directory; Bambu Studio validates it"
            );
            return Ok(PathBuf::from(target));
        }
        if !walked.path.is_dir() {
            return Err(PathError::NotFound);
        }
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if is_rejected_link(&target, &meta) => Err(PathError::ReparsePoint),
            Ok(_) => Ok(PathBuf::from(target)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PathBuf::from(target)),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                tracing::warn!(
                    "pathguard: this account cannot inspect the destination file; Bambu Studio validates it"
                );
                Ok(PathBuf::from(target))
            }
            Err(_) => Err(PathError::NotFound),
        }
    }

    fn is_inside(&self, parsed: &Parsed) -> bool {
        self.roots.iter().any(|r| {
            parse(&r.to_string_lossy())
                .map(|rp| rp.contains(parsed))
                .unwrap_or(false)
        })
    }
}

fn validate_root(root: &Path) -> Result<Walked, PathError> {
    let text = root.to_str().ok_or(PathError::Relative)?;
    let parsed = parse(text)?;
    let walked = walk(&parsed, Inspect::BestEffort)?;
    if walked.verified && !walked.path.is_dir() {
        return Err(PathError::NotFound);
    }
    Ok(walked)
}

/// A lexically valid absolute drive path.
#[derive(Debug)]
struct Parsed {
    drive: char,
    comps: Vec<String>,
}

impl Parsed {
    /// True if `other` is strictly inside `self` (names compared by
    /// [`same_name`]: exact except for ASCII letter case).
    fn contains(&self, other: &Parsed) -> bool {
        self.drive == other.drive
            && other.comps.len() > self.comps.len()
            && self
                .comps
                .iter()
                .zip(&other.comps)
                .all(|(a, b)| same_name(a, b))
    }

    fn root_string(&self) -> String {
        format!("{}:\\", self.drive)
    }
}

/// Name equality for root containment: exact, except for ASCII letter case.
/// Any non-ASCII difference is a mismatch, even one NTFS would fold
/// (`À`/`à`): that can only cause a false rejection. Unicode case mapping is
/// deliberately NOT used, because it equates names NTFS keeps apart
/// (`ß`/`SS`, `ı`/`i`, `ſ`/`s`, `µ`/`μ`, `ς`/`σ`), which would let a path
/// into a sibling directory outside the root.
fn same_name(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.eq_ignore_ascii_case(b)
}

fn is_drive_form(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

fn parse(input: &str) -> Result<Parsed, PathError> {
    if input.is_empty() || input.chars().any(char::is_control) {
        return Err(PathError::Relative);
    }
    let normalized = input.replace('/', "\\");
    let mut s: &str = &normalized;

    if let Some(rest) = s.strip_prefix("\\\\") {
        if let Some(r) = rest.strip_prefix("?\\") {
            if r.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("UNC\\")) {
                return Err(PathError::Unc);
            }
            if is_drive_form(r) {
                s = r; // `\\?\C:\...` long-path form of a plain drive path
            } else {
                return Err(PathError::DevicePath);
            }
        } else if rest.starts_with('.') || rest.starts_with('?') {
            return Err(PathError::DevicePath);
        } else {
            return Err(PathError::Unc);
        }
    } else if s.starts_with("\\??\\") {
        return Err(PathError::DevicePath);
    }

    if !is_drive_form(s) {
        return Err(PathError::Relative);
    }
    let drive = s.chars().next().expect("checked").to_ascii_uppercase();
    let mut comps = Vec::new();
    for c in s[3..].split('\\') {
        match c {
            "" | "." => {}
            ".." => return Err(PathError::Traversal),
            _ => {
                check_component(c)?;
                comps.push(c.to_owned());
            }
        }
    }
    Ok(Parsed { drive, comps })
}

fn check_component(c: &str) -> Result<(), PathError> {
    if c.contains(':') {
        return Err(PathError::AlternateDataStream);
    }
    if c.contains(['<', '>', '"', '|', '?', '*']) {
        return Err(PathError::ReservedName);
    }
    if c.ends_with('.') || c.ends_with(' ') {
        return Err(PathError::TrailingDotOrSpace);
    }
    let stem = c
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_uppercase();
    if is_reserved_stem(&stem) {
        return Err(PathError::ReservedName);
    }
    Ok(())
}

fn is_reserved_stem(stem: &str) -> bool {
    if matches!(
        stem,
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) {
        return true;
    }
    for prefix in ["COM", "LPT"] {
        if let Some(n) = stem.strip_prefix(prefix) {
            let mut it = n.chars();
            if let (Some(d), None) = (it.next(), it.next()) {
                // 1-9 and the superscript digits Windows also reserves.
                return matches!(d, '1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}');
            }
        }
    }
    false
}

/// How [`walk`] treats a component this process may not inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Inspect {
    /// Any failure to read a component refuses (`NotFound`).
    Strict,
    /// "Access denied" ends the inspection; the rest is accepted lexically.
    BestEffort,
}

/// What [`walk`] returns: the normalized path, and whether every component
/// was inspected.
#[derive(Debug)]
struct Walked {
    path: PathBuf,
    verified: bool,
}

/// Walks the path from the drive root, refusing symlinks and junctions at
/// every level; every component must exist. In `BestEffort` mode an "access
/// denied" ends the inspection: the remaining components (already checked
/// lexically by `parse`) are appended as they are and `verified` is false.
fn walk(p: &Parsed, mode: Inspect) -> Result<Walked, PathError> {
    let mut cur = p.root_string();
    let last = p.comps.len();
    let mut verified = true;
    for (i, c) in p.comps.iter().enumerate() {
        if !cur.ends_with('\\') {
            cur.push('\\');
        }
        cur.push_str(c);
        if !verified {
            continue;
        }
        let meta = match std::fs::symlink_metadata(&cur) {
            Ok(m) => m,
            Err(e)
                if mode == Inspect::BestEffort
                    && e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                verified = false;
                continue;
            }
            Err(_) => return Err(PathError::NotFound),
        };
        if is_rejected_link(&cur, &meta) {
            return Err(PathError::ReparsePoint);
        }
        if i + 1 < last && !meta.is_dir() {
            return Err(PathError::NotFound);
        }
    }
    Ok(Walked {
        path: PathBuf::from(cur),
        verified,
    })
}

#[cfg(windows)]
fn is_rejected_link(path: &str, meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
        return false;
    }
    // Other tags (OneDrive cloud placeholders, dedup, ...) are accepted. If
    // the tag cannot be read, refuse.
    match reparse_tag(path) {
        Some(tag) => tag == IO_REPARSE_TAG_MOUNT_POINT || tag == IO_REPARSE_TAG_SYMLINK,
        None => true,
    }
}

#[cfg(windows)]
fn reparse_tag(path: &str) -> Option<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};

    let wide: Vec<u16> = std::ffi::OsStr::new(&format!("\\\\?\\{path}"))
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: a zeroed WIN32_FIND_DATAW is a valid out-parameter; `wide` is
    // NUL-terminated; the search handle is closed before returning.
    unsafe {
        let mut data: WIN32_FIND_DATAW = std::mem::zeroed();
        let h = FindFirstFileW(wide.as_ptr(), &mut data);
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        FindClose(h);
        Some(data.dwReserved0)
    }
}

#[cfg(not(windows))]
fn is_rejected_link(_path: &str, meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}
