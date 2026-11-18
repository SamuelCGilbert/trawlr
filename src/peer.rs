//! Who is on the other end of a connection to the daemon.
//!
//! The daemon uses it to decide whether a client may search. Clients use it
//! the other way round: before believing what a socket says, they make sure
//! the real trawlrd answered and not a program another user started in its
//! place (a socket path in a shared folder can be claimed by anyone first).

#[cfg(unix)]
use std::os::fd::RawFd;

/// The user a Unix socket's peer runs as.
#[cfg(target_os = "macos")]
pub fn peer_uid(fd: RawFd) -> Option<u32> {
    let (mut uid, mut gid) = (0, 0);
    let r = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    (r == 0).then_some(uid)
}

/// The user a Unix socket's peer runs as.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn peer_uid(fd: RawFd) -> Option<u32> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    (r == 0).then_some(cred.uid)
}

/// A daemon is to be believed if it runs as root (the system service) or as
/// the same user as this program (a per-user daemon).
#[cfg(unix)]
pub fn check_daemon(fd: RawFd) -> Result<(), String> {
    match peer_uid(fd) {
        Some(0) => Ok(()),
        Some(u) if u == unsafe { libc::geteuid() } => Ok(()),
        Some(u) => Err(format!("the program answering on the trawlrd socket runs as user {u}, not as root or you; not trusting it")),
        None => Err("cannot tell who is answering on the trawlrd socket".into()),
    }
}

/// The Windows service runs in session 0, where no signed-in user's
/// programs run: a pipe served from any other session is not the service.
#[cfg(windows)]
pub fn check_daemon(pipe: &impl std::os::windows::io::AsRawHandle) -> Result<(), String> {
    let mut session = u32::MAX;
    if unsafe { windows_sys::Win32::System::Pipes::GetNamedPipeServerSessionId(pipe.as_raw_handle() as _, &mut session) } == 0 {
        return Err("cannot tell who is serving the trawlrd pipe".into());
    }
    if session != 0 {
        return Err(format!("the trawlrd pipe is served from session {session}, not by the Trawlr service; not trusting it"));
    }
    Ok(())
}
