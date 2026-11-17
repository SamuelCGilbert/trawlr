//! The indexing daemon.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, AtomicU8};
#[cfg(target_os = "linux")]
use std::sync::mpsc::channel;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::Instant;

#[cfg(target_os = "linux")]
use parking_lot::{Mutex, RwLock};
#[cfg(target_os = "linux")]
use trawlr::config::Settings;
use trawlr::config::Overrides;
#[cfg(target_os = "linux")]
use trawlr::index::Index;
#[cfg(target_os = "linux")]
use trawlr::indexer::{self, Msg, Shared, ST_STARTING};
#[cfg(target_os = "linux")]
use trawlr::mounts::FsRegistry;
#[cfg(target_os = "linux")]
use trawlr::scan::ScanCtx;
#[cfg(target_os = "linux")]
use trawlr::watch::Watcher;
use trawlr::util;
#[cfg(target_os = "linux")]
use trawlr::{info, server, warn};

#[cfg(windows)]
const HELP: &str = "\
trawlrd - keeps a live index of every file name and answers searches

Usage: trawlrd [options]

Runs as the Windows service \"trawlrd\" (installed with the program). It
reads the master file table of every local NTFS drive and follows their
change journals. Run in an administrator console to test it in the open.

Options:
  --install-service    register and start the service (as administrator)
  --uninstall-service  stop and remove the service
  --config FILE        configuration file
                       (default C:\\ProgramData\\Trawlr\\config.toml)
  --root DRIVE         index DRIVE (C:) instead of every NTFS drive (repeatable)
  --db PATH            where the index is saved
  --debug              verbose logging
  -h, --help           show this help
  -V, --version        show the version
";

#[cfg(target_os = "macos")]
const HELP: &str = "\
trawlrd - keeps a live index of every file name and answers searches

Usage: trawlrd [options]

Runs as the launchd daemon io.trawlr.trawlrd (installed with the program),
indexing the whole Mac and following changes through FSEvents. Run as a
normal user it indexes your home folder.

Options:
  --install-service    register and start the daemon (with sudo)
  --uninstall-service  stop and remove it
  --config FILE        configuration file (default
                       /Library/Application Support/Trawlr/config.toml)
  --root DIR           index DIR instead of the configured roots (repeatable)
  --socket PATH        socket to listen on
  --db PATH            where the index is saved
  --no-watch           index once and do not follow changes
  --user               use per-user paths even when running as root
  --debug              verbose logging
  -h, --help           show this help
  -V, --version        show the version
";

#[cfg(target_os = "linux")]
const HELP: &str = "\
trawlrd - keeps a live index of every file name and answers searches

Usage: trawlrd [options]

Run as root (normally as the systemd service) it indexes the whole machine and
watches entire filesystems. Run as a normal user it indexes your home folder
and watches each folder individually.

Options:
  --config FILE   configuration file
                  (default /etc/trawlrd/config.toml, or
                  ~/.config/trawlrd/config.toml as a normal user)
  --root DIR      index DIR instead of the configured roots (repeatable)
  --socket PATH   socket to listen on
  --db PATH       where the index is saved
  --no-watch      index once and do not follow changes
  --user          use per-user paths even when running as root
  --debug         verbose logging
  -h, --help      show this help
  -V, --version   show the version
";

#[derive(PartialEq)]
enum Mode {
    Run,
    #[cfg(any(windows, target_os = "macos"))]
    Service,
    #[cfg(any(windows, target_os = "macos"))]
    Install,
    #[cfg(any(windows, target_os = "macos"))]
    Uninstall,
}

fn parse_args() -> Result<(Overrides, Mode), String> {
    let mut ov = Overrides::default();
    #[cfg_attr(target_os = "linux", allow(unused_mut))]
    let mut mode = Mode::Run;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            #[cfg(any(windows, target_os = "macos"))]
            "--service" => mode = Mode::Service,
            #[cfg(any(windows, target_os = "macos"))]
            "--install-service" => mode = Mode::Install,
            #[cfg(any(windows, target_os = "macos"))]
            "--uninstall-service" => mode = Mode::Uninstall,
            "--config" => ov.config = Some(PathBuf::from(value("--config")?)),
            "--socket" => ov.socket = Some(PathBuf::from(value("--socket")?)),
            "--db" => ov.db = Some(PathBuf::from(value("--db")?)),
            "--root" => ov.roots.push(value("--root")?),
            "--no-watch" => ov.no_watch = true,
            "--user" => ov.user_mode = true,
            "--debug" => util::DEBUG.store(true, Ordering::Relaxed),
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("trawlrd {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other:?} (try --help)")),
        }
    }
    Ok((ov, mode))
}

#[cfg(windows)]
fn main() {
    use trawlr::win::service;
    let (ov, mode) = match parse_args() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("trawlrd: {e}");
            std::process::exit(2);
        }
    };
    let done = |r: Result<(), String>, what: &str| match r {
        Ok(()) => {
            println!("trawlrd: {what}");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("trawlrd: {e}");
            std::process::exit(1)
        }
    };
    match mode {
        Mode::Install => done(service::install(), "the service is installed and running"),
        Mode::Uninstall => done(service::uninstall(), "the service is removed"),
        Mode::Service => std::process::exit(service::run_service(ov)),
        Mode::Run => std::process::exit(service::run_console(ov)),
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use trawlr::mac::service;
    let (ov, mode) = match parse_args() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("trawlrd: {e}");
            std::process::exit(2);
        }
    };
    let done = |r: Result<(), String>, what: &str| match r {
        Ok(()) => {
            println!("trawlrd: {what}");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("trawlrd: {e}");
            std::process::exit(1)
        }
    };
    match mode {
        Mode::Install => done(service::install(), "the daemon is installed and running"),
        Mode::Uninstall => done(service::uninstall(), "the daemon is removed"),
        Mode::Service => std::process::exit(service::run(ov, true)),
        Mode::Run => std::process::exit(service::run(ov, false)),
    }
}

#[cfg(target_os = "linux")]
fn raise_fd_limit() {
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 && lim.rlim_cur < lim.rlim_max {
            lim.rlim_cur = lim.rlim_max.min(65536);
            libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
        }
    }
}

#[cfg(target_os = "linux")]
fn main() {
    let settings = match parse_args().and_then(|(ov, _)| Settings::load(ov)) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("trawlrd: {e}");
            std::process::exit(2);
        }
    };
    raise_fd_limit();

    // Signals are handled by waiting for them on the main thread, so block
    // them before any other thread exists and inherits the mask.
    let mut sigs: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut sigs);
        libc::sigaddset(&mut sigs, libc::SIGINT);
        libc::sigaddset(&mut sigs, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &sigs, std::ptr::null_mut());
    }

    let watcher = if settings.watch {
        match Watcher::new() {
            Ok(w) => {
                if !w.privileged {
                    info!("running without CAP_SYS_ADMIN: watching each folder individually instead of whole filesystems");
                }
                Some(Arc::new(w))
            }
            Err(e) => {
                warn!("live updates are unavailable: {e}");
                None
            }
        }
    } else {
        None
    };

    let fs = Arc::new(FsRegistry::new(settings.clone(), watcher.clone()));
    let scan = Arc::new(ScanCtx::new(settings.clone(), fs.clone(), watcher.clone()));
    let (tx, rx) = channel::<Msg>();
    let sh = Arc::new(Shared {
        settings: settings.clone(),
        index: RwLock::new(Index::new()),
        fs,
        watcher: watcher.clone(),
        scan,
        state: AtomicU8::new(ST_STARTING),
        sessions: Mutex::new(Vec::new()),
        tx: tx.clone(),
        started: Instant::now(),
        events: AtomicU64::new(0),
    });

    let listener = match server::bind(&sh) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("trawlrd: {}: {e}", settings.socket.display());
            std::process::exit(1);
        }
    };

    if let Some(w) = &watcher {
        indexer::spawn_watch_reader(w.clone(), tx.clone());
        indexer::spawn_mount_monitor(tx.clone());
    }
    let indexer_thread = {
        let sh = sh.clone();
        std::thread::Builder::new()
            .name("indexer".into())
            .spawn(move || {
                // The index has a single writer. If it dies, exit and let the
                // service manager restart us rather than serve a stale index.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| indexer::run(sh, rx))).is_err() {
                    eprintln!("<3>trawlrd: the indexer crashed; exiting");
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
    sh.scan.cancel.store(true, Ordering::Relaxed);
    if let Some(w) = &watcher {
        w.stop();
    }
    let _ = tx.send(Msg::Shutdown);
    let _ = indexer_thread.join();
    let _ = std::fs::remove_file(&settings.socket);
}
