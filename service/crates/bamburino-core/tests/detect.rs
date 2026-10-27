// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::sync::Arc;

use bamburino_core::build_info::{EXPECTED_PATCH_ID, STUDIO_VERSION};
use bamburino_core::detect::{
    BuildIdentity, FixedProcessProbe, StatusProbe, StudioStatus, build_identity, version_match,
};
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::driver::{DriverCapabilities, DriverError};
use bamburino_core::errors::ToolErrorCode;

const WRONG_PATCH: &str = "ffffffffffff";

fn caps(app_version: &str, patch: Option<&str>) -> DriverCapabilities {
    DriverCapabilities {
        app_version: app_version.to_string(),
        build_suffix: Some("-bamburino.test".to_string()),
        patch_id: patch.map(str::to_string),
        raw: serde_json::json!({ "appVersion": app_version, "patchId": patch }),
    }
}

fn good_patch() -> String {
    EXPECTED_PATCH_ID.unwrap_or("0123456789ab").to_string()
}

fn probe(mock: &Arc<MockDriver>, running: bool) -> StatusProbe {
    StatusProbe::new(mock.clone(), Arc::new(FixedProcessProbe(running)), true)
}

#[test]
fn identity_from_capabilities() {
    let id = build_identity(&caps(STUDIO_VERSION, Some(&good_patch())));
    assert_eq!(id.base_version, STUDIO_VERSION);
    assert!(version_match(&id));
}

#[test]
fn identity_wrong_patch_is_mismatch() {
    let id = build_identity(&caps(STUDIO_VERSION, Some(WRONG_PATCH)));
    // Without a frozen patch only the base version counts.
    assert_eq!(version_match(&id), EXPECTED_PATCH_ID.is_none());
}

#[test]
fn identity_missing_patch_is_mismatch_when_expected() {
    for patch in [None, Some("unknown"), Some("")] {
        let id = build_identity(&caps(STUDIO_VERSION, patch));
        assert_eq!(id.patch_id, None);
        assert_eq!(version_match(&id), EXPECTED_PATCH_ID.is_none());
    }
}

#[test]
fn identity_wrong_base_is_mismatch() {
    let id = build_identity(&caps("02.08.02.61", Some(&good_patch())));
    assert_eq!(
        id,
        BuildIdentity {
            base_version: "02.08.02.61".into(),
            patch_id: Some(good_patch())
        }
    );
    assert!(!version_match(&id));
}

#[tokio::test]
async fn status_ready_reports_identity() {
    let mock = Arc::new(MockDriver::new());
    mock.set_identity(STUDIO_VERSION, &good_patch());
    let p = probe(&mock, true);
    match p.status().await {
        StudioStatus::Ready {
            app_version,
            identity,
            version_match,
        } => {
            assert_eq!(app_version, STUDIO_VERSION);
            assert_eq!(identity.base_version, STUDIO_VERSION);
            assert!(version_match);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn status_not_running_when_unreachable_and_no_process() {
    let mock = Arc::new(MockDriver::new());
    mock.set_unreachable(true);
    assert_eq!(probe(&mock, false).status().await, StudioStatus::NotRunning);
}

#[tokio::test]
async fn status_driver_off_when_unreachable_and_process() {
    let mock = Arc::new(MockDriver::new());
    mock.set_unreachable(true);
    assert_eq!(probe(&mock, true).status().await, StudioStatus::DriverOff);
}

#[tokio::test]
async fn status_unauthorized_and_token_missing() {
    let mock = Arc::new(MockDriver::new());
    mock.fail_next(DriverError::Unauthorized);
    assert_eq!(
        probe(&mock, true).status().await,
        StudioStatus::Unauthorized
    );

    let none = StatusProbe::new(mock.clone(), Arc::new(FixedProcessProbe(true)), false);
    assert_eq!(none.status().await, StudioStatus::TokenMissing);
    let err = none.require_ready(false, false).await.unwrap_err();
    assert_eq!(err.code, ToolErrorCode::DriverTokenMissing);
    assert_eq!(err.message, "Run: bamburino.exe config set-driver-token");
}

#[tokio::test]
async fn require_ready_error_messages() {
    let mock = Arc::new(MockDriver::new());
    mock.set_unreachable(true);
    let e = probe(&mock, false)
        .require_ready(false, false)
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::StudioNotRunning);
    assert_eq!(
        e.message,
        "Bambu Studio is not running. Open the Bamburino build of Bambu Studio."
    );
    let e = probe(&mock, true)
        .require_ready(false, false)
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::DriverDisabled);
    assert_eq!(
        e.message,
        "Bambu Studio is open but the Local MCP server is off (Preferences > Local MCP server)."
    );
}

#[tokio::test]
async fn require_ready_write_rejects_version_mismatch() {
    let mock = Arc::new(MockDriver::new());
    mock.set_identity("02.08.02.61", &good_patch());
    let e = probe(&mock, true)
        .require_ready(true, false)
        .await
        .unwrap_err();
    assert_eq!(e.code, ToolErrorCode::VersionMismatch);
    assert_eq!(e.http_status, 412);
    assert_eq!(e.details["expected"]["baseVersion"], STUDIO_VERSION);
    assert_eq!(e.details["found"]["baseVersion"], "02.08.02.61");
}

#[tokio::test]
async fn require_ready_read_allows_version_mismatch() {
    let mock = Arc::new(MockDriver::new());
    mock.set_identity("02.08.02.61", &good_patch());
    assert!(probe(&mock, true).require_ready(false, false).await.is_ok());
}

#[tokio::test]
async fn allow_mismatch_override_permits_write() {
    let mock = Arc::new(MockDriver::new());
    mock.set_identity("02.08.02.61", &good_patch());
    assert!(probe(&mock, true).require_ready(true, true).await.is_ok());
}

#[tokio::test]
async fn capabilities_fetched_once_per_session() {
    let mock = Arc::new(MockDriver::new());
    let p = probe(&mock, true);
    p.require_ready(false, false).await.unwrap();
    p.require_ready(false, false).await.unwrap();
    let n = mock
        .calls()
        .iter()
        .filter(|c| c.as_str() == "app_get_capabilities")
        .count();
    assert_eq!(n, 1, "{:?}", mock.calls());
}

#[tokio::test]
async fn status_is_cached_for_a_second() {
    let mock = Arc::new(MockDriver::new());
    let p = probe(&mock, true);
    p.status().await;
    mock.set_unreachable(true);
    assert!(matches!(p.status().await, StudioStatus::Ready { .. }));
}

struct UnknownProcessProbe;
impl bamburino_core::detect::ProcessProbe for UnknownProcessProbe {
    fn studio_running(&self) -> Option<bool> {
        None
    }
}

#[tokio::test]
async fn timeout_is_not_reported_as_not_running_or_off() {
    for running in [true, false] {
        let mock = Arc::new(MockDriver::new());
        mock.fail_next(DriverError::Timeout { after_ms: 5000 });
        let e = probe(&mock, running)
            .require_ready(false, false)
            .await
            .unwrap_err();
        assert_eq!(e.code, ToolErrorCode::DriverError);
        assert!(e.message.contains("timed out"), "{}", e.message);
        assert!(!e.message.contains("not running") && !e.message.contains("is off"));
    }
}

#[tokio::test]
async fn busy_is_propagated_as_busy() {
    let mock = Arc::new(MockDriver::new());
    mock.fail_next(DriverError::Busy {
        message: "modal open".into(),
    });
    let p = probe(&mock, false);
    let e = p.require_ready(false, false).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::Busy);
    assert_eq!(e.message, "modal open");

    mock.fail_next(DriverError::Busy {
        message: "x".into(),
    });
    assert!(matches!(
        p.status().await,
        StudioStatus::DriverFault {
            code: ToolErrorCode::Busy,
            ..
        }
    ));
}

#[tokio::test]
async fn unreadable_process_list_is_distinct() {
    let mock = Arc::new(MockDriver::new());
    mock.set_unreachable(true);
    let p = StatusProbe::new(mock.clone(), Arc::new(UnknownProcessProbe), true);
    assert_eq!(p.status().await, StudioStatus::ProcessUnknown);
    let e = p.require_ready(false, false).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::DriverError);
    assert!(e.message.contains("could not be inspected"));
}

#[tokio::test]
async fn no_token_never_contacts_the_driver() {
    let mock = Arc::new(MockDriver::new());
    let p = StatusProbe::new(mock.clone(), Arc::new(FixedProcessProbe(true)), false);
    let _ = p.status().await;
    let _ = p.require_ready(true, false).await;
    assert!(mock.calls().is_empty(), "{:?}", mock.calls());
}

#[tokio::test]
async fn session_change_refetches_capabilities() {
    let mock = Arc::new(MockDriver::new());
    mock.set_identity(STUDIO_VERSION, &good_patch());
    let p = probe(&mock, true);
    let count = |m: &MockDriver| {
        m.calls()
            .iter()
            .filter(|c| c.as_str() == "app_get_capabilities")
            .count()
    };
    p.require_ready(true, false).await.unwrap();
    p.require_ready(true, false).await.unwrap();
    assert_eq!(count(&mock), 1);
    // Restart with an old build: new session, new identity.
    mock.set_session("session-2");
    mock.set_identity("02.08.02.61", &good_patch());
    let e = p.require_ready(true, false).await.unwrap_err();
    assert_eq!(e.code, ToolErrorCode::VersionMismatch);
    assert_eq!(count(&mock), 2);
}
