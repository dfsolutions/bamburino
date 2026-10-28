// SPDX-License-Identifier: AGPL-3.0-or-later
//! Token generation and secret files. A secret value is never part of an
//! error message, `Debug` output or log line.

use std::fs::File;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use crate::acl::OwnerAccess;
use crate::config::ConfigError;
use crate::secure::create_restricted_file;

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// 32 random bytes from the OS, base64url without padding (43 characters).
pub fn generate_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("OS random source unavailable");
    let mut out = String::with_capacity(43);
    for chunk in buf.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..=chunk.len() {
            out.push(B64URL[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// Name of the temp file used by [`write_secret_atomic`] for `path`.
fn temp_path(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
        .to_string_lossy()
        .into_owned();
    Ok(path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{name}.tmp")))
}

fn access_for(path: &Path) -> OwnerAccess {
    if path.file_name().is_some_and(|n| n == "driver.token") {
        OwnerAccess::Modify
    } else {
        OwnerAccess::Read
    }
}

/// Creates the temp file for `path`, already restricted (protected DACL from
/// creation, exclusive share mode) for `owner_sid`.
pub fn create_secret_temp(path: &Path, owner_sid: &str) -> io::Result<(File, PathBuf)> {
    let tmp = temp_path(path)?;
    let f = create_restricted_file(&tmp, owner_sid, access_for(path))?;
    Ok((f, tmp))
}

/// Writes `value` to a restricted temp file in the same directory, then
/// renames it over `path`. The ACE for the installing user uses `owner_sid`
/// (the configured owner). Needs write access to the directory (an
/// administrator, since the configuration root is locked down).
pub fn write_secret_atomic(path: &Path, value: &str, owner_sid: &str) -> io::Result<()> {
    let (mut f, tmp) = create_secret_temp(path, owner_sid)?;
    let result = (|| {
        f.write_all(value.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Overwrites an EXISTING secret file in place (no create, so no write access
/// to the directory is needed). Used by `config set-driver-token` as the
/// non-elevated installing user; the file's own ACL gates the write.
pub fn overwrite_secret(path: &Path, value: &str) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    f.write_all(value.as_bytes())?;
    f.sync_all()
}

/// Reads a secret file: `None` when absent or empty, whitespace trimmed.
pub fn read_secret(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let t = s.trim();
            Ok((!t.is_empty()).then(|| t.to_owned()))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Reads the driver token from one line of input: trimmed, 64 lowercase hex.
/// The rejected input is never echoed.
pub fn read_driver_token_from_stdin(mut input: impl BufRead) -> Result<String, ConfigError> {
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|_| ConfigError::Token("cannot read the token from stdin".into()))?;
    let t = line.trim();
    if t.len() == 64 && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(t.to_owned())
    } else {
        Err(ConfigError::Token(
            "driver token must be 64 lowercase hexadecimal characters on one line".into(),
        ))
    }
}
