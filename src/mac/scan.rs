//! Reading folders off the disk.
//!
//! `getattrlistbulk` returns the names of a folder's entries together with
//! their type, size, time and file id, many entries per call, which is far
//! quicker than listing a folder and then asking about each entry.

use std::ffi::c_void;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use crate::util::cstr;

pub const KIND_FILE: u8 = 0;
pub const KIND_DIR: u8 = 1;
pub const KIND_LINK: u8 = 2;

const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

pub struct Child {
    /// Byte range of the name in [`Listing::names`].
    pub name: (u32, u32),
    pub kind: u8,
    pub size: u64,
    pub mtime: i64,
    /// Device and file id: what identifies a folder wherever it moves.
    pub dev: u32,
    pub ino: u64,
}

pub struct Listing {
    pub dev: u32,
    pub ino: u64,
    pub mtime: i64,
    pub names: Vec<u8>,
    pub children: Vec<Child>,
}

impl Listing {
    pub fn name(&self, c: &Child) -> &[u8] {
        &self.names[c.name.0 as usize..c.name.1 as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListError {
    /// It is not there, or no longer a folder.
    Gone,
    /// It is there but cannot be read (permissions, or privacy protection).
    Denied,
}

/// A folder's identity in the index's handle lookup.
pub fn handle(dev: u32, ino: u64) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[..4].copy_from_slice(&dev.to_le_bytes());
    h[4..].copy_from_slice(&ino.to_le_bytes());
    h
}

fn rd<T: Copy>(buf: &[u8], at: usize) -> Option<T> {
    let end = at.checked_add(std::mem::size_of::<T>())?;
    if end > buf.len() {
        return None;
    }
    Some(unsafe { std::ptr::read_unaligned(buf.as_ptr().add(at) as *const T) })
}

fn open_dir(path: &[u8]) -> Result<OwnedFd, ListError> {
    let c = cstr(if path.is_empty() { b"/" } else { path });
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        let e = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return Err(if matches!(e, libc::ENOENT | libc::ENOTDIR | libc::ELOOP) { ListError::Gone } else { ListError::Denied });
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

thread_local! {
    static BUF: std::cell::RefCell<Vec<u64>> = std::cell::RefCell::new(vec![0u64; 32 * 1024]);
}

/// List folder `path` (index form: the root `/` is empty).
pub fn list_dir(path: &[u8]) -> Result<Listing, ListError> {
    let fd = open_dir(path)?;
    let raw = fd.as_raw_fd();
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(raw, &mut st) } != 0 {
        return Err(ListError::Gone);
    }
    let mut l = Listing { dev: st.st_dev as u32, ino: st.st_ino, mtime: st.st_mtime, names: Vec::new(), children: Vec::new() };

    let mut al: libc::attrlist = unsafe { std::mem::zeroed() };
    al.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    al.commonattr = libc::ATTR_CMN_RETURNED_ATTRS
        | ATTR_CMN_ERROR
        | libc::ATTR_CMN_NAME
        | libc::ATTR_CMN_DEVID
        | libc::ATTR_CMN_OBJTYPE
        | libc::ATTR_CMN_MODTIME
        | libc::ATTR_CMN_FILEID;
    al.fileattr = libc::ATTR_FILE_DATALENGTH;

    BUF.with(|cell| {
        let mut words = cell.borrow_mut();
        let size = words.len() * 8;
        loop {
            let n = unsafe {
                libc::getattrlistbulk(raw, &mut al as *mut _ as *mut c_void, words.as_mut_ptr() as *mut c_void, size, libc::FSOPT_PACK_INVAL_ATTRS as u64)
            };
            if n < 0 {
                // Unreadable from the start (privacy protection refuses here,
                // not when the folder is opened); otherwise keep what we have.
                return if l.children.is_empty() { Err(ListError::Denied) } else { Ok(()) };
            }
            if n == 0 {
                return Ok(());
            }
            let buf: &[u8] = unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, size) };
            let mut o = 0usize;
            for _ in 0..n {
                let Some(len) = rd::<u32>(buf, o) else { break };
                let len = len as usize;
                if len < 4 || o + len > buf.len() {
                    break;
                }
                let e = &buf[o..o + len];
                o += len;
                parse_entry(e, &mut l);
            }
        }
    })?;
    Ok(l)
}

/// One entry: length, returned attributes, then the attributes asked for in
/// bit order (the error first), all present because of PACK_INVAL_ATTRS.
fn parse_entry(e: &[u8], l: &mut Listing) {
    let Some(returned) = rd::<u32>(e, 4) else { return };
    let mut p = 4 + 20;
    let error: u32 = rd(e, p).unwrap_or(1);
    p += 4;
    let name_at = p;
    let (Some(off), Some(nlen)) = (rd::<i32>(e, p), rd::<u32>(e, p + 4)) else { return };
    p += 8;
    let dev: u32 = rd(e, p).unwrap_or(0);
    p += 4;
    let objtype: u32 = rd(e, p).unwrap_or(0);
    p += 4;
    let mtime: i64 = rd(e, p).unwrap_or(0);
    p += 16;
    let ino: u64 = rd(e, p).unwrap_or(0);
    p += 8;
    let size: i64 = rd(e, p).unwrap_or(0);
    if returned & libc::ATTR_CMN_NAME == 0 || returned & libc::ATTR_CMN_OBJTYPE == 0 {
        return;
    }
    let start = name_at as i64 + off as i64;
    let end = start + nlen as i64;
    if start < 0 || end > e.len() as i64 || nlen == 0 {
        return;
    }
    let mut name = &e[start as usize..end as usize];
    if let Some(z) = memchr::memchr(0, name) {
        name = &name[..z];
    }
    if name.is_empty() || name == b"." || name == b".." {
        return;
    }
    let kind = match objtype {
        VDIR => KIND_DIR,
        VLNK => KIND_LINK,
        _ => KIND_FILE,
    };
    // An entry the system could not describe still has a name and a type.
    let ok = error == 0;
    let n0 = l.names.len() as u32;
    l.names.extend_from_slice(name);
    l.children.push(Child {
        name: (n0, l.names.len() as u32),
        kind,
        size: if kind != KIND_DIR && ok { size.max(0) as u64 } else { 0 },
        mtime: if ok { mtime } else { 0 },
        dev,
        ino,
    });
}

/// Size, time and kind of one path, without following a final symlink.
pub fn stat(path: &[u8]) -> Option<(u8, u64, i64, u32, u64)> {
    let c = cstr(path);
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::lstat(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFDIR => KIND_DIR,
        libc::S_IFLNK => KIND_LINK,
        _ => KIND_FILE,
    };
    let size = if kind == KIND_DIR { 0 } else { st.st_size.max(0) as u64 };
    Some((kind, size, st.st_mtime, st.st_dev as u32, st.st_ino))
}

/// The path with every name spelled as the disk stores it (case, and
/// composed or decomposed accents), as macOS's realpath gives it.
pub fn on_disk(path: &[u8]) -> Option<Vec<u8>> {
    let c = cstr(if path.is_empty() { b"/" } else { path });
    let r = unsafe { libc::realpath(c.as_ptr(), std::ptr::null_mut()) };
    if r.is_null() {
        return None;
    }
    let out = unsafe { std::ffi::CStr::from_ptr(r) }.to_bytes().to_vec();
    unsafe { libc::free(r as *mut c_void) };
    Some(out)
}

extern "C" {
    fn setiopolicy_np(iotype: i32, scope: i32, policy: i32) -> i32;
}

/// Let this thread's disk reads give way to everything else.
#[allow(dead_code)]
pub fn set_low_io_priority() {
    const IOPOL_TYPE_DISK: i32 = 0;
    const IOPOL_SCOPE_THREAD: i32 = 1;
    const IOPOL_UTILITY: i32 = 4;
    unsafe { setiopolicy_np(IOPOL_TYPE_DISK, IOPOL_SCOPE_THREAD, IOPOL_UTILITY) };
}
