// SPDX-License-Identifier: AGPL-3.0-or-later
//! Windows excluded TCP port ranges (`netsh interface ipv4 show
//! excludedportrange protocol=tcp`). Hyper-V, WinNAT and Docker reserve ranges
//! that no process may bind; a service whose port falls in one never starts.

use std::io;

use crate::config::Config;

/// Inclusive `(start, end)` ranges.
pub type Ranges = Vec<(u16, u16)>;

/// Source of the excluded ranges; injectable in tests.
pub type RangeProvider<'a> = &'a dyn Fn() -> io::Result<Ranges>;

/// Parses the `netsh` table: every line that starts with two numbers is a
/// range (header, rule, footnote and the `*` marker are ignored; the output
/// is numeric, so localized Windows versions parse too).
pub fn parse_excluded_ranges(text: &str) -> Ranges {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let start: u16 = it.next()?.parse().ok()?;
            let end: u16 = it.next()?.parse().ok()?;
            (start <= end).then_some((start, end))
        })
        .collect()
}

pub fn is_excluded(port: u16, ranges: &[(u16, u16)]) -> bool {
    ranges.iter().any(|&(s, e)| (s..=e).contains(&port))
}

/// Runs the real `netsh` (absolute System32 path, so nothing earlier in
/// `PATH` is run).
pub fn netsh_excluded_ranges() -> io::Result<Ranges> {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let exe = std::path::PathBuf::from(root)
        .join("System32")
        .join("netsh.exe");
    let out = std::process::Command::new(exe)
        .args([
            "interface",
            "ipv4",
            "show",
            "excludedportrange",
            "protocol=tcp",
        ])
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "netsh exited with {}",
            out.status
        )));
    }
    Ok(parse_excluded_ranges(&String::from_utf8_lossy(&out.stdout)))
}

/// Result of [`preflight`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Preflight {
    /// Set when the service cannot work: the caller must refuse.
    pub refusal: Option<String>,
    pub warnings: Vec<String>,
}

/// Text appended to every "port is reserved" message.
pub const REMEDY: &str = "change `listen_port` in config.toml, or free the range with `net stop winnat && net start winnat` (elevated prompt)";

/// Message for a bind that failed with Windows error 10013.
pub fn reserved_message(port: u16) -> String {
    format!("port {port} is reserved by Windows (excluded port range): {REMEDY}")
}

/// Checks `listen_port` (refusal) and the `driver_url` port (warning).
/// If the ranges cannot be listed, that is only a warning.
pub fn preflight(cfg: &Config, ranges: RangeProvider<'_>) -> Preflight {
    let mut out = Preflight::default();
    let ranges = match ranges() {
        Ok(r) => r,
        Err(e) => {
            out.warnings
                .push(format!("cannot list the excluded port ranges: {e}"));
            return out;
        }
    };
    if is_excluded(cfg.listen_port, &ranges) {
        out.refusal = Some(reserved_message(cfg.listen_port));
    }
    let driver_port = bamburino_core::detect::parse_driver_url(&cfg.driver_url)
        .ok()
        .and_then(|u| u.port_or_known_default());
    if let Some(p) = driver_port
        && is_excluded(p, &ranges)
    {
        out.warnings.push(format!(
            "driver port {p} (driver_url) is in a Windows excluded port range, so Bambu Studio cannot listen on it: set Studio's Preferences > Local MCP server port to 27601 and driver_url in config.toml to http://127.0.0.1:27601/mcp, or {REMEDY}"
        ));
    }
    out
}
