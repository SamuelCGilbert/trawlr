//! Thin wrappers over the Linux syscalls the indexer lives on.

use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

pub const MAX_HANDLE: usize = 128;

pub const AT_EMPTY_PATH: i32 = 0x1000;
pub const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
pub const AT_NO_AUTOMOUNT: i32 = 0x800;
pub const AT_STATX_DONT_SYNC: i32 = 0x4000;

#[repr(C)]
struct FileHandleBuf {
    handle_bytes: u32,
    handle_type: i32,
    f_handle: [u8; MAX_HANDLE],
}

pub fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Append the kernel file handle of `name` (relative to `dirfd`) to `out` as
/// `type (4 bytes LE) + opaque bytes`, returning the mount id it lives on.
///
/// This is the same handle fanotify reports for a directory, which is how an
/// event is matched to an indexed directory without resolving any path.
pub fn name_to_handle(dirfd: RawFd, name: &CStr, flags: i32, out: &mut Vec<u8>) -> io::Result<i32> {
    let mut fh = FileHandleBuf { handle_bytes: MAX_HANDLE as u32, handle_type: 0, f_handle: [0; MAX_HANDLE] };
    let mut mount_id: libc::c_int = -1;
    let r = unsafe {
        libc::syscall(
            libc::SYS_name_to_handle_at,
            dirfd,
            name.as_ptr(),
            &mut fh as *mut FileHandleBuf,
            &mut mount_id as *mut libc::c_int,
            flags,
        )
    };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    let n = (fh.handle_bytes as usize).min(MAX_HANDLE);
    out.extend_from_slice(&fh.handle_type.to_le_bytes());
    out.extend_from_slice(&fh.f_handle[..n]);
    Ok(mount_id)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stat {
    pub mode: u32,
    pub size: u64,
    pub mtime_s: i64,
    pub mtime_nsec: u32,
    pub ino: u64,
    /// `major << 32 | minor`
    pub dev: u64,
}

impl Stat {
    pub fn is_dir(&self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFDIR
    }
    pub fn is_symlink(&self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFLNK
    }
    pub fn mtime_ns(&self) -> i64 {
        self.mtime_s.saturating_mul(1_000_000_000).saturating_add(self.mtime_nsec as i64)
    }
}

pub fn statx_at(dirfd: RawFd, name: &CStr, flags: i32) -> io::Result<Stat> {
    let mut sx: libc::statx = unsafe { std::mem::zeroed() };
    let mask = libc::STATX_TYPE | libc::STATX_SIZE | libc::STATX_MTIME | libc::STATX_INO;
    let r = unsafe { libc::statx(dirfd, name.as_ptr(), flags | AT_NO_AUTOMOUNT | AT_STATX_DONT_SYNC, mask, &mut sx) };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Stat {
        mode: sx.stx_mode as u32,
        size: sx.stx_size,
        mtime_s: sx.stx_mtime.tv_sec,
        mtime_nsec: sx.stx_mtime.tv_nsec,
        ino: sx.stx_ino,
        dev: ((sx.stx_dev_major as u64) << 32) | sx.stx_dev_minor as u64,
    })
}

pub fn fstat(fd: RawFd) -> io::Result<Stat> {
    statx_at(fd, c"", AT_EMPTY_PATH)
}

/// Open a directory without following a final symlink. `path_only` yields an
/// `O_PATH` descriptor, enough to stat children against and far cheaper.
pub fn open_dir(path: &CStr, path_only: bool) -> io::Result<OwnedFd> {
    let base = libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    if path_only {
        let fd = unsafe { libc::open(path.as_ptr(), base | libc::O_PATH) };
        return if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd) }) };
    }
    // O_NOATIME keeps a scan from dirtying every directory inode on disk. It
    // is only permitted on files we own (or as root), so fall back without it.
    let fd = unsafe { libc::open(path.as_ptr(), base | libc::O_RDONLY | libc::O_NOATIME) };
    if fd >= 0 {
        return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
    }
    if errno() != libc::EPERM {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { libc::open(path.as_ptr(), base | libc::O_RDONLY) };
    if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd) }) }
}

pub fn getdents(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let r = unsafe { libc::syscall(libc::SYS_getdents64, fd, buf.as_mut_ptr(), buf.len()) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r as usize) }
}

/// The filesystem id fanotify stamps on every event from this filesystem.
pub fn fsid_of(path: &CStr) -> io::Result<[i32; 2]> {
    let mut sf: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut sf) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // `fsid_t` is two ints with no public accessor in libc.
    Ok(unsafe { std::mem::transmute::<libc::fsid_t, [i32; 2]>(sf.f_fsid) })
}

pub fn fd_raw(fd: &OwnedFd) -> RawFd {
    fd.as_raw_fd()
}

/// Lower this thread's disk priority so background indexing yields to
/// interactive work.
pub fn set_low_io_priority() {
    const IOPRIO_WHO_PROCESS: libc::c_int = 1;
    const IOPRIO_CLASS_BE: libc::c_int = 2;
    let prio = (IOPRIO_CLASS_BE << 13) | 7;
    unsafe { libc::syscall(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, 0, prio) };
}

pub fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Hand freed heap memory back to the kernel. A scan allocates and frees a
/// great many short-lived buffers across many threads, and glibc otherwise
/// keeps that memory around for a process that will mostly sit idle.
pub fn release_free_memory() {
    #[cfg(target_env = "gnu")]
    unsafe {
        libc::malloc_trim(0);
    }
}
