// SPDX-License-Identifier: AGPL-3.0-or-later
//! Service logging: daily-rotated file under `%ProgramData%\Bamburino\logs\`
//! (optionally mirrored to stderr) with secret redaction.
//!
//! Redaction sits at the writer, after formatting, so it covers every event
//! field, every span field, the message and anything a caller forgot to
//! sanitise. Callers still pass external input through [`redact`] first.
//!
//! Policy for call sites: never log request bodies or headers at `info`;
//! settings values appear at `debug` only, and are redacted like the rest.

use std::borrow::Cow;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use tracing::Subscriber;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;

const TOKEN: &str = "<redacted-token>";
const BEARER: &str = "Bearer <redacted>";
const UNC: &str = "<unc-path>";
/// An identifier-like run this long is treated as a credential.
const MIN_SECRET_RUN: usize = 32;
/// Configured secrets shorter than this are not matched literally.
const MIN_LITERAL_SECRET: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("a global log subscriber is already installed")]
    AlreadyInitialized,
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Masks credentials and network paths in `input`:
/// - `Bearer <anything>` becomes `Bearer <redacted>`;
/// - any run of 32 or more `[A-Za-z0-9_-]` characters (this covers 64-hex
///   tokens and base64url tokens) becomes `<redacted-token>`;
/// - UNC paths (`\\server\share...`, `\\?\UNC\...`, also in the escaped
///   `\\\\server` form of `{:?}`) become `<unc-path>`.
pub fn redact(input: &str) -> Cow<'_, str> {
    let b = input.as_bytes();
    let mut out = String::new();
    let mut changed = false;
    let mut copied = 0; // input[..copied] is already in `out`
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let (end, replacement): (usize, Option<&str>) = if is_token_byte(b[i]) {
            let mut j = i;
            while j < b.len() && is_token_byte(b[j]) {
                j += 1;
            }
            if input[i..j].eq_ignore_ascii_case("bearer") {
                match bearer_credential_end(b, j) {
                    Some(e) => (e, Some(BEARER)),
                    None => (j, None),
                }
            } else if j - i >= MIN_SECRET_RUN {
                (j, Some(TOKEN))
            } else {
                (j, None)
            }
        } else if b[i] == b'\\' && unc_may_start(b, i) {
            match unc_end(b, i) {
                Some(e) => (e, Some(UNC)),
                None => (skip_backslashes(b, i), None),
            }
        } else {
            (i + 1, None)
        };
        if let Some(r) = replacement {
            out.push_str(&input[copied..start]);
            out.push_str(r);
            copied = end;
            changed = true;
        }
        i = end;
    }
    if !changed {
        return Cow::Borrowed(input);
    }
    out.push_str(&input[copied..]);
    Cow::Owned(out)
}

/// End of the credential after `Bearer` ending at `j`: horizontal blanks then
/// a run up to whitespace, a quote or a comma.
fn bearer_credential_end(b: &[u8], j: usize) -> Option<usize> {
    let mut k = j;
    while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
        k += 1;
    }
    if k == j {
        return None;
    }
    let s = k;
    while k < b.len() && !b[k].is_ascii_whitespace() && !matches!(b[k], b'"' | b'\'' | b',') {
        k += 1;
    }
    (k > s).then_some(k)
}

fn skip_backslashes(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] == b'\\' {
        i += 1;
    }
    i
}

/// A UNC path starts at a backslash run that does not follow a drive letter
/// (`C:\\`) or a word character.
fn unc_may_start(b: &[u8], i: usize) -> bool {
    i == 0 || !(b[i - 1].is_ascii_alphanumeric() || matches!(b[i - 1], b':' | b'\\' | b'.'))
}

fn unc_end(b: &[u8], i: usize) -> Option<usize> {
    let after = skip_backslashes(b, i);
    if after - i < 2 || after >= b.len() || b[after].is_ascii_whitespace() {
        return None;
    }
    if b[after] == b'?' {
        // `\\?\UNC\...` is a network path; `\\?\C:\...` is a local one.
        let rest = &b[after + 1..];
        let rest = &rest[rest.iter().take_while(|c| **c == b'\\').count()..];
        if rest.len() < 4 || !rest[..3].eq_ignore_ascii_case(b"unc") || rest[3] != b'\\' {
            return None;
        }
    }
    let mut k = after;
    // Stop at whitespace (a path with spaces keeps its tail: over-redacting
    // the head is the safe side) and at quote characters.
    while k < b.len() && !b[k].is_ascii_whitespace() && !matches!(b[k], b'"' | b'\'') {
        k += 1;
    }
    Some(k)
}

/// [`redact`] plus literal masking of configured secret values.
#[derive(Default, Clone)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Redactor({} secrets)", self.secrets.len())
    }
}

impl Redactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .filter(|s| s.len() >= MIN_LITERAL_SECRET)
            .collect();
        // Longest first, so a secret containing another is masked whole.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Self { secrets }
    }

    pub fn redact<'a>(&self, input: &'a str) -> Cow<'a, str> {
        let mut text = Cow::Borrowed(input);
        for s in &self.secrets {
            if text.contains(s.as_str()) {
                text = Cow::Owned(text.replace(s.as_str(), TOKEN));
            }
        }
        match redact(&text) {
            Cow::Borrowed(_) => text,
            Cow::Owned(o) => Cow::Owned(o),
        }
    }
}

/// Collects one formatted event and writes it, redacted, when dropped.
pub struct RedactingWriter<W: Write> {
    inner: W,
    buf: Vec<u8>,
    redactor: Arc<Redactor>,
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<W: Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.buf);
        let clean = self.redactor.redact(&text);
        let _ = self.inner.write_all(clean.as_bytes());
        let _ = self.inner.flush();
    }
}

pub struct RedactingMakeWriter<M> {
    inner: M,
    redactor: Arc<Redactor>,
}

impl<'a, M: MakeWriter<'a>> MakeWriter<'a> for RedactingMakeWriter<M> {
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter {
            inner: self.inner.make_writer(),
            buf: Vec::new(),
            redactor: self.redactor.clone(),
        }
    }
}

/// A plain-text `fmt` layer writing to `make_writer` through `redactor`.
pub fn layer<S, M>(make_writer: M, redactor: Arc<Redactor>) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    M: for<'a> MakeWriter<'a> + 'static,
{
    fmt::layer()
        .with_ansi(false)
        .with_writer(RedactingMakeWriter {
            inner: make_writer,
            redactor,
        })
}

fn level_from_env() -> LevelFilter {
    std::env::var("BAMBURINO_LOG")
        .ok()
        .and_then(|v| v.trim().parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::INFO)
}

/// Applies the service ACL to a log directory for `owner_sid`.
pub type AclFn<'a> = &'a dyn Fn(&Path, &str) -> io::Result<()>;

fn real_acl(dir: &Path, owner_sid: &str) -> io::Result<()> {
    // `apply_acl` only adds grants: clear what an existing dir carries first.
    crate::acl::reset_acl(dir)?;
    crate::acl::apply_acl(dir, owner_sid, crate::acl::OwnerAccess::Modify)?;
    crate::acl::reset_children_acl(dir)
}

/// Why an SDDL string (owner + DACL) is not an acceptable log directory ACL:
/// the DACL must be protected, grant SYSTEM, Administrators, LOCAL SERVICE
/// and `owner_sid`, and grant nothing to Everyone, Users, Authenticated
/// Users or Interactive.
fn sddl_problem(sddl: &str, owner_sid: &str) -> Option<String> {
    // No DACL segment: fail closed.
    let Some(dacl) = sddl.split("D:").nth(1) else {
        return Some("it has no DACL".to_owned());
    };
    if !dacl.starts_with('P') {
        return Some("its DACL is not protected".to_owned());
    }
    let canon = |t: &str| match t {
        "S-1-5-18" => "SY".to_owned(),
        "S-1-5-32-544" => "BA".to_owned(),
        "S-1-5-19" => "LS".to_owned(),
        "S-1-1-0" => "WD".to_owned(),
        "S-1-5-32-545" => "BU".to_owned(),
        "S-1-5-11" => "AU".to_owned(),
        "S-1-5-4" => "IU".to_owned(),
        other => other.to_owned(),
    };
    // Each ACE reads `(type;flags;rights;;;trustee)`.
    let trustees: Vec<String> = dacl
        .split('(')
        .skip(1)
        .filter_map(|ace| ace.trim_end_matches(')').split(';').nth(5).map(canon))
        .collect();
    if let Some(t) = trustees
        .iter()
        .find(|t| matches!(t.as_str(), "WD" | "BU" | "AU" | "IU"))
    {
        return Some(format!("it grants access to a broad group ({t})"));
    }
    for need in ["SY", "BA", "LS"]
        .map(str::to_owned)
        .into_iter()
        .chain([canon(owner_sid)])
    {
        if !trustees.contains(&need) {
            return Some(format!("it has no entry for {need}"));
        }
    }
    None
}

/// Why `dir` does not carry the expected protected ACL, if it does not.
fn dir_problem(dir: &Path, owner_sid: &str) -> Option<String> {
    #[cfg(windows)]
    {
        match crate::secure::security_sddl(dir) {
            Err(e) => Some(format!("cannot read its security descriptor: {e}")),
            Ok(sddl) => sddl_problem(&sddl, owner_sid),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (dir, owner_sid);
        None
    }
}

/// Makes sure `dir` exists and carries the protected service ACL, then opens
/// the rotating appender. Nothing is written unless the ACL is in place: `Err`
/// carries the reason for the stderr fallback.
fn open_appender(
    dir: &Path,
    retention_days: u32,
    owner_sid: &str,
    acl: AclFn<'_>,
) -> Result<RollingFileAppender, String> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    // Log files inherit the directory ACL; (re)apply it, with the files
    // already in the directory, whenever it is not exactly the expected one.
    if dir_problem(dir, owner_sid).is_some() {
        acl(dir, owner_sid)
            .map_err(|e| format!("cannot restrict access to {}: {e}", dir.display()))?;
        if let Some(why) = dir_problem(dir, owner_sid) {
            return Err(format!("{} is still not protected: {why}", dir.display()));
        }
    }
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("bamburino")
        .filename_suffix("log")
        .max_log_files(retention_days.max(1) as usize)
        .build(dir)
        .map_err(|e| format!("cannot open the log file in {}: {e}", dir.display()))
}

/// The subscriber `init_logging` installs, without installing it. If the log
/// directory is unusable or cannot be protected, the subscriber logs to
/// stderr only (with a warning on stderr) and never writes a file: a logging
/// problem never stops the service.
pub fn build_subscriber(
    dir: &Path,
    retention_days: u32,
    to_stderr: bool,
    owner_sid: &str,
    secrets: Vec<String>,
) -> (Box<dyn Subscriber + Send + Sync>, WorkerGuard) {
    build_subscriber_with_acl(
        dir,
        retention_days,
        to_stderr,
        owner_sid,
        secrets,
        &real_acl,
    )
}

/// [`build_subscriber`] with an injectable ACL step (tests).
pub fn build_subscriber_with_acl(
    dir: &Path,
    retention_days: u32,
    to_stderr: bool,
    owner_sid: &str,
    secrets: Vec<String>,
    acl: AclFn<'_>,
) -> (Box<dyn Subscriber + Send + Sync>, WorkerGuard) {
    let redactor = Arc::new(Redactor::new(secrets));
    let registry = tracing_subscriber::registry().with(level_from_env());
    match open_appender(dir, retention_days, owner_sid, acl) {
        Ok(appender) => {
            let (nb, guard): (NonBlocking, WorkerGuard) = tracing_appender::non_blocking(appender);
            let stderr = to_stderr.then(|| layer(io::stderr, redactor.clone()));
            let sub = registry.with(layer(nb, redactor)).with(stderr);
            (Box::new(sub), guard)
        }
        Err(reason) => {
            eprintln!("warning: file logging disabled, using stderr: {reason}");
            let (nb, guard) = tracing_appender::non_blocking(io::stderr());
            (Box::new(registry.with(layer(nb, redactor))), guard)
        }
    }
}

/// Installs the global subscriber. `owner_sid` is the configured installing
/// user (`Config::owner_sid`), who keeps Modify on the log directory. Keep the
/// guard alive for the process lifetime: dropping it flushes the file.
pub fn init_logging(
    dir: &Path,
    retention_days: u32,
    to_stderr: bool,
    owner_sid: &str,
) -> Result<WorkerGuard, LogError> {
    init_logging_with_secrets(dir, retention_days, to_stderr, owner_sid, Vec::new())
}

/// [`init_logging`] that also masks the given literal secret values (the
/// client and driver tokens read from their files).
pub fn init_logging_with_secrets(
    dir: &Path,
    retention_days: u32,
    to_stderr: bool,
    owner_sid: &str,
    secrets: Vec<String>,
) -> Result<WorkerGuard, LogError> {
    let (sub, guard) = build_subscriber(dir, retention_days, to_stderr, owner_sid, secrets);
    tracing::subscriber::set_global_default(sub).map_err(|_| LogError::AlreadyInitialized)?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::sddl_problem;

    #[test]
    fn sddl_without_a_dacl_is_a_problem() {
        let p = sddl_problem("O:SYG:SY", "S-1-5-21-1-2-3-1001");
        assert_eq!(p.as_deref(), Some("it has no DACL"));
    }

    #[test]
    fn protected_dacl_with_the_expected_entries_is_fine() {
        let owner = "S-1-5-21-1-2-3-1001";
        let sddl =
            format!("O:SYD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;LS)(A;OICI;FA;;;{owner})");
        assert_eq!(sddl_problem(&sddl, owner), None);
    }
}
