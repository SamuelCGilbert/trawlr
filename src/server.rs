//! The Unix-socket protocol clients search through.
//!
//! One JSON object per line in each direction. A client sends `query` and
//! gets the total number of matches plus the first rows; the full result set
//! stays here, and the client pages through it with `rows` as the user
//! scrolls. When a change to the index affects a session's current query, the
//! daemon pushes `{"event":"changed"}` once and the client re-runs the query.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::index::{Index, Touched, F_DIR, F_SYMLINK, NONE};
use crate::indexer::{Msg, Shared};
use crate::mounts::WatchMode;
use crate::query::{self, Compiled, QueryOpts, Sort};
use crate::util::lossy;
use crate::{info, warn};

pub const PROTOCOL: u32 = 1;

pub struct SessionShared {
    query: Mutex<Option<Arc<Compiled>>>,
    /// Set once the client has been told its results are out of date, and
    /// cleared by its next query, so a busy disk sends one nudge, not a flood.
    notified: AtomicBool,
    out: Mutex<UnixStream>,
}

impl SessionShared {
    fn send(&self, value: &serde_json::Value) -> bool {
        let mut line = serde_json::to_vec(value).unwrap_or_default();
        line.push(b'\n');
        self.out.lock().write_all(&line).is_ok()
    }
}

/// Tell the sessions whose current query matches any touched entry.
pub fn notify_changed(sh: &Shared, touched: &Touched) {
    let sessions = sh.sessions.lock().clone();
    if sessions.is_empty() {
        return;
    }
    let mut hit = Vec::new();
    {
        let idx = sh.index.read();
        for s in sessions {
            if s.notified.load(Ordering::Relaxed) {
                continue;
            }
            let Some(q) = s.query.lock().clone() else { continue };
            if touched.overflow || touched.ids.iter().any(|&id| query::matches_entry(&idx, &q, id)) {
                hit.push(s);
            }
        }
    }
    // Written outside the index lock: a client that is slow to read must not
    // hold up searches.
    for s in hit {
        s.notified.store(true, Ordering::Relaxed);
        s.send(&json!({"event": "changed"}));
    }
}

pub fn notify_all(sh: &Shared) {
    notify_changed(sh, &Touched { ids: Vec::new(), overflow: true });
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Request {
    op: String,
    qid: u64,
    q: String,
    sort: String,
    desc: bool,
    case: bool,
    path: bool,
    regex: bool,
    word: bool,
    offset: u64,
    limit: u64,
    deep: bool,
    rebuild: bool,
}

#[derive(Serialize)]
struct Row {
    /// Name.
    n: String,
    /// Folder the entry is in.
    p: String,
    /// Size in bytes (0 for folders).
    s: u64,
    /// Modification time, seconds since the epoch.
    m: u32,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    d: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    l: bool,
    /// Deleted since the query ran.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    gone: bool,
    /// Hex of the exact path bytes, when they are not valid UTF-8 and the
    /// text fields above are therefore approximate.
    #[serde(skip_serializing_if = "Option::is_none")]
    raw: Option<String>,
}

fn rows(idx: &Index, ids: &[u32]) -> Vec<Row> {
    let mut out = Vec::with_capacity(ids.len());
    let mut last_parent = NONE;
    let mut parent_path: Vec<u8> = Vec::new();
    for &id in ids {
        if id >= idx.len() {
            continue;
        }
        let p = idx.parent(id);
        if p != last_parent {
            parent_path = if p == NONE { Vec::new() } else { idx.path_of(p) };
            last_parent = p;
        }
        let name = idx.orig_name(id);
        let f = idx.flags(id);
        let shown: &[u8] = if parent_path.is_empty() && p != NONE { b"/" } else { &parent_path };
        let raw = if std::str::from_utf8(name).is_err() || std::str::from_utf8(&parent_path).is_err() {
            let mut full = parent_path.clone();
            full.push(b'/');
            full.extend_from_slice(name);
            Some(full.iter().map(|b| format!("{b:02x}")).collect())
        } else {
            None
        };
        out.push(Row { n: lossy(name), p: lossy(shown), s: idx.file_size(id), m: idx.mtime(id), d: f & F_DIR != 0, l: f & F_SYMLINK != 0, gone: idx.is_dead(id), raw });
    }
    out
}

fn window(ids: &[u32], offset: u64, limit: u64) -> &[u32] {
    let a = (offset as usize).min(ids.len());
    let b = a.saturating_add(limit.clamp(1, 5000) as usize).min(ids.len());
    &ids[a..b]
}

pub fn status(sh: &Shared) -> serde_json::Value {
    let idx = sh.index.read();
    let fs = sh.fs.snapshot();
    let filesystems: Vec<_> = fs
        .iter()
        .filter(|f| !f.retired)
        .map(|f| {
            json!({
                "mount": lossy(if f.mount_point.is_empty() { b"/" } else { &f.mount_point }),
                "type": f.fstype,
                "watch": match f.watch { WatchMode::Filesystem => "filesystem", WatchMode::PerDir => "per-folder", WatchMode::Off => "off" },
            })
        })
        .collect();
    let (marks, failed) = sh.watcher.as_ref().map(|w| (w.dir_marks.load(Ordering::Relaxed), w.mark_failures.load(Ordering::Relaxed))).unwrap_or((0, 0));
    json!({
        "ok": true,
        "protocol": PROTOCOL,
        "version": env!("CARGO_PKG_VERSION"),
        "state": sh.state_name(),
        "mode": if sh.settings.system { "system" } else { "user" },
        "files": idx.live_count() - idx.dir_count(),
        "folders": idx.dir_count(),
        "memory_mb": idx.mem_bytes() >> 20,
        "roots": sh.settings.roots.iter().map(|r| lossy(if r.is_empty() { b"/" } else { r })).collect::<Vec<_>>(),
        "filesystems": filesystems,
        "scanned": sh.scan.dirs_seen.load(Ordering::Relaxed) + sh.scan.files_seen.load(Ordering::Relaxed),
        "events": sh.events.load(Ordering::Relaxed),
        "folder_watches": marks,
        "unwatched_folders": failed,
        "uptime_secs": sh.started.elapsed().as_secs(),
    })
}

fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    (r == 0).then_some(cred.uid)
}

fn handle(sh: Arc<Shared>, stream: UnixStream) {
    let uid = peer_uid(&stream);
    let me = unsafe { libc::geteuid() };
    let allowed = match uid {
        Some(u) => u == me || sh.settings.may_search(u),
        None => false,
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let Ok(read_half) = stream.try_clone() else { return };
    let session = Arc::new(SessionShared { query: Mutex::new(None), notified: AtomicBool::new(false), out: Mutex::new(stream) });
    if !allowed {
        session.send(&json!({"ok": false, "error": "this user is not allowed to search the system index (see allowed_users and allowed_groups in /etc/trawlrd/config.toml)"}));
        return;
    }
    sh.sessions.lock().push(session.clone());

    let mut results: Vec<u32> = Vec::new();
    let mut epoch = 0u64;
    let mut qid = 0u64;
    let mut reader = BufReader::new(read_half);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader).take(1 << 20).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if !line.ends_with(b"\n") {
            break;
        }
        let req: Request = match serde_json::from_slice(&line) {
            Ok(r) => r,
            Err(e) => {
                if !session.send(&json!({"ok": false, "error": format!("bad request: {e}")})) {
                    break;
                }
                continue;
            }
        };
        let reply = match req.op.as_str() {
            "status" => status(&sh),
            "query" => {
                let t0 = Instant::now();
                let opts = QueryOpts { case: req.case, path: req.path, regex: req.regex, word: req.word };
                let idx = sh.index.read();
                match query::compile(&req.q, opts, &idx) {
                    Err(e) => json!({"qid": req.qid, "error": e}),
                    Ok(compiled) => {
                        let mut ids = query::execute(&idx, &compiled, Sort::parse(&req.sort));
                        if req.desc {
                            ids.reverse();
                        }
                        // Registered while the index is still read-locked: a
                        // change applied after this point is sure to find it.
                        *session.query.lock() = Some(Arc::new(compiled));
                        session.notified.store(false, Ordering::Relaxed);
                        results = ids;
                        epoch = idx.epoch;
                        qid = req.qid;
                        let page = rows(&idx, window(&results, req.offset, req.limit.max(1)));
                        drop(idx);
                        json!({
                            "qid": qid,
                            "total": results.len(),
                            "offset": req.offset,
                            "rows": page,
                            "ms": t0.elapsed().as_secs_f64() * 1000.0,
                            "state": sh.state_name(),
                        })
                    }
                }
            }
            "rows" => {
                let idx = sh.index.read();
                if req.qid != qid || epoch != idx.epoch {
                    json!({"qid": req.qid, "stale": true})
                } else {
                    let page = rows(&idx, window(&results, req.offset, req.limit.max(1)));
                    drop(idx);
                    json!({"qid": qid, "offset": req.offset, "rows": page})
                }
            }
            "rescan" => {
                let _ = sh.tx.send(Msg::Rescan { deep: req.deep, rebuild: req.rebuild });
                json!({"ok": true})
            }
            other => json!({"ok": false, "error": format!("unknown op {other:?}")}),
        };
        if !session.send(&reply) {
            break;
        }
    }
    sh.sessions.lock().retain(|s| !Arc::ptr_eq(s, &session));
}

pub fn bind(sh: &Shared) -> std::io::Result<UnixListener> {
    let path = &sh.settings.socket;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        if !sh.settings.system {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    // A leftover socket file from a previous run is fine to replace, but not
    // one a live daemon is listening on.
    if UnixStream::connect(path).is_ok() {
        return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, format!("another trawlrd is already serving {}", path.display())));
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    // The system socket is open to all; who may actually search is decided
    // per connection from the peer's user id.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(if sh.settings.system { 0o666 } else { 0o600 }))?;
    info!("listening on {}", path.display());
    Ok(listener)
}

pub fn serve(sh: Arc<Shared>, listener: UnixListener) {
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let sh = sh.clone();
                let _ = std::thread::Builder::new().name("client".into()).spawn(move || handle(sh, stream));
            }
            Err(e) => {
                warn!("accept: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}
