//! The single writer: builds the index, keeps it current from change events,
//! reconciles it with the disk at startup and saves it.
//!
//! Everything that mutates the index happens on one thread, in a strict
//! order: a scan or reconcile runs to completion while change events queue
//! up, and only then are the queued events applied. An event is never taken
//! as a description of what happened. It only names a directory and an entry
//! that may have changed, and the indexer looks at the disk to see what is
//! there now. Replaying an event, or applying events out of order, therefore
//! converges on the same result.

use std::collections::{HashMap, HashSet};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use rayon::prelude::*;

use crate::config::Settings;
use crate::db;
use crate::index::{Index, Touched, F_DIR, F_NOSCAN, F_ROOT, F_SYMLINK, NONE};
use crate::mounts::{read_mounts, FsRegistry};
use crate::scan::{self, DirTask, Listing, ScanCtx, Status, Verify, KIND_DIR, KIND_SYMLINK};
use crate::server::{self, SessionShared};
use crate::sys::{self, AT_EMPTY_PATH, AT_SYMLINK_NOFOLLOW};
use crate::util::{clamp_mtime, cstr, lossy, lower_into, now_ns};
use crate::watch::{Change, Watcher};
use crate::{debug, info, warn};

pub const ST_STARTING: u8 = 0;
pub const ST_SCANNING: u8 = 1;
pub const ST_RECONCILING: u8 = 2;
pub const ST_READY: u8 = 3;

/// Change events read from the kernel but not yet applied. The kernel queue
/// is unlimited, so if the indexer falls behind (a disk being rewritten while
/// searches keep it busy) this is where they would pile up without end.
/// Beyond a limit they are dropped and the index is checked against the
/// disk instead, as when the kernel's own queue overflows.
static BACKLOG: AtomicUsize = AtomicUsize::new(0);
const MAX_BACKLOG: usize = 100_000;
/// An Overflow is queued and not yet being handled.
static OVERFLOW_QUEUED: AtomicBool = AtomicBool::new(false);

pub enum Msg {
    Changes(Vec<Change>),
    /// The kernel dropped events; the index may have missed changes.
    Overflow,
    Rescan { deep: bool, rebuild: bool },
    /// The mount table changed.
    Mounts,
    Shutdown,
}

pub struct Shared {
    pub settings: Arc<Settings>,
    pub index: RwLock<Index>,
    pub fs: Arc<FsRegistry>,
    pub watcher: Option<Arc<Watcher>>,
    pub scan: Arc<ScanCtx>,
    pub state: AtomicU8,
    pub sessions: Mutex<Vec<Arc<SessionShared>>>,
    pub tx: Sender<Msg>,
    pub started: Instant,
    pub events: AtomicU64,
}

impl Shared {
    pub fn state_name(&self) -> &'static str {
        match self.state.load(Ordering::Relaxed) {
            ST_STARTING => "starting",
            ST_SCANNING => "scanning",
            ST_RECONCILING => "checking",
            _ => "ready",
        }
    }
}

/// The Linux part of the `status` reply: filesystems and how each is watched.
pub fn status_details(sh: &Shared, v: &mut serde_json::Map<String, serde_json::Value>) {
    use crate::mounts::WatchMode;
    use serde_json::json;
    let filesystems: Vec<_> = sh
        .fs
        .snapshot()
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
    v.insert("roots".into(), json!(sh.settings.roots.iter().map(|r| lossy(if r.is_empty() { b"/" } else { r })).collect::<Vec<_>>()));
    v.insert("filesystems".into(), json!(filesystems));
    v.insert("scanned".into(), json!(sh.scan.dirs_seen.load(Ordering::Relaxed) + sh.scan.files_seen.load(Ordering::Relaxed)));
    v.insert("folder_watches".into(), json!(marks));
    v.insert("unwatched_folders".into(), json!(failed));
}

/// An open directory an event batch is syncing entries of. Once opened and
/// checked against the indexed handle, the descriptor keeps referring to that
/// directory even if it is renamed meanwhile.
struct DirFd {
    fd: OwnedFd,
    mount_id: i32,
    dev: u64,
    handle: Vec<u8>,
}

/// Directories a scan could not open at the path the index has for them.
type Gone = Vec<(u16, Vec<u8>)>;

struct Indexer {
    sh: Arc<Shared>,
    rx: Receiver<Msg>,
    pool: rayon::ThreadPool,
    /// Directories whose indexed path did not lead to them when an event
    /// arrived (an ancestor was just renamed and that event is still queued).
    /// They are re-read once the index has caught up.
    stale: HashMap<(u16, Vec<u8>), Instant>,
    unsaved: bool,
    last_save: Instant,
    last_change: Instant,
    shutdown: bool,
}

fn merge_perm(sh: &Shared) {
    let (perm, upto) = sh.index.read().merged_perm();
    sh.index.write().install_perm(perm, upto);
}

/// Swap in a different index. Entry ids of the old one mean nothing in the
/// new one, so its epoch continues from the old epoch: every result set a
/// session still holds is thereby marked void.
fn replace_index(sh: &Shared, mut new: Index) {
    let mut idx = sh.index.write();
    new.epoch = idx.epoch + 1;
    *idx = new;
}

/// Apply the listing of a directory that is new to the index: every child is
/// simply added.
fn apply_fresh(sh: &Shared, pending: &mut HashMap<u64, u32>, l: Listing, touched: &mut Touched, gone: &mut Gone) {
    let Some(dir_id) = pending.remove(&l.token) else { return };
    let mut idx = sh.index.write();
    if idx.is_dead(dir_id) || !idx.is_dir(dir_id) {
        return;
    }
    match l.status {
        Status::Gone => {
            // Not found where the index says it is. Either it was deleted,
            // and the event for that will remove it, or it (or a folder above
            // it) was renamed while we were scanning, and the event for that
            // will move it. Keep the entry and read it once events caught up.
            let slot = idx.dir_slot(dir_id);
            let handle = idx.dir_handle(slot);
            if !handle.is_empty() {
                gone.push((idx.dir(slot).fs, handle.to_vec()));
            } else if idx.flags(dir_id) & F_ROOT == 0 {
                idx.remove_subtree(dir_id, touched);
            }
        }
        Status::Denied | Status::Unchanged => {}
        Status::Ok => {
            let slot = idx.dir_slot(dir_id);
            for c in &l.children {
                let (name, lower) = (l.name(c), l.lower(c));
                let id = if c.kind == KIND_DIR {
                    let handle = l.handle(c);
                    // A handle belongs to one directory. If it is already
                    // indexed elsewhere, that copy is where the directory
                    // used to be; what the disk shows now wins.
                    if let Some(old) = idx.find_dir_by_handle(c.fs, handle) {
                        let old = idx.dir(old).entry;
                        if idx.flags(old) & F_ROOT != 0 {
                            continue;
                        }
                        idx.remove_subtree(old, touched);
                        if idx.is_dead(dir_id) {
                            return;
                        }
                    }
                    let id = idx.add_dir(dir_id, name, lower, if c.noscan { F_NOSCAN } else { 0 }, c.mtime, c.fs, handle, false);
                    if c.token != 0 {
                        pending.insert(c.token, id);
                    }
                    id
                } else {
                    idx.add_file(dir_id, name, lower, if c.kind == KIND_SYMLINK { F_SYMLINK } else { 0 }, c.size, c.mtime, false)
                };
                touched.push(id);
            }
            idx.finish_children(dir_id);
            idx.set_dir_mtime_ns(slot, l.mtime_ns);
            idx.set_meta(dir_id, 0, l.mtime);
        }
    }
}

/// Bring an already indexed directory in line with a fresh listing of it.
/// Returns the child directories that are new and still need scanning.
fn apply_relist(sh: &Shared, dir_id: u32, l: &Listing, subs: Vec<DirTask>, touched: &mut Touched) -> Vec<(u32, DirTask)> {
    let mut new_scans = Vec::new();
    let mut idx = sh.index.write();
    if dir_id >= idx.len() || idx.is_dead(dir_id) || !idx.is_dir(dir_id) {
        return new_scans;
    }
    let mut by_name: HashMap<Vec<u8>, u32> = idx.children(dir_id).iter().map(|&c| (idx.orig_name(c).to_vec(), c)).collect();
    let mut subs = subs.into_iter();
    let mut added = false;
    for c in &l.children {
        let (name, lower) = (l.name(c), l.lower(c));
        let task = if c.token != 0 { subs.next() } else { None };
        if let Some(cid) = by_name.remove(name) {
            let f = idx.flags(cid);
            if c.kind == KIND_DIR && f & F_DIR != 0 {
                let cs = idx.dir_slot(cid);
                if idx.dir(cs).fs == c.fs && idx.dir_handle(cs) == l.handle(c) && (f & F_NOSCAN != 0) == c.noscan {
                    if idx.set_meta(cid, 0, c.mtime) {
                        touched.push(cid);
                    }
                    continue;
                }
            } else if c.kind != KIND_DIR && f & F_DIR == 0 && (f & F_SYMLINK != 0) == (c.kind == KIND_SYMLINK) {
                if idx.set_meta(cid, c.size, c.mtime) {
                    touched.push(cid);
                }
                continue;
            }
            idx.remove_subtree(cid, touched);
        }
        let id = if c.kind == KIND_DIR {
            let handle = l.handle(c);
            if let Some(old) = idx.find_dir_by_handle(c.fs, handle) {
                let old = idx.dir(old).entry;
                if idx.flags(old) & F_ROOT != 0 {
                    continue;
                }
                // Indexed at another path: it was moved here while we were
                // not watching. Its old contents may be out of date too, so
                // read it afresh rather than carry them over.
                by_name.retain(|_, v| *v != old);
                idx.remove_subtree(old, touched);
                if idx.is_dead(dir_id) {
                    return Vec::new();
                }
            }
            let id = idx.add_dir(dir_id, name, lower, if c.noscan { F_NOSCAN } else { 0 }, c.mtime, c.fs, handle, false);
            if let Some(t) = task {
                new_scans.push((id, t));
            }
            id
        } else {
            idx.add_file(dir_id, name, lower, if c.kind == KIND_SYMLINK { F_SYMLINK } else { 0 }, c.size, c.mtime, false)
        };
        added = true;
        touched.push(id);
    }
    for (_, cid) in by_name {
        idx.remove_subtree(cid, touched);
    }
    if added {
        idx.finish_children(dir_id);
    }
    let slot = idx.dir_slot(dir_id);
    idx.set_dir_mtime_ns(slot, l.mtime_ns);
    idx.set_meta(dir_id, 0, l.mtime);
    new_scans
}

impl Indexer {
    // ---- scanning ----------------------------------------------------

    /// Walk `tasks` in parallel and add everything found. `pending` maps each
    /// task's token to the directory entry its listing belongs to.
    fn run_scan(&self, tasks: Vec<DirTask>, pending: &mut HashMap<u64, u32>, touched: &mut Touched, gone: &mut Gone, bulk: bool) {
        let sh = &*self.sh;
        let ctx = sh.scan.clone();
        let pool = &self.pool;
        let (tx, rx) = sync_channel::<Listing>(4096);
        std::thread::scope(|s| {
            s.spawn(move || pool.install(|| scan::scan_tree(&ctx, tasks, tx)));
            let mut since_merge = 0usize;
            for l in rx {
                since_merge += l.children.len();
                apply_fresh(sh, pending, l, touched, gone);
                // Keep the bulk of a long first scan in name order so that
                // searches made while it runs stay fast.
                if bulk && since_merge > 2_000_000 {
                    merge_perm(sh);
                    since_merge = 0;
                }
            }
        });
    }

    /// Fill in a directory entry that was just added to the index.
    fn scan_into(&mut self, dir_id: u32, mut task: DirTask, touched: &mut Touched) {
        let sh = self.sh.clone();
        task.token = sh.scan.next_token();
        let mut pending = HashMap::new();
        pending.insert(task.token, dir_id);
        let mut gone = Gone::new();
        // Most new directories are empty or small: read the first level here
        // and only start the parallel walk if there is more below it.
        let (l, subs) = scan::list_dir(&sh.scan, &task, true, None);
        apply_fresh(&sh, &mut pending, l, touched, &mut gone);
        if !subs.is_empty() {
            touched.overflow = true;
            self.run_scan(subs, &mut pending, touched, &mut gone, false);
        }
        self.note_gone(gone);
    }

    fn note_gone(&mut self, gone: Gone) {
        for key in gone {
            self.stale.entry(key).or_insert_with(Instant::now);
        }
    }

    fn full_scan(&mut self) {
        let sh = self.sh.clone();
        let t0 = Instant::now();
        sh.state.store(ST_SCANNING, Ordering::Relaxed);
        sh.fs.clear();
        replace_index(&sh, Index::new());
        self.stale.clear();
        sh.scan.dirs_seen.store(0, Ordering::Relaxed);
        sh.scan.files_seen.store(0, Ordering::Relaxed);

        let mut pending = HashMap::new();
        let mut tasks = Vec::new();
        for root in &sh.settings.roots {
            let shown = if root.is_empty() { &b"/"[..] } else { root };
            let cpath = cstr(shown);
            let opened = sys::open_dir(&cpath, true).and_then(|fd| sys::fstat(fd.as_raw_fd()).map(|st| (fd, st)));
            let Ok((fd, st)) = opened else {
                warn!("cannot open root {}", lossy(shown));
                continue;
            };
            let mut handle = Vec::new();
            let mount_id = sys::name_to_handle(fd.as_raw_fd(), c"", AT_EMPTY_PATH, &mut handle).unwrap_or_else(|_| {
                handle.clear();
                -1
            });
            let Some(fs) = sh.fs.register_root(root, st.dev, mount_id) else { continue };
            let id = sh.index.write().add_root(shown, fs, &handle);
            let token = sh.scan.next_token();
            pending.insert(token, id);
            tasks.push(DirTask { path: root.clone(), fs, mount_id, verify: Verify::InoDev(st.ino, st.dev), token });
        }
        let mut touched = Touched::default();
        let mut gone = Gone::new();
        self.run_scan(tasks, &mut pending, &mut touched, &mut gone, true);
        self.note_gone(gone);
        merge_perm(&sh);
        {
            let idx = sh.index.read();
            info!(
                "indexed {} files and {} folders in {:.1}s ({} MB in memory)",
                idx.live_count() - idx.dir_count(),
                idx.dir_count(),
                t0.elapsed().as_secs_f64(),
                idx.mem_bytes() >> 20
            );
        }
        self.report_watch_limits();
        sh.state.store(ST_READY, Ordering::Relaxed);
        server::notify_all(&sh);
        self.save(false);
    }

    fn report_watch_limits(&self) {
        if let Some(w) = &self.sh.watcher {
            let failed = w.mark_failures.load(Ordering::Relaxed);
            if failed > 0 {
                warn!(
                    "{failed} folders could not be watched (limit fs.fanotify.max_user_marks reached); changes inside them will be missed until a rescan. \
                     Run trawlrd as a system service to watch whole filesystems instead."
                );
            }
        }
    }

    /// Compare the index with the disk and repair differences.
    ///
    /// Every indexed directory is opened and its modification time compared
    /// with the one recorded when its listing was last in sync. Adding,
    /// removing or renaming an entry changes that time, so a directory whose
    /// time is unchanged is not read. With `deep`, every directory is read,
    /// which also refreshes sizes and times of files that were only written
    /// to. `racy_after` forces a read of directories modified at or after
    /// that instant, where a later change could hide behind the same
    /// timestamp.
    fn reconcile(&mut self, deep: bool, racy_after: i64) {
        let sh = self.sh.clone();
        let t0 = Instant::now();
        sh.state.store(ST_RECONCILING, Ordering::Relaxed);
        let ctx = sh.scan.clone();
        let total = sh.index.read().dir_slots();
        let mut touched = Touched::default();
        let (mut read_dirs, mut slot) = (0usize, 0usize);
        while slot < total && !sh.scan.cancel.load(Ordering::Relaxed) {
            let end = (slot + 16384).min(total);
            let jobs: Vec<(DirTask, i64)> = {
                let idx = sh.index.read();
                (slot..end.min(idx.dir_slots()))
                    .filter_map(|s| {
                        let d = idx.dir(s);
                        if d.entry == NONE || idx.flags(d.entry) & F_NOSCAN != 0 {
                            return None;
                        }
                        let h = idx.dir_handle(s);
                        let verify = if h.is_empty() { Verify::None } else { Verify::Handle(h.to_vec()) };
                        Some((DirTask { path: idx.path_of(d.entry), fs: d.fs, mount_id: -1, verify, token: d.entry as u64 }, d.mtime_ns))
                    })
                    .collect()
            };
            slot = end;
            let mut new_scans = Vec::new();
            let (tx, rx) = sync_channel::<(Listing, Vec<DirTask>)>(256);
            let pool = &self.pool;
            let (ctx, jobs) = (&*ctx, &jobs);
            std::thread::scope(|s| {
                s.spawn(move || {
                    pool.install(|| {
                        jobs.par_iter().for_each_with(tx, |tx, (task, stored)| {
                            let skip = if deep || *stored == 0 || *stored >= racy_after { None } else { Some(*stored) };
                            let (l, subs) = scan::list_dir(ctx, task, true, skip);
                            // A directory that vanished is dealt with when its
                            // parent, whose time must have changed, is read.
                            if l.status == Status::Ok {
                                let _ = tx.send((l, subs));
                            }
                        })
                    })
                });
                for (l, subs) in rx {
                    read_dirs += 1;
                    new_scans.extend(apply_relist(&sh, l.token as u32, &l, subs, &mut touched));
                }
            });
            // New folders are scanned only after the batch has drained: the
            // scan needs the same thread pool the batch was running on.
            for (id, task) in new_scans {
                self.scan_into(id, task, &mut touched);
            }
        }
        if !touched.is_empty() {
            self.unsaved = true;
            merge_perm(&sh);
        }
        info!(
            "checked {total} folders against the disk in {:.1}s: {read_dirs} re-read{}",
            t0.elapsed().as_secs_f64(),
            if touched.is_empty() { ", nothing had changed" } else { ", index updated" }
        );
        self.report_watch_limits();
        sh.state.store(ST_READY, Ordering::Relaxed);
        server::notify_all(&sh);
        sys::release_free_memory();
    }

    /// A marker that exists for as long as a daemon is running on this
    /// index. Finding it at startup means the previous run was killed or the
    /// machine lost power, so changes may have gone unrecorded.
    fn running_marker(&self) -> std::path::PathBuf {
        self.sh.settings.db.with_extension("db.running")
    }

    fn startup(&mut self) {
        let sh = self.sh.clone();
        let t0 = Instant::now();
        let marker = self.running_marker();
        let crashed = marker.exists();
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&marker, b"");
        match db::load(&sh.settings.db) {
            Ok((idx, meta)) => {
                if meta.fingerprint != sh.settings.fingerprint() {
                    info!("configuration changed since the index was saved; rebuilding");
                } else if !sh.fs.restore(&meta.fs) {
                    info!("mounted filesystems changed since the index was saved; rebuilding");
                } else {
                    info!("loaded {} entries from {} in {:.1}s", idx.live_count(), sh.settings.db.display(), t0.elapsed().as_secs_f64());
                    replace_index(&sh, idx);
                    // If the daemon was down while the machine stayed up for
                    // a while, or did not shut down cleanly, files may have
                    // been written without our seeing it: re-read everything.
                    let down_ns = now_ns() - meta.saved_at_ns;
                    let deep = crashed || !meta.clean || (meta.boot_id == sys::boot_id() && down_ns > 600 * 1_000_000_000);
                    if crashed {
                        info!("the previous run did not shut down cleanly; checking every folder");
                    }
                    self.reconcile(deep, meta.saved_at_ns - 2_000_000_000);
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => info!("no saved index; scanning"),
            Err(e) => warn!("{}: {e}; rebuilding", sh.settings.db.display()),
        }
        self.full_scan();
    }

    // ---- live changes ------------------------------------------------

    /// Open directory `slot` by its indexed path and confirm it is the
    /// directory the index means.
    fn ensure_fd(&mut self, slot: usize, fds: &mut HashMap<usize, DirFd>) -> bool {
        if let Some(open) = fds.get(&slot) {
            // An earlier event of this batch may have removed the folder that
            // was opened here and given its slot to a new one; the descriptor
            // then points at the wrong folder.
            let idx = self.sh.index.read();
            if idx.dir(slot).entry != NONE && idx.dir_handle(slot) == &open.handle[..] {
                return true;
            }
            drop(idx);
            fds.remove(&slot);
        }
        let (path, handle, fs) = {
            let idx = self.sh.index.read();
            let d = idx.dir(slot);
            if d.entry == NONE || d.h_len == 0 {
                return false;
            }
            (idx.path_of(d.entry), idx.dir_handle(slot).to_vec(), d.fs)
        };
        let cpath = cstr(if path.is_empty() { b"/" } else { &path });
        let opened = sys::open_dir(&cpath, true).ok().and_then(|fd| {
            let mut h = Vec::with_capacity(handle.len());
            let mount_id = sys::name_to_handle(fd.as_raw_fd(), c"", AT_EMPTY_PATH, &mut h).ok()?;
            if h != handle {
                return None;
            }
            let st = sys::fstat(fd.as_raw_fd()).ok()?;
            Some((fd, mount_id, st.dev))
        });
        match opened {
            Some((fd, mount_id, dev)) => {
                fds.insert(slot, DirFd { fd, mount_id, dev, handle });
                true
            }
            None => {
                self.stale.entry((fs, handle)).or_insert_with(Instant::now);
                false
            }
        }
    }

    /// Record the current modification time of every directory this batch
    /// touched, so that startup knows their listings are in sync.
    fn finish_fds(&mut self, fds: &mut HashMap<usize, DirFd>, touched: &mut Touched) {
        if fds.is_empty() {
            return;
        }
        let stats: Vec<(usize, &DirFd, sys::Stat)> = fds.iter().filter_map(|(&s, d)| sys::fstat(d.fd.as_raw_fd()).ok().map(|st| (s, d, st))).collect();
        // With events lost and the check against the disk still to come, a
        // folder's time now also covers changes nobody applied: recording it
        // as in sync would make that check pass over the folder.
        let lost = OVERFLOW_QUEUED.load(Ordering::SeqCst);
        let mut idx = self.sh.index.write();
        for (slot, d, st) in stats {
            if slot >= idx.dir_slots() || idx.dir(slot).entry == NONE || idx.dir_handle(slot) != &d.handle[..] {
                continue;
            }
            if !lost {
                idx.set_dir_mtime_ns(slot, st.mtime_ns());
            }
            let entry = idx.dir(slot).entry;
            if idx.set_meta(entry, 0, clamp_mtime(st.mtime_s)) {
                touched.push(entry);
            }
        }
        drop(idx);
        fds.clear();
    }

    /// Make the index agree with the disk about `name` in directory `slot`.
    fn sync_child(&mut self, slot: usize, name: &[u8], fds: &mut HashMap<usize, DirFd>, touched: &mut Touched) {
        if name.is_empty() || name.contains(&b'/') || !self.ensure_fd(slot, fds) {
            return;
        }
        let sh = self.sh.clone();
        let dfd = &fds[&slot];
        let raw = dfd.fd.as_raw_fd();
        let mut lower = Vec::with_capacity(name.len());
        lower_into(name, &mut lower);
        let cname = cstr(name);

        // The path is taken from the index now, not remembered from when the
        // descriptor was opened: the folder may have been moved since, by an
        // earlier event of this very batch.
        let (dir_id, dir_fs, existing, mut child_path) = {
            let idx = sh.index.read();
            let d = idx.dir(slot);
            if d.entry == NONE || idx.flags(d.entry) & F_NOSCAN != 0 {
                return;
            }
            (d.entry, d.fs, idx.child_by_name(d.entry, name, &lower).map(|c| (c, idx.flags(c))), idx.path_of(d.entry))
        };
        child_path.push(b'/');
        child_path.extend_from_slice(name);
        let st = if sh.settings.is_excluded(&child_path, name) { None } else { sys::statx_at(raw, &cname, AT_SYMLINK_NOFOLLOW).ok() };
        let Some(st) = st else {
            if let Some((cid, _)) = existing {
                sh.index.write().remove_subtree(cid, touched);
            }
            return;
        };
        let mtime = clamp_mtime(st.mtime_s);

        if !st.is_dir() {
            let kind = if st.is_symlink() { F_SYMLINK } else { 0 };
            let mut idx = sh.index.write();
            if let Some((cid, f)) = existing {
                if f & F_DIR == 0 && f & F_SYMLINK == kind {
                    if idx.set_meta(cid, st.size, mtime) {
                        touched.push(cid);
                    }
                    return;
                }
                idx.remove_subtree(cid, touched);
            }
            let id = idx.add_file(dir_id, name, &lower, kind, st.size, mtime, true);
            touched.push(id);
            return;
        }

        let Ok(probe) = scan::probe_child_dir(&sh.scan, raw, &cname, dir_fs, dfd.mount_id, dfd.dev, st.dev, &child_path) else { return };
        let new_id = {
            let mut idx = sh.index.write();
            if let Some((cid, f)) = existing {
                if f & F_DIR != 0 {
                    let cs = idx.dir_slot(cid);
                    if idx.dir(cs).fs == probe.fs && idx.dir_handle(cs) == &probe.handle[..] && (f & F_NOSCAN == 0) == probe.scan {
                        if idx.set_meta(cid, 0, mtime) {
                            touched.push(cid);
                        }
                        return;
                    }
                }
                idx.remove_subtree(cid, touched);
            }
            if let Some(s2) = idx.find_dir_by_handle(probe.fs, &probe.handle) {
                // This very directory is indexed under another path: it was
                // moved here. Re-parent it and keep its contents.
                let old = idx.dir(s2).entry;
                if idx.flags(old) & F_ROOT == 0 {
                    let id = idx.move_dir(old, dir_id, name, &lower);
                    idx.set_meta(id, 0, mtime);
                    touched.push(old);
                    touched.push(id);
                    touched.overflow = true;
                }
                return;
            }
            let id = idx.add_dir(dir_id, name, &lower, if probe.scan { 0 } else { F_NOSCAN }, mtime, probe.fs, &probe.handle, true);
            touched.push(id);
            id
        };
        if probe.scan {
            let task = DirTask { path: child_path, fs: probe.fs, mount_id: probe.mount_id, verify: Verify::InoDev(st.ino, st.dev), token: 0 };
            self.scan_into(new_id, task, touched);
        }
    }

    /// Re-read one indexed directory and apply the differences.
    fn relist(&mut self, fs: u16, handle: &[u8], touched: &mut Touched) -> bool {
        let sh = self.sh.clone();
        let job = {
            let idx = sh.index.read();
            idx.find_dir_by_handle(fs, handle).map(|s| {
                let entry = idx.dir(s).entry;
                (entry, DirTask { path: idx.path_of(entry), fs, mount_id: -1, verify: Verify::Handle(handle.to_vec()), token: 0 })
            })
        };
        // No longer indexed: nothing left to repair.
        let Some((entry, task)) = job else { return true };
        let (l, subs) = scan::list_dir(&sh.scan, &task, true, None);
        if l.status != Status::Ok {
            return l.status == Status::Denied;
        }
        for (id, t) in apply_relist(&sh, entry, &l, subs, touched) {
            self.scan_into(id, t, touched);
        }
        true
    }

    fn retry_stale(&mut self, touched: &mut Touched) {
        if self.stale.is_empty() {
            return;
        }
        let keys: Vec<(u16, Vec<u8>)> = self.stale.keys().cloned().collect();
        for key in keys {
            if self.relist(key.0, &key.1, touched) {
                self.stale.remove(&key);
            } else if let Some(since) = self.stale.get(&key) {
                if since.elapsed() > Duration::from_secs(30) {
                    // Still not where the index says, and no event explained
                    // why: it is gone as far as we can tell.
                    debug!("dropping a folder that can no longer be found");
                    self.stale.remove(&key);
                    let mut idx = self.sh.index.write();
                    if let Some(s) = idx.find_dir_by_handle(key.0, &key.1) {
                        let e = idx.dir(s).entry;
                        if idx.flags(e) & F_ROOT == 0 {
                            idx.remove_subtree(e, touched);
                        }
                    }
                }
            }
        }
    }

    fn process_changes(&mut self, batch: Vec<Change>) {
        // Every batch was counted on its way in.
        BACKLOG.fetch_sub(batch.len(), Ordering::Relaxed);
        let sh = self.sh.clone();
        let mut fds: HashMap<usize, DirFd> = HashMap::new();
        let mut touched = Touched::default();
        let mut seen: HashSet<([i32; 2], &[u8], &[u8])> = HashSet::with_capacity(batch.len());
        let fs_count = sh.fs.count() as u16;
        for ch in &batch {
            if !seen.insert((ch.fsid, &ch.handle, &ch.name)) {
                continue;
            }
            let slot = {
                let idx = sh.index.read();
                // The filesystem id narrows the lookup, but some filesystems
                // stamp events with another id than statfs reports, so fall
                // back to the handle alone.
                sh.fs
                    .by_fsid(ch.fsid)
                    .and_then(|fs| idx.find_dir_by_handle(fs, &ch.handle))
                    .or_else(|| (0..fs_count).find_map(|fs| idx.find_dir_by_handle(fs, &ch.handle)))
            };
            // An unknown directory is one we do not index (excluded, or on a
            // part of the filesystem outside the roots).
            let Some(slot) = slot else { continue };
            if fds.len() >= 256 {
                self.finish_fds(&mut fds, &mut touched);
            }
            if &*ch.name == b"." {
                self.ensure_fd(slot, &mut fds);
            } else {
                self.sync_child(slot, &ch.name, &mut fds, &mut touched);
            }
        }
        self.finish_fds(&mut fds, &mut touched);
        self.retry_stale(&mut touched);
        sh.events.fetch_add(batch.len() as u64, Ordering::Relaxed);
        if !touched.is_empty() {
            self.unsaved = true;
            self.last_change = Instant::now();
            server::notify_changed(&sh, &touched);
        }
    }

    /// A filesystem was mounted or unmounted. Mounting generates no change
    /// event, so look at every mount point the index knows about.
    fn handle_mounts(&mut self) {
        let sh = self.sh.clone();
        let mut paths: Vec<Vec<u8>> = read_mounts().into_iter().map(|m| m.mount_point).collect();
        paths.extend(sh.fs.snapshot().into_iter().map(|f| f.mount_point));
        paths.sort();
        paths.dedup();
        let mut fds = HashMap::new();
        let mut touched = Touched::default();
        for p in paths {
            let Some(cut) = memchr::memrchr(b'/', &p) else { continue };
            let (parent, name) = (&p[..cut], &p[cut + 1..]);
            if name.is_empty() {
                continue;
            }
            let slot = {
                let idx = sh.index.read();
                idx.resolve_path(parent).filter(|&d| idx.is_dir(d) && !idx.is_dead(d)).map(|d| idx.dir_slot(d))
            };
            if let Some(slot) = slot {
                self.sync_child(slot, name, &mut fds, &mut touched);
            }
        }
        self.finish_fds(&mut fds, &mut touched);
        if !touched.is_empty() {
            self.unsaved = true;
            self.last_change = Instant::now();
            server::notify_changed(&sh, &touched);
        }
    }

    // ---- housekeeping ------------------------------------------------

    fn save(&mut self, clean: bool) {
        let sh = self.sh.clone();
        let t0 = Instant::now();
        let compacted = {
            let mut idx = sh.index.write();
            let need = idx.needs_compaction();
            if need {
                idx.compact();
            }
            need
        };
        if compacted {
            server::notify_all(&sh);
        }
        if sh.index.read().delta_len() > 0 {
            merge_perm(&sh);
        }
        let meta = db::Meta { saved_at_ns: now_ns(), clean, boot_id: sys::boot_id(), fingerprint: sh.settings.fingerprint(), fs: sh.fs.saved() };
        let idx = sh.index.read();
        match db::save(&sh.settings.db, &idx, &meta) {
            Ok(bytes) => info!("saved index ({} entries, {} MB on disk) in {:.1}s", idx.live_count(), bytes >> 20, t0.elapsed().as_secs_f64()),
            Err(e) => warn!("could not save index to {}: {e}", sh.settings.db.display()),
        }
        self.unsaved = false;
        self.last_save = Instant::now();
        drop(idx);
        sys::release_free_memory();
    }

    fn housekeeping(&mut self) {
        let sh = self.sh.clone();
        let (delta, dead, live) = {
            let idx = sh.index.read();
            (idx.delta_len(), idx.dead_count(), idx.live_count())
        };
        // Fold new entries into the name order once enough pile up, or once
        // things have been quiet for a moment.
        if delta > 4096 || (delta > 0 && self.last_change.elapsed() > Duration::from_secs(3)) {
            merge_perm(&sh);
        }
        // Folders that could not be read when last tried: try again now that
        // the events explaining why have had time to arrive.
        if !self.stale.is_empty() {
            let mut touched = Touched::default();
            self.retry_stale(&mut touched);
            if !touched.is_empty() {
                self.unsaved = true;
                self.last_change = Instant::now();
                server::notify_changed(&sh, &touched);
            }
        }
        if dead > 200_000 && dead > live / 3 {
            sh.index.write().compact();
            server::notify_all(&sh);
        }
        if self.unsaved && self.last_save.elapsed() > Duration::from_secs(sh.settings.save_interval_secs()) {
            self.save(false);
        }
    }

    fn control(&mut self, msg: Msg) {
        match msg {
            Msg::Changes(v) => self.process_changes(v),
            Msg::Overflow => {
                // Cleared before the disk is read: events dropped from now on
                // need another check, and will queue another Overflow.
                OVERFLOW_QUEUED.store(false, Ordering::SeqCst);
                warn!("change queue overflowed; checking the index against the disk");
                self.reconcile(false, i64::MAX);
            }
            Msg::Rescan { rebuild: true, .. } => self.full_scan(),
            Msg::Rescan { deep, .. } => self.reconcile(deep, i64::MAX),
            Msg::Mounts => self.handle_mounts(),
            Msg::Shutdown => self.shutdown = true,
        }
    }

    fn event_loop(&mut self) {
        while !self.shutdown {
            match self.rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Msg::Changes(mut batch)) => {
                    // Let a burst land, then take everything that is queued.
                    std::thread::sleep(Duration::from_millis(15));
                    let mut later = Vec::new();
                    while batch.len() < 200_000 {
                        match self.rx.try_recv() {
                            Ok(Msg::Changes(v)) => batch.extend(v),
                            Ok(Msg::Mounts) if later.iter().any(|m| matches!(m, Msg::Mounts)) => {}
                            Ok(m) => later.push(m),
                            Err(_) => break,
                        }
                    }
                    self.process_changes(batch);
                    for m in later {
                        self.control(m);
                    }
                }
                Ok(m) => self.control(m),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if !self.shutdown {
                self.housekeeping();
            }
        }
        // Apply what is already queued so the saved index is as current as
        // possible, then write it out marked clean.
        let mut rest = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            if let Msg::Changes(v) = m {
                rest.extend(v);
            }
        }
        if !rest.is_empty() {
            self.process_changes(rest);
        }
        self.save(true);
        let _ = std::fs::remove_file(self.running_marker());
    }
}

/// Body of the indexer thread. Returns after an orderly shutdown.
pub fn run(sh: Arc<Shared>, rx: Receiver<Msg>) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(sh.settings.scan_threads())
        .thread_name(|i| format!("scan-{i}"))
        .start_handler(|_| sys::set_low_io_priority())
        .build()
        .expect("scan thread pool");
    let mut ix = Indexer { sh, rx, pool, stale: HashMap::new(), unsaved: false, last_save: Instant::now(), last_change: Instant::now(), shutdown: false };
    ix.startup();
    ix.event_loop();
}

/// Read fanotify events on their own thread so the kernel queue is always
/// being drained, even during a long scan.
pub fn spawn_watch_reader(w: Arc<Watcher>, tx: Sender<Msg>) {
    let _ = std::thread::Builder::new().name("fanotify".into()).spawn(move || {
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let mut out = Vec::new();
            let mut overflow = false;
            match w.read(&mut buf, &mut out, &mut overflow) {
                Ok(true) => {
                    if !out.is_empty() && BACKLOG.load(Ordering::Relaxed) + out.len() > MAX_BACKLOG {
                        out.clear();
                        overflow = true;
                    }
                    if overflow && !OVERFLOW_QUEUED.swap(true, Ordering::SeqCst) && tx.send(Msg::Overflow).is_err() {
                        break;
                    }
                    if !out.is_empty() {
                        BACKLOG.fetch_add(out.len(), Ordering::Relaxed);
                        if tx.send(Msg::Changes(out)).is_err() {
                            break;
                        }
                    }
                }
                Ok(false) => break,
                Err(e) => {
                    warn!("reading change events: {e}");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    });
}

/// Tell the indexer whenever a filesystem is mounted or unmounted. The
/// kernel flags `/proc/self/mountinfo` as readable-with-priority on change.
pub fn spawn_mount_monitor(tx: Sender<Msg>) {
    let _ = std::thread::Builder::new().name("mounts".into()).spawn(move || {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open("/proc/self/mountinfo") else { return };
        let mut sink = Vec::new();
        loop {
            sink.clear();
            let _ = f.seek(SeekFrom::Start(0));
            let _ = f.read_to_end(&mut sink);
            let mut p = libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLPRI, revents: 0 };
            let r = unsafe { libc::poll(&mut p, 1, -1) };
            if r < 0 {
                if sys::errno() == libc::EINTR {
                    continue;
                }
                return;
            }
            // Mount changes come in bursts (a snap, a container): settle first.
            std::thread::sleep(Duration::from_millis(300));
            if tx.send(Msg::Mounts).is_err() {
                return;
            }
        }
    });
}
