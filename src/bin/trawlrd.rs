//! The indexing daemon.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Instant;

use trawlr::config::{Overrides, Settings};
use trawlr::index::Index;
use trawlr::indexer::{self, Msg, Shared, ST_STARTING};
use trawlr::mounts::FsRegistry;
use trawlr::scan::ScanCtx;
use trawlr::watch::Watcher;
use trawlr::{info, server, util, warn};
use parking_lot::{Mutex, RwLock};

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

fn parse_args() -> Result<Overrides, String> {
    let mut ov = Overrides::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
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
    Ok(ov)
}

fn raise_fd_limit() {
    unsafe {
        let mut lim: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) == 0 && lim.rlim_cur < lim.rlim_max {
            lim.rlim_cur = lim.rlim_max.min(65536);
            libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
        }
    }
}

fn main() {
    let settings = match parse_args().and_then(Settings::load) {
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
