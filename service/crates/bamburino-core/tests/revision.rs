// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]

use std::sync::Arc;
use std::time::Duration;

use bamburino_core::driver::mock::MockDriver;
use bamburino_core::driver::{
    ActivePresets, AppState, DriverClient, EffectiveSetting, PlateList, SettingSource,
};
use bamburino_core::revision::{MAX_SAFE_REVISION, RevisionState, fingerprint, revision_value};

fn seq(rev: u64) -> u64 {
    rev & 0xF_FFFF
}

#[tokio::test]
async fn first_refresh_sets_revision_one() {
    let driver = MockDriver::new();
    let rev = RevisionState::new();
    assert_eq!(rev.current(), 0);
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 1);
    assert_eq!(seq(rev.current()), 1);
}

#[tokio::test]
async fn unchanged_state_keeps_revision() {
    let driver = MockDriver::new();
    let rev = RevisionState::new();
    rev.refresh(&driver).await.unwrap();
    // The mock issues a new plate_revision on every read.
    let a = driver.plate_list().await.unwrap().plate_revision;
    let b = driver.plate_list().await.unwrap().plate_revision;
    assert_ne!(a, b);
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 1);
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 1);
}

#[tokio::test]
async fn user_edit_bumps_revision() {
    let driver = MockDriver::new();
    let rev = RevisionState::new();
    rev.refresh(&driver).await.unwrap();
    driver.user_edit("layer_height", "0.12");
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 2);
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 2);
}

#[tokio::test]
async fn user_undo_bumps_revision() {
    let driver = MockDriver::new();
    driver.user_edit("layer_height", "0.12");
    let rev = RevisionState::new();
    rev.refresh(&driver).await.unwrap();
    driver.user_undo();
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 2);
}

fn base() -> (AppState, ActivePresets, PlateList, Vec<EffectiveSetting>) {
    let app = AppState {
        session_id: "s".into(),
        app_version: "1".into(),
        project_name: "p".into(),
        project_path: String::new(),
        dirty: false,
        presets_dirty: false,
        ui_job_running: false,
        slicing_running: false,
        modal_open: false,
        object_count: 1,
        plate_count: 1,
        selected_plate_index: 0,
        undo_snapshot: None,
    };
    let presets = ActivePresets {
        printer: Some("P".into()),
        process: None,
        filaments: vec![],
    };
    let plates = PlateList {
        plates: vec![],
        selected_index: 0,
        plate_revision: "r1".into(),
    };
    let settings = vec![EffectiveSetting {
        key: "k".into(),
        value: "v".into(),
        source: SettingSource::Project,
    }];
    (app, presets, plates, settings)
}

#[test]
fn fingerprint_ignores_volatile_fields_and_detects_hash_only_changes() {
    let (app, presets, plates, settings) = base();
    let f0 = fingerprint(&app, &presets, &[], &plates, &settings, &[]);

    let mut volatile = app.clone();
    volatile.session_id = "other".into();
    volatile.ui_job_running = true;
    volatile.slicing_running = true;
    volatile.modal_open = true;
    let mut plates2 = plates.clone();
    plates2.plate_revision = "r2".into();
    assert_eq!(
        f0,
        fingerprint(&volatile, &presets, &[], &plates2, &settings, &[])
    );

    // Variant (b): no undo_snapshot, a settings change alone is visible.
    let mut s2 = settings.clone();
    s2[0].value = "w".into();
    assert_ne!(f0, fingerprint(&app, &presets, &[], &plates, &s2, &[]));

    // Variant (a): the undo snapshot id is part of the fingerprint.
    let mut with_undo = app.clone();
    with_undo.undo_snapshot = Some("7".into());
    assert_ne!(
        f0,
        fingerprint(&with_undo, &presets, &[], &plates, &settings, &[])
    );
}

#[tokio::test(start_paused = true)]
async fn poller_idle_does_not_call_driver() {
    let driver = Arc::new(MockDriver::new());
    let rev = Arc::new(RevisionState::new());
    let handle = rev.clone().spawn_poller(
        driver.clone(),
        Duration::from_millis(1500),
        Duration::from_secs(60),
    );
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(driver.calls().is_empty());
    assert_eq!(rev.current(), 0);
    handle.abort();
}

#[tokio::test(start_paused = true)]
async fn poller_active_detects_user_edit() {
    let driver = Arc::new(MockDriver::new());
    let rev = Arc::new(RevisionState::new());
    let handle = rev.clone().spawn_poller(
        driver.clone(),
        Duration::from_millis(1500),
        Duration::from_secs(60),
    );
    rev.touch_client();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(seq(rev.current()), 1);
    driver.user_edit("layer_height", "0.12");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(seq(rev.current()), 2);
    // Client goes quiet: polling stops.
    tokio::time::sleep(Duration::from_secs(120)).await;
    let n = driver.calls().len();
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(driver.calls().len(), n);
    handle.abort();
}

#[tokio::test]
async fn object_override_bumps_revision_without_undo_snapshot() {
    let driver = MockDriver::new();
    driver.set_undo_snapshot_supported(false);
    let rev = RevisionState::new();
    rev.refresh(&driver).await.unwrap();
    assert_eq!(driver.app_state().await.unwrap().undo_snapshot, None);
    driver.user_object_override("layer_height", "0.08");
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 2);
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 2);
}

#[tokio::test]
async fn plate_override_bumps_revision_without_undo_snapshot() {
    let driver = MockDriver::new();
    driver.set_undo_snapshot_supported(false);
    let rev = RevisionState::new();
    rev.refresh(&driver).await.unwrap();
    driver.user_plate_override("layer_height", "0.08");
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 2);
    assert_eq!(seq(rev.refresh(&driver).await.unwrap()), 2);
}

#[tokio::test(start_paused = true)]
async fn concurrent_refreshes_bump_once() {
    let driver = MockDriver::new();
    let rev = RevisionState::new();
    driver.delay_next(Duration::from_millis(100));
    let (a, b) = tokio::join!(rev.refresh(&driver), rev.refresh(&driver));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a, b);
    assert_eq!(seq(rev.current()), 1);
}

#[test]
fn revisions_of_different_process_epochs_never_collide() {
    let a = RevisionState::with_epoch(1_000);
    let b = RevisionState::with_epoch(1_001);
    let driver = MockDriver::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let ra = rt.block_on(a.refresh(&driver)).unwrap();
    let rb = rt.block_on(b.refresh(&driver)).unwrap();
    assert_ne!(ra, rb);
    assert!(rb > ra);
    // new() seeds from the wall clock.
    let c = RevisionState::new();
    std::thread::sleep(Duration::from_millis(3));
    let d = RevisionState::new();
    let rc = rt.block_on(c.refresh(&driver)).unwrap();
    let rd = rt.block_on(d.refresh(&driver)).unwrap();
    assert_ne!(rc, rd);
}

#[tokio::test(start_paused = true)]
async fn poller_counts_errors_and_stops_on_signal() {
    let driver = Arc::new(MockDriver::new());
    let rev = Arc::new(RevisionState::new());
    let handle = rev.clone().spawn_poller(
        driver.clone(),
        Duration::from_millis(1500),
        Duration::from_secs(60),
    );
    rev.touch_client();
    driver.set_unreachable(true);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(rev.poll_errors(), 2);
    assert!(rev.last_poll_error().is_some());
    driver.set_unreachable(false);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(rev.poll_errors(), 0);
    assert_eq!(rev.last_poll_error(), None);
    rev.stop_poller();
    handle.await.unwrap();
}

const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[test]
fn max_safe_revision_is_javascript_max_safe_integer() {
    assert_eq!(MAX_SAFE_REVISION, JS_MAX_SAFE_INTEGER);
}

fn first_revision(state: &RevisionState) -> u64 {
    let driver = MockDriver::new();
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(state.refresh(&driver))
        .unwrap()
}

#[test]
fn revision_generated_now_fits_in_a_javascript_number() {
    let rev = first_revision(&RevisionState::new());
    assert!(rev <= JS_MAX_SAFE_INTEGER, "{rev}");
    assert_eq!(rev as f64 as u64, rev);
    // A JSON round trip through f64, as a JavaScript client does it.
    let back: f64 = serde_json::from_str(&serde_json::to_string(&rev).unwrap()).unwrap();
    assert_eq!(back as u64, rev);
}

#[test]
fn revision_in_year_2100_fits_in_a_javascript_number() {
    // 2100-01-01T00:00:00Z
    let rev = first_revision(&RevisionState::with_epoch(4_102_444_800));
    assert!(rev <= JS_MAX_SAFE_INTEGER, "{rev}");
    assert_eq!(rev as f64 as u64, rev);
    // Even with the largest sequence the value stays exact.
    let top = revision_value(4_102_444_800, 0xF_FFFF);
    assert!(top <= JS_MAX_SAFE_INTEGER, "{top}");
    assert_eq!(top as f64 as u64, top);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "wrapped")]
fn sequence_wrap_is_caught_in_debug_builds() {
    let _ = revision_value(1_000, 0x10_0000);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "2^53")]
fn epoch_beyond_the_safe_range_is_caught_in_debug_builds() {
    let _ = revision_value(1 << 33, 1);
}
