// SPDX-License-Identifier: AGPL-3.0-or-later
//! Windows service: install, uninstall and the service entry point.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bamburino::config::{Config, ConfigDir};
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

/// A Known Folder of the user running this process (`FOLDERID_Documents`,
/// ...), which follows OneDrive redirection.
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
        let path = PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(raw, len)));
        CoTaskMemFree(raw.cast());
        Some(path)
    }
}

/// Documents and Downloads of the installing user.
fn default_allowed_dirs() -> Vec<PathBuf> {
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, FOLDERID_Downloads};
    [FOLDERID_Documents, FOLDERID_Downloads]
        .iter()
        .filter_map(known_folder)
        .collect()
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
    let defaults = Config {
        allowed_dirs: default_allowed_dirs(),
        ..Config::default()
    };
    dir.init_with(defaults).map_err(|e| e.to_string())?;
    println!("configuration ready in {}", dir.0.display());

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
    if !same_file(&me, &target) {
        let parent = target.parent().expect("target has a parent");
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        std::fs::copy(&me, &target)
            .map_err(|e| format!("cannot copy the executable to {}: {e}", target.display()))?;
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
    service
        .start::<&str>(&[])
        .map_err(|e| svc_err("cannot start the service", e))?;
    wait_for(&service, ServiceState::Running, Duration::from_secs(30))?;
    println!("service {SERVICE_NAME} installed and running");
    println!();
    println!("Next steps:");
    println!("  1. bamburino config set-driver-token   (token on stdin, never echoed)");
    println!(
        "  2. Restart-Service {SERVICE_NAME}             (elevated; the service reads the token at start)"
    );
    println!("  3. bamburino config client-snippet     (prints the command for your MCP client)");
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
        let _ = handle.set_service_status(status(ServiceState::Running, accept, 0, 0));
        let result = rt.block_on(crate::runtime::serve(&prepared, async {
            stop.notified().await;
            let _ = handle.set_service_status(status(ServiceState::StopPending, none, 0, 15));
        }));
        if let Err(e) = &result {
            tracing::error!("server stopped with an error: {e}");
        }
        result
    })();

    let exit = match &outcome {
        Ok(()) => 0,
        Err(e) => {
            record_startup_error(&dir, e);
            1
        }
    };
    let _ = handle.set_service_status(status(ServiceState::Stopped, none, exit, 0));
    outcome
}

/// Leaves a trace in `logs\startup-error.log` when the service cannot even
/// start (bad configuration, port in use). The message never contains a token.
fn record_startup_error(dir: &ConfigDir, message: &str) {
    use std::io::Write;
    let logs = dir.logs_dir();
    if std::fs::create_dir_all(&logs).is_err() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join("startup-error.log"))
    {
        let _ = writeln!(f, "{message}");
    }
}
