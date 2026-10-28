// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg(feature = "mock")]
//! Driver timeouts and Busy outcomes are logged at `warn`.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bamburino_core::detect::FixedProcessProbe;
use bamburino_core::driver::DriverError;
use bamburino_core::driver::mock::MockDriver;
use bamburino_core::tools::{ToolConfig, Tools};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl<'a> MakeWriter<'a> for Buf {
    type Writer = Buf;
    fn make_writer(&'a self) -> Buf {
        self.clone()
    }
}

fn tools(mock: &Arc<MockDriver>) -> Tools {
    Tools::new(
        mock.clone(),
        Arc::new(FixedProcessProbe(true)),
        ToolConfig {
            allow_version_mismatch: false,
            poll_every: Duration::from_secs(60),
            idle_after: Duration::from_secs(60),
        },
        std::env::temp_dir().join("bamburino-tests-unused"),
    )
}

/// Runs `studio_get_state` with the nth driver call failing, for each n, and
/// returns the captured log of every run.
async fn logs_for(err: impl Fn() -> DriverError) -> Vec<String> {
    let mut out = Vec::new();
    for n in 0..10 {
        let buf = Buf::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        let _g = tracing::subscriber::set_default(sub);
        let mock = Arc::new(MockDriver::new());
        mock.fail_after(n, err());
        let _ = tools(&mock).studio_get_state().await;
        out.push(String::from_utf8(buf.0.lock().unwrap().clone()).unwrap());
    }
    out
}

#[tokio::test]
async fn read_timeout_is_logged_at_warn() {
    let logs = logs_for(|| DriverError::Timeout { after_ms: 4321 }).await;
    let hit: Vec<_> = logs
        .iter()
        .filter(|l| l.contains("driver read timed out"))
        .collect();
    assert!(!hit.is_empty(), "no warn in any run: {logs:?}");
    assert!(
        hit[0].contains("WARN") && hit[0].contains("after_ms=4321"),
        "{}",
        hit[0]
    );
}

#[tokio::test]
async fn read_busy_is_logged_at_warn_without_the_driver_message() {
    let logs = logs_for(|| DriverError::Busy {
        message: "secret-modal-text".into(),
    })
    .await;
    let hit: Vec<_> = logs.iter().filter(|l| l.contains("driver busy")).collect();
    assert!(!hit.is_empty(), "no warn in any run: {logs:?}");
    assert!(
        hit.iter()
            .all(|l| l.contains("WARN") && !l.contains("secret-modal-text"))
    );
}
