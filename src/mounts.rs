//! Which filesystems are indexed, and how each one is watched.

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::config::Settings;
use crate::sys;
use crate::util::{cstr, lossy};
use crate::watch::Watcher;
use crate::{info, warn};

pub struct MountInfo {
    pub id: i32,
    pub mount_point: Vec<u8>,
    pub fstype: String,
}

fn unescape(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push(((b[i + 1] - b'0') << 6) | ((b[i + 2] - b'0') << 3) | (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// Parse `/proc/self/mountinfo`.
pub fn read_mounts() -> Vec<MountInfo> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mut v = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else { continue };
        let l: Vec<&str> = left.split(' ').collect();
        let fstype = right.split(' ').next().unwrap_or("");
        if l.len() < 5 {
            continue;
        }
        let Ok(id) = l[0].parse::<i32>() else { continue };
        let mut mp = unescape(l[4]);
        if mp.len() > 1 && mp.ends_with(b"/") {
            mp.pop();
        }
        v.push(MountInfo { id, mount_point: mp, fstype: fstype.to_string() });
    }
    v
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WatchMode {
    /// One mark covers the whole filesystem.
    Filesystem,
    /// Each directory is marked as it is scanned.
    PerDir,
    /// Not watched; the index only changes on a rescan.
    Off,
}

#[derive(Clone)]
pub struct FsEntry {
    pub dev: u64,
    pub fsid: [i32; 2],
    pub fstype: String,
    pub mount_point: Vec<u8>,
    pub watch: WatchMode,
    /// The filesystem was unmounted. The slot stays, because directories in
    /// the index refer to filesystems by position, but nothing matches it.
    pub retired: bool,
}

/// What the database remembers about a filesystem, to recognise it again.
#[derive(Serialize, Deserialize, Clone)]
pub struct SavedFs {
    pub fsid: [i32; 2],
    pub fstype: String,
    pub mount_point: Vec<u8>,
    #[serde(default)]
    pub retired: bool,
}

pub struct FsRegistry {
    entries: RwLock<Vec<FsEntry>>,
    mounts: Mutex<Vec<MountInfo>>,
    settings: Arc<Settings>,
    watcher: Option<Arc<Watcher>>,
}

impl FsRegistry {
    pub fn new(settings: Arc<Settings>, watcher: Option<Arc<Watcher>>) -> FsRegistry {
        FsRegistry { entries: RwLock::new(Vec::new()), mounts: Mutex::new(read_mounts()), settings, watcher }
    }

    fn fstype_of(&self, mount_id: i32, path: &[u8]) -> Option<String> {
        let find = |m: &Vec<MountInfo>| -> Option<String> {
            if mount_id >= 0 {
                if let Some(mi) = m.iter().find(|mi| mi.id == mount_id) {
                    return Some(mi.fstype.clone());
                }
            }
            // Later lines shadow earlier ones mounted at the same place.
            m.iter().rev().find(|mi| mi.mount_point == path || (path.is_empty() && mi.mount_point == b"/")).map(|mi| mi.fstype.clone())
        };
        let mut m = self.mounts.lock();
        if let Some(t) = find(&m) {
            return Some(t);
        }
        *m = read_mounts();
        find(&m)
    }

    fn add(&self, entries: &mut Vec<FsEntry>, path: &[u8], dev: u64, fstype: String) -> Option<u16> {
        if entries.len() >= u16::MAX as usize {
            return None;
        }
        let cpath = cstr(if path.is_empty() { b"/" } else { path });
        let fsid = sys::fsid_of(&cpath).unwrap_or([0, 0]);
        let watch = match &self.watcher {
            None => WatchMode::Off,
            Some(w) => match w.mark_filesystem(&cpath) {
                Ok(()) => WatchMode::Filesystem,
                Err(e) => {
                    if w.privileged {
                        warn!("{}: cannot watch the whole {fstype} filesystem ({e}); watching each folder instead", lossy(cpath.to_bytes()));
                    }
                    WatchMode::PerDir
                }
            },
        };
        info!("indexing {} ({fstype}, {})", lossy(cpath.to_bytes()), match watch {
            WatchMode::Filesystem => "live, filesystem-wide",
            WatchMode::PerDir => "live, per-folder",
            WatchMode::Off => "not watched",
        });
        entries.push(FsEntry { dev, fsid, fstype, mount_point: path.to_vec(), watch, retired: false });
        Some((entries.len() - 1) as u16)
    }

    /// Register the filesystem a configured root sits on. Roots are indexed
    /// whatever their type: naming one is asking for it.
    pub fn register_root(&self, path: &[u8], dev: u64, mount_id: i32) -> Option<u16> {
        let mut e = self.entries.write();
        if let Some(i) = e.iter().position(|f| f.dev == dev && !f.retired) {
            return Some(i as u16);
        }
        let fstype = self.fstype_of(mount_id, path).unwrap_or_else(|| "unknown".into());
        self.add(&mut e, path, dev, fstype)
    }

    /// Decide what to do with a directory that is the root of another mount.
    /// `Some(fs)` means descend and index it as filesystem `fs`; `None` means
    /// show the mount point but stay out (virtual or network filesystem, or a
    /// second view of one that is already indexed elsewhere).
    pub fn resolve_mount(&self, path: &[u8], dev: u64, mount_id: i32) -> Option<u16> {
        let cpath = cstr(if path.is_empty() { b"/" } else { path });
        let fsid = sys::fsid_of(&cpath).unwrap_or([0, 0]);
        let mut e = self.entries.write();
        if let Some(i) = e.iter().position(|f| f.dev == dev && !f.retired) {
            if e[i].mount_point == path && e[i].fsid == fsid {
                // Known, and mounted where we know it. It may have been
                // unmounted and mounted again in between, which silently
                // drops the watch, so (re)place it; doing so twice is free.
                if e[i].watch == WatchMode::Filesystem {
                    if let Some(w) = &self.watcher {
                        let _ = w.mark_filesystem(&cpath);
                    }
                }
                return Some(i as u16);
            }
            // The device number is known from somewhere else. If that mount
            // is still there, this is a second view of a filesystem that is
            // already indexed. If not, it went away (and the number may now
            // belong to a different disk): retire it and start afresh.
            let old = cstr(if e[i].mount_point.is_empty() { b"/" } else { &e[i].mount_point });
            let still_there = e[i].fsid == fsid && sys::open_dir(&old, true).and_then(|fd| sys::fstat(sys::fd_raw(&fd))).map(|st| st.dev == dev).unwrap_or(false);
            if still_there {
                return None;
            }
            e[i].retired = true;
        }
        let fstype = self.fstype_of(mount_id, path)?;
        if !self.settings.fs_allowed(&fstype) && !self.settings.is_root(path) {
            return None;
        }
        self.add(&mut e, path, dev, fstype)
    }

    pub fn by_fsid(&self, fsid: [i32; 2]) -> Option<u16> {
        self.entries.read().iter().position(|f| f.fsid == fsid && !f.retired && f.watch != WatchMode::Off).map(|i| i as u16)
    }

    pub fn watch_mode(&self, fs: u16) -> WatchMode {
        self.entries.read().get(fs as usize).map(|f| f.watch).unwrap_or(WatchMode::Off)
    }

    pub fn count(&self) -> usize {
        self.entries.read().len()
    }

    pub fn snapshot(&self) -> Vec<FsEntry> {
        self.entries.read().clone()
    }

    pub fn saved(&self) -> Vec<SavedFs> {
        self.entries.read().iter().map(|f| SavedFs { fsid: f.fsid, fstype: f.fstype.clone(), mount_point: f.mount_point.clone(), retired: f.retired }).collect()
    }

    pub fn clear(&self) {
        self.entries.write().clear();
    }

    /// Re-adopt the filesystems a saved index refers to, in the same order so
    /// its filesystem numbers stay valid. Fails if any of them is no longer
    /// mounted where it was, in which case the index must be rebuilt.
    pub fn restore(&self, saved: &[SavedFs]) -> bool {
        let mut e = self.entries.write();
        e.clear();
        for s in saved {
            if s.retired {
                e.push(FsEntry { dev: u64::MAX, fsid: s.fsid, fstype: s.fstype.clone(), mount_point: s.mount_point.clone(), watch: WatchMode::Off, retired: true });
                continue;
            }
            let cpath = cstr(if s.mount_point.is_empty() { b"/" } else { &s.mount_point });
            let same = sys::fsid_of(&cpath).map(|f| f == s.fsid).unwrap_or(false);
            let dev = sys::open_dir(&cpath, true).and_then(|fd| sys::fstat(sys::fd_raw(&fd))).map(|st| st.dev);
            let (true, Ok(dev)) = (same, dev) else {
                info!("{} is not the filesystem the saved index describes", lossy(cpath.to_bytes()));
                e.clear();
                return false;
            };
            if e.iter().any(|f| f.dev == dev && !f.retired) || self.add(&mut e, &s.mount_point, dev, s.fstype.clone()).is_none() {
                e.clear();
                return false;
            }
        }
        true
    }
}
