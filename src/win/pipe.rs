//! The named pipe clients connect to, `\\.\pipe\trawlrd`.
//!
//! Each connection is read on its own thread while the indexer may write a
//! change notice to it at any moment. A pipe opened for ordinary (blocking)
//! use cannot be read and written at the same time, so it is opened for
//! overlapped use and each call waits for its own completion.

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::Duration;

use windows_sys::Win32::Foundation::{LocalFree, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, WAIT_OBJECT_0};
use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use super::{security, wide, Handle};
use crate::indexer::Shared;
use crate::server::{self, Access};
use crate::{info, warn};

/// Who may open the pipe at all: the system and administrators fully, and
/// any signed-in user to read and write, but not to create further
/// instances of it, which would let a program pose as the service.
/// (0x12019b is read and write access minus FILE_CREATE_PIPE_INSTANCE.)
/// Whether a user may search is then decided per connection.
const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;AU)";

struct Conn {
    h: Handle,
}

/// An event for one overlapped call.
fn event() -> io::Result<Handle> {
    Handle::new(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })
}

impl Conn {
    /// Wait for an overlapped call to finish, up to `timeout`; on timeout
    /// the call is cancelled.
    fn finish(&self, started: i32, ov: &mut OVERLAPPED, ev: &Handle, timeout: Option<Duration>) -> io::Result<usize> {
        if started == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(e);
            }
            let ms = timeout.map(|t| t.as_millis().min(u32::MAX as u128 - 1) as u32).unwrap_or(INFINITE);
            if unsafe { WaitForSingleObject(ev.raw(), ms) } != WAIT_OBJECT_0 {
                unsafe { CancelIoEx(self.h.raw(), ov) };
                let mut n = 0u32;
                unsafe { GetOverlappedResult(self.h.raw(), ov, &mut n, 1) };
                return Err(io::ErrorKind::TimedOut.into());
            }
        }
        let mut n = 0u32;
        if unsafe { GetOverlappedResult(self.h.raw(), ov, &mut n, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let ev = event()?;
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = ev.raw();
        let started = unsafe { ReadFile(self.h.raw(), buf.as_mut_ptr(), buf.len().min(1 << 20) as u32, std::ptr::null_mut(), &mut ov) };
        match self.finish(started, &mut ov, &ev, None) {
            Ok(n) => Ok(n),
            // The client hung up: the end of the conversation.
            Err(e) if e.raw_os_error() == Some(109) => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let ev = event()?;
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = ev.raw();
        let started = unsafe { WriteFile(self.h.raw(), buf.as_ptr(), buf.len().min(1 << 20) as u32, std::ptr::null_mut(), &mut ov) };
        // A client that stops reading must not hold up whoever is writing
        // to it (the indexer, sending change notices).
        self.finish(started, &mut ov, &ev, Some(Duration::from_secs(10)))
    }
}

struct Reader(Arc<Conn>);
struct Writer(Arc<Conn>);

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A pipe instance waiting for a client.
pub struct Listener {
    first: Option<Handle>,
}

fn create(first: bool) -> io::Result<Handle> {
    let sddl = wide(PIPE_SDDL);
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
    let name = wide(crate::config::SYSTEM_SOCKET);
    let open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            &sa,
        )
    };
    unsafe { LocalFree(sd as _) };
    Handle::new(h)
}

/// Claim the pipe name. Fails if another program already has it, which is
/// either a running trawlrd or something pretending to be one.
pub fn bind() -> io::Result<Listener> {
    let h = create(true).map_err(|e| {
        if e.raw_os_error() == Some(5) || e.raw_os_error() == Some(231) {
            io::Error::new(io::ErrorKind::AddrInUse, format!("another program already serves {} (is trawlrd already running?)", crate::config::SYSTEM_SOCKET))
        } else {
            e
        }
    })?;
    info!("listening on {}", crate::config::SYSTEM_SOCKET);
    Ok(Listener { first: Some(h) })
}

fn connect(h: &Handle) -> io::Result<()> {
    let ev = event()?;
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = ev.raw();
    if unsafe { ConnectNamedPipe(h.raw(), &mut ov) } != 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        // The client got in between creating the instance and this call.
        Some(c) if c == ERROR_PIPE_CONNECTED as i32 => Ok(()),
        Some(c) if c == ERROR_IO_PENDING as i32 => {
            let mut n = 0u32;
            if unsafe { GetOverlappedResult(h.raw(), &ov, &mut n, 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        _ => Err(e),
    }
}

/// Accept clients for as long as the service runs, each on its own thread.
pub fn serve(sh: Arc<Shared>, mut listener: Listener) {
    // As on Linux, whoever runs the daemon may search it.
    let me = security::own_sid().unwrap_or_default();
    loop {
        let h = match listener.first.take().map(Ok).unwrap_or_else(|| create(false)) {
            Ok(h) => h,
            Err(e) => {
                warn!("cannot open another pipe instance: {e}");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if let Err(e) = connect(&h) {
            warn!("pipe connection failed: {e}");
            continue;
        }
        let conn = Arc::new(Conn { h });
        let sh = sh.clone();
        let me = me.clone();
        let _ = std::thread::Builder::new().name("client".into()).spawn(move || {
            let check_conn = conn.clone();
            let settings = sh.settings.clone();
            let access = Access::AfterFirstRequest(Box::new(move || match security::client_identity(check_conn.h.raw()) {
                Some((user, groups)) => user == me || settings.may_search_sids(&user, &groups),
                None => false,
            }));
            server::session(sh, Reader(conn.clone()), Box::new(Writer(conn)), access);
        });
    }
}
