// SPDX-License-Identifier: AGPL-3.0-or-later
//! File ACLs for the configuration directory (Windows, via `icacls`).
//!
//! The DACL is protected (no inheritance): `SYSTEM` and `Administrators`
//! full control, `LOCAL SERVICE` (the service account) read - or modify on a
//! directory such as `logs\` - and the installing user read or modify.

use std::io;
use std::path::Path;

/// What the installing user may do on a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerAccess {
    Read,
    Modify,
}

const SYSTEM_SID: &str = "S-1-5-18";
const ADMINS_SID: &str = "S-1-5-32-544";
const LOCAL_SERVICE_SID: &str = "S-1-5-19";

fn valid_sid(sid: &str) -> bool {
    let Some(rest) = sid.strip_prefix("S-1-") else {
        return false;
    };
    !rest.is_empty()
        && rest
            .split('-')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Builds the `icacls` grant arguments.
fn grant_args(is_dir: bool, owner_sid: &str, owner: OwnerAccess) -> Vec<String> {
    let inherit = if is_dir { "(OI)(CI)" } else { "" };
    let owner_perm = match owner {
        OwnerAccess::Read => "R",
        OwnerAccess::Modify => "M",
    };
    // `logs\` is written by the service; files are only read by it.
    let service_perm = if is_dir { "M" } else { "R" };
    let ace = |sid: &str, perm: &str| format!("*{sid}:{inherit}{perm}");
    vec![
        ace(SYSTEM_SID, "F"),
        ace(ADMINS_SID, "F"),
        ace(LOCAL_SERVICE_SID, service_perm),
        ace(owner_sid, owner_perm),
    ]
}

/// SID of the user running this process (`whoami /user`).
#[cfg(windows)]
pub fn current_user_sid() -> io::Result<String> {
    let out = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split('"')
        .find(|p| valid_sid(p))
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("cannot determine the current user SID"))
}

/// Non-Windows builds have no ACLs; the service is Windows-only.
#[cfg(not(windows))]
pub fn current_user_sid() -> io::Result<String> {
    Ok("S-1-5-21-0-0-0-1000".to_owned())
}

/// Replaces the DACL of `path` with the protected set described above.
#[cfg(windows)]
pub fn apply_acl(path: &Path, owner_sid: &str, owner_access: OwnerAccess) -> io::Result<()> {
    if !valid_sid(owner_sid) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "owner_sid is not a valid SID",
        ));
    }
    let mut cmd = std::process::Command::new("icacls");
    cmd.arg(path).arg("/inheritance:r");
    for a in grant_args(path.is_dir(), owner_sid, owner_access) {
        cmd.arg("/grant:r").arg(a);
    }
    let out = cmd.output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "icacls failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// No-op outside Windows.
#[cfg(not(windows))]
pub fn apply_acl(_path: &Path, owner_sid: &str, _owner_access: OwnerAccess) -> io::Result<()> {
    if valid_sid(owner_sid) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "owner_sid is not a valid SID",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sid_validation() {
        assert!(valid_sid("S-1-5-21-1-2-3-1001"));
        assert!(!valid_sid("S-1-5-18 /deny Everyone:F"));
        assert!(!valid_sid("Everyone"));
        assert!(!valid_sid("S-1-"));
    }

    #[test]
    fn grant_args_shape() {
        let f = grant_args(false, "S-1-5-21-1", OwnerAccess::Read);
        assert_eq!(f[2], "*S-1-5-19:R");
        assert_eq!(f[3], "*S-1-5-21-1:R");
        let d = grant_args(true, "S-1-5-21-1", OwnerAccess::Modify);
        assert_eq!(d[2], "*S-1-5-19:(OI)(CI)M");
    }
}
