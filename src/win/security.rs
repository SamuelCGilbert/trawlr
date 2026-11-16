//! Who may search: Windows accounts, groups and the identity of a client.
//!
//! The index lists every file name on the computer, other users' included,
//! so the service only answers the users and groups the configuration
//! names. A client is identified by briefly impersonating it on its pipe
//! connection and reading its access token: the user it runs as and every
//! group it belongs to.

use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, IsValidSid, LookupAccountNameW, RevertToSelf, TokenGroups, TokenUser, PSID, SID_NAME_USE, TOKEN_GROUPS, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Pipes::ImpersonateNamedPipeClient;
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

use super::{wide, Handle};
use crate::warn;

const SE_GROUP_ENABLED: u32 = 0x4;
const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x10;

/// S-1-5-18, the account services such as this one run as.
const LOCAL_SYSTEM: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];

pub fn is_system(sid: &[u8]) -> bool {
    sid == LOCAL_SYSTEM
}

fn sid_bytes(sid: PSID) -> Option<Vec<u8>> {
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return None;
    }
    let len = unsafe { GetLengthSid(sid) } as usize;
    Some(unsafe { std::slice::from_raw_parts(sid as *const u8, len) }.to_vec())
}

fn sid_from_string(s: &str) -> Option<Vec<u8>> {
    let w = wide(s);
    let mut sid: PSID = std::ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(w.as_ptr(), &mut sid) } == 0 {
        return None;
    }
    let out = sid_bytes(sid);
    unsafe { LocalFree(sid as _) };
    out
}

fn sid_of_account(name: &str) -> Option<Vec<u8>> {
    let w = wide(name);
    let mut sid = vec![0u8; 256];
    let mut sid_len = sid.len() as u32;
    let mut domain = vec![0u16; 256];
    let mut domain_len = domain.len() as u32;
    let mut kind: SID_NAME_USE = 0;
    let ok = unsafe { LookupAccountNameW(std::ptr::null(), w.as_ptr(), sid.as_mut_ptr().cast(), &mut sid_len, domain.as_mut_ptr(), &mut domain_len, &mut kind) };
    if ok == 0 {
        return None;
    }
    sid_bytes(sid.as_mut_ptr().cast())
}

/// Resolve names from the configuration to security identifiers. A name
/// may be an account (`"sam"`, `"PC\\sam"`), a group, or a SID written out
/// (`"S-1-5-32-544"`). Built-in groups are found by their English names
/// even on Windows set up in another language.
pub fn resolve_accounts<'a>(users: impl Iterator<Item = &'a String>, groups: impl Iterator<Item = &'a String>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for (name, is_group) in users.map(|u| (u, false)).chain(groups.map(|g| (g, true))) {
        let sid = if name.to_ascii_uppercase().starts_with("S-1-") {
            sid_from_string(name)
        } else {
            sid_of_account(name).or_else(|| match name.to_ascii_lowercase().as_str() {
                "administrators" => sid_from_string("S-1-5-32-544"),
                "users" => sid_from_string("S-1-5-32-545"),
                _ => None,
            })
        };
        match sid {
            Some(s) => out.push(s),
            // A group that does not exist here is simply not used, as on
            // Linux; a user that does not exist is probably a typo.
            None if !is_group => warn!("allowed_users: unknown user {name:?}"),
            None => {}
        }
    }
    out
}

fn token_info(token: &Handle, class: i32) -> Option<Vec<u64>> {
    let mut need = 0u32;
    unsafe { GetTokenInformation(token.raw(), class, std::ptr::null_mut(), 0, &mut need) };
    if need == 0 || need > 1 << 20 {
        return None;
    }
    let mut buf = vec![0u64; (need as usize).div_ceil(8)];
    let ok = unsafe { GetTokenInformation(token.raw(), class, buf.as_mut_ptr().cast(), need, &mut need) };
    (ok != 0).then_some(buf)
}

/// Make `dir` (created if need be) readable by the system and
/// administrators only, and everything in it, whatever it held before. The
/// service keeps the index there, and the index names every file on the
/// computer: a user who may not search must not be able to read it either.
pub fn lock_down_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
    std::fs::create_dir_all(dir)?;
    // Full control for SYSTEM and Administrators, inherited by everything
    // inside; nothing for anyone else; nothing inherited from ProgramData.
    let sddl = wide("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let (mut present, mut defaulted) = (0, 0);
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let got = unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) };
    let path = wide(dir);
    let r = if got == 0 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(1) as u32
    } else {
        unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            )
        }
    };
    unsafe { LocalFree(sd as _) };
    if r != 0 { Err(std::io::Error::from_raw_os_error(r as i32)) } else { Ok(()) }
}

/// The user this program runs as.
pub fn own_sid() -> Option<Vec<u8>> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let token = Handle(token);
    let user = token_info(&token, TokenUser)?;
    let user = unsafe { &*(user.as_ptr() as *const TOKEN_USER) };
    sid_bytes(user.User.Sid)
}

/// The user a pipe client runs as and the groups it belongs to. Groups the
/// token holds only for denying access count too: they are how Windows
/// marks an administrator's groups while the user has not elevated, and
/// such a user may still search.
pub fn client_identity(pipe: HANDLE) -> Option<(Vec<u8>, Vec<Vec<u8>>)> {
    if unsafe { ImpersonateNamedPipeClient(pipe) } == 0 {
        return None;
    }
    let mut token: HANDLE = std::ptr::null_mut();
    let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) };
    // Carrying on under the client's identity would be a hole: if the
    // thread cannot go back to its own, stop the whole service.
    if unsafe { RevertToSelf() } == 0 {
        std::process::abort();
    }
    if opened == 0 {
        return None;
    }
    let token = Handle(token);

    let user = token_info(&token, TokenUser)?;
    let user = unsafe { &*(user.as_ptr() as *const TOKEN_USER) };
    let user_sid = sid_bytes(user.User.Sid)?;

    let mut groups = Vec::new();
    if let Some(buf) = token_info(&token, TokenGroups) {
        let tg = unsafe { &*(buf.as_ptr() as *const TOKEN_GROUPS) };
        let room = (buf.len() * 8).saturating_sub(std::mem::offset_of!(TOKEN_GROUPS, Groups)) / std::mem::size_of_val(&tg.Groups[0]);
        if tg.GroupCount as usize > room {
            return None;
        }
        let list =unsafe { std::slice::from_raw_parts(tg.Groups.as_ptr(), tg.GroupCount as usize) };
        for g in list {
            if g.Attributes & (SE_GROUP_ENABLED | SE_GROUP_USE_FOR_DENY_ONLY) != 0 {
                if let Some(s) = sid_bytes(g.Sid) {
                    groups.push(s);
                }
            }
        }
    }
    Some((user_sid, groups))
}
