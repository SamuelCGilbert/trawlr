//! The in-memory index: every file and folder name, kept current by the
//! indexer and scanned by queries.
//!
//! Layout is structure-of-arrays, one slot per entry, and append-only:
//!
//! * An entry id never changes meaning and is never reused until
//!   [`Index::compact`] renumbers everything (bumping `epoch`). Deleting marks
//!   a tombstone; renaming a folder appends a new entry and re-parents the
//!   children, so moving a tree of a million files is O(direct children).
//! * Names live back to back in one buffer in id order. A substring search is
//!   therefore a single pass over contiguous memory, and a match offset maps
//!   back to its entry by binary search over `name_off`.
//! * Paths are not stored. Each entry points at its parent, and a path is
//!   rebuilt on demand for the few rows a client actually displays.

use std::cmp::Ordering;
use std::sync::Arc;

use hashbrown::HashTable;
use parking_lot::Mutex;
use rayon::prelude::*;

use crate::util::{hash_bytes, lower_into};

pub const NONE: u32 = u32::MAX;

pub const F_DIR: u8 = 1;
pub const F_SYMLINK: u8 = 2;
pub const F_DEAD: u8 = 4;
/// The original name differs from its folded form and is stored separately.
pub const F_ORIG: u8 = 8;
pub const F_ROOT: u8 = 16;
/// A directory we list as an entry but never descend into: a mount point of a
/// filesystem that is not indexed, or a second view of one that already is.
pub const F_NOSCAN: u8 = 32;

pub struct DirInfo {
    /// Entry id of this directory, or `NONE` when the slot is free.
    pub entry: u32,
    pub fs: u16,
    pub h_len: u8,
    pub h_off: u32,
    /// Directory mtime when its listing was last known to be in sync, or 0 if
    /// it has never been listed. Startup uses it to skip unchanged folders.
    pub mtime_ns: i64,
    /// Child entry ids, ordered by folded name for lookup by name.
    pub children: Vec<u32>,
}

/// Entries a batch of changes added, removed or altered, so that only the
/// search sessions those entries could affect are told to refresh.
#[derive(Default)]
pub struct Touched {
    pub ids: Vec<u32>,
    pub overflow: bool,
}

impl Touched {
    const CAP: usize = 2048;
    pub fn push(&mut self, id: u32) {
        if self.ids.len() < Self::CAP {
            self.ids.push(id);
        } else {
            self.overflow = true;
        }
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && !self.overflow
    }
}

pub struct Index {
    /// `len + 1` offsets into `lower`; entry `i` owns
    /// `lower[name_off[i] .. name_off[i + 1] - 1]` (the last byte is a NUL).
    pub(crate) name_off: Vec<u32>,
    pub(crate) parent: Vec<u32>,
    /// File size in bytes. For a directory, its slot in `dirs` instead.
    pub(crate) size: Vec<u64>,
    pub(crate) mtime: Vec<u32>,
    pub(crate) flags: Vec<u8>,
    pub(crate) lower: Vec<u8>,
    pub(crate) orig: Vec<u8>,
    /// Parallel arrays, sorted by id: where each `F_ORIG` entry's original
    /// name starts in `orig`.
    pub(crate) orig_id: Vec<u32>,
    pub(crate) orig_off: Vec<u32>,
    pub(crate) dirs: Vec<DirInfo>,
    free_slots: Vec<u32>,
    pub(crate) handles: Vec<u8>,
    handle_map: HashTable<u32>,
    /// Ids below `sorted_upto` in name order. Newer entries are the unsorted
    /// "delta", folded in periodically by `merged_perm`/`install_perm`.
    pub(crate) perm: Vec<u32>,
    pub(crate) sorted_upto: u32,
    pub(crate) roots: Vec<u32>,
    live: u32,
    dead: u32,
    live_dirs: u32,
    /// Bumped when ids are renumbered; result sets from an older epoch are void.
    pub epoch: u64,
    /// Bumped whenever the directory tree changes shape.
    dir_gen: u64,
    rank_cache: Mutex<Option<(u64, Arc<Vec<u32>>)>>,
}

impl Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

fn handle_hash(fs: u16, h: &[u8]) -> u64 {
    hash_bytes(fs as u64 + 1, h)
}

impl Index {
    pub fn new() -> Index {
        Index {
            name_off: vec![0],
            parent: Vec::new(),
            size: Vec::new(),
            mtime: Vec::new(),
            flags: Vec::new(),
            lower: Vec::new(),
            orig: Vec::new(),
            orig_id: Vec::new(),
            orig_off: Vec::new(),
            dirs: Vec::new(),
            free_slots: Vec::new(),
            handles: Vec::new(),
            handle_map: HashTable::new(),
            perm: Vec::new(),
            sorted_upto: 0,
            roots: Vec::new(),
            live: 0,
            dead: 0,
            live_dirs: 0,
            epoch: 1,
            dir_gen: 0,
            rank_cache: Mutex::new(None),
        }
    }

    // ---- read access -------------------------------------------------

    /// Number of slots, tombstones included.
    #[inline]
    pub fn len(&self) -> u32 {
        self.parent.len() as u32
    }
    pub fn is_empty(&self) -> bool {
        self.parent.is_empty()
    }
    pub fn live_count(&self) -> u32 {
        self.live
    }
    pub fn dir_count(&self) -> u32 {
        self.live_dirs
    }
    pub fn dead_count(&self) -> u32 {
        self.dead
    }
    /// True while tombstones or freed directory slots are present; the index
    /// cannot be written to disk in that state.
    pub fn needs_compaction(&self) -> bool {
        self.dead > 0 || !self.free_slots.is_empty()
    }
    pub fn delta_len(&self) -> u32 {
        self.len() - self.sorted_upto
    }
    pub fn roots(&self) -> &[u32] {
        &self.roots
    }
    #[inline]
    pub fn flags(&self, i: u32) -> u8 {
        self.flags[i as usize]
    }
    #[inline]
    pub fn is_dir(&self, i: u32) -> bool {
        self.flags[i as usize] & F_DIR != 0
    }
    #[inline]
    pub fn is_dead(&self, i: u32) -> bool {
        self.flags[i as usize] & F_DEAD != 0
    }
    #[inline]
    pub fn parent(&self, i: u32) -> u32 {
        self.parent[i as usize]
    }
    /// Size in bytes; directories report 0.
    #[inline]
    pub fn file_size(&self, i: u32) -> u64 {
        if self.is_dir(i) { 0 } else { self.size[i as usize] }
    }
    #[inline]
    pub fn mtime(&self, i: u32) -> u32 {
        self.mtime[i as usize]
    }
    #[inline]
    pub fn lower_name(&self, i: u32) -> &[u8] {
        let a = self.name_off[i as usize] as usize;
        let b = self.name_off[i as usize + 1] as usize - 1;
        &self.lower[a..b]
    }
    pub fn orig_name(&self, i: u32) -> &[u8] {
        if self.flags[i as usize] & F_ORIG == 0 {
            return self.lower_name(i);
        }
        match self.orig_id.binary_search(&i) {
            Ok(k) => {
                let off = self.orig_off[k] as usize;
                let end = off + memchr::memchr(0, &self.orig[off..]).unwrap_or(0);
                &self.orig[off..end]
            }
            Err(_) => self.lower_name(i),
        }
    }
    /// Slot in `dirs` of a live directory entry.
    #[inline]
    pub fn dir_slot(&self, i: u32) -> usize {
        debug_assert!(self.is_dir(i));
        self.size[i as usize] as usize
    }
    pub fn dir(&self, slot: usize) -> &DirInfo {
        &self.dirs[slot]
    }
    pub fn dir_slots(&self) -> usize {
        self.dirs.len()
    }
    pub fn dir_handle(&self, slot: usize) -> &[u8] {
        let d = &self.dirs[slot];
        &self.handles[d.h_off as usize..d.h_off as usize + d.h_len as usize]
    }
    pub fn children(&self, dir: u32) -> &[u32] {
        &self.dirs[self.dir_slot(dir)].children
    }

    pub fn find_dir_by_handle(&self, fs: u16, handle: &[u8]) -> Option<usize> {
        if handle.is_empty() {
            return None;
        }
        let h = handle_hash(fs, handle);
        self.handle_map
            .find(h, |&s| {
                let d = &self.dirs[s as usize];
                d.fs == fs && &self.handles[d.h_off as usize..d.h_off as usize + d.h_len as usize] == handle
            })
            .map(|&s| s as usize)
    }

    pub fn child_by_name(&self, dir: u32, name: &[u8], lower: &[u8]) -> Option<u32> {
        let ch = &self.dirs[self.dir_slot(dir)].children;
        let start = ch.partition_point(|&c| self.lower_name(c) < lower);
        for &c in &ch[start..] {
            if self.lower_name(c) != lower {
                break;
            }
            if self.orig_name(c) == name {
                return Some(c);
            }
        }
        None
    }

    /// Absolute path of an entry. The root `/` is the empty string, so a
    /// child path is always `parent + "/" + name`.
    pub fn path_of(&self, id: u32) -> Vec<u8> {
        let mut chain = Vec::with_capacity(16);
        let mut e = id;
        while e != NONE {
            chain.push(e);
            e = self.parent[e as usize];
        }
        let mut out = Vec::with_capacity(chain.len() * 16);
        for (k, &e) in chain.iter().rev().enumerate() {
            if k > 0 {
                out.push(b'/');
            }
            out.extend_from_slice(self.orig_name(e));
        }
        out
    }

    /// The folded (or original, when `exact`) path of an entry, for matching.
    pub fn match_path_into(&self, id: u32, exact: bool, out: &mut Vec<u8>) {
        let mut chain = [0u32; 64];
        let mut n = 0;
        let mut e = id;
        while e != NONE && n < chain.len() {
            chain[n] = e;
            n += 1;
            e = self.parent[e as usize];
        }
        out.clear();
        for k in (0..n).rev() {
            if k != n - 1 {
                out.push(b'/');
            }
            let e = chain[k];
            out.extend_from_slice(if exact { self.orig_name(e) } else { self.lower_name(e) });
        }
    }

    pub fn resolve_path(&self, path: &[u8]) -> Option<u32> {
        let path = if path.len() > 1 && path.ends_with(b"/") { &path[..path.len() - 1] } else { path };
        let path = if path == b"/" { &b""[..] } else { path };
        let mut best: Option<(u32, usize)> = None;
        for &r in &self.roots {
            let rp = self.orig_name(r);
            let fits = path == rp || (path.starts_with(rp) && path.get(rp.len()) == Some(&b'/'));
            if fits && best.is_none_or(|(_, l)| rp.len() >= l) {
                best = Some((r, rp.len()));
            }
        }
        let (mut cur, plen) = best?;
        let mut lower = Vec::new();
        for comp in path[plen..].split(|&b| b == b'/').filter(|c| !c.is_empty()) {
            if !self.is_dir(cur) {
                return None;
            }
            lower.clear();
            lower_into(comp, &mut lower);
            cur = self.child_by_name(cur, comp, &lower)?;
        }
        Some(cur)
    }

    // ---- mutation ----------------------------------------------------

    fn push_entry(&mut self, parent: u32, name: &[u8], lower: &[u8], flags: u8, size: u64, mtime: u32) -> u32 {
        let id = self.parent.len() as u32;
        assert!(
            id < NONE - 1 && self.lower.len() + lower.len() + 1 < u32::MAX as usize && self.orig.len() + name.len() + 1 < u32::MAX as usize,
            "index is full: more names than a 32-bit offset can address"
        );
        self.lower.extend_from_slice(lower);
        self.lower.push(0);
        self.name_off.push(self.lower.len() as u32);
        let mut f = flags;
        if name != lower {
            f |= F_ORIG;
            self.orig_id.push(id);
            self.orig_off.push(self.orig.len() as u32);
            self.orig.extend_from_slice(name);
            self.orig.push(0);
        }
        self.parent.push(parent);
        self.size.push(size);
        self.mtime.push(mtime);
        self.flags.push(f);
        self.live += 1;
        id
    }

    fn insert_child_sorted(&mut self, dir: u32, child: u32) {
        let slot = self.dir_slot(dir);
        let pos = {
            let key = self.lower_name(child);
            self.dirs[slot].children.partition_point(|&c| self.lower_name(c) < key)
        };
        self.dirs[slot].children.insert(pos, child);
    }

    fn attach(&mut self, parent: u32, id: u32, sorted: bool) {
        if parent == NONE {
            return;
        }
        if sorted {
            self.insert_child_sorted(parent, id);
        } else {
            let slot = self.dir_slot(parent);
            self.dirs[slot].children.push(id);
        }
    }

    /// Add a file, symlink or other non-directory. With `sorted == false` the
    /// caller is bulk-loading a listing and must call `finish_children`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_file(&mut self, parent: u32, name: &[u8], lower: &[u8], flags: u8, size: u64, mtime: u32, sorted: bool) -> u32 {
        let id = self.push_entry(parent, name, lower, flags & !F_DIR, size, mtime);
        self.attach(parent, id, sorted);
        id
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_dir(&mut self, parent: u32, name: &[u8], lower: &[u8], flags: u8, mtime: u32, fs: u16, handle: &[u8], sorted: bool) -> u32 {
        let slot = match self.free_slots.pop() {
            Some(s) => s as usize,
            None => {
                self.dirs.push(DirInfo { entry: NONE, fs: 0, h_len: 0, h_off: 0, mtime_ns: 0, children: Vec::new() });
                self.dirs.len() - 1
            }
        };
        let id = self.push_entry(parent, name, lower, flags | F_DIR, slot as u64, mtime);
        // A handle identifies one directory. If it is somehow already taken,
        // leave this one unregistered rather than corrupt the lookup.
        let register = !handle.is_empty() && handle.len() <= u8::MAX as usize && self.find_dir_by_handle(fs, handle).is_none();
        let h_off = self.handles.len() as u32;
        if register {
            self.handles.extend_from_slice(handle);
        }
        self.dirs[slot] = DirInfo { entry: id, fs, h_len: if register { handle.len() as u8 } else { 0 }, h_off, mtime_ns: 0, children: Vec::new() };
        if register {
            let (dirs, handles) = (&self.dirs, &self.handles);
            let rehash = |&s: &u32| {
                let d = &dirs[s as usize];
                handle_hash(d.fs, &handles[d.h_off as usize..d.h_off as usize + d.h_len as usize])
            };
            self.handle_map.insert_unique(handle_hash(fs, handle), slot as u32, rehash);
        }
        self.live_dirs += 1;
        self.dir_gen += 1;
        self.attach(parent, id, sorted);
        id
    }

    pub fn add_root(&mut self, path: &[u8], fs: u16, handle: &[u8]) -> u32 {
        let name = if path == b"/" { &b""[..] } else { path.strip_suffix(b"/").unwrap_or(path) };
        let mut lower = Vec::new();
        lower_into(name, &mut lower);
        let id = self.add_dir(NONE, name, &lower, F_ROOT, 0, fs, handle, true);
        self.roots.push(id);
        id
    }

    /// Restore name order in a directory after bulk `add_*(.., sorted = false)`.
    pub fn finish_children(&mut self, dir: u32) {
        let slot = self.dir_slot(dir);
        let mut ch = std::mem::take(&mut self.dirs[slot].children);
        ch.sort_unstable_by(|&a, &b| self.lower_name(a).cmp(self.lower_name(b)));
        ch.shrink_to_fit();
        self.dirs[slot].children = ch;
    }

    fn detach_child(&mut self, parent: u32, id: u32) {
        let slot = self.dir_slot(parent);
        let start = {
            let key = self.lower_name(id);
            self.dirs[slot].children.partition_point(|&c| self.lower_name(c) < key)
        };
        let ch = &mut self.dirs[slot].children;
        let pos = ch[start..].iter().position(|&c| c == id).map(|p| p + start).or_else(|| ch.iter().position(|&c| c == id));
        if let Some(p) = pos {
            ch.remove(p);
        }
    }

    fn free_dir_slot(&mut self, slot: usize) {
        let (fs, h_off, h_len) = {
            let d = &self.dirs[slot];
            (d.fs, d.h_off as usize, d.h_len as usize)
        };
        if h_len > 0 {
            let hash = handle_hash(fs, &self.handles[h_off..h_off + h_len]);
            if let Ok(e) = self.handle_map.find_entry(hash, |&s| s as usize == slot) {
                e.remove();
            }
        }
        let d = &mut self.dirs[slot];
        d.entry = NONE;
        d.h_len = 0;
        d.mtime_ns = 0;
        d.children = Vec::new();
        self.free_slots.push(slot as u32);
        self.live_dirs -= 1;
        self.dir_gen += 1;
    }

    /// Tombstone an entry and everything beneath it.
    pub fn remove_subtree(&mut self, id: u32, touched: &mut Touched) {
        if self.is_dead(id) {
            return;
        }
        let p = self.parent[id as usize];
        if p != NONE && !self.is_dead(p) {
            self.detach_child(p, id);
        }
        let mut stack = vec![id];
        while let Some(e) = stack.pop() {
            let f = self.flags[e as usize];
            if f & F_DEAD != 0 {
                continue;
            }
            self.flags[e as usize] = f | F_DEAD;
            self.live -= 1;
            self.dead += 1;
            touched.push(e);
            if f & F_DIR != 0 {
                let slot = self.size[e as usize] as usize;
                let children = std::mem::take(&mut self.dirs[slot].children);
                stack.extend_from_slice(&children);
                self.free_dir_slot(slot);
            }
        }
    }

    /// Give a directory a new parent and name without touching its contents.
    /// Returns the new entry id; the old one becomes a tombstone.
    pub fn move_dir(&mut self, id: u32, new_parent: u32, name: &[u8], lower: &[u8]) -> u32 {
        debug_assert!(self.is_dir(id) && !self.is_dead(id));
        let old_parent = self.parent[id as usize];
        if old_parent != NONE && !self.is_dead(old_parent) {
            self.detach_child(old_parent, id);
        }
        let slot = self.size[id as usize] as usize;
        let keep = self.flags[id as usize] & (F_DIR | F_NOSCAN);
        let mtime = self.mtime[id as usize];
        let new_id = self.push_entry(new_parent, name, lower, keep, slot as u64, mtime);
        self.dirs[slot].entry = new_id;
        let children = std::mem::take(&mut self.dirs[slot].children);
        for &c in &children {
            self.parent[c as usize] = new_id;
        }
        self.dirs[slot].children = children;
        self.flags[id as usize] |= F_DEAD;
        self.live -= 1;
        self.dead += 1;
        self.dir_gen += 1;
        self.attach(new_parent, new_id, true);
        new_id
    }

    /// Update size and time; returns whether anything changed.
    pub fn set_meta(&mut self, id: u32, size: u64, mtime: u32) -> bool {
        let i = id as usize;
        let mut changed = false;
        if self.flags[i] & F_DIR == 0 && self.size[i] != size {
            self.size[i] = size;
            changed = true;
        }
        if self.mtime[i] != mtime {
            self.mtime[i] = mtime;
            changed = true;
        }
        changed
    }

    pub fn set_flag(&mut self, id: u32, flag: u8, on: bool) {
        if on {
            self.flags[id as usize] |= flag;
        } else {
            self.flags[id as usize] &= !flag;
        }
    }

    pub fn set_dir_mtime_ns(&mut self, slot: usize, mtime_ns: i64) {
        self.dirs[slot].mtime_ns = mtime_ns;
    }

    // ---- name order --------------------------------------------------

    #[inline]
    pub fn cmp_name(&self, a: u32, b: u32) -> Ordering {
        self.lower_name(a).cmp(self.lower_name(b)).then(a.cmp(&b))
    }

    /// Sort ids by name. Large inputs sort on an 8-byte name prefix first so
    /// most comparisons never leave the key array.
    pub fn sort_by_name(&self, ids: &mut [u32]) {
        if ids.len() < 20_000 {
            ids.sort_unstable_by(|&a, &b| self.cmp_name(a, b));
            return;
        }
        let mut keyed: Vec<(u64, u32)> = ids
            .par_iter()
            .map(|&i| {
                let n = self.lower_name(i);
                let mut k = [0u8; 8];
                let l = n.len().min(8);
                k[..l].copy_from_slice(&n[..l]);
                (u64::from_be_bytes(k), i)
            })
            .collect();
        keyed.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| self.cmp_name(a.1, b.1)));
        ids.par_iter_mut().zip(keyed.par_iter()).for_each(|(o, k)| *o = k.1);
    }

    /// Merge two name-ordered id lists. Each `b` element finds its place in
    /// `a` by binary search, so the cost is `|b| log |a|` comparisons plus a
    /// linear copy, not `|a|` cache-missing comparisons.
    pub fn merge_sorted(&self, a: &[u32], b: &[u32]) -> Vec<u32> {
        if b.is_empty() {
            return a.to_vec();
        }
        if a.is_empty() {
            return b.to_vec();
        }
        let pos: Vec<usize> = b.par_iter().map(|&x| a.partition_point(|&m| self.cmp_name(m, x) == Ordering::Less)).collect();
        let mut out = Vec::with_capacity(a.len() + b.len());
        let mut prev = 0;
        for (j, &p) in pos.iter().enumerate() {
            out.extend_from_slice(&a[prev..p]);
            out.push(b[j]);
            prev = p;
        }
        out.extend_from_slice(&a[prev..]);
        out
    }

    /// Compute the name order covering every live entry. Read-only, so the
    /// indexer can do the work without blocking queries, then `install_perm`.
    pub fn merged_perm(&self) -> (Vec<u32>, u32) {
        let upto = self.len();
        let mut delta: Vec<u32> = (self.sorted_upto..upto).into_par_iter().filter(|&i| !self.is_dead(i)).collect();
        self.sort_by_name(&mut delta);
        let main: Vec<u32> = self.perm.par_iter().copied().filter(|&i| !self.is_dead(i)).collect();
        (self.merge_sorted(&main, &delta), upto)
    }

    pub fn install_perm(&mut self, perm: Vec<u32>, upto: u32) {
        self.perm = perm;
        self.sorted_upto = upto;
    }

    /// Rank of every directory in tree order, indexed by slot, for sorting
    /// results by path. Cached until the tree changes shape.
    pub fn dir_ranks(&self) -> Arc<Vec<u32>> {
        let mut cache = self.rank_cache.lock();
        if let Some((gen, r)) = cache.as_ref() {
            if *gen == self.dir_gen {
                return r.clone();
            }
        }
        let mut rank = vec![0u32; self.dirs.len()];
        let mut next = 0u32;
        let mut roots = self.roots.clone();
        roots.sort_unstable_by(|&a, &b| self.cmp_name(a, b));
        let mut stack: Vec<u32> = roots.into_iter().rev().collect();
        while let Some(d) = stack.pop() {
            let slot = self.dir_slot(d);
            rank[slot] = next;
            next += 1;
            for &c in self.dirs[slot].children.iter().rev() {
                if self.flags[c as usize] & F_DIR != 0 {
                    stack.push(c);
                }
            }
        }
        let r = Arc::new(rank);
        *cache = Some((self.dir_gen, r.clone()));
        r
    }

    // ---- maintenance -------------------------------------------------

    /// Drop tombstones and renumber. Every outstanding entry id becomes
    /// invalid, which `epoch` signals to sessions.
    pub fn compact(&mut self) {
        let n = self.len() as usize;
        let mut remap = vec![NONE; n];
        let mut next = 0u32;
        for (i, r) in remap.iter_mut().enumerate() {
            if self.flags[i] & F_DEAD == 0 {
                *r = next;
                next += 1;
            }
        }
        let m = next as usize;

        let mut slot_remap = vec![NONE; self.dirs.len()];
        let mut dirs = Vec::with_capacity(self.live_dirs as usize);
        let mut handles = Vec::with_capacity(self.handles.len());
        for (s, mut d) in std::mem::take(&mut self.dirs).into_iter().enumerate() {
            if d.entry == NONE {
                continue;
            }
            slot_remap[s] = dirs.len() as u32;
            let h = &self.handles[d.h_off as usize..d.h_off as usize + d.h_len as usize];
            d.h_off = handles.len() as u32;
            handles.extend_from_slice(h);
            d.entry = remap[d.entry as usize];
            for c in d.children.iter_mut() {
                *c = remap[*c as usize];
            }
            d.children.shrink_to_fit();
            dirs.push(d);
        }

        let mut name_off = Vec::with_capacity(m + 1);
        name_off.push(0u32);
        let mut lower = Vec::with_capacity(self.lower.len());
        let mut orig = Vec::with_capacity(self.orig.len());
        let mut orig_id = Vec::with_capacity(self.orig_id.len());
        let mut orig_off = Vec::with_capacity(self.orig_off.len());
        let mut parent = Vec::with_capacity(m);
        let mut size = Vec::with_capacity(m);
        let mut mtime = Vec::with_capacity(m);
        let mut flags = Vec::with_capacity(m);
        let mut ok = 0usize;
        for i in 0..n {
            let f = self.flags[i];
            let has_orig = f & F_ORIG != 0;
            let oi = ok;
            if has_orig {
                ok += 1;
            }
            if f & F_DEAD != 0 {
                continue;
            }
            lower.extend_from_slice(&self.lower[self.name_off[i] as usize..self.name_off[i + 1] as usize]);
            name_off.push(lower.len() as u32);
            if has_orig {
                let off = self.orig_off[oi] as usize;
                let end = off + memchr::memchr(0, &self.orig[off..]).unwrap_or(0) + 1;
                orig_id.push(remap[i]);
                orig_off.push(orig.len() as u32);
                orig.extend_from_slice(&self.orig[off..end]);
            }
            let p = self.parent[i];
            parent.push(if p == NONE { NONE } else { remap[p as usize] });
            size.push(if f & F_DIR != 0 { slot_remap[self.size[i] as usize] as u64 } else { self.size[i] });
            mtime.push(self.mtime[i]);
            flags.push(f);
        }
        lower.shrink_to_fit();
        orig.shrink_to_fit();
        handles.shrink_to_fit();

        let upto = self.sorted_upto as usize;
        let new_upto = (upto..n).find(|&i| remap[i] != NONE).map(|i| remap[i]).unwrap_or(m as u32);
        let perm: Vec<u32> = self.perm.iter().map(|&i| remap[i as usize]).filter(|&r| r != NONE).collect();
        let roots = self.roots.iter().map(|&r| remap[r as usize]).collect();

        self.name_off = name_off;
        self.lower = lower;
        self.orig = orig;
        self.orig_id = orig_id;
        self.orig_off = orig_off;
        self.parent = parent;
        self.size = size;
        self.mtime = mtime;
        self.flags = flags;
        self.dirs = dirs;
        self.handles = handles;
        self.perm = perm;
        self.sorted_upto = new_upto;
        self.roots = roots;
        self.free_slots = Vec::new();
        self.dead = 0;
        self.epoch += 1;
        self.dir_gen += 1;
        self.rebuild_handle_map();
    }

    fn rebuild_handle_map(&mut self) {
        let mut map = HashTable::with_capacity(self.dirs.len());
        let (dirs, handles) = (&self.dirs, &self.handles);
        let hash_of = |s: u32| {
            let d = &dirs[s as usize];
            handle_hash(d.fs, &handles[d.h_off as usize..d.h_off as usize + d.h_len as usize])
        };
        for (s, d) in dirs.iter().enumerate() {
            if d.entry != NONE && d.h_len > 0 {
                map.insert_unique(hash_of(s as u32), s as u32, |&x| hash_of(x));
            }
        }
        self.handle_map = map;
    }

    /// Rebuild everything that is not persisted (child lists, the handle
    /// lookup, counters) after the raw arrays were loaded from disk.
    pub fn rebuild_derived(&mut self) {
        let n = self.len() as usize;
        let mut counts = vec![0u32; self.dirs.len()];
        let (mut live, mut live_dirs) = (0u32, 0u32);
        for i in 0..n {
            live += 1;
            if self.flags[i] & F_DIR != 0 {
                live_dirs += 1;
            }
            let p = self.parent[i];
            if p != NONE {
                counts[self.size[p as usize] as usize] += 1;
            }
        }
        for (d, &c) in self.dirs.iter_mut().zip(&counts) {
            d.children = Vec::with_capacity(c as usize);
        }
        for i in 0..n {
            let p = self.parent[i];
            if p != NONE {
                let slot = self.size[p as usize] as usize;
                self.dirs[slot].children.push(i as u32);
            }
        }
        let mut dirs = std::mem::take(&mut self.dirs);
        dirs.par_iter_mut().for_each(|d| d.children.sort_unstable_by(|&a, &b| self.lower_name(a).cmp(self.lower_name(b))));
        self.dirs = dirs;
        self.live = live;
        self.live_dirs = live_dirs;
        self.dead = 0;
        self.free_slots = Vec::new();
        self.rebuild_handle_map();
    }

    /// Approximate memory the index occupies, in bytes.
    pub fn mem_bytes(&self) -> u64 {
        let n = self.parent.len() as u64;
        let children: u64 = self.dirs.iter().map(|d| d.children.len() as u64 * 4).sum();
        n * (4 + 4 + 8 + 4 + 1)
            + self.lower.len() as u64
            + self.orig.len() as u64
            + self.orig_id.len() as u64 * 8
            + self.dirs.len() as u64 * std::mem::size_of::<DirInfo>() as u64
            + children
            + self.handles.len() as u64
            + self.handle_map.capacity() as u64 * 5
            + self.perm.len() as u64 * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::to_lower;

    fn file(idx: &mut Index, parent: u32, name: &str) -> u32 {
        idx.add_file(parent, name.as_bytes(), &to_lower(name.as_bytes()), 0, 10, 100, true)
    }
    fn dir(idx: &mut Index, parent: u32, name: &str, handle: &[u8]) -> u32 {
        idx.add_dir(parent, name.as_bytes(), &to_lower(name.as_bytes()), 0, 100, 0, handle, true)
    }
    fn path(idx: &Index, id: u32) -> String {
        String::from_utf8(idx.path_of(id)).unwrap()
    }

    #[test]
    fn paths_and_lookup() {
        let mut idx = Index::new();
        let root = idx.add_root(b"/", 0, b"r");
        let home = dir(&mut idx, root, "Home", b"h");
        let f = file(&mut idx, home, "Notes.TXT");
        assert_eq!(path(&idx, f), "/Home/Notes.TXT");
        assert_eq!(idx.lower_name(f), b"notes.txt");
        assert_eq!(idx.orig_name(f), b"Notes.TXT");
        assert_eq!(idx.child_by_name(home, b"Notes.TXT", b"notes.txt"), Some(f));
        // Same folded name, different case: a distinct file on Linux.
        assert_eq!(idx.child_by_name(home, b"notes.txt", b"notes.txt"), None);
        assert_eq!(idx.resolve_path(b"/Home/Notes.TXT"), Some(f));
        assert_eq!(idx.resolve_path(b"/Home/"), Some(home));
        assert_eq!(idx.resolve_path(b"/"), Some(root));
        assert_eq!(idx.resolve_path(b"/nope"), None);
        assert_eq!(idx.find_dir_by_handle(0, b"h"), Some(idx.dir_slot(home)));
    }

    #[test]
    fn remove_and_move() {
        let mut idx = Index::new();
        let root = idx.add_root(b"/", 0, b"r");
        let a = dir(&mut idx, root, "a", b"ha");
        let b = dir(&mut idx, root, "b", b"hb");
        let sub = dir(&mut idx, a, "sub", b"hs");
        let f = file(&mut idx, sub, "x.txt");
        assert_eq!(idx.live_count(), 5);

        // Moving a folder keeps its contents and their ids.
        let sub2 = idx.move_dir(sub, b, b"moved", b"moved");
        assert!(idx.is_dead(sub) && !idx.is_dead(sub2));
        assert_eq!(path(&idx, f), "/b/moved/x.txt");
        assert_eq!(idx.children(a), &[] as &[u32]);
        assert_eq!(idx.find_dir_by_handle(0, b"hs"), Some(idx.dir_slot(sub2)));
        assert_eq!(idx.live_count(), 5);

        let mut t = Touched::default();
        idx.remove_subtree(b, &mut t);
        assert_eq!(idx.live_count(), 2);
        assert!(idx.is_dead(f));
        assert_eq!(idx.find_dir_by_handle(0, b"hs"), None);
        assert_eq!(t.ids.len(), 3);
    }

    #[test]
    fn name_order_and_compaction() {
        let mut idx = Index::new();
        let root = idx.add_root(b"/", 0, b"r");
        let d = dir(&mut idx, root, "d", b"hd");
        let names = ["pear", "Apple", "fig", "banana"];
        let ids: Vec<u32> = names.iter().map(|n| file(&mut idx, d, n)).collect();
        let (perm, upto) = idx.merged_perm();
        idx.install_perm(perm, upto);
        let order = |idx: &Index| -> Vec<String> {
            idx.perm.iter().filter(|&&i| !idx.is_dead(i) && !idx.is_dir(i)).map(|&i| String::from_utf8(idx.orig_name(i).to_vec()).unwrap()).collect()
        };
        assert_eq!(order(&idx), ["Apple", "banana", "fig", "pear"]);

        // New entries land in the delta and merge in on the next pass.
        file(&mut idx, d, "cherry");
        let mut t = Touched::default();
        idx.remove_subtree(ids[2], &mut t);
        let (perm, upto) = idx.merged_perm();
        idx.install_perm(perm, upto);
        assert_eq!(order(&idx), ["Apple", "banana", "cherry", "pear"]);

        let epoch = idx.epoch;
        idx.compact();
        assert_eq!(idx.epoch, epoch + 1);
        assert_eq!(idx.dead_count(), 0);
        assert_eq!(idx.len(), idx.live_count());
        assert_eq!(order(&idx), ["Apple", "banana", "cherry", "pear"]);
        let apple = idx.resolve_path(b"/d/Apple").unwrap();
        assert_eq!(idx.orig_name(apple), b"Apple");
        assert_eq!(idx.find_dir_by_handle(0, b"hd"), Some(idx.dir_slot(idx.resolve_path(b"/d").unwrap())));
    }
}
