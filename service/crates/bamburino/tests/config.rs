// SPDX-License-Identifier: AGPL-3.0-or-later
use std::io::Cursor;

use bamburino::config::{Config, ConfigDir, ConfigError};
use bamburino::secrets::{
    generate_token, read_driver_token_from_stdin, read_secret, write_secret_atomic,
};

fn load_str(toml: &str) -> Result<Config, ConfigError> {
    let tmp = tempfile::tempdir().unwrap();
    let dir = ConfigDir(tmp.path().to_path_buf());
    std::fs::write(dir.config_file(), toml).unwrap();
    Config::load(&dir)
}

#[test]
fn default_config_is_valid() {
    let c = Config::default();
    c.validate().unwrap();
    assert_eq!(c.listen_port, 27190);
    assert_eq!(c.driver_url, "http://127.0.0.1:27183/mcp");
    assert_eq!(c.read_timeout_ms, 10000);
    assert_eq!(c.mutation_timeout_ms, 35000);
    assert_eq!(c.poll_interval_ms, 1500);
    assert!(!c.allow_version_mismatch);
    assert_eq!(c.log_retention_days, 7);
}

#[test]
fn empty_file_loads_defaults() {
    let c = load_str("").unwrap();
    assert_eq!(c.listen_port, 27190);
}

#[test]
fn missing_file_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let err = Config::load(&ConfigDir(tmp.path().to_path_buf())).unwrap_err();
    assert!(matches!(err, ConfigError::Missing(_)));
}

#[test]
fn rejects_listen_host_key() {
    for key in ["listen_host", "bind"] {
        let err = load_str(&format!("{key} = \"0.0.0.0\"\n")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown"), "{msg}");
        assert!(msg.contains(key), "{msg}");
    }
}

#[test]
fn rejects_non_loopback_driver_url() {
    for url in [
        "http://10.0.0.5:27183/mcp",
        "https://127.0.0.1:27183/mcp",
        "http://[::1]:27183/mcp",
        "http://127.0.0.1/mcp",
    ] {
        let err = load_str(&format!("driver_url = \"{url}\"\n")).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)), "{url}");
    }
    load_str("driver_url = \"http://localhost:27183/mcp\"\n").unwrap();
}

#[test]
fn rejects_mutation_timeout_below_driver_queue() {
    for ms in [30000, 34999] {
        let err = load_str(&format!("mutation_timeout_ms = {ms}\n")).unwrap_err();
        assert!(err.to_string().contains("mutation_timeout_ms"));
    }
    load_str("mutation_timeout_ms = 35000\n").unwrap();
    assert!(load_str("mutation_timeout_ms = 120001\n").is_err());
}

#[test]
fn rejects_out_of_range_values() {
    assert!(load_str("read_timeout_ms = 999\n").is_err());
    assert!(load_str("read_timeout_ms = 30001\n").is_err());
    assert!(load_str("listen_port = 80\n").is_err());
    assert!(load_str("poll_interval_ms = 10\n").is_err());
}

#[test]
fn invalid_allowed_dirs_are_warnings_not_errors() {
    let c = load_str("allowed_dirs = ['Z:\\definitely\\not\\here']\n").unwrap();
    let w = c.allowed_dir_warnings();
    assert_eq!(w.len(), 1);
}

#[test]
fn config_dir_resolve_prefers_override() {
    let d = ConfigDir::resolve(Some(r"C:\x".into()));
    assert_eq!(d.0, std::path::PathBuf::from(r"C:\x"));
    assert!(d.config_file().ends_with("config.toml"));
    assert!(d.client_token_file().ends_with("client.token"));
    assert!(d.driver_token_file().ends_with("driver.token"));
    assert!(d.logs_dir().ends_with("logs"));
}

#[test]
fn generate_token_is_base64url_32_bytes_and_unique() {
    let a = generate_token();
    let b = generate_token();
    assert_ne!(a, b);
    assert_eq!(a.len(), 43);
    assert!(
        a.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    );
}

#[test]
fn driver_token_from_stdin_trims_and_validates() {
    assert!(read_driver_token_from_stdin(Cursor::new("abc\n")).is_err());
    let good = "0123456789abcdef".repeat(4);
    let t = read_driver_token_from_stdin(Cursor::new(format!("{good}\r\n"))).unwrap();
    assert_eq!(t, good);
    assert!(read_driver_token_from_stdin(Cursor::new("")).is_err());
    assert!(read_driver_token_from_stdin(Cursor::new(good.to_uppercase())).is_err());
}

#[test]
fn rejected_driver_token_is_never_echoed() {
    let bad = "SECRET-NOT-HEX-VALUE";
    let err = read_driver_token_from_stdin(Cursor::new(bad)).unwrap_err();
    assert!(!err.to_string().contains(bad));
    assert!(!format!("{err:?}").contains(bad));
}

#[test]
fn write_secret_atomic_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("client.token");
    assert_eq!(read_secret(&p).unwrap(), None);
    write_secret_atomic(&p, "first").unwrap();
    assert_eq!(read_secret(&p).unwrap().as_deref(), Some("first"));
    write_secret_atomic(&p, "second").unwrap();
    assert_eq!(read_secret(&p).unwrap().as_deref(), Some("second"));
    // no leftover temp files
    let n = std::fs::read_dir(tmp.path()).unwrap().count();
    assert_eq!(n, 1);
}

#[cfg(windows)]
fn sddl_of(path: &std::path::Path) -> String {
    // `/save` writes the SDDL as UTF-16 and is locale independent.
    let out_file = path.with_extension("sddl");
    let st = std::process::Command::new("icacls")
        .arg(path)
        .arg("/save")
        .arg(&out_file)
        .output()
        .unwrap();
    assert!(st.status.success());
    let raw = std::fs::read(&out_file).unwrap();
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(windows)]
fn aces(sddl: &str) -> Vec<String> {
    let d = sddl.split("D:").nth(1).unwrap();
    d.split('(')
        .skip(1)
        .map(|a| a.split(')').next().unwrap().to_owned())
        .collect()
}

#[cfg(windows)]
#[test]
fn acl_grants_only_expected_principals() {
    use bamburino::acl::{OwnerAccess, apply_acl, current_user_sid};
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("driver.token");
    std::fs::write(&p, "x").unwrap();
    let sid = current_user_sid().unwrap();
    apply_acl(&p, &sid, OwnerAccess::Modify).unwrap();
    let sddl = sddl_of(&p);
    // Protected DACL: no inherited ACEs.
    assert!(sddl.contains("D:P"), "DACL must be protected: {sddl}");
    let aces = aces(&sddl);
    assert_eq!(aces.len(), 4, "unexpected ACL: {sddl}");
    // Trustee is the last field; aliases SY, BA, LS or the user SID.
    let mut trustees: Vec<String> = aces
        .iter()
        .map(|a| a.rsplit(';').next().unwrap().to_owned())
        .collect();
    trustees.sort();
    let mut expected = vec!["SY".to_owned(), "BA".to_owned(), "LS".to_owned(), sid];
    expected.sort();
    assert_eq!(trustees, expected, "{sddl}");
    assert!(aces.iter().all(|a| a.starts_with("A;")), "{sddl}");
    // LOCAL SERVICE has read only on a file.
    let ls = aces.iter().find(|a| a.ends_with(";LS")).unwrap();
    assert!(
        ls.contains("FR") && !ls.contains("FW") && !ls.contains("FA"),
        "{ls}"
    );
}

#[cfg(windows)]
#[test]
fn acl_on_logs_dir_gives_local_service_modify() {
    use bamburino::acl::{OwnerAccess, apply_acl, current_user_sid};
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("logs");
    std::fs::create_dir(&d).unwrap();
    apply_acl(&d, &current_user_sid().unwrap(), OwnerAccess::Modify).unwrap();
    let sddl = sddl_of(&d);
    let aces = aces(&sddl);
    let ls = aces.iter().find(|a| a.ends_with(";LS")).unwrap();
    // Modify = write + delete rights, inheritable to children (OICI).
    assert!(ls.contains("OICI"), "{ls}");
    assert!(ls.contains("FW") || ls.contains("0x1301bf"), "{ls}");
}
