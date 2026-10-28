// SPDX-License-Identifier: AGPL-3.0-or-later
//! Production wiring shared by `--run` and `--service`: configuration, tokens,
//! logging, driver client and tools.

use std::sync::Arc;
use std::time::Duration;

use bamburino::acl::current_user_sid;
use bamburino::config::{Config, ConfigDir};
use bamburino::logging::{init_logging_with_secrets, redact};
use bamburino::secrets::read_secret;
use bamburino_core::detect::parse_driver_url;
use bamburino_core::driver::http::{HttpDriverClient, HttpDriverConfig};
use bamburino_core::pathguard::PathGuard;
use bamburino_core::tools::{ToolConfig, Tools};
use tracing_appender::non_blocking::WorkerGuard;

use crate::process::WindowsProcessProbe;

/// How long without client requests before the revision poller idles.
const POLL_IDLE_AFTER: Duration = Duration::from_secs(60);

/// Everything the server needs. Keep it alive for the whole run: dropping
/// `_log_guard` flushes the log file.
pub struct Prepared {
    pub cfg: Config,
    pub client_token: String,
    pub tools: Arc<Tools>,
    _log_guard: WorkerGuard,
}

/// Loads the configuration in `dir`, installs logging and builds the tools.
/// The driver token is not frozen here: the driver client re-reads
/// `driver.token` live and reports `driver_token_missing` itself. Waits up
/// to one second before building the tools so that the revision epoch of
/// this run differs from the previous run's (see `bamburino_core::revision`).
/// The G-code export directory is `<dir>\tmp`.
pub fn prepare(dir: &ConfigDir, to_stderr: bool) -> Result<Prepared, String> {
    let cfg = Config::load(dir).map_err(|e| e.to_string())?;
    let client_path = dir.client_token_file();
    let client_token = read_secret(&client_path)
        .map_err(|e| format!("cannot read {}: {e}", client_path.display()))?
        .ok_or_else(|| {
            format!(
                "{} is missing or empty: run `bamburino --install` from an elevated prompt",
                client_path.display()
            )
        })?;
    let driver_path = dir.driver_token_file();
    let driver_token = read_secret(&driver_path)
        .map_err(|e| format!("cannot read {}: {e}", driver_path.display()))?;

    let owner = match cfg.owner_sid.clone() {
        Some(s) => s,
        None => current_user_sid().map_err(|e| format!("cannot determine the user SID: {e}"))?,
    };
    let mut secrets = vec![client_token.clone()];
    secrets.extend(driver_token.clone());
    let guard = init_logging_with_secrets(
        &dir.logs_dir(),
        cfg.log_retention_days,
        to_stderr,
        &owner,
        secrets,
    )
    .map_err(|e| e.to_string())?;
    // `allowed_dirs` comes from config.toml only (written by `config init` as
    // the owner); nothing is resolved here, where the process may be
    // LocalService with a profile that is not the owner's.
    let (path_guard, excluded_roots) = PathGuard::new(cfg.allowed_dirs.clone());
    for (root, why) in &excluded_roots {
        tracing::warn!(
            "allowed_dirs: ignoring {}: {why}",
            redact(&root.display().to_string())
        );
    }
    for root in path_guard.unverified_roots() {
        tracing::warn!(
            "allowed_dirs: {} cannot be inspected by this account; links inside it are not checked here, Bambu Studio validates destinations itself",
            redact(&root.display().to_string())
        );
    }
    if path_guard.roots().is_empty() {
        tracing::warn!(
            "allowed_dirs: no usable directory in config.toml; studio_save_project and studio_export_plate refuse every path"
        );
    }

    if driver_token.is_none() {
        tracing::warn!("no driver token configured: run `bamburino config set-driver-token`");
    }
    let url = parse_driver_url(&cfg.driver_url).map_err(|e| format!("driver_url: {e}"))?;
    let driver = HttpDriverClient::new(HttpDriverConfig {
        url,
        token: driver_token,
        read_timeout: Duration::from_millis(cfg.read_timeout_ms),
        mutation_timeout: Duration::from_millis(cfg.mutation_timeout_ms),
    })
    .map_err(|e| e.to_string())?
    // `set-driver-token` takes effect without a service restart.
    .with_token_file(driver_path);

    let export_dir = dir.tmp_dir();
    if let Err(e) = std::fs::create_dir_all(&export_dir) {
        tracing::warn!("cannot create {}: {e}", export_dir.display());
    }
    bamburino_core::revision::wait_for_next_epoch_second();
    let tools = Tools::new(
        Arc::new(driver),
        Arc::new(WindowsProcessProbe),
        ToolConfig {
            allow_version_mismatch: cfg.allow_version_mismatch,
            poll_every: Duration::from_millis(cfg.poll_interval_ms),
            idle_after: POLL_IDLE_AFTER,
        },
        export_dir,
    );
    tools.set_path_guard(path_guard);
    let tools = Arc::new(tools);
    Ok(Prepared {
        cfg,
        client_token,
        tools,
        _log_guard: guard,
    })
}

/// Windows error "access to the socket is forbidden" (WSAEACCES): what a bind
/// on a port inside an excluded range returns.
const WSAEACCES: i32 = 10013;

/// Binds `127.0.0.1:<listen_port>`. Done before the service reports Running.
pub async fn bind(cfg: &Config) -> Result<tokio::net::TcpListener, String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], cfg.listen_port));
    tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        if e.raw_os_error() == Some(WSAEACCES) {
            bamburino::ports::reserved_message(cfg.listen_port)
        } else {
            format!("cannot bind {addr}: {e}")
        }
    })
}

/// Serves on `listener` until `shutdown` completes. The server owns the
/// revision poller.
pub async fn serve_on(
    p: &Prepared,
    listener: tokio::net::TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), String> {
    bamburino::server::serve_on(listener, p.client_token.clone(), p.tools.clone(), shutdown)
        .await
        .map_err(|e| e.to_string())
}

/// Creates `config.toml` and `client.token` in an explicit (test/smoke)
/// directory when they are missing. No service ACLs: such a directory is the
/// caller's own.
pub fn ensure_plain_config(dir: &ConfigDir) -> Result<Vec<String>, String> {
    let mut made = Vec::new();
    std::fs::create_dir_all(&dir.0)
        .map_err(|e| format!("cannot create {}: {e}", dir.0.display()))?;
    if !dir.config_file().exists() {
        std::fs::write(
            dir.config_file(),
            Config::defaults_for_current_user().to_toml(),
        )
        .map_err(|e| format!("cannot write {}: {e}", dir.config_file().display()))?;
        made.push(dir.config_file().display().to_string());
    }
    let tok = dir.client_token_file();
    if read_secret(&tok).map_err(|e| e.to_string())?.is_none() {
        std::fs::write(&tok, bamburino::secrets::generate_token())
            .map_err(|e| format!("cannot write {}: {e}", tok.display()))?;
        made.push(tok.display().to_string());
    }
    Ok(made)
}
