//! Live change notification through fanotify.
//!
//! With `CAP_SYS_ADMIN` one mark covers a whole filesystem, so every create,
//! delete and rename on the disk is reported no matter how many directories
//! there are. This is the Linux counterpart of the NTFS change journal that
//! voidtools' Everything reads. Without the capability the kernel only allows a mark per
//! directory, which works the same way but is capped by
//! `fs.fanotify.max_user_marks`.
//!
//! Events are requested in "directory handle + name" form: each one says
//! *which directory* (as a kernel file handle) and *which name* changed. The
//! indexer looks the handle up in its own table, so no path is ever resolved
//! from an event.

use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const FAN_CLOEXEC: u32 = 0x1;
const FAN_NONBLOCK: u32 = 0x2;
const FAN_UNLIMITED_QUEUE: u32 = 0x10;
const FAN_UNLIMITED_MARKS: u32 = 0x20;
const FAN_REPORT_DFID_NAME: u32 = 0x400 | 0x800;

const FAN_MARK_ADD: u32 = 0x1;
const FAN_MARK_ONLYDIR: u32 = 0x8;
const FAN_MARK_FILESYSTEM: u32 = 0x100;

const FAN_ATTRIB: u64 = 0x4;
const FAN_CLOSE_WRITE: u64 = 0x8;
const FAN_MOVED_FROM: u64 = 0x40;
const FAN_MOVED_TO: u64 = 0x80;
const FAN_CREATE: u64 = 0x100;
const FAN_DELETE: u64 = 0x200;
const FAN_Q_OVERFLOW: u64 = 0x4000;
const FAN_RENAME: u64 = 0x1000_0000;
const FAN_ONDIR: u64 = 0x4000_0000;
const FAN_EVENT_ON_CHILD: u64 = 0x0800_0000;

const INFO_DFID_NAME: u8 = 2;
const INFO_OLD_DFID_NAME: u8 = 10;
const INFO_NEW_DFID_NAME: u8 = 12;

/// "Entry `name` in the directory with this handle may have changed."
pub struct Change {
    pub fsid: [i32; 2],
    /// `type (4 bytes LE) + opaque bytes`, as `sys::name_to_handle` produces.
    pub handle: Box<[u8]>,
    pub name: Box<[u8]>,
}

pub struct Watcher {
    fd: OwnedFd,
    wake_r: OwnedFd,
    wake_w: OwnedFd,
    /// Whether filesystem-wide marks are available.
    pub privileged: bool,
    mask: u64,
    stop: AtomicBool,
    pub mark_failures: AtomicU64,
    pub dir_marks: AtomicU64,
}

impl Watcher {
    pub fn new() -> io::Result<Watcher> {
        let base = FAN_CLOEXEC | FAN_NONBLOCK | FAN_REPORT_DFID_NAME;
        let mut privileged = true;
        let mut fd = unsafe { libc::fanotify_init(base | FAN_UNLIMITED_QUEUE | FAN_UNLIMITED_MARKS, libc::O_RDONLY as u32) };
        if fd < 0 {
            privileged = false;
            fd = unsafe { libc::fanotify_init(base, libc::O_RDONLY as u32) };
        }
        if fd < 0 {
            let e = io::Error::last_os_error();
            return Err(io::Error::new(e.kind(), format!("fanotify_init: {e} (Linux 5.9 or newer is required; 5.13 without root)")));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut pipe = [0; 2];
        if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let (wake_r, wake_w) = unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
        let mut w = Watcher {
            fd,
            wake_r,
            wake_w,
            privileged,
            mask: FAN_CREATE | FAN_DELETE | FAN_RENAME | FAN_CLOSE_WRITE | FAN_ATTRIB | FAN_ONDIR,
            stop: AtomicBool::new(false),
            mark_failures: AtomicU64::new(0),
            dir_marks: AtomicU64::new(0),
        };
        // FAN_RENAME (Linux 5.17) reports both ends of a rename in one event.
        // Older kernels need the pair of MOVED_FROM / MOVED_TO instead. Probe
        // on a directory that always exists.
        if w.mark(FAN_MARK_ADD | FAN_MARK_ONLYDIR, w.mask | FAN_EVENT_ON_CHILD, libc::AT_FDCWD, Some(c"/")).is_err() {
            w.mask = (w.mask & !FAN_RENAME) | FAN_MOVED_FROM | FAN_MOVED_TO;
        }
        // Drop the probe mark again; real marks are added by the indexer.
        unsafe {
            libc::fanotify_mark(w.fd.as_raw_fd(), 0x2 /* FAN_MARK_REMOVE */, w.mask | FAN_EVENT_ON_CHILD, libc::AT_FDCWD, c"/".as_ptr());
        }
        Ok(w)
    }

    fn mark(&self, flags: u32, mask: u64, dirfd: RawFd, path: Option<&CStr>) -> io::Result<()> {
        let p = path.map(|p| p.as_ptr()).unwrap_or(std::ptr::null());
        let r = unsafe { libc::fanotify_mark(self.fd.as_raw_fd(), flags, mask, dirfd, p) };
        if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// Watch every directory on the filesystem containing `path`.
    pub fn mark_filesystem(&self, path: &CStr) -> io::Result<()> {
        if !self.privileged {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        self.mark(FAN_MARK_ADD | FAN_MARK_FILESYSTEM, self.mask, libc::AT_FDCWD, Some(path))
    }

    /// Watch one directory (and events on its direct children).
    pub fn mark_dir(&self, dirfd: RawFd) -> bool {
        match self.mark(FAN_MARK_ADD | FAN_MARK_ONLYDIR, self.mask | FAN_EVENT_ON_CHILD, dirfd, None) {
            Ok(()) => {
                self.dir_marks.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(_) => {
                self.mark_failures.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        unsafe { libc::write(self.wake_w.as_raw_fd(), b"x".as_ptr() as *const libc::c_void, 1) };
    }

    /// Block until events arrive and parse them into `out`. Returns
    /// `Ok(false)` once `stop` was called. `overflow` is set when the kernel
    /// dropped events and the index must be reconciled against the disk.
    pub fn read(&self, buf: &mut [u8], out: &mut Vec<Change>, overflow: &mut bool) -> io::Result<bool> {
        let mut fds = [
            libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.wake_r.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Ok(false);
            }
            let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if r < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if fds[1].revents != 0 {
                return Ok(false);
            }
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) {
                    continue;
                }
                return Err(e);
            }
            parse(&buf[..n as usize], out, overflow);
            return Ok(true);
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> usize {
    u16::from_ne_bytes([b[o], b[o + 1]]) as usize
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_ne_bytes(b[o..o + 4].try_into().unwrap())
}

/// Walk a buffer of `fanotify_event_metadata` records, each followed by its
/// info records (`header, fsid, file_handle, name`).
fn parse(buf: &[u8], out: &mut Vec<Change>, overflow: &mut bool) {
    const META: usize = 24;
    let mut o = 0;
    while o + META <= buf.len() {
        let event_len = u32_at(buf, o) as usize;
        let meta_len = u16_at(buf, o + 6);
        if event_len < META || o + event_len > buf.len() {
            break;
        }
        let mask = u64::from_ne_bytes(buf[o + 8..o + 16].try_into().unwrap());
        // In handle-reporting mode the kernel passes no open descriptor, but
        // close one defensively if it ever does.
        let fd = i32::from_ne_bytes(buf[o + 16..o + 20].try_into().unwrap());
        if fd >= 0 {
            unsafe { libc::close(fd) };
        }
        if mask & FAN_Q_OVERFLOW != 0 {
            *overflow = true;
        }
        let end = o + event_len;
        let mut q = o + meta_len.max(META);
        let (mut old, mut new, mut plain) = (None, None, None);
        while q + 4 <= end {
            let info_type = buf[q];
            let len = u16_at(buf, q + 2);
            if len < 4 || q + len > end {
                break;
            }
            // header(4) fsid(8) handle_bytes(4) handle_type(4) f_handle[..] name\0
            if matches!(info_type, INFO_DFID_NAME | INFO_OLD_DFID_NAME | INFO_NEW_DFID_NAME) && len >= 20 {
                let fsid = [u32_at(buf, q + 4) as i32, u32_at(buf, q + 8) as i32];
                let hbytes = u32_at(buf, q + 12) as usize;
                let hstart = q + 20;
                if hstart + hbytes <= q + len {
                    let mut handle = Vec::with_capacity(4 + hbytes);
                    handle.extend_from_slice(&buf[q + 16..q + 20]);
                    handle.extend_from_slice(&buf[hstart..hstart + hbytes]);
                    let rest = &buf[hstart + hbytes..q + len];
                    let name = &rest[..memchr::memchr(0, rest).unwrap_or(rest.len())];
                    let c = Change { fsid, handle: handle.into(), name: name.into() };
                    match info_type {
                        INFO_OLD_DFID_NAME => old = Some(c),
                        INFO_NEW_DFID_NAME => new = Some(c),
                        _ => plain = Some(c),
                    }
                }
            }
            q += len;
        }
        // Destination before source: when a folder was renamed, seeing it at
        // its new place first lets the index move the subtree instead of
        // dropping and rescanning it.
        out.extend(new);
        out.extend(old);
        out.extend(plain);
        o = end;
    }
}
