//! The single writer on macOS: builds the index by walking the disk and
//! keeps it current from FSEvents.
//!
//! As on the other systems, an event is never taken as a description of
//! what happened. FSEvents merges events and its flags only say roughly
//! what kind of thing happened to a path, so an event just names a folder
//! to look at again: the folder is listed and the index made to agree with
//! the listing. Applying an event twice, or late, comes to the same result.
//! A folder that turns up in a new place is recognised by its file id and
//! moved in the index rather than read again.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

use super::fsevents::{self as fse, Event, Stream};
use super::scan::{self, handle, ListError, Listing, KIND_DIR, KIND_LINK};
use super::volumes::{self, lossy_path, Mount};
use super::SavedFs;
use crate::config::Settings;
use crate::db;
use crate::index::{Index, Touched, F_NOSCAN, F_ROOT, F_SYMLINK, NONE};
use crate::server::{self, SessionShared};
use crate::util::{clamp_mtime, lower_into, now_ns};
use crate::{debug, info, warn};

pub const ST_STARTING: u8 = 0;
pub const ST_SCANNING: u8 = 1;
pub const ST_READY: u8 = 3;

pub enum Msg {
    /// Events from the stream started as generation `gen`.
    Events { gen: u64, events: Vec<Event> },
    Rescan { deep: bool, rebuild: bool },
    Shutdown,
}

pub struct Shared {
    pub settings: Arc<Settings>,
    pub index: RwLock<Index>,
    pub state: AtomicU8,
    pub sessions: Mutex<Vec<Arc<SessionShared>>>,
    pub tx: SyncSender<Msg>,
    pub started: Instant,
    pub events: AtomicU64,
    pub cancel: AtomicBool,
    pub scanned: AtomicU64,
    pub live: AtomicBool,
    pub mounts: RwLock<Vec<Mount>>,
    /// Folders that could not be read, most often because macOS keeps
    /// them private from programs without Full Disk Access.
    pub denied: Mutex<BTreeSet<Vec<u8>>>,
}

impl Shared {
    pub fn new(settings: Arc<Settings>, tx: SyncSender<Msg>) -> Shared {
        Shared {
            settings,
            index: RwLock::new(Index::new()),
            state: AtomicU8::new(ST_STARTING),
            sessions: Mutex::new(Vec::new()),
            tx,
            started: Instant::now(),
            events: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            scanned: AtomicU64::new(0),
            live: AtomicBool::new(false),
            mounts: RwLock::new(Vec::new()),
            denied: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn state_name(&self) -> &'static str {
        match self.state.load(Ordering::Relaxed) {
            ST_STARTING => "starting",
            ST_SCANNING => "scanning",
            _ => "ready",
        }
    }
}

/// The macOS part of the `status` reply.
pub fn status_details(sh: &Shared, v: &mut serde_json::Map<String, serde_json::Value>) {
    use serde_json::json;
    let live = sh.live.load(Ordering::Relaxed);
    v.insert("roots".into(), json!(sh.settings.roots.iter().map(|r| lossy_path(r)).collect::<Vec<_>>()));
    let fs: Vec<_> = sh
        .mounts
        .read()
        .iter()
        .filter(|m| m.indexed(&sh.settings) && sh.settings.roots.iter().any(|r| r.is_empty() || m.path.starts_with(r) || r.starts_with(&m.path)))
        .map(|m| json!({"mount": lossy_path(&m.path), "type": m.fstype, "watch": if live { "FSEvents" } else { "off" }}))
        .collect();
    v.insert("filesystems".into(), json!(fs));
    v.insert("scanned".into(), json!(sh.scanned.load(Ordering::Relaxed)));
    v.insert("folder_watches".into(), json!(0));
    v.insert("unwatched_folders".into(), json!(0));
    let denied = sh.denied.lock();
    v.insert("unreadable_folders".into(), json!(denied.len()));
    v.insert("unreadable".into(), json!(denied.iter().take(20).map(|p| lossy_path(p)).collect::<Vec<_>>()));
    // Folders in someone's home that only Full Disk Access opens up.
    let private = denied.iter().any(|p| p.starts_with(b"/Users/") && p.iter().filter(|&&b| b == b'/').count() >= 3);
    v.insert("needs_full_disk_access".into(), json!(sh.settings.system && private));
}

fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(dir.len() + 1 + name.len());
    p.extend_from_slice(dir);
    p.push(b'/');
    p.extend_from_slice(name);
    p
}

fn parent_of(path: &[u8]) -> Vec<u8> {
    match memchr::memrchr(b'/', path) {
        Some(i) => path[..i].to_vec(),
        None => Vec::new(),
    }
}

fn merge_perm(sh: &Shared) {
    let (perm, upto) = sh.index.read().merged_perm();
    sh.index.write().install_perm(perm, upto);
}

fn replace_index(sh: &Shared, mut new: Index) {
    let mut idx = sh.index.write();
    new.epoch = idx.epoch + 1;
    *idx = new;
}

// ---- walking -----------------------------------------------------------

/// A folder listed by the walk, with what to do about each entry.
struct Walked {
    token: u64,
    path: Vec<u8>,
    result: Result<Listing, ListError>,
    /// Per entry: 0 to leave out, 1 to add, 2 to add as a folder that is not
    /// entered, or the token of the listing that will follow for it.
    plan: Vec<u64>,
}

const SKIP: u64 = 0;
const ADD: u64 = 1;
const NOSCAN: u64 = 2;

struct WalkCtx {
    settings: Arc<Settings>,
    /// Mount points (other than the roots), and whether to enter them.
    mounts: HashMap<Vec<u8>, bool>,
    tokens: AtomicU64,
    sh: Arc<Shared>,
}

impl WalkCtx {
    fn plan(&self, path: &[u8], l: &Listing) -> (Vec<u64>, Vec<(Vec<u8>, u64)>) {
        let mut plan = Vec::with_capacity(l.children.len());
        let mut subs = Vec::new();
        for c in &l.children {
            let name = l.name(c);
            let cp = join(path, name);
            if self.settings.is_excluded(&cp, name) {
                plan.push(SKIP);
                continue;
            }
            if c.kind != KIND_DIR {
                plan.push(ADD);
                continue;
            }
            if self.mounts.get(&cp) == Some(&false) {
                plan.push(NOSCAN);
                continue;
            }
            let t = self.tokens.fetch_add(1, Ordering::Relaxed);
            plan.push(t);
            subs.push((cp, t));
        }
        (plan, subs)
    }
}

fn walk<'s>(ctx: &'s WalkCtx, path: Vec<u8>, token: u64, tx: SyncSender<Walked>, scope: &rayon::Scope<'s>) {
    if ctx.sh.cancel.load(Ordering::Relaxed) {
        return;
    }
    let result = scan::list_dir(&path);
    let (plan, subs) = match &result {
        Ok(l) => ctx.plan(&path, l),
        Err(_) => (Vec::new(), Vec::new()),
    };
    if let Ok(l) = &result {
        ctx.sh.scanned.fetch_add(l.children.len() as u64, Ordering::Relaxed);
    }
    // The folder's listing goes before any of its subfolders' can.
    if tx.send(Walked { token, path, result, plan }).is_err() {
        return;
    }
    for (p, t) in subs {
        let tx = tx.clone();
        scope.spawn(move |s| walk(ctx, p, t, tx, s));
    }
}

/// Add a walked folder's entries under `dir`. Subfolders to be walked are
/// remembered by their token.
fn add_listing(idx: &mut Index, dir: u32, l: &Listing, plan: &[u64], ids: &mut HashMap<u64, u32>, touched: Option<&mut Touched>) -> usize {
    let mut lower = Vec::new();
    let mut added = Vec::new();
    for (c, &what) in l.children.iter().zip(plan) {
        if what == SKIP {
            continue;
        }
        let name = l.name(c);
        lower.clear();
        lower_into(name, &mut lower);
        let id = if c.kind == KIND_DIR {
            let flags = if what == NOSCAN { F_NOSCAN } else { 0 };
            let id = idx.add_dir(dir, name, &lower, flags, clamp_mtime(c.mtime), 0, &handle(c.dev, c.ino), false);
            if what > NOSCAN {
                ids.insert(what, id);
            }
            id
        } else {
            let flags = if c.kind == KIND_LINK { F_SYMLINK } else { 0 };
            idx.add_file(dir, name, &lower, flags, c.size, clamp_mtime(c.mtime), false)
        };
        added.push(id);
    }
    idx.finish_children(dir);
    if let Some(t) = touched {
        for id in &added {
            t.push(*id);
        }
    }
    added.len()
}

struct Indexer {
    sh: Arc<Shared>,
    rx: Receiver<Msg>,
    pool: Arc<rayon::ThreadPool>,
    stream: Option<Stream>,
    gen: u64,
    /// The last event id the index has applied.
    applied: u64,
    /// Folders moved in the index since their contents were last checked.
    moved: Vec<u32>,
    mounts: Vec<Mount>,
    unsaved: bool,
    last_save: Instant,
    last_change: Instant,
    last_mount_check: Instant,
    last_retry: Instant,
    shutdown: bool,
    /// False while the index is missing something a walk was cut short
    /// on; such an index is never saved (see the Windows indexer).
    complete: bool,
}

impl Indexer {
    fn walk_ctx(&self) -> WalkCtx {
        let roots = &self.sh.settings.roots;
        let mounts = self.mounts.iter().filter(|m| !roots.contains(&m.path)).map(|m| (m.path.clone(), m.indexed(&self.sh.settings))).collect();
        WalkCtx { settings: self.sh.settings.clone(), mounts, tokens: AtomicU64::new(NOSCAN + 1), sh: self.sh.clone() }
    }

    /// Walk the given folders (already in the index) and everything beneath
    /// them, adding what is found. Folders are listed on the scan threads
    /// while this thread adds them, a few thousand at a time.
    fn walk_into(&mut self, starts: Vec<(u32, Vec<u8>)>, mut touched: Option<&mut Touched>) -> bool {
        if starts.is_empty() {
            return true;
        }
        let sh = self.sh.clone();
        let ctx = Arc::new(self.walk_ctx());
        let mut ids: HashMap<u64, u32> = HashMap::new();
        let mut tasks = Vec::new();
        {
            // Where each folder is now: it may have moved since it was queued.
            let idx = sh.index.read();
            for (id, _) in starts {
                if idx.is_dead(id) || !idx.is_dir(id) {
                    continue;
                }
                let t = ctx.tokens.fetch_add(1, Ordering::Relaxed);
                ids.insert(t, id);
                tasks.push((idx.path_of(id), t));
            }
        }
        let (tx, rx) = sync_channel::<Walked>(4096);
        let pool = self.pool.clone();
        let wctx = ctx.clone();
        let walker = std::thread::spawn(move || {
            pool.install(|| {
                rayon::scope(|s| {
                    for (p, t) in tasks {
                        let tx = tx.clone();
                        let c = &*wctx;
                        s.spawn(move |s| walk(c, p, t, tx, s));
                    }
                })
            })
        });
        let mut since_merge = 0usize;
        let mut denied = Vec::new();
        while let Ok(first) = rx.recv() {
            let mut idx = sh.index.write();
            let mut batch = 0usize;
            let mut next = Some(first);
            while let Some(w) = next.take() {
                let Some(&dir) = ids.get(&w.token) else { continue };
                ids.remove(&w.token);
                match &w.result {
                    Ok(l) if !idx.is_dead(dir) => {
                        idx.set_meta(dir, 0, clamp_mtime(l.mtime));
                        let n = add_listing(&mut idx, dir, l, &w.plan, &mut ids, touched.as_deref_mut());
                        batch += n + 1;
                        since_merge += n;
                    }
                    Ok(_) => {}
                    Err(ListError::Denied) => denied.push(w.path),
                    Err(ListError::Gone) => {}
                }
                if batch > 20_000 {
                    break;
                }
                next = rx.try_recv().ok();
            }
            drop(idx);
            if since_merge > 2_000_000 {
                merge_perm(&sh);
                since_merge = 0;
            }
            if sh.cancel.load(Ordering::Relaxed) {
                break;
            }
        }
        drop(rx);
        let _ = walker.join();
        if !denied.is_empty() {
            sh.denied.lock().extend(denied);
        }
        !sh.cancel.load(Ordering::Relaxed)
    }

    // ---- full scan ---------------------------------------------------

    fn start_stream(&mut self, since: u64) {
        self.stream = None;
        self.gen += 1;
        if !self.sh.settings.watch {
            return;
        }
        let paths: Vec<Vec<u8>> = self.sh.settings.roots.iter().map(|r| if r.is_empty() { b"/".to_vec() } else { r.clone() }).collect();
        let tx = self.sh.tx.clone();
        let gen = self.gen;
        let sink = Box::new(move |events: Vec<Event>| {
            let _ = tx.send(Msg::Events { gen, events });
        });
        match Stream::start(&paths, since, 0.25, sink) {
            Ok(s) => {
                self.stream = Some(s);
                self.sh.live.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                warn!("live updates are unavailable: {e}");
                self.sh.live.store(false, Ordering::Relaxed);
            }
        }
    }

    fn set_mounts(&mut self, m: Vec<Mount>) {
        *self.sh.mounts.write() = m.clone();
        self.mounts = m;
    }

    fn full_scan(&mut self) {
        let sh = self.sh.clone();
        let t0 = Instant::now();
        sh.state.store(ST_SCANNING, Ordering::Relaxed);
        self.set_mounts(volumes::mounts());
        replace_index(&sh, Index::new());
        sh.scanned.store(0, Ordering::Relaxed);
        sh.denied.lock().clear();
        self.moved.clear();
        // Changes from this moment on are delivered after the walk, so
        // nothing that happens while it runs is missed.
        self.applied = fse::current_id();
        self.start_stream(self.applied);
        let mut starts = Vec::new();
        {
            let mut idx = sh.index.write();
            for r in sh.settings.roots.clone() {
                let st = scan::stat(&r);
                let h = st.map(|(_, _, _, dev, ino)| handle(dev, ino)).unwrap_or_default();
                let id = idx.add_root(if r.is_empty() { b"/" } else { &r }, 0, &h);
                starts.push((id, r));
            }
        }
        self.complete = self.walk_into(starts, None);
        merge_perm(&sh);
        {
            let idx = sh.index.read();
            info!(
                "indexed {} files and {} folders in {:.1}s ({} MB in memory){}",
                idx.live_count() - idx.dir_count(),
                idx.dir_count(),
                t0.elapsed().as_secs_f64(),
                idx.mem_bytes() >> 20,
                match sh.denied.lock().len() {
                    0 => String::new(),
                    n => format!("; {n} folders could not be read"),
                }
            );
        }
        sh.state.store(ST_READY, Ordering::Relaxed);
        server::notify_all(&sh);
        self.save(false);
    }

    // ---- live changes ------------------------------------------------

    /// The index form of an event's path, or None if it is outside what is
    /// indexed. Files on the data volume may be reported by their path there.
    fn normalize(&self, raw: &[u8]) -> Option<Vec<u8>> {
        let mut p: &[u8] = raw;
        if let Some(rest) = p.strip_prefix(b"/System/Volumes/Data") {
            if rest.is_empty() || rest[0] == b'/' {
                p = rest;
            }
        }
        while p.len() > 1 && p.ends_with(b"/") {
            p = &p[..p.len() - 1];
        }
        if p == b"/" {
            p = b"";
        }
        let inside = self.sh.settings.roots.iter().any(|r| p == r.as_slice() || r.is_empty() || (p.starts_with(r) && p.get(r.len()) == Some(&b'/')));
        if !inside {
            return None;
        }
        // Inside a mounted volume that is not indexed.
        for m in &self.mounts {
            if !m.path.is_empty() && p.starts_with(&m.path) && p.get(m.path.len()) == Some(&b'/') && !m.indexed(&self.sh.settings) {
                return None;
            }
        }
        // Inside (or) an excluded folder: nothing there is indexed.
        let mut i = 1;
        while i <= p.len() {
            if i == p.len() || p[i] == b'/' {
                let prefix = &p[..i];
                let name = &prefix[memchr::memrchr(b'/', prefix).map(|j| j + 1).unwrap_or(0)..];
                if !name.is_empty() && self.sh.settings.is_excluded(prefix, name) {
                    return None;
                }
            }
            i += 1;
        }
        Some(p.to_vec())
    }

    fn is_root(&self, p: &[u8]) -> bool {
        self.sh.settings.roots.iter().any(|r| r == p)
    }

    /// The nearest folder at or above `path` that the index has, with its path.
    fn indexed_dir(&self, idx: &Index, path: &[u8]) -> Option<(u32, Vec<u8>)> {
        let mut p = path.to_vec();
        loop {
            if let Some(id) = idx.resolve_path(&p) {
                if idx.is_dir(id) && idx.flags(id) & F_NOSCAN == 0 {
                    return Some((id, p));
                }
            }
            if self.is_root(&p) || p.is_empty() {
                return None;
            }
            p = parent_of(&p);
        }
    }

    fn process(&mut self, events: Vec<Event>) {
        let sh = self.sh.clone();
        sh.events.fetch_add(events.len() as u64, Ordering::Relaxed);
        const CHILDREN: u32 = fse::ITEM_CREATED | fse::ITEM_REMOVED | fse::ITEM_RENAMED | fse::ITEM_CLONED;
        const META: u32 = fse::ITEM_MODIFIED | fse::ITEM_INODE_META_MOD | fse::ITEM_CHANGE_OWNER | fse::ITEM_XATTR_MOD | fse::ITEM_FINDER_INFO_MOD;
        const ITEM: u32 = CHILDREN | META | fse::ITEM_IS_FILE | fse::ITEM_IS_DIR | fse::ITEM_IS_SYMLINK;
        let mut full = false;
        let mut mounts_moved = false;
        let mut rescan: BTreeSet<Vec<u8>> = BTreeSet::new();
        let mut relist: BTreeSet<Vec<u8>> = BTreeSet::new();
        let mut restat: BTreeSet<Vec<u8>> = BTreeSet::new();
        for ev in events {
            if ev.id != 0 && ev.id != fse::SINCE_NOW && ev.id > self.applied {
                self.applied = ev.id;
                self.unsaved = true;
            }
            let f = ev.flags;
            if f & fse::IDS_WRAPPED != 0 {
                full = true;
                continue;
            }
            if f & fse::HISTORY_DONE != 0 {
                debug!("caught up with the events recorded while trawlrd was stopped");
                continue;
            }
            if f & (fse::MOUNT | fse::UNMOUNT) != 0 {
                mounts_moved = true;
            }
            let Some(path) = self.normalize(&ev.path) else { continue };
            debug!("event {:#x} {:?}", f, lossy_path(&path));
            if f & (fse::MUST_SCAN_SUBDIRS | fse::USER_DROPPED | fse::KERNEL_DROPPED | fse::ROOT_CHANGED) != 0 {
                if self.is_root(&path) {
                    full = true;
                } else {
                    rescan.insert(path);
                }
                continue;
            }
            if f & (fse::MOUNT | fse::UNMOUNT) != 0 {
                relist.insert(parent_of(&path));
                continue;
            }
            if f & CHILDREN != 0 && !self.is_root(&path) {
                relist.insert(parent_of(&path));
            }
            if f & META != 0 {
                restat.insert(path.clone());
            }
            if f & ITEM == 0 {
                // A folder-level event: something in it changed.
                relist.insert(path);
            }
        }
        if full {
            warn!("FSEvents asked for a full rescan");
            self.full_scan();
            return;
        }
        if mounts_moved {
            self.check_mounts();
        }
        let mut touched = Touched::default();

        // Files whose contents or details changed: update them in place,
        // unless their folder is listed anyway or they are not where the
        // index thinks (then their folder is listed).
        for p in restat {
            let parent = parent_of(&p);
            if relist.contains(&parent) {
                continue;
            }
            let cur = {
                let idx = sh.index.read();
                idx.resolve_path(&p).map(|id| (id, idx.is_dir(id), idx.flags(id) & F_SYMLINK != 0))
            };
            let st = scan::stat(&p);
            match (cur, st) {
                (Some((id, is_dir, link)), Some((kind, size, mtime, _, _))) if is_dir == (kind == KIND_DIR) && link == (kind == KIND_LINK) => {
                    let mut idx = sh.index.write();
                    if !idx.is_dead(id) && idx.set_meta(id, size, clamp_mtime(mtime)) {
                        touched.push(id);
                    }
                }
                _ => {
                    relist.insert(parent);
                }
            }
        }

        // Folders whose entries may have changed. Listing some of them can
        // move folders in the index, which makes paths that did not resolve
        // before resolve now, and leaves folders listed at a path they have
        // left: those are listed again where they are now, in later rounds.
        let mut walks = Vec::new();
        let mut want = relist;
        let mut done: HashSet<(u32, Vec<u8>)> = HashSet::new();
        for _ in 0..8 {
            let mut dirs = Vec::new();
            let mut later = BTreeSet::new();
            // A path FSEvents spells differently from the folder listing (an
            // accented letter composed in one and decomposed in the other):
            // the name stored on disk is what the index has.
            let unresolved: Vec<Vec<u8>> = {
                let idx = sh.index.read();
                want.iter().filter(|p| idx.resolve_path(p).is_none()).cloned().collect()
            };
            for p in unresolved {
                if let Some(q) = scan::on_disk(&p).and_then(|q| self.normalize(&q)) {
                    if q != p {
                        want.remove(&p);
                        want.insert(q);
                    }
                }
            }
            {
                let idx = sh.index.read();
                let walking: HashSet<u32> = walks.iter().map(|(id, _)| *id).collect();
                for p in &want {
                    let Some((id, path)) = self.indexed_dir(&idx, p) else { continue };
                    if path != *p {
                        later.insert(p.clone());
                    }
                    if !walking.contains(&id) && done.insert((id, path.clone())) {
                        dirs.push((id, path));
                    }
                }
            }
            if dirs.is_empty() {
                break;
            }
            for (_, now) in self.reconcile(dirs, &mut touched, &mut walks) {
                later.insert(now);
            }
            want = later;
        }

        // Whole subtrees FSEvents could not describe: list every folder in them.
        let mut level: Vec<(u32, Vec<u8>)> = {
            let idx = sh.index.read();
            let mut out: Vec<(u32, Vec<u8>)> = Vec::new();
            for p in &rescan {
                if out.iter().any(|(_, q)| p.starts_with(q) && p.get(q.len()) == Some(&b'/')) {
                    continue;
                }
                if let Some(d) = self.indexed_dir(&idx, p) {
                    out.push(d);
                }
            }
            out
        };
        if !level.is_empty() {
            info!("FSEvents lost track of {} folder(s); listing them again", level.len());
        }
        // So are folders that moved: events for what changed inside one while
        // it moved may name either place, so what the index holds for it is
        // checked against the disk, every folder beneath it too.
        let mut verified: HashSet<u32> = HashSet::new();
        loop {
            if level.is_empty() {
                let moved = std::mem::take(&mut self.moved);
                let idx = sh.index.read();
                level = moved.into_iter().filter(|&id| !idx.is_dead(id) && !verified.contains(&id)).map(|id| (id, idx.path_of(id))).collect();
            }
            if level.is_empty() {
                break;
            }
            verified.extend(level.iter().map(|(id, _)| *id));
            self.reconcile(level.clone(), &mut touched, &mut walks);
            // Folders that are new get walked whole; the rest are listed in turn.
            let walking: HashSet<u32> = walks.iter().map(|(id, _)| *id).collect();
            let idx = sh.index.read();
            let mut next = Vec::new();
            for (id, _) in &level {
                if idx.is_dead(*id) || !idx.is_dir(*id) {
                    continue;
                }
                let path = idx.path_of(*id);
                for &c in idx.children(*id) {
                    if idx.is_dir(c) && idx.flags(c) & F_NOSCAN == 0 && !walking.contains(&c) && !verified.contains(&c) {
                        next.push((c, join(&path, idx.orig_name(c))));
                    }
                }
            }
            level = next;
        }

        // A walk cut short leaves folders half filled in: such an index
        // must not be saved as current.
        if !walks.is_empty() && !self.walk_into(walks, Some(&mut touched)) {
            self.complete = false;
        }
        if !touched.is_empty() {
            self.unsaved = true;
            self.last_change = Instant::now();
            server::notify_changed(&sh, &touched);
        }
    }

    /// Make the index agree with each folder's listing. Entries are added
    /// and updated in every folder before any are removed, so a folder that
    /// moved between two of them is found by its id and moved, not removed
    /// and read again. New folders to walk are added to `walks`.
    /// Returns the folders that were not where they were listed because they
    /// moved (with a folder above them), with their path now.
    fn reconcile(&mut self, dirs: Vec<(u32, Vec<u8>)>, touched: &mut Touched, walks: &mut Vec<(u32, Vec<u8>)>) -> Vec<(u32, Vec<u8>)> {
        let sh = self.sh.clone();
        let listings: Vec<(u32, Vec<u8>, Result<Listing, ListError>)> = dirs
            .into_iter()
            .map(|(id, p)| {
                let l = scan::list_dir(&p);
                debug!("list {:?}: {}", lossy_path(&p), match &l { Ok(l) => format!("{} entries", l.children.len()), Err(e) => format!("{e:?}") });
                (id, p, l)
            })
            .collect();
        let ctx = self.walk_ctx();
        let mut idx = sh.index.write();
        let mut lower = Vec::new();
        let mut moves: HashMap<u32, u32> = HashMap::new();
        let mut elsewhere = Vec::new();
        // Pass one: what is there now.
        for (dir, path, res) in &listings {
            let Ok(l) = res else { continue };
            if idx.is_dead(*dir) {
                continue;
            }
            sh.denied.lock().remove(path);
            if idx.set_meta(*dir, 0, clamp_mtime(l.mtime)) {
                touched.push(*dir);
            }
            let (plan, _) = ctx.plan(path, l);
            for (c, &what) in l.children.iter().zip(&plan) {
                if what == SKIP {
                    continue;
                }
                let name = l.name(c);
                lower.clear();
                lower_into(name, &mut lower);
                let h = handle(c.dev, c.ino);
                let link = if c.kind == KIND_LINK { F_SYMLINK } else { 0 };
                let cur = idx.child_by_name(*dir, name, &lower);
                if let Some(e) = cur {
                    let same = if c.kind == KIND_DIR {
                        idx.is_dir(e) && idx.dir_handle(idx.dir_slot(e)) == h && (idx.flags(e) & F_NOSCAN != 0) == (what == NOSCAN)
                    } else {
                        !idx.is_dir(e) && idx.flags(e) & F_SYMLINK == link
                    };
                    if same {
                        if idx.set_meta(e, c.size, clamp_mtime(c.mtime)) {
                            touched.push(e);
                        }
                        continue;
                    }
                    idx.remove_subtree(e, touched);
                }
                let id = if c.kind == KIND_DIR {
                    // The same folder somewhere else in the index: it moved here.
                    let moved = idx.find_dir_by_handle(0, &h).map(|s| idx.dir(s).entry).filter(|&e| {
                        e != NONE && !idx.is_dead(e) && idx.flags(e) & F_ROOT == 0 && what != NOSCAN && !is_within(&idx, *dir, e)
                    });
                    match moved {
                        Some(e) => {
                            let id = idx.move_dir(e, *dir, name, &lower);
                            idx.set_meta(id, 0, clamp_mtime(c.mtime));
                            moves.insert(e, id);
                            self.moved.push(id);
                            debug!("moved folder to {:?}", lossy_path(&join(path, name)));
                            for w in walks.iter_mut().filter(|w| w.0 == e) {
                                w.0 = id;
                            }
                            id
                        }
                        None => {
                            let flags = if what == NOSCAN { F_NOSCAN } else { 0 };
                            let id = idx.add_dir(*dir, name, &lower, flags, clamp_mtime(c.mtime), 0, &h, true);
                            if what != NOSCAN {
                                walks.push((id, join(path, name)));
                            }
                            id
                        }
                    }
                } else {
                    idx.add_file(*dir, name, &lower, link, c.size, clamp_mtime(c.mtime), true)
                };
                touched.push(id);
            }
        }
        // Pass two: what is not there any more.
        for (dir, path, res) in &listings {
            if let Err(ListError::Gone) = res {
                // Gone from that path; moved, if the index has it elsewhere now.
                let cur = moves.get(dir).copied().unwrap_or(*dir);
                if idx.is_dead(cur) || idx.flags(cur) & F_ROOT != 0 {
                    continue;
                }
                let now = idx.path_of(cur);
                if now != *path {
                    debug!("{:?} is now at {:?}", lossy_path(path), lossy_path(&now));
                    elsewhere.push((cur, now));
                } else if scan::stat(path).is_none_or(|(k, ..)| k != KIND_DIR) {
                    idx.remove_subtree(cur, touched);
                }
                continue;
            }
            if idx.is_dead(*dir) {
                continue;
            }
            match res {
                Ok(l) => {
                    let (plan, _) = ctx.plan(path, l);
                    let keep: HashSet<&[u8]> = l.children.iter().zip(&plan).filter(|(_, &w)| w != SKIP).map(|(c, _)| l.name(c)).collect();
                    let gone: Vec<u32> = idx.children(*dir).iter().copied().filter(|&c| !keep.contains(idx.orig_name(c))).collect();
                    for c in gone {
                        idx.remove_subtree(c, touched);
                    }
                }
                Err(ListError::Gone) => {}
                Err(ListError::Denied) => {
                    sh.denied.lock().insert(path.clone());
                }
            }
        }
        elsewhere
    }

    // ---- housekeeping ------------------------------------------------

    /// A volume was mounted or unmounted: list the folders that hold the
    /// mount points that came or went.
    fn check_mounts(&mut self) {
        let now = volumes::mounts();
        if now == self.mounts {
            return;
        }
        let old: HashSet<Vec<u8>> = self.mounts.iter().map(|m| m.path.clone()).collect();
        let new: HashSet<Vec<u8>> = now.iter().map(|m| m.path.clone()).collect();
        let changed: Vec<Vec<u8>> = old.symmetric_difference(&new).cloned().collect();
        self.set_mounts(now);
        let dirs: Vec<(u32, Vec<u8>)> = {
            let idx = self.sh.index.read();
            let mut seen = HashSet::new();
            changed.iter().filter(|p| !p.is_empty()).filter_map(|p| self.indexed_dir(&idx, &parent_of(p))).filter(|(id, _)| seen.insert(*id)).collect()
        };
        if dirs.is_empty() {
            return;
        }
        info!("volumes were mounted or unmounted: {}", changed.iter().map(|p| lossy_path(p)).collect::<Vec<_>>().join(", "));
        let mut touched = Touched::default();
        let mut walks = Vec::new();
        self.reconcile(dirs, &mut touched, &mut walks);
        if !self.walk_into(walks, Some(&mut touched)) {
            self.complete = false;
        }
        if !touched.is_empty() {
            self.unsaved = true;
            server::notify_changed(&self.sh, &touched);
        }
    }

    /// Try the folders that could not be read again: Full Disk Access may
    /// have been granted since.
    fn retry_denied(&mut self) {
        let paths: Vec<Vec<u8>> = self.sh.denied.lock().iter().cloned().collect();
        if paths.is_empty() {
            return;
        }
        let readable: Vec<Vec<u8>> = paths.into_iter().filter(|p| scan::list_dir(p).is_ok()).collect();
        if readable.is_empty() {
            return;
        }
        info!("{} folder(s) that could not be read before can be now", readable.len());
        let dirs: Vec<(u32, Vec<u8>)> = {
            let idx = self.sh.index.read();
            readable.iter().filter_map(|p| idx.resolve_path(p).filter(|&id| idx.is_dir(id)).map(|id| (id, p.clone()))).collect()
        };
        {
            let mut d = self.sh.denied.lock();
            for p in &readable {
                d.remove(p);
            }
        }
        let mut touched = Touched::default();
        let mut walks = Vec::new();
        self.reconcile(dirs, &mut touched, &mut walks);
        if !self.walk_into(walks, Some(&mut touched)) {
            self.complete = false;
        }
        if !touched.is_empty() {
            self.unsaved = true;
            server::notify_changed(&self.sh, &touched);
        }
    }

    fn save(&mut self, clean: bool) {
        let sh = self.sh.clone();
        if !self.complete {
            info!("not saving the index: it is incomplete");
            return;
        }
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
        let fs = vec![SavedFs { volumes: volumes::identity(&sh.settings.roots), event_id: self.applied }];
        let meta = db::Meta { saved_at_ns: now_ns(), clean, boot_id: String::new(), fingerprint: sh.settings.fingerprint(), fs };
        let idx = sh.index.read();
        match db::save(&sh.settings.db, &idx, &meta) {
            Ok(bytes) => info!("saved index ({} entries, {} MB on disk) in {:.1}s", idx.live_count(), bytes >> 20, t0.elapsed().as_secs_f64()),
            Err(e) => warn!("could not save index to {}: {e}", sh.settings.db.display()),
        }
        self.unsaved = false;
        self.last_save = Instant::now();
    }

    /// Start from the saved index if FSEvents can replay what happened
    /// since it was saved.
    fn try_load(&mut self) -> bool {
        let sh = self.sh.clone();
        let (idx, meta) = match db::load(&sh.settings.db) {
            Ok(x) => x,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!("saved index not used: {e}");
                }
                return false;
            }
        };
        if meta.fingerprint != sh.settings.fingerprint() {
            info!("the configuration changed since the index was saved; rebuilding it");
            return false;
        }
        let Some(saved) = meta.fs.first() else { return false };
        if !sh.settings.watch || saved.volumes != volumes::identity(&sh.settings.roots) || saved.event_id > fse::current_id() {
            info!("the disks changed since the index was saved; rebuilding it");
            return false;
        }
        if idx.roots().len() != sh.settings.roots.len() {
            warn!("the saved index is incomplete; rebuilding it");
            return false;
        }
        info!("loaded saved index ({} entries); catching up with FSEvents", idx.live_count());
        self.applied = saved.event_id;
        replace_index(&sh, idx);
        true
    }

    fn startup(&mut self) {
        self.set_mounts(volumes::mounts());
        if self.try_load() {
            let since = self.applied;
            self.start_stream(since);
            self.sh.state.store(ST_READY, Ordering::Relaxed);
            server::notify_all(&self.sh);
        } else {
            self.full_scan();
        }
    }

    fn housekeeping(&mut self) {
        let sh = self.sh.clone();
        let (delta, dead, live) = {
            let idx = sh.index.read();
            (idx.delta_len(), idx.dead_count(), idx.live_count())
        };
        if delta > 4096 || (delta > 0 && self.last_change.elapsed() > Duration::from_secs(3)) {
            merge_perm(&sh);
        }
        if dead > 200_000 && dead > live / 3 {
            sh.index.write().compact();
            server::notify_all(&sh);
        }
        if self.unsaved && self.last_save.elapsed() > Duration::from_secs(sh.settings.save_interval_secs()) {
            self.save(false);
        }
        if self.last_mount_check.elapsed() > Duration::from_secs(15) {
            self.last_mount_check = Instant::now();
            self.check_mounts();
        }
        if self.last_retry.elapsed() > Duration::from_secs(60) {
            self.last_retry = Instant::now();
            self.retry_denied();
        }
    }

    fn control(&mut self, msg: Msg) {
        match msg {
            Msg::Events { gen, events } => {
                if gen == self.gen {
                    self.process(events);
                }
            }
            Msg::Rescan { .. } => self.full_scan(),
            Msg::Shutdown => self.shutdown = true,
        }
    }

    fn event_loop(&mut self) {
        while !self.shutdown {
            match self.rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Msg::Events { gen, events }) => {
                    // Take whatever else is waiting along with it.
                    let mut all = if gen == self.gen { events } else { Vec::new() };
                    let mut later = Vec::new();
                    while all.len() < 200_000 {
                        match self.rx.try_recv() {
                            Ok(Msg::Events { gen, events }) if gen == self.gen => all.extend(events),
                            Ok(Msg::Events { .. }) => {}
                            Ok(m) => later.push(m),
                            Err(_) => break,
                        }
                    }
                    if !all.is_empty() {
                        self.process(all);
                    }
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
        // Deliver and apply what FSEvents still holds, then save.
        if let Some(s) = self.stream.take() {
            s.flush();
            drop(s);
        }
        let mut rest = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            if let Msg::Events { gen, events } = m {
                if gen == self.gen {
                    rest.extend(events);
                }
            }
        }
        if !rest.is_empty() && !self.sh.cancel.load(Ordering::Relaxed) {
            self.process(rest);
        }
        self.save(true);
    }
}

/// Whether `e` is `dir` or one of its ancestors (so moving `e` into `dir`
/// would put it inside itself).
fn is_within(idx: &Index, dir: u32, e: u32) -> bool {
    let mut cur = dir;
    while cur != NONE {
        if cur == e {
            return true;
        }
        cur = idx.parent(cur);
    }
    false
}

/// Body of the indexer thread. Returns after an orderly shutdown.
pub fn run(sh: Arc<Shared>, rx: Receiver<Msg>) {
    let threads = sh.settings.scan_threads();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("scan-{i}"))
        .build()
        .expect("scan thread pool");
    let mut ix = Indexer {
        sh,
        rx,
        pool: Arc::new(pool),
        stream: None,
        gen: 0,
        applied: 0,
        moved: Vec::new(),
        mounts: Vec::new(),
        unsaved: false,
        last_save: Instant::now(),
        last_change: Instant::now(),
        last_mount_check: Instant::now(),
        last_retry: Instant::now(),
        shutdown: false,
        complete: true,
    };
    ix.startup();
    ix.event_loop();
}
