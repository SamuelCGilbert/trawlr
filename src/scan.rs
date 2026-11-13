//! Reading directories off the disk.
//!
//! The scanner knows nothing about the index. It turns a directory into a
//! [`Listing`] (names plus size, time and type for every child) and, for a
//! tree, sends one listing per directory down a channel, a parent always
//! before its children. The indexer is the only thing that applies them.

use std::cell::RefCell;
use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

use crate::config::Settings;
use crate::mounts::{FsRegistry, WatchMode};
use crate::sys::{self, AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW};
use crate::util::{clamp_mtime, cstr, lower_into};
use crate::watch::Watcher;

pub const KIND_FILE: u8 = 0;
pub const KIND_DIR: u8 = 1;
pub const KIND_SYMLINK: u8 = 2;

pub struct RawChild {
    /// Byte range of the name in `Listing::names`, and of its folded form in
    /// `Listing::lowers`.
    pub name: (u32, u32),
    pub lower: (u32, u32),
    pub kind: u8,
    pub size: u64,
    pub mtime: u32,
    // The rest is for directories.
    pub fs: u16,
    /// Byte range of the file handle in `Listing::handles`; empty if the
    /// filesystem has none.
    pub handle: (u32, u32),
    /// Non-zero when the scanner is descending into this directory and a
    /// listing carrying this token will follow.
    pub token: u64,
    /// A mount point we show but do not enter.
    pub noscan: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Ok,
    /// The directory vanished or is no longer the one that was expected.
    Gone,
    /// It exists but cannot be read.
    Denied,
    /// Its modification time equals the one the index already holds.
    Unchanged,
}

pub struct Listing {
    pub token: u64,
    pub status: Status,
    pub mtime_ns: i64,
    pub mtime: u32,
    pub names: Vec<u8>,
    pub lowers: Vec<u8>,
    pub handles: Vec<u8>,
    pub children: Vec<RawChild>,
}

impl Listing {
    pub fn name(&self, c: &RawChild) -> &[u8] {
        &self.names[c.name.0 as usize..c.name.1 as usize]
    }
    pub fn lower(&self, c: &RawChild) -> &[u8] {
        &self.lowers[c.lower.0 as usize..c.lower.1 as usize]
    }
    pub fn handle(&self, c: &RawChild) -> &[u8] {
        &self.handles[c.handle.0 as usize..c.handle.1 as usize]
    }
}

/// How to make sure the directory we opened by path is the one we meant,
/// since the tree can change between learning a path and opening it.
pub enum Verify {
    None,
    InoDev(u64, u64),
    Handle(Vec<u8>),
}

pub struct DirTask {
    /// Index-form path: the root `/` is empty.
    pub path: Vec<u8>,
    pub fs: u16,
    /// Mount the directory is on, or -1 if not known yet.
    pub mount_id: i32,
    pub verify: Verify,
    pub token: u64,
}

pub struct ScanCtx {
    pub settings: Arc<Settings>,
    pub fs: Arc<FsRegistry>,
    pub watcher: Option<Arc<Watcher>>,
    tokens: AtomicU64,
    pub cancel: AtomicBool,
    pub dirs_seen: AtomicU64,
    pub files_seen: AtomicU64,
}

impl ScanCtx {
    pub fn new(settings: Arc<Settings>, fs: Arc<FsRegistry>, watcher: Option<Arc<Watcher>>) -> ScanCtx {
        ScanCtx { settings, fs, watcher, tokens: AtomicU64::new(1), cancel: AtomicBool::new(false), dirs_seen: AtomicU64::new(0), files_seen: AtomicU64::new(0) }
    }
    pub fn next_token(&self) -> u64 {
        self.tokens.fetch_add(1, Ordering::Relaxed)
    }
}

pub struct ChildProbe {
    pub fs: u16,
    pub handle: Vec<u8>,
    pub mount_id: i32,
    /// False for a mount point that is listed but not entered.
    pub scan: bool,
}

/// Identify a child directory: its handle, and whether it starts another
/// filesystem that should (or should not) be indexed.
#[allow(clippy::too_many_arguments)]
pub fn probe_child_dir(
    ctx: &ScanCtx,
    dirfd: RawFd,
    name: &CStr,
    parent_fs: u16,
    parent_mount: i32,
    parent_dev: u64,
    child_dev: u64,
    child_path: &[u8],
) -> io::Result<ChildProbe> {
    let mut handle = Vec::with_capacity(16);
    let mount_id = match sys::name_to_handle(dirfd, name, 0, &mut handle) {
        Ok(m) => m,
        Err(e) => match e.raw_os_error() {
            // No file handles on this filesystem: it can be indexed, though
            // not watched, and device numbers alone mark its boundaries.
            Some(libc::EOPNOTSUPP) | Some(libc::EOVERFLOW) | Some(libc::EINVAL) => {
                handle.clear();
                -1
            }
            _ => return Err(e),
        },
    };
    let crosses = child_dev != parent_dev || (mount_id >= 0 && parent_mount >= 0 && mount_id != parent_mount);
    if !crosses {
        return Ok(ChildProbe { fs: parent_fs, handle, mount_id: if mount_id >= 0 { mount_id } else { parent_mount }, scan: true });
    }
    Ok(match ctx.fs.resolve_mount(child_path, child_dev, mount_id) {
        Some(fs) => ChildProbe { fs, handle, mount_id, scan: true },
        None => ChildProbe { fs: parent_fs, handle: Vec::new(), mount_id, scan: false },
    })
}

thread_local! {
    static DENTS: RefCell<Vec<u8>> = RefCell::new(vec![0u8; 64 * 1024]);
}

fn empty(token: u64, status: Status) -> (Listing, Vec<DirTask>) {
    (Listing { token, status, mtime_ns: 0, mtime: 0, names: Vec::new(), lowers: Vec::new(), handles: Vec::new(), children: Vec::new() }, Vec::new())
}

/// Read one directory.
///
/// With `recurse`, every enterable child directory gets a token and a
/// [`DirTask`]; the tasks come back in the same order as the token-bearing
/// children. With `unchanged_if = Some(mtime)`, a directory whose mtime still
/// equals that value is not read at all and reports [`Status::Unchanged`].
pub fn list_dir(ctx: &ScanCtx, task: &DirTask, recurse: bool, unchanged_if: Option<i64>) -> (Listing, Vec<DirTask>) {
    let cpath = cstr(if task.path.is_empty() { b"/" } else { &task.path });
    let fd = match sys::open_dir(&cpath, false) {
        Ok(fd) => fd,
        Err(e) => {
            let gone = matches!(e.raw_os_error(), Some(libc::ENOENT) | Some(libc::ENOTDIR) | Some(libc::ELOOP));
            return empty(task.token, if gone { Status::Gone } else { Status::Denied });
        }
    };
    let raw = fd.as_raw_fd();
    let Ok(st) = sys::fstat(raw) else { return empty(task.token, Status::Gone) };
    let mut mount_id = task.mount_id;
    match &task.verify {
        Verify::None => {}
        Verify::InoDev(ino, dev) => {
            if st.ino != *ino || st.dev != *dev {
                return empty(task.token, Status::Gone);
            }
        }
        Verify::Handle(want) => {
            let mut cur = Vec::with_capacity(want.len());
            match sys::name_to_handle(raw, c"", AT_EMPTY_PATH, &mut cur) {
                Ok(m) if cur == *want => mount_id = m,
                _ => return empty(task.token, Status::Gone),
            }
        }
    }
    // Mark before reading: anything that changes from here on is reported,
    // and anything that changed before is in the listing.
    if ctx.fs.watch_mode(task.fs) == WatchMode::PerDir {
        if let Some(w) = &ctx.watcher {
            w.mark_dir(raw);
        }
    }
    let mtime_ns = st.mtime_ns();
    let (mut l, mut subs) = empty(task.token, Status::Ok);
    l.mtime_ns = mtime_ns;
    l.mtime = clamp_mtime(st.mtime_s);
    if unchanged_if == Some(mtime_ns) {
        l.status = Status::Unchanged;
        return (l, subs);
    }

    let mut child_path = Vec::with_capacity(task.path.len() + 64);
    child_path.extend_from_slice(&task.path);
    child_path.push(b'/');
    let base = child_path.len();
    let mut files = 0u64;

    let ok = DENTS.with(|cell| {
        let mut buf = cell.borrow_mut();
        loop {
            let n = match sys::getdents(raw, &mut buf) {
                Ok(0) => return true,
                Ok(n) => n,
                Err(_) => return false,
            };
            let mut o = 0;
            // struct linux_dirent64 { u64 ino; i64 off; u16 reclen; u8 type; char name[]; }
            while o + 19 <= n {
                let reclen = u16::from_ne_bytes([buf[o + 16], buf[o + 17]]) as usize;
                if reclen < 20 || o + reclen > n {
                    break;
                }
                let rec = &buf[o + 19..o + reclen];
                o += reclen;
                let nlen = memchr::memchr(0, rec).unwrap_or(rec.len());
                let name = &rec[..nlen];
                if name == b"." || name == b".." || nlen == rec.len() {
                    continue;
                }
                child_path.truncate(base);
                child_path.extend_from_slice(name);
                if ctx.settings.is_excluded(&child_path, name) {
                    continue;
                }
                let cname = unsafe { CStr::from_bytes_with_nul_unchecked(&rec[..=nlen]) };
                let Ok(cst) = sys::statx_at(raw, cname, AT_SYMLINK_NOFOLLOW) else { continue };

                let mut child = RawChild {
                    name: (0, 0),
                    lower: (0, 0),
                    kind: KIND_FILE,
                    size: cst.size,
                    mtime: clamp_mtime(cst.mtime_s),
                    fs: task.fs,
                    handle: (0, 0),
                    token: 0,
                    noscan: false,
                };
                if cst.is_dir() {
                    let Ok(p) = probe_child_dir(ctx, raw, cname, task.fs, mount_id, st.dev, cst.dev, &child_path) else { continue };
                    child.kind = KIND_DIR;
                    child.size = 0;
                    child.fs = p.fs;
                    child.noscan = !p.scan;
                    let h0 = l.handles.len() as u32;
                    l.handles.extend_from_slice(&p.handle);
                    child.handle = (h0, l.handles.len() as u32);
                    if recurse && p.scan {
                        child.token = ctx.next_token();
                        subs.push(DirTask { path: child_path.clone(), fs: p.fs, mount_id: p.mount_id, verify: Verify::InoDev(cst.ino, cst.dev), token: child.token });
                    }
                } else {
                    if cst.is_symlink() {
                        child.kind = KIND_SYMLINK;
                    }
                    files += 1;
                }
                let n0 = l.names.len() as u32;
                l.names.extend_from_slice(name);
                child.name = (n0, l.names.len() as u32);
                let l0 = l.lowers.len() as u32;
                lower_into(name, &mut l.lowers);
                child.lower = (l0, l.lowers.len() as u32);
                l.children.push(child);
            }
        }
    });
    if !ok && l.children.is_empty() {
        return empty(task.token, Status::Denied);
    }
    ctx.dirs_seen.fetch_add(1, Ordering::Relaxed);
    ctx.files_seen.fetch_add(files, Ordering::Relaxed);
    (l, subs)
}

fn walk<'s>(ctx: &'s ScanCtx, task: DirTask, tx: SyncSender<Listing>, scope: &rayon::Scope<'s>) {
    if ctx.cancel.load(Ordering::Relaxed) {
        return;
    }
    let (listing, subs) = list_dir(ctx, &task, true, None);
    drop(task);
    // The parent's listing is sent before any child task exists, so the
    // receiver always learns about a directory before it sees its contents.
    if tx.send(listing).is_err() {
        return;
    }
    for t in subs {
        let tx = tx.clone();
        scope.spawn(move |s| walk(ctx, t, tx, s));
    }
}

/// Walk the given directories and everything beneath them in parallel,
/// sending a listing for each. Call from inside the scan thread pool.
pub fn scan_tree(ctx: &ScanCtx, tasks: Vec<DirTask>, tx: SyncSender<Listing>) {
    rayon::scope(|s| {
        for t in tasks {
            let tx = tx.clone();
            s.spawn(move |s| walk(ctx, t, tx, s));
        }
    });
}
