// SPDX-License-Identifier: AGPL-3.0-or-later
//! `bamburino.exe` entry point: command line, foreground mode, configuration
//! commands. Service installation and the service entry point are in `winsvc`.

mod process;
mod runtime;
#[cfg(windows)]
mod winsvc;

use std::path::PathBuf;

use bamburino::config::{Config, ConfigDir, ConfigError};
use clap::error::ErrorKind;
use clap::{ArgGroup, CommandFactory, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "bamburino",
    about = "Local MCP bridge for Bambu Studio",
    disable_version_flag = true,
    args_conflicts_with_subcommands = true,
    group(ArgGroup::new("mode").args(["run", "service", "install", "uninstall", "version"]).multiple(false))
)]
struct Cli {
    /// Run the server in the foreground (logs also on stderr; stop with Ctrl+C).
    #[arg(long)]
    run: bool,
    /// Windows service entry point (started by the service control manager only).
    #[arg(long)]
    service: bool,
    /// Install and start the Windows service (needs an elevated prompt).
    #[arg(long)]
    install: bool,
    /// Stop and remove the Windows service (needs an elevated prompt).
    #[arg(long)]
    uninstall: bool,
    /// With --uninstall: also delete the configuration directory.
    #[arg(long)]
    purge: bool,
    /// Print the version, the pinned catalog and the expected driver patch.
    #[arg(long)]
    version: bool,
    /// Configuration directory (only with --run or the config commands).
    #[arg(long, global = true, value_name = "DIR")]
    config_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Configuration commands.
    #[command(subcommand)]
    Config(ConfigCmd),
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Create the configuration directory (needs an elevated prompt).
    Init,
    /// Save the driver token read from stdin (never printed).
    SetDriverToken,
    /// Replace the client token with a new random one (never printed; needs
    /// an elevated prompt). Restart the service and update the MCP client.
    RotateClientToken,
    /// Print the command that registers Bamburino in an MCP client. This is
    /// the only command that shows the client token.
    ClientSnippet {
        #[arg(long, value_enum, default_value = "claude-code")]
        client: Client,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Client {
    ClaudeCode,
    Generic,
}

fn main() {
    let cli = Cli::parse();
    if cli.config_dir.is_some() && !cli.run && cli.command.is_none() {
        Cli::command()
            .error(
                ErrorKind::ArgumentConflict,
                "--config-dir is only allowed with --run or a `config` command",
            )
            .exit();
    }
    if cli.purge && !cli.uninstall {
        Cli::command()
            .error(
                ErrorKind::MissingRequiredArgument,
                "--purge requires --uninstall",
            )
            .exit();
    }
    let code = match (&cli.command, &cli) {
        (Some(Cmd::Config(c)), _) => run_config(c, cli.config_dir.clone()),
        (None, c) if c.version => version(),
        (None, c) if c.run => run_foreground(cli.config_dir.clone()),
        (None, c) if c.service => service(),
        (None, c) if c.install => install(),
        (None, c) if c.uninstall => uninstall(c.purge),
        _ => {
            Cli::command()
                .error(
                    ErrorKind::MissingRequiredArgument,
                    "choose one of --run, --service, --install, --uninstall, --version or a `config` command",
                )
                .exit();
        }
    };
    std::process::exit(code);
}

fn version() -> i32 {
    use bamburino_core::build_info::{
        BAMBURINO_VERSION, EXPECTED_PATCH_ID, STUDIO_VERSION, TAG_COMMIT,
    };
    println!(
        "bamburino {BAMBURINO_VERSION} (catalog {STUDIO_VERSION} @ {}, expected driver patch {})",
        TAG_COMMIT.get(..8).unwrap_or(TAG_COMMIT),
        EXPECTED_PATCH_ID.unwrap_or("none")
    );
    0
}

/// `--run`: same wiring as the service, logs also on stderr, Ctrl+C stops.
/// A directory given with `--config-dir` that lacks `config.toml` or
/// `client.token` gets them (new token, no service ACLs).
fn run_foreground(config_dir: Option<PathBuf>) -> i32 {
    let explicit = config_dir.is_some();
    let dir = ConfigDir::resolve(config_dir);
    if explicit {
        match runtime::ensure_plain_config(&dir) {
            Ok(made) => {
                for f in made {
                    eprintln!("created {f} (no service ACLs: foreground test directory)");
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }
    let result = (|| {
        let prepared = runtime::prepare(&dir, true)?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start the async runtime: {e}"))?;
        rt.block_on(async {
            let listener = runtime::bind(&prepared.cfg).await?;
            runtime::serve_on(&prepared, listener, async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
        })
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(windows)]
fn service() -> i32 {
    winsvc::run_dispatcher()
}
#[cfg(windows)]
fn install() -> i32 {
    winsvc::install()
}
#[cfg(windows)]
fn uninstall(purge: bool) -> i32 {
    winsvc::uninstall(purge)
}
#[cfg(not(windows))]
fn service() -> i32 {
    not_windows()
}
#[cfg(not(windows))]
fn install() -> i32 {
    not_windows()
}
#[cfg(not(windows))]
fn uninstall(_purge: bool) -> i32 {
    not_windows()
}
#[cfg(not(windows))]
fn not_windows() -> i32 {
    eprintln!("error: the Windows service is only available on Windows");
    2
}

/// `config init` (elevated), `config set-driver-token` (token on STDIN, never
/// echoed, never an argument), `config rotate-client-token` (elevated,
/// regenerates `client.token` atomically, never prints it), `config
/// client-snippet`.
///
/// `set-driver-token` does NOT need elevation and does not call `init`: it
/// overwrites the existing `driver.token` in place, which the installing
/// user may modify. It fails loudly if the installation never ran. In an
/// explicit `--config-dir` (tests, smoke) a missing file is created.
fn run_config(cmd: &ConfigCmd, config_dir: Option<PathBuf>) -> i32 {
    let explicit = config_dir.is_some();
    let dir = ConfigDir::resolve(config_dir);
    let result = match cmd {
        ConfigCmd::Init => init_config(&dir),
        ConfigCmd::SetDriverToken => set_driver_token(&dir, explicit),
        ConfigCmd::RotateClientToken => rotate_client_token(&dir, explicit),
        ConfigCmd::ClientSnippet { client } => client_snippet(&dir, *client),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// `config init`, then the port preflight (a reserved `listen_port` is an
/// error: the config stays and can be edited).
fn init_config(dir: &ConfigDir) -> Result<(), ConfigError> {
    dir.init()?;
    println!("configuration ready in {}", dir.0.display());
    let cfg = Config::load(dir)?;
    let pre = bamburino::ports::preflight(&cfg, &bamburino::ports::netsh_excluded_ranges);
    for w in &pre.warnings {
        eprintln!("warning: {w}");
    }
    match pre.refusal {
        Some(r) => Err(ConfigError::Invalid(r)),
        None => Ok(()),
    }
}

fn set_driver_token(dir: &ConfigDir, explicit: bool) -> Result<(), ConfigError> {
    use bamburino::console::EchoOff;
    use bamburino::secrets::{overwrite_secret, read_driver_token_from_stdin};

    let path = dir.driver_token_file();
    if !path.exists() {
        if !explicit {
            return Err(ConfigError::Invalid(format!(
                "{} not found: run `bamburino --install` from an elevated prompt first",
                path.display()
            )));
        }
        std::fs::create_dir_all(&dir.0)
            .and_then(|()| std::fs::write(&path, ""))
            .map_err(|e| ConfigError::Io(path.clone(), e.to_string()))?;
    }
    let echo = EchoOff::disable();
    if echo.is_some() {
        eprint!("Driver token (64 hex characters, input hidden): ");
    }
    let token = read_driver_token_from_stdin(std::io::stdin().lock());
    if echo.is_some() {
        eprintln!();
    }
    drop(echo);
    overwrite_secret(&path, &token?).map_err(|e| ConfigError::Io(path.clone(), e.to_string()))?;
    println!("driver token saved");
    Ok(())
}

/// Replaces `client.token` with a fresh token (temp file + rename, same ACL
/// rules as `config init`). Fails if the installation does not exist yet. The
/// token is never printed. Outside an explicit `--config-dir` the root is
/// vetted like `init` does (owner SYSTEM/Administrators, protected DACL, no
/// reparse point) before anything is written.
fn rotate_client_token(dir: &ConfigDir, explicit: bool) -> Result<(), ConfigError> {
    use bamburino::acl::{OwnerAccess, apply_acl, current_user_sid};
    use bamburino::secrets::{generate_token, read_secret, write_secret_atomic};
    use bamburino::secure::root_problem;

    let path = dir.client_token_file();
    let no_install = || {
        ConfigError::Invalid(format!(
            "{} is missing or empty: run `bamburino config init` (or `--install`) from an elevated prompt first",
            path.display()
        ))
    };
    let io = |e: std::io::Error| {
        let hint = if e.kind() == std::io::ErrorKind::PermissionDenied {
            " (rotating the token needs an elevated Administrator prompt)"
        } else {
            ""
        };
        ConfigError::Io(path.clone(), format!("{e}{hint}"))
    };
    if !dir.0.exists() {
        return Err(no_install());
    }
    if !explicit && let Some(problem) = root_problem(&dir.0) {
        return Err(ConfigError::Invalid(format!(
            "refusing to use {}: {problem}. Remove or fix the directory as Administrator and run `bamburino config init` again",
            dir.0.display()
        )));
    }
    let cfg = match Config::load(dir) {
        Err(ConfigError::Missing(_)) => return Err(no_install()),
        other => other?,
    };
    if read_secret(&path).map_err(io)?.is_none() {
        return Err(no_install());
    }
    let sid = match cfg.owner_sid {
        Some(s) => s,
        None => current_user_sid().map_err(io)?,
    };
    write_secret_atomic(&path, &generate_token(), &sid).map_err(io)?;
    apply_acl(&path, &sid, OwnerAccess::Read).map_err(io)?;
    println!(
        "Client token rotated. The old token stays valid until the Bamburino service is restarted: restart it now, then update your MCP client (run `bamburino config client-snippet` in a terminal, not inside an AI session)."
    );
    Ok(())
}

fn client_snippet(dir: &ConfigDir, client: Client) -> Result<(), ConfigError> {
    let cfg = Config::load(dir)?;
    let path = dir.client_token_file();
    let token = bamburino::secrets::read_secret(&path)
        .map_err(|e| ConfigError::Io(path.clone(), e.to_string()))?
        .ok_or_else(|| ConfigError::Invalid(format!("{} is missing or empty", path.display())))?;
    let url = format!("http://127.0.0.1:{}/mcp", cfg.listen_port);
    eprintln!(
        "warning: this output contains the client token; run it in a terminal you control, never inside an AI-agent session or a shared log."
    );
    match client {
        Client::ClaudeCode => println!(
            "claude mcp add --transport http bamburino {url} --header \"Authorization: Bearer {token}\""
        ),
        Client::Generic => {
            println!("URL:    {url}");
            println!("Header: Authorization: Bearer {token}");
        }
    }
    Ok(())
}
