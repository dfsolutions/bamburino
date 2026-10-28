// SPDX-License-Identifier: AGPL-3.0-or-later
//! Windows service: install, uninstall and the service entry point.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bamburino::config::{Config, ConfigDir};
use bamburino::ports;
use tokio::sync::Notify;
use windows_service::service::{
    Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

pub const SERVICE_NAME: &str = "Bamburino";
const DISPLAY_NAME: &str = "Bamburino MCP bridge";
const DESCRIPTION: &str = "Local MCP server that lets AI clients drive Bambu Studio.";
const ACCOUNT: &str = r"NT AUTHORITY\LocalService";
/// Exit code when the command needs an elevated prompt (ERROR_ACCESS_DENIED).
pub const EXIT_NOT_ELEVATED: i32 = 5;
/// Errors that stop the service are appended here (inside `logs\`).
const ERROR_LOG: &str = "service-errors.log";
/// The log is restarted when it grows past this size.
const ERROR_LOG_CAP: u64 = 256 * 1024;
const ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: i32 = 1063;
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;

// ---------------------------------------------------------------- helpers

/// Whether the process token is elevated (an administrator prompt).
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    // SAFETY: queries the current process token into correctly sized locals;
    // the handle is closed before returning.
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

fn program_files_exe() -> PathBuf {
    let base = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    base.join("Bamburino").join("bamburino.exe")
}

fn winapi_code(e: &windows_service::Error) -> Option<i32> {
    match e {
        windows_service::Error::Winapi(io) => io.raw_os_error(),
        _ => None,
    }
}

fn svc_err(what: &str, e: windows_service::Error) -> String {
    format!("{what}: {e}")
}

fn open_service(
    manager: &ServiceManager,
    access: ServiceAccess,
) -> Result<Option<Service>, String> {
    match manager.open_service(SERVICE_NAME, access) {
        Ok(s) => Ok(Some(s)),
        Err(e) if winapi_code(&e) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => Ok(None),
        Err(e) => Err(svc_err("cannot open the service", e)),
    }
}

fn wait_for(service: &Service, want: ServiceState, limit: Duration) -> Result<(), String> {
    let end = Instant::now() + limit;
    loop {
        let st = service
            .query_status()
            .map_err(|e| svc_err("cannot query the service", e))?;
        if st.current_state == want {
            return Ok(());
        }
        if Instant::now() > end {
            return Err(format!(
                "the service did not reach {want:?} within {} s (now {:?})",
                limit.as_secs(),
                st.current_state
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn stop_and_wait(service: &Service) -> Result<(), String> {
    let st = service
        .query_status()
        .map_err(|e| svc_err("cannot query the service", e))?;
    if st.current_state != ServiceState::Stopped {
        if st.current_state != ServiceState::StopPending {
            service
                .stop()
                .map_err(|e| svc_err("cannot stop the service", e))?;
        }
        wait_for(service, ServiceState::Stopped, Duration::from_secs(30))?;
    }
    Ok(())
}

fn require_elevation() -> Result<(), i32> {
    if is_elevated() {
        Ok(())
    } else {
        eprintln!("error: this command needs an elevated prompt (Run as administrator)");
        Err(EXIT_NOT_ELEVATED)
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Copies `me` next to `target` as `bamburino.exe.new`, then renames it over
/// the target, so a failed copy never leaves a truncated binary.
fn install_binary(me: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().expect("target has a parent");
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let staged = target.with_extension("exe.new");
    let result = std::fs::copy(me, &staged)
        .and_then(|_| std::fs::rename(&staged, target))
        .map_err(|e| format!("cannot install the executable in {}: {e}", target.display()));
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

/// Waits for Running; a service that falls back to Stopped while starting is
/// an immediate error.
fn wait_running(service: &Service, limit: Duration) -> Result<(), String> {
    let end = Instant::now() + limit;
    loop {
        match service
            .query_status()
            .map_err(|e| svc_err("cannot query the service", e))?
            .current_state
        {
            ServiceState::Running => return Ok(()),
            ServiceState::Stopped => {
                return Err("the service stopped while starting".to_owned());
            }
            _ if Instant::now() > end => {
                return Err(format!(
                    "the service did not reach Running within {} s",
                    limit.as_secs()
                ));
            }
            _ => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

/// Last line of the service error log, if any.
fn last_error_line(dir: &ConfigDir) -> Option<String> {
    let text = std::fs::read_to_string(dir.logs_dir().join(ERROR_LOG)).ok()?;
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(str::to_owned)
}

// ---------------------------------------------------------------- install

/// `--install`: configuration, ACLs, binary, service registration, start.
/// Idempotent: an existing service gets the new binary and is restarted; an
/// existing configuration and client token are kept.
pub fn install() -> i32 {
    if let Err(code) = require_elevation() {
        return code;
    }
    match try_install() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn try_install() -> Result<(), String> {
    let dir = ConfigDir::resolve(None);
    dir.init().map_err(|e| e.to_string())?;
    println!("configuration ready in {}", dir.0.display());
    let cfg = Config::load(&dir).map_err(|e| e.to_string())?;
    let pre = ports::preflight(&cfg, &ports::netsh_excluded_ranges);
    for w in &pre.warnings {
        eprintln!("warning: {w}");
    }
    if let Some(refusal) = pre.refusal {
        return Err(format!("{refusal}; no service was created"));
    }

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| svc_err("cannot open the service control manager", e))?;
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::CHANGE_CONFIG;
    let existing = open_service(&manager, access)?;
    if let Some(s) = &existing {
        stop_and_wait(s)?;
    }

    let target = program_files_exe();
    let me = std::env::current_exe().map_err(|e| format!("cannot locate this executable: {e}"))?;
    if !same_file(&me, &target)
        && let Err(e) = install_binary(&me, &target)
    {
        // The old service (if any) was stopped for the copy: bring it back.
        if let Some(s) = &existing {
            let _ = s.start::<&str>(&[]);
        }
        return Err(e);
    }
    println!("binary installed in {}", target.display());

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: target,
        launch_arguments: vec![OsString::from("--service")],
        dependencies: vec![],
        account_name: Some(OsString::from(ACCOUNT)),
        account_password: None,
    };
    let service = match existing {
        Some(s) => {
            s.change_config(&info)
                .map_err(|e| svc_err("cannot update the service", e))?;
            s
        }
        None => manager
            .create_service(&info, access | ServiceAccess::CHANGE_CONFIG)
            .map_err(|e| svc_err("cannot create the service", e))?,
    };
    service
        .set_description(DESCRIPTION)
        .map_err(|e| svc_err("cannot set the description", e))?;
    service
        .set_delayed_auto_start(true)
        .map_err(|e| svc_err("cannot set delayed automatic start", e))?;
    // Best effort: restart after a crash.
    let _ = service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![ServiceAction {
            action_type: ServiceActionType::Restart,
            delay: Duration::from_secs(10),
        }]),
    });
    let started = service
        .start::<&str>(&[])
        .map_err(|e| svc_err("cannot start the service", e))
        .and_then(|()| wait_running(&service, Duration::from_secs(30)));
    if let Err(e) = started {
        let last =
            last_error_line(&dir).map_or(String::new(), |l| format!(" (last service error: {l})"));
        return Err(format!(
            "{e}{last}; the service stays installed: fix the cause (see {}) and run `Start-Service {SERVICE_NAME}`",
            dir.logs_dir().join(ERROR_LOG).display()
        ));
    }
    println!("service {SERVICE_NAME} installed and running");
    println!();
    println!("Next steps:");
    println!("  1. bamburino config set-driver-token   (token on stdin, never echoed)");
    println!("      (the running service picks the new token up by itself: no restart needed)");
    println!("  2. bamburino config client-snippet     (prints the command for your MCP client)");
    println!(
        "      (run it in your own terminal, never inside an AI session; to replace a leaked token: bamburino config rotate-client-token, then restart the service: the old token stays valid until then)"
    );
    Ok(())
}

// -------------------------------------------------------------- uninstall

/// `--uninstall [--purge]`.
pub fn uninstall(purge: bool) -> i32 {
    if let Err(code) = require_elevation() {
        return code;
    }
    match try_uninstall(purge) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn try_uninstall(purge: bool) -> Result<(), String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| svc_err("cannot open the service control manager", e))?;
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
    match open_service(&manager, access)? {
        Some(s) => {
            stop_and_wait(&s)?;
            s.delete()
                .map_err(|e| svc_err("cannot remove the service", e))?;
            println!("service {SERVICE_NAME} removed");
        }
        None => println!("service {SERVICE_NAME} is not installed"),
    }
    let exe = program_files_exe();
    if exe.exists() {
        // Best effort: this very process may be the installed binary.
        match std::fs::remove_file(&exe) {
            Ok(()) => {
                if let Some(parent) = exe.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
            }
            Err(e) => println!("note: could not delete {}: {e}", exe.display()),
        }
    }
    let dir = ConfigDir::resolve(None);
    if purge {
        if dir.0.exists() {
            std::fs::remove_dir_all(&dir.0)
                .map_err(|e| format!("cannot delete {}: {e}", dir.0.display()))?;
            println!("deleted {}", dir.0.display());
        }
    } else {
        println!(
            "configuration kept in {} (use --purge to delete)",
            dir.0.display()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------- service

define_windows_service!(ffi_service_main, service_main);

/// `--service`: hand the thread to the SCM dispatcher. Started from a console
/// the dispatcher cannot connect: exit 2 with a message.
pub fn run_dispatcher() -> i32 {
    match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => 0,
        Err(e) if winapi_code(&e) == Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT) => {
            eprintln!(
                "error: --service can only be started by the Windows service control manager; use --run to run in a console"
            );
            2
        }
        Err(e) => {
            eprintln!("error: cannot start the service dispatcher: {e}");
            1
        }
    }
}

fn service_main(_args: Vec<OsString>) {
    // The outcome is already reported to the SCM (exit code) inside.
    let _ = run_service();
}

fn status(
    state: ServiceState,
    accept: ServiceControlAccept,
    code: u32,
    hint: u64,
) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: if code == 0 {
            ServiceExitCode::Win32(0)
        } else {
            ServiceExitCode::ServiceSpecific(code)
        },
        checkpoint: 0,
        wait_hint: Duration::from_secs(hint),
        process_id: None,
    }
}

fn run_service() -> Result<(), String> {
    let stop = Arc::new(Notify::new());
    let stop_for_handler = stop.clone();
    let handle = service_control_handler::register(SERVICE_NAME, move |ctl| match ctl {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop_for_handler.notify_one();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })
    .map_err(|e| svc_err("cannot register the control handler", e))?;

    let none = ServiceControlAccept::empty();
    let _ = handle.set_service_status(status(ServiceState::StartPending, none, 0, 15));

    let dir = ConfigDir::resolve(None);
    let outcome = (|| {
        let prepared = crate::runtime::prepare(&dir, false)?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start the async runtime: {e}"))?;
        let accept = ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN;
        let result = rt.block_on(async {
            // Bind first: Running is only reported once the port is ours.
            let listener = crate::runtime::bind(&prepared.cfg).await?;
            let _ = handle.set_service_status(status(ServiceState::Running, accept, 0, 0));
            crate::runtime::serve_on(&prepared, listener, async {
                stop.notified().await;
                let _ = handle.set_service_status(status(ServiceState::StopPending, none, 0, 15));
            })
            .await
        });
        if let Err(e) = &result {
            tracing::error!("server stopped with an error: {e}");
        }
        result
    })();

    let exit = match &outcome {
        Ok(()) => 0,
        Err(e) => {
            record_service_error(&dir, e);
            1
        }
    };
    let _ = handle.set_service_status(status(ServiceState::Stopped, none, exit, 0));
    outcome
}

/// Appends to `logs\service-errors.log` when the service cannot start or
/// dies (bad configuration, port reserved or in use). The message never
/// contains a token. The file restarts once it exceeds `ERROR_LOG_CAP`.
fn record_service_error(dir: &ConfigDir, message: &str) {
    use std::io::Write;
    let logs = dir.logs_dir();
    if std::fs::create_dir_all(&logs).is_err() {
        return;
    }
    let path = logs.join(ERROR_LOG);
    let big = std::fs::metadata(&path).is_ok_and(|m| m.len() > ERROR_LOG_CAP);
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true);
    if big {
        opts.write(true).truncate(true);
    } else {
        opts.append(true);
    }
    if let Ok(mut f) = opts.open(&path) {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = writeln!(f, "[unix {secs}] {message}");
    }
}
