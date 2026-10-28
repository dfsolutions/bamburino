// SPDX-License-Identifier: AGPL-3.0-or-later
//! Service configuration (`%ProgramData%\Bamburino\config.toml`).
//!
//! The file holds no secrets. There is deliberately no key for the listen
//! address: the service binds loopback only, and any unknown key (for example
//! `listen_host` or `bind`) is rejected at load.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use bamburino_core::detect::parse_driver_url;

/// The driver's own mutation queue is 30 s; the service must wait longer.
pub const MIN_MUTATION_TIMEOUT_MS: u64 = 35_000;
pub const MAX_MUTATION_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration file not found: {0}")]
    Missing(PathBuf),
    #[error("cannot access {0}: {1}")]
    Io(PathBuf, String),
    #[error("invalid configuration file: {0}")]
    Parse(String),
    #[error("invalid configuration: {0}")]
    Invalid(String),
    #[error("{0}")]
    Token(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub listen_port: u16,
    pub driver_url: String,
    pub read_timeout_ms: u64,
    pub mutation_timeout_ms: u64,
    pub poll_interval_ms: u64,
    pub allow_version_mismatch: bool,
    pub allowed_dirs: Vec<PathBuf>,
    pub log_retention_days: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_sid: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_port: 27600,
            driver_url: "http://127.0.0.1:27183/mcp".to_owned(),
            read_timeout_ms: 10_000,
            mutation_timeout_ms: MIN_MUTATION_TIMEOUT_MS,
            poll_interval_ms: 1_500,
            allow_version_mismatch: false,
            // Deliberately empty: the defaults are the OWNER's folders, resolved
            // by `config init` (run by the owner) and written to the file. The
            // service may run as LocalService, whose Known Folders are not the
            // owner's, so nothing is resolved when the file is loaded: a file
            // without `allowed_dirs` allows no directory.
            allowed_dirs: Vec::new(),
            log_retention_days: 7,
            owner_sid: None,
        }
    }
}

impl Config {
    /// The defaults written by `config init` and by a plain test/smoke
    /// configuration: [`Config::default`] plus the allowed directories of the
    /// user running this process ([`default_allowed_dirs`]). Never called by
    /// the service at load time.
    pub fn defaults_for_current_user() -> Config {
        Config {
            allowed_dirs: default_allowed_dirs(),
            ..Config::default()
        }
    }
}

/// Documents and Downloads of the user running this process (the owner, at
/// `config init`), from the Known Folder API (follows OneDrive redirection);
/// `%USERPROFILE%` is only the fallback.
pub fn default_allowed_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, FOLDERID_Downloads};
        let known: Vec<PathBuf> = [FOLDERID_Documents, FOLDERID_Downloads]
            .iter()
            .filter_map(known_folder)
            .collect();
        if known.len() == 2 {
            return known;
        }
    }
    match std::env::var_os("USERPROFILE") {
        Some(p) => {
            let p = PathBuf::from(p);
            vec![p.join("Documents"), p.join("Downloads")]
        }
        None => Vec::new(),
    }
}

/// A Known Folder of the user running this process.
#[cfg(windows)]
fn known_folder(id: &windows_sys::core::GUID) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
    // SAFETY: on success the call returns a NUL-terminated wide string that
    // we copy and then free with CoTaskMemFree, as documented.
    unsafe {
        let mut raw: *mut u16 = std::ptr::null_mut();
        let hr = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut raw);
        if hr != 0 || raw.is_null() {
            if !raw.is_null() {
                CoTaskMemFree(raw.cast());
            }
            return None;
        }
        let mut len = 0usize;
        while *raw.add(len) != 0 {
            len += 1;
        }
        let path = PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(
            raw, len,
        )));
        CoTaskMemFree(raw.cast());
        Some(path)
    }
}

impl Config {
    /// Reads and validates `config.toml` from `dir`.
    pub fn load(dir: &ConfigDir) -> Result<Config, ConfigError> {
        let path = dir.config_file();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::Missing(path));
            }
            Err(e) => return Err(ConfigError::Io(path, e.to_string())),
        };
        let cfg: Config = toml::from_str(&text).map_err(|e| {
            ConfigError::Parse(format!(
                "{e} (unknown keys are rejected: the service listens on loopback only and has no listen address setting)"
            ))
        })?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: String| Err(ConfigError::Invalid(m));
        if let Err(e) = parse_driver_url(&self.driver_url) {
            return bad(format!("driver_url: {e}"));
        }
        if !(MIN_MUTATION_TIMEOUT_MS..=MAX_MUTATION_TIMEOUT_MS).contains(&self.mutation_timeout_ms)
        {
            return bad(format!(
                "mutation_timeout_ms must be in {MIN_MUTATION_TIMEOUT_MS}..={MAX_MUTATION_TIMEOUT_MS} (the driver queue is 30000 ms), got {}",
                self.mutation_timeout_ms
            ));
        }
        if !(1_000..=30_000).contains(&self.read_timeout_ms) {
            return bad(format!(
                "read_timeout_ms must be in 1000..=30000, got {}",
                self.read_timeout_ms
            ));
        }
        if !(100..=60_000).contains(&self.poll_interval_ms) {
            return bad(format!(
                "poll_interval_ms must be in 100..=60000, got {}",
                self.poll_interval_ms
            ));
        }
        if self.listen_port < 1024 {
            return bad(format!(
                "listen_port must be >= 1024, got {}",
                self.listen_port
            ));
        }
        Ok(())
    }

    /// One warning per allowed root that is not an absolute existing
    /// directory. Never a startup failure.
    pub fn allowed_dir_warnings(&self) -> Vec<String> {
        self.allowed_dirs
            .iter()
            .filter_map(|d| {
                if !d.is_absolute() {
                    Some(format!("allowed_dirs: {} is not absolute", d.display()))
                } else if !d.is_dir() {
                    Some(format!("allowed_dirs: {} is not a directory", d.display()))
                } else {
                    None
                }
            })
            .collect()
    }

    /// TOML text of this configuration.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serializes")
    }
}

/// The configuration directory.
#[derive(Debug, Clone)]
pub struct ConfigDir(pub PathBuf);

impl ConfigDir {
    /// `override_dir`, else `%ProgramData%\Bamburino`.
    pub fn resolve(override_dir: Option<PathBuf>) -> ConfigDir {
        if let Some(d) = override_dir {
            return ConfigDir(d);
        }
        let base = std::env::var_os("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        ConfigDir(base.join("Bamburino"))
    }

    pub fn config_file(&self) -> PathBuf {
        self.0.join("config.toml")
    }
    pub fn client_token_file(&self) -> PathBuf {
        self.0.join("client.token")
    }
    pub fn driver_token_file(&self) -> PathBuf {
        self.0.join("driver.token")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.0.join("logs")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.0.join("tmp")
    }

    /// Migration run by `config init` on an EXISTING `config.toml`: when the
    /// `allowed_dirs` key is absent, appends `allowed_dirs = <dirs>` and
    /// returns true. Nothing else in the file is touched (comments and other
    /// keys stay byte for byte) and the file is opened in place, so its ACL
    /// is preserved. A present key, even `allowed_dirs = []`, is left alone
    /// (returns false), so a second run changes nothing. When `dirs` is empty
    /// (Known Folders resolved to nothing) the key is NOT written, a warning is
    /// printed and false is returned, so a later `config init` retries.
    pub fn add_missing_allowed_dirs(&self, dirs: &[PathBuf]) -> Result<bool, ConfigError> {
        use std::io::Write;
        let path = self.config_file();
        let io = |e: std::io::Error| ConfigError::Io(path.clone(), e.to_string());
        let text = std::fs::read_to_string(&path).map_err(io)?;
        let table: toml::Table =
            toml::from_str(&text).map_err(|e| ConfigError::Parse(format!("{e}")))?;
        if table.contains_key("allowed_dirs") {
            return Ok(false);
        }
        if dirs.is_empty() {
            // Writing `allowed_dirs = []` would look like the owner's choice and stop every
            // later `config init` from retrying; leave the key absent instead.
            eprintln!(
                "warning: could not resolve your Documents and Downloads folders; allowed_dirs was not added to {}. Run `bamburino config init` again from the user's session.",
                path.display()
            );
            return Ok(false);
        }
        #[derive(Serialize)]
        struct Only<'a> {
            allowed_dirs: &'a [PathBuf],
        }
        let line = toml::to_string(&Only { allowed_dirs: dirs })
            .map_err(|e| ConfigError::Invalid(format!("allowed_dirs: {e}")))?;
        let mut addition = String::new();
        if !text.is_empty() && !text.ends_with('\n') {
            addition.push('\n');
        }
        addition.push_str(&line);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(io)?;
        f.write_all(addition.as_bytes()).map_err(io)?;
        Ok(true)
    }

    /// Prepares the configuration directory. Needs an elevated (Administrator)
    /// prompt the first time, because `%ProgramData%\Bamburino` is created
    /// with a protected DACL and Administrators as owner.
    ///
    /// Order matters: the root is created (or verified) FIRST, so no other
    /// local user can pre-create `config.toml`, `client.token` or a temp file
    /// in it. If the root already exists its owner must be SYSTEM or
    /// Administrators and its DACL protected, otherwise init refuses.
    /// Idempotent; never overwrites an existing config or token.
    ///
    /// Known limitation: the location comes from the `PROGRAMDATA`
    /// environment variable, not from the Known Folder API.
    pub fn init(&self) -> Result<(), ConfigError> {
        use crate::acl::{OwnerAccess, apply_acl, current_user_sid};
        use crate::secrets::{generate_token, read_secret, write_secret_atomic};
        use crate::secure::{create_protected_root, create_restricted_file, root_problem};
        use std::io::Write;
        let io = |p: &Path, e: std::io::Error| ConfigError::Io(p.to_path_buf(), e.to_string());

        let installer = current_user_sid().map_err(|e| io(&self.0, e))?;
        if self.0.exists() {
            if let Some(problem) = root_problem(&self.0) {
                return Err(ConfigError::Invalid(format!(
                    "refusing to use {}: {problem}. Remove or fix the directory as Administrator and run `bamburino config init` again",
                    self.0.display()
                )));
            }
        } else {
            create_protected_root(&self.0, &installer).map_err(|e| {
                ConfigError::Io(
                    self.0.clone(),
                    format!("{e} (creating the configuration root needs an elevated Administrator prompt)"),
                )
            })?;
        }

        let cfg_path = self.config_file();
        if !cfg_path.exists() {
            // `config init` runs as the owner: this is where the owner's
            // Documents and Downloads are resolved and stored.
            let cfg = Config {
                owner_sid: Some(installer.clone()),
                ..Config::defaults_for_current_user()
            };
            let mut f = create_restricted_file(&cfg_path, &installer, OwnerAccess::Read)
                .map_err(|e| io(&cfg_path, e))?;
            f.write_all(cfg.to_toml().as_bytes())
                .map_err(|e| io(&cfg_path, e))?;
        } else if self.add_missing_allowed_dirs(&default_allowed_dirs())? {
            println!(
                "added allowed_dirs (your Documents and Downloads) to {}",
                cfg_path.display()
            );
        }
        let cfg = Config::load(self)?;
        let sid = cfg.owner_sid.unwrap_or(installer);

        let client = self.client_token_file();
        if read_secret(&client).map_err(|e| io(&client, e))?.is_none() {
            write_secret_atomic(&client, &generate_token(), &sid).map_err(|e| io(&client, e))?;
        }
        // Empty placeholder so `set-driver-token` can overwrite it in place
        // without write access to the (locked) root.
        let driver = self.driver_token_file();
        if !driver.exists() {
            create_restricted_file(&driver, &sid, OwnerAccess::Modify)
                .map_err(|e| io(&driver, e))?;
        }
        for d in [self.logs_dir(), self.tmp_dir()] {
            std::fs::create_dir_all(&d).map_err(|e| io(&d, e))?;
        }
        for (p, a) in [
            (cfg_path, OwnerAccess::Read),
            (client, OwnerAccess::Read),
            (driver, OwnerAccess::Modify),
            (self.logs_dir(), OwnerAccess::Modify),
            (self.tmp_dir(), OwnerAccess::Modify),
        ] {
            apply_acl(&p, &sid, a).map_err(|e| io(&p, e))?;
        }
        Ok(())
    }
}
