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
            listen_port: 27190,
            driver_url: "http://127.0.0.1:27183/mcp".to_owned(),
            read_timeout_ms: 10_000,
            mutation_timeout_ms: MIN_MUTATION_TIMEOUT_MS,
            poll_interval_ms: 1_500,
            allow_version_mismatch: false,
            allowed_dirs: default_allowed_dirs(),
            log_retention_days: 7,
            owner_sid: None,
        }
    }
}

/// Documents and Downloads of the user running the installer.
fn default_allowed_dirs() -> Vec<PathBuf> {
    match std::env::var_os("USERPROFILE") {
        Some(p) => {
            let p = PathBuf::from(p);
            vec![p.join("Documents"), p.join("Downloads")]
        }
        None => Vec::new(),
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
        self.init_with(Config::default())
    }

    /// [`ConfigDir::init`] with the defaults a NEW `config.toml` is written
    /// from (`owner_sid` is always set to the installing user). An existing
    /// configuration is left untouched.
    pub fn init_with(&self, defaults: Config) -> Result<(), ConfigError> {
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
            let cfg = Config {
                owner_sid: Some(installer.clone()),
                ..defaults
            };
            let mut f = create_restricted_file(&cfg_path, &installer, OwnerAccess::Read)
                .map_err(|e| io(&cfg_path, e))?;
            f.write_all(cfg.to_toml().as_bytes())
                .map_err(|e| io(&cfg_path, e))?;
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
