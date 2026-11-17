//! Mounted volumes, and which of them are indexed.
//!
//! The startup disk is two volumes: the sealed, read-only system volume at
//! `/` and the data volume, mounted at `/System/Volumes/Data` and woven into
//! `/` by firmlinks (`/Users`, `/Applications`, `/Library`, ...). Walking `/`
//! already reaches everything on both, so the mounts under
//! `/System/Volumes` are never entered. External disks appear under
//! `/Volumes`.

use crate::config::Settings;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// Index form: `/` is empty.
    pub path: Vec<u8>,
    pub fstype: String,
    pub flags: u32,
}

impl Mount {
    /// Whether the volume's contents belong in the index.
    pub fn indexed(&self, settings: &Settings) -> bool {
        if self.path.is_empty() {
            return true;
        }
        if self.path.starts_with(b"/System/Volumes/") {
            return false;
        }
        if self.flags & libc::MNT_SNAPSHOT as u32 != 0 && !settings.cfg.index_snapshots {
            return false;
        }
        // Volumes the system keeps out of sight (Finder does not show
        // them): software assets, recovery and the like.
        if self.flags & libc::MNT_DONTBROWSE as u32 != 0 && !self.path.starts_with(b"/Volumes/") {
            return false;
        }
        if settings.fs_explicit(&self.fstype) {
            return true;
        }
        self.flags & libc::MNT_LOCAL as u32 != 0 && settings.fs_allowed(&self.fstype)
    }
}

fn text(c: &[libc::c_char]) -> &[u8] {
    let bytes = unsafe { std::slice::from_raw_parts(c.as_ptr() as *const u8, c.len()) };
    let n = memchr::memchr(0, bytes).unwrap_or(bytes.len());
    &bytes[..n]
}

/// Everything mounted now.
pub fn mounts() -> Vec<Mount> {
    let mut p: *mut libc::statfs = std::ptr::null_mut();
    let n = unsafe { libc::getmntinfo(&mut p, libc::MNT_NOWAIT) };
    if n <= 0 || p.is_null() {
        return Vec::new();
    }
    let list = unsafe { std::slice::from_raw_parts(p, n as usize) };
    list.iter()
        .map(|s| {
            let on = text(&s.f_mntonname);
            Mount { path: if on == b"/" { Vec::new() } else { on.to_vec() }, fstype: String::from_utf8_lossy(text(&s.f_fstypename)).into_owned(), flags: s.f_flags }
        })
        .collect()
}

/// The device a path is on.
pub fn dev_of(path: &[u8]) -> Option<i32> {
    let c = crate::util::cstr(if path.is_empty() { b"/" } else { path });
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::stat(c.as_ptr(), &mut st) } == 0).then_some(st.st_dev)
}

/// A name for the volumes holding `roots`, as FSEvents identifies them.
pub fn identity(roots: &[Vec<u8>]) -> String {
    let mut ids = Vec::new();
    let mut paths: Vec<&[u8]> = roots.iter().map(|r| r.as_slice()).collect();
    // The startup disk's files are on its data volume.
    if roots.iter().any(|r| r.is_empty()) {
        paths.push(b"/System/Volumes/Data");
    }
    for p in paths {
        if let Some(dev) = dev_of(p) {
            ids.push(super::fsevents::volume_uuid(dev).unwrap_or_else(|| format!("dev{dev}")));
        }
    }
    ids.join(",")
}

pub fn lossy_path(p: &[u8]) -> String {
    if p.is_empty() { "/".into() } else { String::from_utf8_lossy(p).into_owned() }
}
