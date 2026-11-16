//! The Windows side of trawlrd.
//!
//! Instead of walking folders, the index of an NTFS drive is read straight
//! from its master file table, which lists every file and folder with its
//! parent, size and time; a drive with a million files takes seconds. Live
//! updates come from the drive's change journal (the USN journal), a log
//! NTFS keeps of every change. Both need administrator rights, so trawlrd
//! runs as a Windows service and serves searches over a named pipe.

pub mod indexer;
pub mod ntfs;
pub mod pipe;
pub mod security;
pub mod service;
pub mod usn;
pub mod volumes;

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// A NUL-terminated UTF-16 string for the Windows API.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

/// A handle that is closed when dropped.
pub struct Handle(pub HANDLE);

// A kernel handle may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Handle {
    /// Take ownership of a handle a Windows function returned, turning the
    /// failure values into the error behind them.
    pub fn new(h: HANDLE) -> io::Result<Handle> {
        if h.is_null() || h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(Handle(h)) }
    }
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// The bytes of a plain-data structure, for passing it to `DeviceIoControl`.
pub fn bytes_of<T: Copy>(v: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v as *const T as *const u8, std::mem::size_of::<T>()) }
}

/// Send a control code to a device or volume and wait for the answer.
/// Returns how many bytes of `out` were filled.
pub fn ioctl(h: &Handle, code: u32, input: &[u8], out: &mut [u8]) -> io::Result<usize> {
    let mut got = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h.raw(),
            code,
            if input.is_empty() { std::ptr::null() } else { input.as_ptr().cast() },
            input.len() as u32,
            out.as_mut_ptr().cast(),
            out.len() as u32,
            &mut got,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(got as usize) }
}

pub fn u16_at(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).unwrap_or(0)
}
pub fn u32_at(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
pub fn u64_at(b: &[u8], o: usize) -> u64 {
    b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).unwrap_or(0)
}
pub fn i64_at(b: &[u8], o: usize) -> i64 {
    u64_at(b, o) as i64
}

/// Windows file times count 100 ns steps since 1601; the index keeps
/// seconds since 1970.
pub fn filetime_to_unix(ft: i64) -> i64 {
    ft / 10_000_000 - 11_644_473_600
}

/// A name as Windows stores it (UTF-16) in the UTF-8 the index keeps.
pub fn name_from_utf16(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}
