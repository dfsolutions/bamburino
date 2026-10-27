// SPDX-License-Identifier: AGPL-3.0-or-later
//! Detection of a running Bamburino build of Bambu Studio and the
//! version/identity check against the pinned upstream.
//!
//! The identity of the build comes only from the driver's capabilities
//! (`appVersion`, `patchId`), never from the executable's file version.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::build_info;
use crate::driver::{AppState, DriverCapabilities, DriverClient, DriverError};
use crate::errors::{ToolError, ToolErrorCode};

/// How long a computed [`StudioStatus`] is reused by [`StatusProbe::status`].
const STATUS_TTL: Duration = Duration::from_secs(1);

const SET_TOKEN_HINT: &str = "Run: bamburino.exe config set-driver-token";

/// Tells whether a Bambu Studio process exists.
pub trait ProcessProbe: Send + Sync {
    fn studio_running(&self) -> bool;
}

/// Fixed answer, for tests.
#[cfg(feature = "mock")]
pub struct FixedProcessProbe(pub bool);

#[cfg(feature = "mock")]
impl ProcessProbe for FixedProcessProbe {
    fn studio_running(&self) -> bool {
        self.0
    }
}

/// What the running build says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity {
    pub base_version: String,
    pub patch_id: Option<String>,
}

pub fn build_identity(caps: &DriverCapabilities) -> BuildIdentity {
    BuildIdentity {
        base_version: caps.app_version.clone(),
        // An unconfigured build reports "unknown" or an empty id: no patch.
        patch_id: caps
            .patch_id
            .clone()
            .filter(|p| !p.is_empty() && p != "unknown"),
    }
}

/// True when the build matches the pinned upstream (and the pinned patch set,
/// when one is frozen).
pub fn version_match(id: &BuildIdentity) -> bool {
    version_match_against(
        id,
        build_info::STUDIO_VERSION,
        build_info::EXPECTED_PATCH_ID,
    )
}

fn version_match_against(id: &BuildIdentity, studio: &str, expected_patch: Option<&str>) -> bool {
    if id.base_version != studio {
        return false;
    }
    match expected_patch {
        None => true,
        Some(expected) => id.patch_id.as_deref() == Some(expected),
    }
}

/// Parses the driver URL, accepting only `http://127.0.0.1:<port>` or
/// `http://localhost:<port>` (the driver listens on IPv4 loopback only and
/// rejects other `Host` values).
pub fn parse_driver_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("invalid driver URL: {e}"))?;
    if url.scheme() != "http" {
        return Err("driver URL must use http".to_string());
    }
    let host_ok = url
        .host_str()
        .is_some_and(|h| h == "127.0.0.1" || h.eq_ignore_ascii_case("localhost"));
    if !host_ok {
        return Err("driver URL host must be 127.0.0.1 or localhost".to_string());
    }
    if url.port().is_none() {
        return Err("driver URL must include an explicit port".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("driver URL must not contain credentials".to_string());
    }
    Ok(url)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StudioStatus {
    NotRunning,
    DriverOff,
    Unauthorized,
    TokenMissing,
    Ready {
        app_version: String,
        identity: BuildIdentity,
        version_match: bool,
    },
}

/// Uncached probe result; `Ready` carries the fresh `app_state`.
enum Probe {
    Ready {
        state: AppState,
        identity: BuildIdentity,
        version_match: bool,
    },
    Other(StudioStatus),
}

impl Probe {
    fn into_status(self) -> StudioStatus {
        match self {
            Probe::Ready {
                state,
                identity,
                version_match,
            } => StudioStatus::Ready {
                app_version: state.app_version,
                identity,
                version_match,
            },
            Probe::Other(s) => s,
        }
    }
}

pub struct StatusProbe {
    driver: Arc<dyn DriverClient>,
    process: Arc<dyn ProcessProbe>,
    token_present: bool,
    /// `(session_id, identity)` of the last successful `capabilities()`.
    identity: Mutex<Option<(String, BuildIdentity)>>,
    cache: Mutex<Option<(Instant, StudioStatus)>>,
}

impl StatusProbe {
    /// `token_present` is false when no driver token is configured; the
    /// driver is then not contacted at all.
    pub fn new(
        driver: Arc<dyn DriverClient>,
        process: Arc<dyn ProcessProbe>,
        token_present: bool,
    ) -> Self {
        Self {
            driver,
            process,
            token_present,
            identity: Mutex::new(None),
            cache: Mutex::new(None),
        }
    }

    /// Current status, reused for one second.
    pub async fn status(&self) -> StudioStatus {
        if let Some((at, s)) = self.cache.lock().expect("cache poisoned").as_ref()
            && at.elapsed() < STATUS_TTL
        {
            return s.clone();
        }
        let status = self.probe().await.into_status();
        *self.cache.lock().expect("cache poisoned") = Some((Instant::now(), status.clone()));
        status
    }

    /// Fails unless Studio is ready. Always asks the driver for a fresh
    /// `app_state` (never served from the status cache) and returns it.
    pub async fn require_ready(
        &self,
        for_write: bool,
        allow_mismatch: bool,
    ) -> Result<AppState, ToolError> {
        match self.probe().await {
            Probe::Other(StudioStatus::NotRunning) => Err(ToolError::new(
                ToolErrorCode::StudioNotRunning,
                "Bambu Studio is not running. Open the Bamburino build of Bambu Studio.",
            )),
            Probe::Other(StudioStatus::DriverOff) => Err(ToolError::new(
                ToolErrorCode::DriverDisabled,
                "Bambu Studio is open but the Local MCP server is off (Preferences > Local MCP server).",
            )),
            Probe::Other(StudioStatus::Unauthorized) => Err(ToolError::new(
                ToolErrorCode::DriverUnauthorized,
                SET_TOKEN_HINT,
            )),
            Probe::Other(_) => Err(ToolError::new(
                ToolErrorCode::DriverTokenMissing,
                SET_TOKEN_HINT,
            )),
            Probe::Ready {
                state,
                identity,
                version_match,
            } => {
                if for_write && !version_match && !allow_mismatch {
                    return Err(ToolError::new(
                        ToolErrorCode::VersionMismatch,
                        "The running Bambu Studio does not match the build Bamburino expects.",
                    )
                    .with_details(json!({
                        "expected": {
                            "baseVersion": build_info::STUDIO_VERSION,
                            "patchId": build_info::EXPECTED_PATCH_ID,
                        },
                        "found": {
                            "baseVersion": identity.base_version,
                            "patchId": identity.patch_id,
                        },
                    })));
                }
                Ok(state)
            }
        }
    }

    async fn probe(&self) -> Probe {
        if !self.token_present {
            return Probe::Other(StudioStatus::TokenMissing);
        }
        let state = match self.driver.app_state().await {
            Ok(s) => s,
            Err(e) => return Probe::Other(self.classify(e)),
        };
        let cached = self
            .identity
            .lock()
            .expect("identity poisoned")
            .as_ref()
            .filter(|(sid, _)| *sid == state.session_id)
            .map(|(_, id)| id.clone());
        let identity = match cached {
            Some(id) => id,
            None => match self.driver.capabilities().await {
                Ok(caps) => {
                    let id = build_identity(&caps);
                    *self.identity.lock().expect("identity poisoned") =
                        Some((state.session_id.clone(), id.clone()));
                    id
                }
                Err(e) => return Probe::Other(self.classify(e)),
            },
        };
        let version_match = version_match(&identity);
        Probe::Ready {
            state,
            identity,
            version_match,
        }
    }

    /// Maps a failed driver call. Everything except an explicit rejection of
    /// the credentials means "no usable driver": whether Studio is running at
    /// all decides between `NotRunning` and `DriverOff`.
    fn classify(&self, err: DriverError) -> StudioStatus {
        match err {
            DriverError::Unauthorized => StudioStatus::Unauthorized,
            _ if self.process.studio_running() => StudioStatus::DriverOff,
            _ => StudioStatus::NotRunning,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(base: &str, patch: Option<&str>) -> BuildIdentity {
        BuildIdentity {
            base_version: base.to_string(),
            patch_id: patch.map(str::to_string),
        }
    }

    #[test]
    fn match_rules_against_explicit_pins() {
        let ok = id("02.08.04.57", Some("aaaaaaaaaaaa"));
        assert!(version_match_against(
            &ok,
            "02.08.04.57",
            Some("aaaaaaaaaaaa")
        ));
        assert!(!version_match_against(
            &ok,
            "02.08.04.57",
            Some("bbbbbbbbbbbb")
        ));
        assert!(!version_match_against(
            &id("02.08.04.57", None),
            "02.08.04.57",
            Some("a")
        ));
        assert!(version_match_against(
            &id("02.08.04.57", None),
            "02.08.04.57",
            None
        ));
        assert!(!version_match_against(&ok, "02.08.02.61", None));
    }

    #[test]
    fn driver_url_accepts_only_ipv4_loopback_and_localhost() {
        assert!(parse_driver_url("http://127.0.0.1:27183/mcp").is_ok());
        assert!(parse_driver_url("http://localhost:27183").is_ok());
        for bad in [
            "https://127.0.0.1:27183",
            "http://127.0.0.2:27183",
            "http://[::1]:27183",
            "http://192.168.1.5:27183",
            "http://example.com:27183",
            "http://127.0.0.1",
            "http://user:pw@127.0.0.1:27183",
            "nonsense",
        ] {
            assert!(parse_driver_url(bad).is_err(), "{bad}");
        }
    }
}
