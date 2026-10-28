// SPDX-License-Identifier: AGPL-3.0-or-later
//! Command-line surface of `bamburino.exe` (no live Bambu Studio needed).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;

const CLIENT_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DRIVER_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn bamburino() -> Command {
    Command::new(cargo_bin("bamburino"))
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// A configuration directory with a known port and client token.
fn config_dir(port: u16) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!("listen_port = {port}\n"),
    )
    .unwrap();
    std::fs::write(dir.path().join("client.token"), CLIENT_TOKEN).unwrap();
    dir
}

fn http_get(port: u16, path: &str) -> Option<String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

struct KillOnDrop(std::process::Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn both_streams(out: &assert_cmd::assert::Assert) -> String {
    let o = out.get_output();
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn version_prints_catalog_and_patch() {
    let out = bamburino().arg("--version").assert().success();
    let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let want = format!(
        "bamburino {} (catalog 02.08.04.57 @ f977235e, expected driver patch ",
        env!("CARGO_PKG_VERSION")
    );
    assert!(text.starts_with(&want), "{text}");
    assert!(text.trim_end().ends_with(')'), "{text}");
}

#[test]
fn run_with_config_dir_starts_and_answers_healthz() {
    let port = free_port();
    let dir = config_dir(port);
    let child = std::process::Command::new(cargo_bin("bamburino"))
        .args(["--run", "--config-dir"])
        .arg(dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = KillOnDrop(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    let reply = loop {
        if let Some(r) = http_get(port, "/healthz") {
            break r;
        }
        assert!(Instant::now() < deadline, "server did not answer /healthz");
        assert!(child.0.try_wait().unwrap().is_none(), "server exited early");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.trim_end().ends_with("ok"), "{reply}");
}

#[test]
fn run_starts_with_invalid_allowed_dir() {
    let port = free_port();
    let dir = config_dir(port);
    std::fs::write(
        dir.path().join("config.toml"),
        format!("listen_port = {port}\nallowed_dirs = ['Z:\\missing']\n"),
    )
    .unwrap();
    let child = std::process::Command::new(cargo_bin("bamburino"))
        .args(["--run", "--config-dir"])
        .arg(dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = KillOnDrop(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    let reply = loop {
        if let Some(r) = http_get(port, "/healthz") {
            break r;
        }
        assert!(Instant::now() < deadline, "server did not answer /healthz");
        assert!(child.0.try_wait().unwrap().is_none(), "server exited early");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    // Stop it and read what it logged on stderr.
    let _ = child.0.kill();
    let _ = child.0.wait();
    let mut err = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(err.contains("WARN"), "{err}");
    assert!(err.contains("allowed_dirs: ignoring"), "{err}");
    assert!(err.contains("missing"), "{err}");
}

#[test]
fn service_flag_from_console_exits_2() {
    let out = bamburino().arg("--service").assert().failure().code(2);
    assert!(
        both_streams(&out).contains("service control manager"),
        "{}",
        both_streams(&out)
    );
}

#[test]
fn config_dir_rejected_without_run_or_config_subcommand() {
    for flag in ["--service", "--install", "--uninstall", "--version"] {
        bamburino()
            .args([flag, "--config-dir", "x"])
            .assert()
            .failure()
            .code(2);
    }
}

#[test]
fn mode_flag_with_config_subcommand_is_a_usage_error() {
    bamburino()
        .args(["--install", "config", "init"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn run_reports_a_port_that_cannot_be_bound() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dir = config_dir(taken.local_addr().unwrap().port());
    let out = bamburino()
        .args(["--run", "--config-dir"])
        .arg(dir.path())
        .assert()
        .failure()
        .code(1);
    assert!(
        both_streams(&out).contains("cannot bind"),
        "{}",
        both_streams(&out)
    );
}

#[test]
fn purge_requires_uninstall() {
    bamburino().arg("--purge").assert().failure().code(2);
    bamburino()
        .args(["--install", "--purge"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn modes_are_mutually_exclusive_and_one_is_required() {
    bamburino().assert().failure().code(2);
    bamburino()
        .args(["--run", "--service"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn set_driver_token_reads_stdin_and_never_echoes() {
    let dir = config_dir(free_port());
    let out = bamburino()
        .args(["config", "set-driver-token", "--config-dir"])
        .arg(dir.path())
        .write_stdin(format!("{DRIVER_TOKEN}\n"))
        .assert()
        .success();
    let all = both_streams(&out);
    assert!(!all.contains(DRIVER_TOKEN), "token leaked: {all}");
    assert!(all.contains("driver token saved"), "{all}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("driver.token"))
            .unwrap()
            .trim(),
        DRIVER_TOKEN
    );
}

#[test]
fn set_driver_token_rejects_bad_input_without_echo() {
    let dir = config_dir(free_port());
    let out = bamburino()
        .args(["config", "set-driver-token", "--config-dir"])
        .arg(dir.path())
        .write_stdin("not-a-token-SECRETVALUE\n")
        .assert()
        .failure()
        .code(1);
    assert!(!both_streams(&out).contains("SECRETVALUE"));
}

fn snippet(dir: &Path, extra: &[&str]) -> String {
    let out = bamburino()
        .args(["config", "client-snippet", "--config-dir"])
        .arg(dir)
        .args(extra)
        .assert()
        .success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

#[test]
fn client_snippet_contains_url_and_header() {
    let dir = config_dir(27555);
    let claude = snippet(dir.path(), &["--client", "claude-code"]);
    assert_eq!(
        claude.trim(),
        format!(
            "claude mcp add --transport http bamburino http://127.0.0.1:27555/mcp --header \"Authorization: Bearer {CLIENT_TOKEN}\""
        )
    );
    assert_eq!(
        snippet(dir.path(), &[]),
        claude,
        "claude-code is the default"
    );
    let generic = snippet(dir.path(), &["--client", "generic"]);
    assert!(generic.contains("http://127.0.0.1:27555/mcp"), "{generic}");
    assert!(
        generic.contains(&format!("Authorization: Bearer {CLIENT_TOKEN}")),
        "{generic}"
    );
}

#[test]
fn client_snippet_fails_cleanly_without_config() {
    let dir = tempfile::tempdir().unwrap();
    bamburino()
        .args(["config", "client-snippet", "--config-dir"])
        .arg(dir.path())
        .assert()
        .failure()
        .code(1);
}

/// Manual, elevated: see the Task 16 report for the procedure.
#[test]
#[ignore = "needs an elevated prompt and changes the machine: run by hand"]
fn install_uninstall_roundtrip_as_admin() {
    bamburino().arg("--install").assert().success();
    let q = std::process::Command::new("sc")
        .args(["query", "Bamburino"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&q.stdout).contains("RUNNING"));
    let installed = bamburino::config::ConfigDir::resolve(None);
    let port = bamburino::config::Config::load(&installed)
        .unwrap()
        .listen_port;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if http_get(port, "/healthz").is_some_and(|r| r.trim_end().ends_with("ok")) {
            break;
        }
        assert!(Instant::now() < deadline, "healthz did not answer");
        std::thread::sleep(Duration::from_millis(200));
    }
    bamburino()
        .args(["--uninstall", "--purge"])
        .assert()
        .success();
    let q = std::process::Command::new("sc")
        .args(["query", "Bamburino"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&q.stdout).contains("1060"));
}
