//! trawlrd as a launchd daemon: installing, removing and running it.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::Duration;

use super::indexer::{self, Msg, Shared};
use super::{DATA_DIR, LOG_FILE};
use crate::config::{Overrides, Settings};
use crate::{info, server, util};

pub const LABEL: &str = "io.trawlr.trawlrd";
const PLIST: &str = "/Library/LaunchDaemons/io.trawlr.trawlrd.plist";
const DEFAULT_CONFIG: &str = include_str!("../../dist/macos/config.toml");

/// Create the data folder, readable by root only: the index in it names
/// every file on the Mac, and a user who may not search must not read it.
fn lock_down_data_dir() -> std::io::Result<()> {
    std::fs::create_dir_all(DATA_DIR)?;
    std::fs::set_permissions(DATA_DIR, std::fs::Permissions::from_mode(0o700))?;
    let c = util::cstr(DATA_DIR.as_bytes());
    unsafe { libc::chown(c.as_ptr(), 0, 0) };
    Ok(())
}

/// Run until SIGTERM (launchd stopping the service) or Ctrl+C.
pub fn run(ov: Overrides, as_service: bool) -> i32 {
    if as_service {
        if let Err(e) = lock_down_data_dir() {
            eprintln!("trawlrd: {DATA_DIR}: {e}");
        }
        if let Err(e) = util::log_to_file(Path::new(LOG_FILE)) {
            eprintln!("trawlrd: {LOG_FILE}: {e}");
        }
    }
    let settings = match Settings::load(ov) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            crate::warn!("{e}");
            eprintln!("trawlrd: {e}");
            return 2;
        }
    };
    if settings.system && !as_service && unsafe { libc::geteuid() } == 0 {
        let _ = lock_down_data_dir();
    }

    // Signals are taken by waiting for them on this thread, so block them
    // before any other thread exists and inherits the mask.
    let mut sigs: libc::sigset_t = 0;
    unsafe {
        libc::sigemptyset(&mut sigs);
        libc::sigaddset(&mut sigs, libc::SIGINT);
        libc::sigaddset(&mut sigs, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &sigs, std::ptr::null_mut());
    }

    info!("trawlrd {} starting ({} mode)", env!("CARGO_PKG_VERSION"), if settings.system { "system" } else { "user" });
    // Bounded: if the indexer falls behind, the change history waits where
    // it is kept (on disk) instead of piling up in memory here.
    let (tx, rx) = sync_channel::<Msg>(4096);
    let sh = Arc::new(Shared::new(settings.clone(), tx.clone()));
    let listener = match server::bind(&sh) {
        Ok(l) => l,
        Err(e) => {
            crate::warn!("{}: {e}", settings.socket.display());
            eprintln!("trawlrd: {}: {e}", settings.socket.display());
            return 1;
        }
    };
    let indexer_thread = {
        let sh = sh.clone();
        std::thread::Builder::new()
            .name("indexer".into())
            .spawn(move || {
                // The index has a single writer. If it dies, exit and let
                // launchd start us again rather than serve a stale index.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| indexer::run(sh, rx))).is_err() {
                    crate::warn!("the indexer crashed; exiting");
                    std::process::exit(1);
                }
            })
            .expect("spawn indexer")
    };
    {
        let sh = sh.clone();
        std::thread::Builder::new().name("server".into()).spawn(move || server::serve(sh, listener)).expect("spawn server");
    }

    let mut sig: libc::c_int = 0;
    unsafe { libc::sigwait(&sigs, &mut sig) };
    info!("shutting down");
    sh.cancel.store(true, Ordering::Relaxed);
    let _ = tx.send(Msg::Shutdown);
    let _ = indexer_thread.join();
    let _ = std::fs::remove_file(&settings.socket);
    0
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn launchctl(args: &[&str]) -> Result<(), String> {
    let out = Command::new("/bin/launchctl").args(args).output().map_err(|e| format!("launchctl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("launchctl {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn need_root() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("this needs administrator rights: run it with sudo".into());
    }
    Ok(())
}

/// Register trawlrd with launchd as a system daemon and start it. Run again
/// after an upgrade, it restarts the new program.
pub fn install() -> Result<(), String> {
    need_root()?;
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| format!("cannot tell where trawlrd is: {e}"))?;
    lock_down_data_dir().map_err(|e| format!("{DATA_DIR}: {e}"))?;
    let config = Path::new(DATA_DIR).join("config.toml");
    if !config.exists() {
        std::fs::write(&config, DEFAULT_CONFIG).map_err(|e| format!("{}: {e}", config.display()))?;
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{}</string>
		<string>--service</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ExitTimeOut</key>
	<integer>60</integer>
	<key>ProcessType</key>
	<string>Standard</string>
</dict>
</plist>
"#,
        xml_escape(&exe.to_string_lossy())
    );
    // Stop a running copy first; it may be the program being replaced.
    if registered() {
        let _ = launchctl(&["bootout", &format!("system/{LABEL}")]);
        for _ in 0..120 {
            if !registered() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    std::fs::write(PLIST, plist).map_err(|e| format!("{PLIST}: {e}"))?;
    let _ = std::fs::set_permissions(PLIST, std::fs::Permissions::from_mode(0o644));
    let _ = launchctl(&["enable", &format!("system/{LABEL}")]);
    // launchd may still be letting go of the old copy.
    let mut last = Ok(());
    for _ in 0..10 {
        last = launchctl(&["bootstrap", "system", PLIST]);
        if last.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    last
}

fn registered() -> bool {
    launchctl(&["print", &format!("system/{LABEL}")]).is_ok()
}

/// Stop the daemon and unregister it. The index and configuration stay.
pub fn uninstall() -> Result<(), String> {
    need_root()?;
    let _ = launchctl(&["bootout", &format!("system/{LABEL}")]);
    // launchd lets go of it only once it has saved the index and exited.
    for _ in 0..120 {
        if !registered() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if registered() {
        return Err(format!("launchd still has {LABEL}; try again in a moment"));
    }
    match std::fs::remove_file(PLIST) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{PLIST}: {e}")),
    }
}
