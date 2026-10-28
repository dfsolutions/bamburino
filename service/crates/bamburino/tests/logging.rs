// SPDX-License-Identifier: AGPL-3.0-or-later
//! Log redaction and rotation. No network.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use bamburino::logging::{self, Redactor, build_subscriber, build_subscriber_with_acl, redact};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

const HEX64: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const B64: &str = "Zm9vYmFyYmF6cXV4MTIzNDU2Nzg5MGFiY2RlZmdoaWo";

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Buf {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
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

fn with_memory_layer(redactor: Redactor, f: impl FnOnce()) -> String {
    let buf = Buf::default();
    let sub = tracing_subscriber::registry().with(logging::layer(buf.clone(), Arc::new(redactor)));
    tracing::subscriber::with_default(sub, f);
    buf.text()
}

fn sid() -> String {
    bamburino::acl::current_user_sid().unwrap()
}

fn log_files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn redacts_hex_token() {
    let out = redact(&format!("token={HEX64} end")).into_owned();
    assert_eq!(out, "token=<redacted-token> end");
    assert!(!out.contains("0123456789abcdef0123"));
}

#[test]
fn redacts_bearer_header() {
    assert_eq!(
        redact("Authorization: Bearer abc.DEF-123_xyz== next"),
        "Authorization: Bearer <redacted> next"
    );
    assert_eq!(
        redact("authorization: bearer short"),
        "authorization: Bearer <redacted>"
    );
}

#[test]
fn redacts_unc_path() {
    assert_eq!(
        redact(r"open \\fileserver\share\a b.3mf now"),
        r"open <unc-path> b.3mf now"
    );
    assert_eq!(redact(r"open \\?\UNC\srv\share\x.3mf"), "open <unc-path>");
    // Debug-escaped form, as produced by `{:?}` on a path.
    assert_eq!(
        redact(r#"path="\\\\srv\\share\\x.3mf""#),
        r#"path="<unc-path>""#
    );
    // Local paths are left alone.
    assert_eq!(redact(r"C:\Users\me\a.3mf"), r"C:\Users\me\a.3mf");
    assert_eq!(redact(r"C:\\Users\\me\\a.3mf"), r"C:\\Users\\me\\a.3mf");
    assert_eq!(redact(r"\\?\C:\Users\me\a.3mf"), r"\\?\C:\Users\me\a.3mf");
}

#[test]
fn redacts_base64url_and_configured_values() {
    assert_eq!(redact(&format!("t {B64} t")), "t <redacted-token> t");
    // Short identifiers and ordinary prose survive.
    assert_eq!(
        redact("plate 1 layer_height=0.2"),
        "plate 1 layer_height=0.2"
    );
    let r = Redactor::new(["hunter2hunter2".to_owned(), "x".to_owned()]);
    assert_eq!(r.redact("pw hunter2hunter2!"), "pw <redacted-token>!");
    // Too-short configured values are ignored (they would shred the log).
    assert_eq!(r.redact("x marks"), "x marks");
}

#[test]
fn redaction_is_idempotent_and_utf8_safe() {
    let once = redact(&format!("é {HEX64} è Bearer zzz ü")).into_owned();
    assert_eq!(redact(&once), once);
    assert!(once.contains('é') && once.contains('ü'));
}

#[test]
fn layer_redacts_event_fields_and_span_fields() {
    let secret = "s3cr3t-configured-value";
    let out = with_memory_layer(Redactor::new([secret.to_owned()]), || {
        let span = tracing::info_span!("req", auth = %format!("Bearer {B64}"), tok = HEX64);
        let _g = span.enter();
        tracing::info!(cfg = secret, unc = r"\\srv\share\f.3mf", "hello {HEX64}");
        tracing::warn!("Bearer abc123 {B64}");
    });
    for needle in [secret, HEX64, B64, "abc123", "share"] {
        assert!(!out.contains(needle), "leaked {needle:?} in {out}");
    }
    assert!(out.contains("<redacted-token>") && out.contains("<unc-path>"));
    assert!(out.contains("hello") && out.contains("INFO") && out.contains("WARN"));
}

#[test]
fn log_file_rotates_with_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let (sub, guard) = build_subscriber(tmp.path(), 7, false, &sid(), Vec::new());
        tracing::subscriber::with_default(sub, || tracing::info!("first event"));
        drop(guard);
    }
    let files = log_files(tmp.path());
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(
        files[0].starts_with("bamburino.") && files[0].ends_with(".log"),
        "{files:?}"
    );
}

#[test]
fn retention_bounds_the_number_of_files() {
    let tmp = tempfile::tempdir().unwrap();
    for d in 1..=5 {
        std::fs::write(
            tmp.path().join(format!("bamburino.2020-01-0{d}.log")),
            "old",
        )
        .unwrap();
    }
    let (sub, guard) = build_subscriber(tmp.path(), 2, false, &sid(), Vec::new());
    tracing::subscriber::with_default(sub, || tracing::info!("now"));
    drop(guard);
    let files = log_files(tmp.path());
    assert!(files.len() <= 2, "{files:?}");
    assert!(
        !files.contains(&"bamburino.2020-01-01.log".to_owned()),
        "{files:?}"
    );
}

#[test]
fn log_never_contains_token() {
    let tmp = tempfile::tempdir().unwrap();
    let configured = "configured-driver-token-value".to_owned();
    {
        let (sub, guard) = build_subscriber(tmp.path(), 7, false, &sid(), vec![configured.clone()]);
        tracing::subscriber::with_default(sub, || {
            tracing::info!(
                "{}",
                redact(&format!("driver said {HEX64} and Bearer {B64}"))
            );
            tracing::info!(raw = HEX64, "unredacted by the caller: {configured}");
            tracing::debug!("dropped by the default level {HEX64}");
        });
        drop(guard);
    }
    let name = &log_files(tmp.path())[0];
    let text = std::fs::read_to_string(tmp.path().join(name)).unwrap();
    for needle in [HEX64, B64, configured.as_str()] {
        assert!(!text.contains(needle), "leaked {needle:?} in {text}");
    }
    assert!(text.contains("<redacted-token>"));
    assert!(!text.contains("dropped by the default level"));
}

#[test]
fn missing_log_dir_is_created_and_unusable_dir_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let nested = tmp.path().join("a").join("logs");
    let (sub, guard) = build_subscriber(&nested, 7, false, &sid(), Vec::new());
    tracing::subscriber::with_default(sub, || tracing::info!("x"));
    drop(guard);
    assert_eq!(log_files(&nested).len(), 1);

    // A path below a regular file cannot be created: stderr fallback, no panic.
    let file = tmp.path().join("plain");
    std::fs::write(&file, "x").unwrap();
    let (sub, guard) = build_subscriber(&file.join("logs"), 7, false, &sid(), Vec::new());
    tracing::subscriber::with_default(sub, || tracing::info!("to stderr"));
    drop(guard);
}

#[test]
fn init_logging_installs_the_global_subscriber_once() {
    let tmp = tempfile::tempdir().unwrap();
    let guard = logging::init_logging(tmp.path(), 7, false, &sid()).expect("first init");
    tracing::info!("global event");
    assert!(matches!(
        logging::init_logging(tmp.path(), 7, false, &sid()),
        Err(logging::LogError::AlreadyInitialized)
    ));
    drop(guard);
    assert_eq!(log_files(tmp.path()).len(), 1);
}

#[test]
fn acl_failure_means_no_file_and_no_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("logs");
    let failing = |_: &Path, _: &str| Err(io::Error::other("injected"));
    let (sub, guard) = build_subscriber_with_acl(&dir, 7, false, &sid(), Vec::new(), &failing);
    tracing::subscriber::with_default(sub, || tracing::info!("must not reach a file"));
    drop(guard);
    assert!(log_files(&dir).is_empty(), "{:?}", log_files(&dir));

    // An existing directory with an unprotected DACL fails the same way.
    let existing = tmp.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    let (sub, guard) = build_subscriber_with_acl(&existing, 7, false, &sid(), Vec::new(), &failing);
    tracing::subscriber::with_default(sub, || tracing::info!("must not reach a file"));
    drop(guard);
    if cfg!(windows) {
        assert!(
            log_files(&existing).is_empty(),
            "{:?}",
            log_files(&existing)
        );
    }
}

#[cfg(windows)]
#[test]
fn logs_dir_and_log_file_carry_the_protected_dacl() {
    use bamburino::secure::security_sddl;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("logs");
    let me = sid();
    for round in 0..2 {
        // Round 0 creates the directory, round 1 re-uses it.
        let (sub, guard) = build_subscriber(&dir, 7, false, &me, Vec::new());
        tracing::subscriber::with_default(sub, || tracing::info!("round {round}"));
        drop(guard);
    }
    let d = security_sddl(&dir).unwrap();
    assert!(d.contains("D:P"), "{d}");
    for who in [";;;SY)", ";;;BA)", ";;;LS)", &format!(";;;{me})")] {
        assert!(d.contains(who), "{who} missing: {d}");
    }
    assert!(
        !d.contains(";;;WD") && !d.contains(";;;BU") && !d.contains(";;;AU"),
        "{d}"
    );
    // LOCAL SERVICE can modify (write logs), not just read.
    assert!(d.contains("(A;OICI;0x1301bf;;;LS)"), "{d}");

    let file = log_files(&dir)[0].clone();
    let f = security_sddl(&dir.join(file)).unwrap();
    assert!(
        f.contains(";;;SY)") && f.contains(";;;LS)") && f.contains(&format!(";;;{me})")),
        "{f}"
    );
    assert!(
        !f.contains(";;;WD") && !f.contains(";;;BU") && !f.contains(";;;AU"),
        "{f}"
    );
}

#[cfg(windows)]
#[test]
fn an_existing_unprotected_dir_gets_the_acl_reapplied() {
    use bamburino::secure::security_sddl;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("logs");
    std::fs::create_dir(&dir).unwrap();
    assert!(!security_sddl(&dir).unwrap().contains("D:P"));
    let (sub, guard) = build_subscriber(&dir, 7, false, &sid(), Vec::new());
    tracing::subscriber::with_default(sub, || tracing::info!("x"));
    drop(guard);
    assert!(security_sddl(&dir).unwrap().contains("D:P"));
    assert_eq!(log_files(&dir).len(), 1);
}

#[test]
fn redacts_multi_line_input() {
    let unc = r"\\srv\share\f.3mf";
    let text = format!("line1 {HEX64}\nAuthorization: Bearer abc\r\nopen {unc}\nok");
    assert_eq!(
        redact(&text),
        "line1 <redacted-token>\nAuthorization: Bearer <redacted>\r\nopen <unc-path>\nok"
    );
}
