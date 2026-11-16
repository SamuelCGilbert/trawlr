//! The single writer on Windows: builds the index from each drive's master
//! file table and keeps it current from the drives' change journals.
//!
//! As on Linux, everything that changes the index happens on this one
//! thread, and a journal record is never taken as a description of what
//! happened: it only names a file that may have changed, and the file's
//! MFT record is read to see what is true now. Applying a record twice, or
//! after a later change already showed up, therefore comes to the same
//! result.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

use super::ntfs::{self, rec_of, seq_of, Ntfs, EXTEND_REC, FIRST_USER_REC, ROOT_REC};
use super::usn::{self, Change, ReadError, USN_REASON_FILE_CREATE};
use super::volumes::{self, SavedFs};
use crate::config::Settings;
use crate::db;
use crate::index::{Index, Touched, F_ROOT, F_SYMLINK, SEP};
use crate::server::{self, SessionShared};
use crate::util::{clamp_mtime, lower_into, now_ns};
use crate::{info, warn};

pub const ST_STARTING: u8 = 0;
pub const ST_SCANNING: u8 = 1;
pub const ST_READY: u8 = 3;

pub enum Msg {
    /// Records read from drive `vol`'s journal; the index is current up to
    /// `next` once they are applied. `gen` tells apart readers started
    /// before the last full scan, whose positions mean nothing any more.
    Changes { vol: u16, gen: u64, changes: Vec<Change>, next: i64 },
    /// A journal lost records the index had not seen yet.
    JournalLost { vol: u16, gen: u64 },
    Rescan { deep: bool, rebuild: bool },
    Shutdown,
}

/// What `status` reports about a drive.
#[derive(Clone)]
pub struct VolStatus {
    pub drive: String,
    pub live: bool,
}

pub struct Shared {
    pub settings: Arc<Settings>,
    pub index: RwLock<Index>,
    pub state: AtomicU8,
    pub sessions: Mutex<Vec<Arc<SessionShared>>>,
    pub tx: Sender<Msg>,
    pub started: Instant,
    pub events: AtomicU64,
    pub cancel: AtomicBool,
    pub volumes: RwLock<Vec<VolStatus>>,
    pub scanned: AtomicU64,
}

impl Shared {
    pub fn new(settings: Arc<Settings>, tx: Sender<Msg>) -> Shared {
        Shared {
            settings,
            index: RwLock::new(Index::new()),
            state: AtomicU8::new(ST_STARTING),
            sessions: Mutex::new(Vec::new()),
            tx,
            started: Instant::now(),
            events: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            volumes: RwLock::new(Vec::new()),
            scanned: AtomicU64::new(0),
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

/// The Windows part of the `status` reply.
pub fn status_details(sh: &Shared, v: &mut serde_json::Map<String, serde_json::Value>) {
    use serde_json::json;
    let vols = sh.volumes.read().clone();
    v.insert("roots".into(), json!(vols.iter().map(|d| format!("{}\\", d.drive)).collect::<Vec<_>>()));
    v.insert(
        "filesystems".into(),
        json!(vols.iter().map(|d| json!({"mount": format!("{}\\", d.drive), "type": "NTFS", "watch": if d.live { "change journal" } else { "off" }})).collect::<Vec<_>>()),
    );
    v.insert("scanned".into(), json!(sh.scanned.load(Ordering::Relaxed)));
    v.insert("folder_watches".into(), json!(0));
    v.insert("unwatched_folders".into(), json!(0));
}

/// A directory's key in the index's handle lookup: its file reference.
fn key(frn: u64) -> [u8; 8] {
    frn.to_le_bytes()
}

struct Reader {
    stop: Arc<AtomicBool>,
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct Vol {
    letter: u8,
    ntfs: Ntfs,
    journal: usn::Journal,
    /// The journal position the index is current with.
    applied: i64,
    reader: Option<Reader>,
}

impl Vol {
    fn drive(&self) -> String {
        format!("{}:", self.letter as char)
    }
}

/// Read a drive's journal on a thread of its own, from `start` on, and
/// send what it finds to the indexer.
fn spawn_reader(tx: Sender<Msg>, vol: u16, gen: u64, letter: u8, journal: u64, start: i64) -> Reader {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let _ = std::thread::Builder::new().name(format!("journal-{}", letter as char)).spawn(move || {
        let h = match ntfs::open_volume(letter) {
            Ok(h) => h,
            Err(e) => {
                warn!("{}: cannot read the change journal: {e}", letter as char);
                return;
            }
        };
        let mut buf = vec![0u8; 256 * 1024];
        let mut pos = start;
        while !flag.load(Ordering::Relaxed) {
            let mut out = Vec::new();
            match usn::read(&h, journal, pos, &mut buf, &mut out) {
                Ok(next) => {
                    let idle = next == pos;
                    if !idle && tx.send(Msg::Changes { vol, gen, changes: out, next }).is_err() {
                        return;
                    }
                    pos = next;
                    if idle {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
                Err(ReadError::Lost) => {
                    let _ = tx.send(Msg::JournalLost { vol, gen });
                    return;
                }
                Err(ReadError::Io(e)) => {
                    warn!("{}: reading the change journal: {e}", letter as char);
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        }
    });
    Reader { stop }
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

/// One file's journal records within a batch.
struct Group {
    vol: u16,
    frn: u64,
    /// Where the records say the file was or is: (folder, name).
    places: Vec<(u64, String)>,
    created: bool,
}

struct Indexer {
    sh: Arc<Shared>,
    rx: Receiver<Msg>,
    vols: Vec<Vol>,
    gen: u64,
    unsaved: bool,
    last_save: Instant,
    last_change: Instant,
    last_drive_check: Instant,
    shutdown: bool,
    /// False while the index is missing a drive whose MFT could not be read
    /// to the end (or the scan was cut short by a shutdown). Such an index
    /// is never saved: the journal positions saved with it would claim it is
    /// complete, and the next start would carry on from it.
    complete: bool,
    /// Folders that hold excluded paths (lower case).
    exclude_parents: HashSet<Vec<u8>>,
}

impl Indexer {
    fn excluded(&self, idx: &Index, parent: u32, lower: &[u8]) -> bool {
        if self.sh.settings.is_excluded_name(lower) {
            return true;
        }
        if self.exclude_parents.is_empty() {
            return false;
        }
        let mut p = Vec::new();
        idx.match_path_into(parent, false, &mut p);
        if !self.exclude_parents.contains(&p) {
            return false;
        }
        p.push(SEP);
        p.extend_from_slice(lower);
        self.sh.settings.is_excluded(&p, lower)
    }

    // ---- drives ------------------------------------------------------

    fn wanted_drives(&self) -> Vec<u8> {
        if self.sh.settings.roots.is_empty() {
            volumes::local_ntfs_drives()
        } else {
            self.sh.settings.roots.iter().map(|r| r[0]).collect()
        }
    }

    /// Open the drives to index, without reading them yet.
    fn open_drives(&mut self) {
        self.vols.clear();
        for letter in self.wanted_drives() {
            match volumes::fs_of(letter) {
                Some((fs, _)) if fs == "NTFS" => {}
                Some((fs, _)) => {
                    warn!("{}: is {fs}; only NTFS drives can be indexed", letter as char);
                    continue;
                }
                None => {
                    warn!("{}: is not available", letter as char);
                    continue;
                }
            }
            let opened = Ntfs::open(letter).and_then(|n| usn::ensure(&n.h).map(|j| (n, j)));
            match opened {
                Ok((ntfs, journal)) => self.vols.push(Vol { letter, ntfs, journal, applied: journal.next, reader: None }),
                Err(e) => warn!("{}: cannot be indexed: {e}", letter as char),
            }
        }
        *self.sh.volumes.write() = self.vols.iter().map(|v| VolStatus { drive: v.drive(), live: false }).collect();
    }

    fn start_readers(&mut self) {
        self.gen += 1;
        for (i, v) in self.vols.iter_mut().enumerate() {
            v.reader = Some(spawn_reader(self.sh.tx.clone(), i as u16, self.gen, v.letter, v.journal.id, v.applied));
        }
        *self.sh.volumes.write() = self.vols.iter().map(|v| VolStatus { drive: v.drive(), live: v.reader.is_some() }).collect();
    }

    fn stop_readers(&mut self) {
        for v in &mut self.vols {
            v.reader = None;
        }
    }

    // ---- full scan ---------------------------------------------------

    /// Read drive `v`'s MFT into the index under a new root. False if it
    /// could not be read to the end.
    fn scan_volume(&mut self, v: usize) -> bool {
        const IN_USE: u8 = 1;
        const DIR: u8 = 2;
        const LINK: u8 = 4;
        let sh = self.sh.clone();
        let t0 = Instant::now();
        let ntfs = &self.vols[v].ntfs;
        let total = (ntfs.mft_valid / ntfs.record_size as u64) as usize;
        // The MFT lists records by number, a child often before its folder,
        // so it is first read into flat tables and then walked from the root.
        let mut seq = vec![0u16; total];
        let mut flags = vec![0u8; total];
        let mut size = vec![0u64; total];
        let mut mtime = vec![0u32; total];
        // (record, folder reference, offset and length in `text`)
        let mut names: Vec<(u32, u64, u32, u32)> = Vec::new();
        let mut text: Vec<u8> = Vec::new();
        let scanned = &sh.scanned;
        let res = ntfs.scan(&sh.cancel, |rec, r| {
            let target = if r.base != 0 { rec_of(r.base) as usize } else { rec as usize };
            if target >= total {
                return;
            }
            if r.base == 0 {
                seq[target] = r.seq;
                flags[target] = IN_USE | if r.is_dir { DIR } else { 0 } | if r.is_link() { LINK } else { 0 };
                mtime[target] = clamp_mtime(r.mtime.unwrap_or(0));
            }
            if let Some(s) = r.size {
                size[target] = s;
            }
            for (parent, name) in r.names {
                if text.len() + name.len() < u32::MAX as usize {
                    names.push((target as u32, parent, text.len() as u32, name.len() as u32));
                    text.extend_from_slice(name.as_bytes());
                }
            }
            scanned.fetch_add(1, Ordering::Relaxed);
        });
        if let Err(e) = res {
            if e.kind() != std::io::ErrorKind::Interrupted {
                warn!("{}: reading the MFT failed: {e}", self.vols[v].drive());
            }
            return false;
        }
        let read_secs = t0.elapsed().as_secs_f64();

        // Children of each folder: names sorted by the folder's record.
        names.sort_unstable_by_key(|n| rec_of(n.1));
        let mut start = vec![0u32; total + 1];
        for n in &names {
            let p = rec_of(n.1) as usize;
            if p < total {
                start[p + 1] += 1;
            }
        }
        for i in 0..total {
            start[i + 1] += start[i];
        }

        let fs = v as u16;
        let root_frn = ROOT_REC | (seq.get(ROOT_REC as usize).copied().unwrap_or(0) as u64) << 48;
        let drive = self.vols[v].drive();
        let root = sh.index.write().add_root(drive.as_bytes(), fs, &key(root_frn));
        let mut visited = vec![false; total];
        let mut stack: Vec<(usize, u32)> = vec![(ROOT_REC as usize, root)];
        let mut lower = Vec::new();
        let mut since_unlock = 0usize;
        let mut since_merge = 0usize;
        let mut idx = sh.index.write();
        while let Some((dir_rec, dir_id)) = stack.pop() {
            if dir_rec >= total || std::mem::replace(&mut visited[dir_rec], true) {
                continue;
            }
            let (a, b) = (start[dir_rec] as usize, start[dir_rec + 1] as usize);
            for &(c, parent, off, len) in &names[a..b] {
                let c = c as usize;
                // A name left over from an earlier use of the folder's record,
                // NTFS's own files, and anything not in use are skipped.
                if seq_of(parent) != seq[dir_rec] || flags[c] & IN_USE == 0 || (c as u64) < FIRST_USER_REC || c == dir_rec {
                    continue;
                }
                let name = &text[off as usize..(off + len) as usize];
                lower.clear();
                lower_into(name, &mut lower);
                if self.excluded(&idx, dir_id, &lower) {
                    continue;
                }
                let link = if flags[c] & LINK != 0 { F_SYMLINK } else { 0 };
                if flags[c] & DIR != 0 {
                    if c as u64 == EXTEND_REC {
                        continue;
                    }
                    let frn = c as u64 | (seq[c] as u64) << 48;
                    let id = idx.add_dir(dir_id, name, &lower, link, mtime[c], fs, &key(frn), false);
                    stack.push((c, id));
                } else {
                    idx.add_file(dir_id, name, &lower, link, size[c], mtime[c], false);
                }
            }
            idx.finish_children(dir_id);
            since_unlock += b - a + 1;
            since_merge += b - a;
            // Let searches in now and then while a big drive is added.
            if since_unlock > 50_000 {
                drop(idx);
                if since_merge > 2_000_000 {
                    merge_perm(&sh);
                    since_merge = 0;
                }
                idx = sh.index.write();
                since_unlock = 0;
            }
        }
        drop(idx);
        info!("{drive} read {} MFT records in {read_secs:.1}s, indexed in {:.1}s", names.len(), t0.elapsed().as_secs_f64());
        !sh.cancel.load(Ordering::Relaxed)
    }

    fn full_scan(&mut self) {
        let sh = self.sh.clone();
        let t0 = Instant::now();
        sh.state.store(ST_SCANNING, Ordering::Relaxed);
        self.stop_readers();
        self.open_drives();
        replace_index(&sh, Index::new());
        sh.scanned.store(0, Ordering::Relaxed);
        self.complete = true;
        for v in 0..self.vols.len() {
            // Changes from this moment on are read from the journal, so
            // nothing that happens while the MFT is read is missed.
            if let Ok(Some(j)) = usn::query(&self.vols[v].ntfs.h) {
                self.vols[v].journal = j;
            }
            self.vols[v].applied = self.vols[v].journal.next;
            if !self.scan_volume(v) {
                self.complete = false;
            }
        }
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
        self.start_readers();
        sh.state.store(ST_READY, Ordering::Relaxed);
        server::notify_all(&sh);
        self.save(false);
    }

    /// Fill in a folder that turned up without the journal describing its
    /// contents (moved in from an excluded folder), by listing it.
    fn walk(&self, vol: u16, dir_id: u32, frn: u64, touched: &mut Touched) {
        let mut stack = vec![(dir_id, frn)];
        let mut lower = Vec::new();
        while let Some((id, frn)) = stack.pop() {
            let Ok(list) = self.vols[vol as usize].ntfs.list_dir(frn) else { continue };
            let mut idx = self.sh.index.write();
            if idx.is_dead(id) {
                continue;
            }
            for e in list {
                lower.clear();
                lower_into(e.name.as_bytes(), &mut lower);
                if self.excluded(&idx, id, &lower) || idx.child_by_name_nocase(id, e.name.as_bytes(), &lower).is_some() {
                    continue;
                }
                let link = if e.is_link { F_SYMLINK } else { 0 };
                let c = if e.is_dir {
                    if idx.find_dir_by_handle(vol, &key(e.frn)).is_some() {
                        continue;
                    }
                    let c = idx.add_dir(id, e.name.as_bytes(), &lower, link, clamp_mtime(e.mtime), vol, &key(e.frn), false);
                    stack.push((c, e.frn));
                    c
                } else {
                    idx.add_file(id, e.name.as_bytes(), &lower, link, e.size, clamp_mtime(e.mtime), false)
                };
                touched.push(c);
            }
            idx.finish_children(id);
        }
    }

    // ---- live changes ------------------------------------------------

    fn process_changes(&mut self, batch: Vec<(u16, Change)>) {
        let sh = self.sh.clone();
        sh.events.fetch_add(batch.len() as u64, Ordering::Relaxed);
        // Gather each file's records, in the order files first appear.
        let mut groups: Vec<Group> = Vec::new();
        let mut at: HashMap<(u16, u64), usize> = HashMap::new();
        for (vol, c) in batch {
            let i = *at.entry((vol, c.frn)).or_insert_with(|| {
                groups.push(Group { vol, frn: c.frn, places: Vec::new(), created: false });
                groups.len() - 1
            });
            let g = &mut groups[i];
            g.created |= c.reason & USN_REASON_FILE_CREATE != 0;
            if !g.places.iter().any(|(p, n)| *p == c.parent && *n == c.name) {
                g.places.push((c.parent, c.name));
            }
        }
        let mut touched = Touched::default();
        // Names a file of this batch holds now. Another file's record, which
        // only says the name was its own earlier, must not remove it.
        let mut claimed: HashSet<(u16, u64, Vec<u8>)> = HashSet::new();
        let mut walks = Vec::new();
        // A file is applied once the folder it is in now is: a folder created
        // later in the batch than the file's first record would otherwise not
        // be in the index yet.
        let mut pending: HashSet<(u16, u64)> = groups.iter().map(|g| (g.vol, g.frn)).collect();
        let mut queue: Vec<usize> = (0..groups.len()).collect();
        let mut patient = true;
        while !queue.is_empty() {
            let mut deferred = Vec::new();
            for &gi in &queue {
                let g = &groups[gi];
                match self.apply_group(g, &pending, patient, &mut claimed, &mut touched, &mut walks) {
                    true => {
                        pending.remove(&(g.vol, g.frn));
                    }
                    false => deferred.push(gi),
                }
            }
            // No progress: whatever is left waits on something that will not
            // come, so apply it as it is.
            patient = deferred.len() < queue.len();
            queue = deferred;
        }
        for (vol, id, frn) in walks {
            self.walk(vol, id, frn, &mut touched);
        }
        if !touched.is_empty() {
            self.unsaved = true;
            self.last_change = Instant::now();
            server::notify_changed(&sh, &touched);
        }
    }

    /// Make the index agree with the MFT about one file. Returns false to
    /// be asked again later (its folder is still waiting in this batch).
    fn apply_group(
        &self,
        g: &Group,
        pending: &HashSet<(u16, u64)>,
        patient: bool,
        claimed: &mut HashSet<(u16, u64, Vec<u8>)>,
        touched: &mut Touched,
        walks: &mut Vec<(u16, u32, u64)>,
    ) -> bool {
        let Some(vol) = self.vols.get(g.vol as usize) else { return true };
        let rec = match vol.ntfs.file(g.frn) {
            Ok(r) => r,
            Err(e) => {
                warn!("{}: cannot read the record of a changed file: {e}", vol.drive());
                return true;
            }
        };
        let fs = g.vol;
        let mut idx = self.sh.index.write();
        // Where the file is now, among folders that are indexed.
        let mut want: Vec<(u32, u64, String, Vec<u8>)> = Vec::new();
        if let Some(r) = &rec {
            for (parent, name) in &r.names {
                let slot = idx.find_dir_by_handle(fs, &key(*parent));
                let Some(slot) = slot else {
                    if patient && pending.contains(&(g.vol, *parent)) && *parent != g.frn {
                        return false;
                    }
                    continue;
                };
                let p = idx.dir(slot).entry;
                let mut lower = Vec::new();
                lower_into(name.as_bytes(), &mut lower);
                if rec_of(g.frn) < FIRST_USER_REC || self.excluded(&idx, p, &lower) {
                    continue;
                }
                want.push((p, *parent, name.clone(), lower));
            }
        }
        let is_dir = rec.as_ref().is_some_and(|r| r.is_dir);
        let link = if rec.as_ref().is_some_and(|r| r.is_link()) { F_SYMLINK } else { 0 };
        let mtime = clamp_mtime(rec.as_ref().and_then(|r| r.mtime).unwrap_or(0));
        let size = rec.as_ref().and_then(|r| r.size).unwrap_or(0);

        // A folder is found by its reference wherever it is indexed.
        let existing = idx.find_dir_by_handle(fs, &key(g.frn)).map(|s| idx.dir(s).entry);
        if let Some(e) = existing {
            if idx.flags(e) & F_ROOT != 0 {
                return true;
            }
            if !is_dir || want.is_empty() {
                idx.remove_subtree(e, touched);
            }
        }
        if is_dir {
            let Some((p, pfrn, name, lower)) = want.first() else { return true };
            claimed.insert((fs, *pfrn, lower.clone()));
            // Something else indexed under the name it has now is out of date.
            if let Some(other) = idx.child_by_name_nocase(*p, name.as_bytes(), lower) {
                if Some(other) != existing {
                    idx.remove_subtree(other, touched);
                }
            }
            match existing {
                Some(e) if idx.parent(e) == *p && idx.orig_name(e) == name.as_bytes() => {
                    if idx.set_meta(e, 0, mtime) {
                        touched.push(e);
                    }
                }
                Some(e) => {
                    let id = idx.move_dir(e, *p, name.as_bytes(), lower);
                    idx.set_meta(id, 0, mtime);
                    touched.push(id);
                }
                None => {
                    let id = idx.add_dir(*p, name.as_bytes(), lower, link, mtime, fs, &key(g.frn), true);
                    touched.push(id);
                    // A new folder's contents arrive as records of their own;
                    // one that came from elsewhere has to be listed.
                    if !g.created {
                        walks.push((fs, id, g.frn));
                    }
                }
            }
            return true;
        }

        // A file: every place the records mention, and every place it is now.
        let mut places: Vec<(u64, &str)> = g.places.iter().map(|(p, n)| (*p, n.as_str())).collect();
        for (_, pfrn, name, _) in &want {
            if !places.iter().any(|(p, n)| p == pfrn && *n == name) {
                places.push((*pfrn, name));
            }
        }
        for (pfrn, name) in places {
            let Some(slot) = idx.find_dir_by_handle(fs, &key(pfrn)) else { continue };
            let p = idx.dir(slot).entry;
            let mut lower = Vec::new();
            lower_into(name.as_bytes(), &mut lower);
            let wanted = want.iter().any(|(wp, _, n, _)| *wp == p && n == name);
            let cur = idx.child_by_name_nocase(p, name.as_bytes(), &lower);
            let k = (fs, pfrn, lower.clone());
            if wanted {
                claimed.insert(k);
                match cur {
                    Some(c) if !idx.is_dir(c) && idx.orig_name(c) == name.as_bytes() && idx.flags(c) & F_SYMLINK == link => {
                        if idx.set_meta(c, size, mtime) {
                            touched.push(c);
                        }
                    }
                    other => {
                        if let Some(c) = other {
                            idx.remove_subtree(c, touched);
                        }
                        let id = idx.add_file(p, name.as_bytes(), &lower, link, size, mtime, true);
                        touched.push(id);
                    }
                }
            } else if let Some(c) = cur {
                // Gone from here, unless a folder or another file holds the
                // name now.
                if !idx.is_dir(c) && !claimed.contains(&k) && idx.orig_name(c) == name.as_bytes() {
                    idx.remove_subtree(c, touched);
                }
            }
        }
        true
    }

    // ---- housekeeping ------------------------------------------------

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
        let fs = self.vols.iter().map(|v| SavedFs { drive: v.drive(), serial: v.ntfs.serial, journal_id: v.journal.id, next_usn: v.applied }).collect();
        let meta = db::Meta { saved_at_ns: now_ns(), clean, boot_id: String::new(), fingerprint: sh.settings.fingerprint(), fs };
        let idx = sh.index.read();
        match db::save(&sh.settings.db, &idx, &meta) {
            Ok(bytes) => info!("saved index ({} entries, {} MB on disk) in {:.1}s", idx.live_count(), bytes >> 20, t0.elapsed().as_secs_f64()),
            Err(e) => warn!("could not save index to {}: {e}", sh.settings.db.display()),
        }
        self.unsaved = false;
        self.last_save = Instant::now();
    }

    /// Start from the saved index if it still describes these drives and
    /// their journals still reach back to where it was saved.
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
        let same = meta.fs.len() == self.vols.len()
            && meta.fs.iter().zip(&self.vols).all(|(s, v)| s.drive == v.drive() && s.serial == v.ntfs.serial && s.journal_id == v.journal.id && s.next_usn >= v.journal.first && s.next_usn <= v.journal.next);
        if !same {
            info!("the drives or their change journals changed since the index was saved; rebuilding it");
            return false;
        }
        // Each drive is a root of the index; one without is not a whole index.
        if idx.roots().len() != self.vols.len() {
            warn!("the saved index is incomplete; rebuilding it");
            return false;
        }
        for (s, v) in meta.fs.iter().zip(&mut self.vols) {
            v.applied = s.next_usn;
        }
        info!("loaded saved index ({} entries); catching up from the change journals", idx.live_count());
        replace_index(&sh, idx);
        true
    }

    fn startup(&mut self) {
        self.open_drives();
        if self.vols.is_empty() {
            warn!("no NTFS drive to index");
        }
        if self.try_load() {
            self.start_readers();
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
        // A drive plugged in or removed: index what is there now.
        if sh.settings.roots.is_empty() && self.last_drive_check.elapsed() > Duration::from_secs(30) {
            self.last_drive_check = Instant::now();
            let now: Vec<(u8, u64)> = volumes::local_ntfs_drives().into_iter().map(|l| (l, volumes::fs_of(l).map(|(_, s)| s as u64).unwrap_or(0))).collect();
            let had: Vec<(u8, u64)> = self.vols.iter().map(|v| (v.letter, v.ntfs.serial & 0xFFFF_FFFF)).collect();
            if now != had {
                info!("the set of drives changed; indexing them again");
                self.full_scan();
            }
        }
    }

    fn control(&mut self, msg: Msg) {
        match msg {
            Msg::Changes { vol, gen, changes, next } => self.apply_batch(vec![(vol, gen, changes, next)]),
            Msg::JournalLost { gen, vol } => {
                if gen == self.gen {
                    warn!("{}: the change journal lost changes; indexing again", self.vols.get(vol as usize).map(|v| v.drive()).unwrap_or_default());
                    self.full_scan();
                }
            }
            Msg::Rescan { .. } => self.full_scan(),
            Msg::Shutdown => self.shutdown = true,
        }
    }

    fn apply_batch(&mut self, parts: Vec<(u16, u64, Vec<Change>, i64)>) {
        let mut batch = Vec::new();
        let mut done = Vec::new();
        for (vol, gen, changes, next) in parts {
            // Records from a reader started before the last full scan
            // were already covered by it.
            if gen != self.gen {
                continue;
            }
            batch.extend(changes.into_iter().map(|c| (vol, c)));
            done.push((vol, next));
        }
        if !batch.is_empty() {
            self.process_changes(batch);
        }
        for (vol, next) in done {
            if let Some(v) = self.vols.get_mut(vol as usize) {
                if next > v.applied {
                    v.applied = next;
                    self.unsaved = true;
                }
            }
        }
    }

    fn event_loop(&mut self) {
        while !self.shutdown {
            match self.rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Msg::Changes { vol, gen, changes, next }) => {
                    let mut parts = vec![(vol, gen, changes, next)];
                    let mut later = Vec::new();
                    let mut n: usize = parts[0].2.len();
                    while n < 200_000 {
                        match self.rx.try_recv() {
                            Ok(Msg::Changes { vol, gen, changes, next }) => {
                                n += changes.len();
                                parts.push((vol, gen, changes, next));
                            }
                            Ok(m) => later.push(m),
                            Err(_) => break,
                        }
                    }
                    self.apply_batch(parts);
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
        self.stop_readers();
        let mut rest = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            if let Msg::Changes { vol, gen, changes, next } = m {
                rest.push((vol, gen, changes, next));
            }
        }
        if !rest.is_empty() {
            self.apply_batch(rest);
        }
        self.save(true);
    }
}

/// Body of the indexer thread. Returns after an orderly shutdown.
pub fn run(sh: Arc<Shared>, rx: Receiver<Msg>) {
    let exclude_parents = sh.settings.exclude_parents();
    let mut ix = Indexer {
        sh,
        rx,
        vols: Vec::new(),
        gen: 0,
        unsaved: false,
        last_save: Instant::now(),
        last_change: Instant::now(),
        last_drive_check: Instant::now(),
        shutdown: false,
        complete: true,
        exclude_parents,
    };
    ix.startup();
    ix.event_loop();
}
