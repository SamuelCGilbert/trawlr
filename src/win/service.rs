//! trawlrd as a Windows service, and in a console for testing.

use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::core::PWSTR;
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW, OpenServiceW,
    QueryServiceStatus, RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW, StartServiceW, SC_ACTION, SC_ACTION_RESTART, SC_HANDLE,
    SC_MANAGER_ALL_ACCESS, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS, SERVICE_AUTO_START, SERVICE_CONFIG_DESCRIPTION,
    SERVICE_CONFIG_FAILURE_ACTIONS, SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO, SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP,
    SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL, SERVICE_FAILURE_ACTIONSW, SERVICE_NO_CHANGE, SERVICE_REQUIRED_PRIVILEGES_INFOW, SERVICE_RUNNING,
    SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};

use super::indexer::{self, Msg, Shared};
use super::{pipe, wide};
use crate::config::{data_dir, Overrides, Settings};
use crate::{info, util, warn};

pub const SERVICE_NAME: &str = "trawlrd";
const DISPLAY_NAME: &str = "Trawlr file index";
const DESCRIPTION: &str = "Keeps an index of every file and folder name on this computer's NTFS drives, for searching with Trawlr.";

/// Where a stop request (from the service manager or Ctrl+C) is delivered.
static STOP: OnceLock<Mutex<Option<Sender<()>>>> = OnceLock::new();
static STATUS: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
static OVERRIDES: OnceLock<Mutex<Option<Overrides>>> = OnceLock::new();

fn request_stop() {
    if let Some(s) = STOP.get().and_then(|m| m.lock().ok()?.take()) {
        let _ = s.send(());
    }
}

fn stop_channel() -> Receiver<()> {
    let (tx, rx) = channel();
    *STOP.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(tx);
    rx
}

/// Run the daemon until asked to stop.
pub fn run_daemon(settings: Arc<Settings>, stop: Receiver<()>) -> Result<(), String> {
    // Bounded: if the indexer falls behind, the change history waits where
    // it is kept (on disk) instead of piling up in memory here.
    let (tx, rx) = sync_channel::<Msg>(256);
    let sh = Arc::new(Shared::new(settings, tx.clone()));
    let listener = pipe::bind().map_err(|e| e.to_string())?;
    let indexer_thread = {
        let sh = sh.clone();
        std::thread::Builder::new()
            .name("indexer".into())
            .spawn(move || {
                // The index has a single writer. If it dies, exit and let the
                // service manager restart us rather than serve a stale index.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| indexer::run(sh, rx))).is_err() {
                    warn!("the indexer crashed; exiting");
                    std::process::exit(1);
                }
            })
            .map_err(|e| e.to_string())?
    };
    {
        let sh = sh.clone();
        std::thread::Builder::new().name("server".into()).spawn(move || pipe::serve(sh, listener)).map_err(|e| e.to_string())?;
    }
    let _ = stop.recv();
    info!("shutting down");
    sh.cancel.store(true, Ordering::Relaxed);
    let _ = tx.send(Msg::Shutdown);
    let _ = indexer_thread.join();
    Ok(())
}

unsafe extern "system" fn console_handler(_ctrl: u32) -> i32 {
    request_stop();
    1
}

/// Run in this console until Ctrl+C. Needs an administrator console.
pub fn run_console(ov: Overrides) -> i32 {
    let settings = match Settings::load(ov) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("trawlrd: {e}");
            return 2;
        }
    };
    let stop = stop_channel();
    unsafe { SetConsoleCtrlHandler(Some(console_handler), 1) };
    match run_daemon(settings, stop) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("trawlrd: {e}");
            1
        }
    }
}

fn set_status(state: u32, exit_code: u32, wait_ms: u32) {
    let h: SERVICE_STATUS_HANDLE = STATUS.load(Ordering::Relaxed);
    if h.is_null() {
        return;
    }
    let accepts = if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 };
    let st = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepts,
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_ms,
    };
    unsafe { SetServiceStatus(h, &st) };
}

unsafe extern "system" fn control_handler(control: u32, _event: u32, _data: *mut std::ffi::c_void, _ctx: *mut std::ffi::c_void) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            // Saving the index takes a moment.
            set_status(SERVICE_STOP_PENDING, 0, 30_000);
            request_stop();
            0
        }
        SERVICE_CONTROL_INTERROGATE => 0,
        // ERROR_CALL_NOT_IMPLEMENTED
        _ => 120,
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = wide(SERVICE_NAME);
    let h = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control_handler), std::ptr::null_mut()) };
    if h.is_null() {
        return;
    }
    STATUS.store(h, Ordering::Relaxed);
    set_status(SERVICE_START_PENDING, 0, 10_000);
    let lock = super::security::lock_down_dir(&data_dir());
    let _ = util::log_to_file(&data_dir().join("trawlrd.log"));
    if let Err(e) = lock {
        warn!("cannot restrict access to {}: {e}", data_dir().display());
    }
    let ov = OVERRIDES.get().and_then(|m| m.lock().ok()?.take()).unwrap_or_default();
    let settings = match Settings::load(ov) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            warn!("{e}");
            set_status(SERVICE_STOPPED, 1, 0);
            return;
        }
    };
    let stop = stop_channel();
    set_status(SERVICE_RUNNING, 0, 0);
    let code = match run_daemon(settings, stop) {
        Ok(()) => 0,
        Err(e) => {
            warn!("{e}");
            1
        }
    };
    set_status(SERVICE_STOPPED, code, 0);
}

/// Hand this process to the service manager, which calls `service_main`.
pub fn run_service(ov: Overrides) -> i32 {
    *OVERRIDES.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(ov);
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: name.as_mut_ptr(), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW { lpServiceName: std::ptr::null_mut(), lpServiceProc: None },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        eprintln!("trawlrd: --service is for the Windows service manager: {}", std::io::Error::last_os_error());
        return 1;
    }
    0
}

struct Sc(SC_HANDLE);
impl Drop for Sc {
    fn drop(&mut self) {
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn sc(h: SC_HANDLE) -> Result<Sc, String> {
    if h.is_null() { Err(std::io::Error::last_os_error().to_string()) } else { Ok(Sc(h)) }
}

/// Register trawlrd as a service that starts with Windows, and start it.
pub fn install() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let cmd = wide(format!("\"{}\" --service", exe.display()));
    let (name, display) = (wide(SERVICE_NAME), wide(DISPLAY_NAME));
    let scm = sc(unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) }).map_err(|e| format!("cannot reach the service manager (run as administrator): {e}"))?;
    let created = unsafe {
        CreateServiceW(
            scm.0,
            name.as_ptr(),
            display.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            cmd.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    let svc = if created.is_null() {
        // Already there (an upgrade): point it at this program.
        let svc = sc(unsafe { OpenServiceW(scm.0, name.as_ptr(), SERVICE_ALL_ACCESS) })?;
        let ok = unsafe {
            ChangeServiceConfigW(
                svc.0,
                SERVICE_NO_CHANGE,
                SERVICE_AUTO_START,
                SERVICE_NO_CHANGE,
                cmd.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                display.as_ptr(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        svc
    } else {
        Sc(created)
    };
    let mut desc = wide(DESCRIPTION);
    let d = SERVICE_DESCRIPTIONW { lpDescription: desc.as_mut_ptr() };
    unsafe { ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_DESCRIPTION, &d as *const _ as *const _) };
    // Restart after a crash: 5 seconds, then again, then after a minute.
    let actions = [
        SC_ACTION { Type: SC_ACTION_RESTART, Delay: 5_000 },
        SC_ACTION { Type: SC_ACTION_RESTART, Delay: 5_000 },
        SC_ACTION { Type: SC_ACTION_RESTART, Delay: 60_000 },
    ];
    let fa = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 24 * 3600,
        lpRebootMsg: std::ptr::null_mut(),
        lpCommand: std::ptr::null_mut(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_ptr() as *mut _,
    };
    unsafe { ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_FAILURE_ACTIONS, &fa as *const _ as *const _) };
    // Run with only the privileges it needs: none of the powerful ones
    // (debugging other programs, taking ownership, loading drivers ...)
    // the system account otherwise holds. Reading drives comes from being
    // an administrator, not from a privilege.
    let mut privs: Vec<u16> = "SeChangeNotifyPrivilege\0SeImpersonatePrivilege\0\0".encode_utf16().collect();
    let rp = SERVICE_REQUIRED_PRIVILEGES_INFOW { pmszRequiredPrivileges: privs.as_mut_ptr() };
    unsafe { ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO, &rp as *const _ as *const _) };
    if unsafe { StartServiceW(svc.0, 0, std::ptr::null()) } == 0 {
        let e = std::io::Error::last_os_error();
        // ERROR_SERVICE_ALREADY_RUNNING
        if e.raw_os_error() != Some(1056) {
            return Err(format!("installed, but it did not start: {e}"));
        }
    }
    Ok(())
}

fn wait_stopped(svc: &Sc, limit: Duration) -> bool {
    let t0 = Instant::now();
    loop {
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { QueryServiceStatus(svc.0, &mut st) } == 0 || st.dwCurrentState == SERVICE_STOPPED {
            return true;
        }
        if t0.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Stop the service and remove it.
pub fn uninstall() -> Result<(), String> {
    let name = wide(SERVICE_NAME);
    let scm = sc(unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_ALL_ACCESS) }).map_err(|e| format!("cannot reach the service manager (run as administrator): {e}"))?;
    let svc = match sc(unsafe { OpenServiceW(scm.0, name.as_ptr(), SERVICE_ALL_ACCESS) }) {
        Ok(s) => s,
        // Not installed: nothing to do.
        Err(_) => return Ok(()),
    };
    let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st) };
    if !wait_stopped(&svc, Duration::from_secs(60)) {
        warn!("the service did not stop in time; removing it anyway");
    }
    if unsafe { DeleteService(svc.0) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
