//! The NTFS change journal (USN journal).
//!
//! NTFS appends a record to a drive's journal for every change: a file
//! created, deleted, renamed, written to. Each record names the file (by
//! its reference number), the folder it is in and its name. Records are
//! numbered by their position (the USN), so after a restart the index picks
//! up exactly where it stopped, as long as the journal still holds that far
//! back. Records are only read here; what changed is then looked up in the
//! MFT, so a record is a hint about which file to look at, never trusted as
//! a description of what happened.

use std::io;

use windows_sys::Win32::System::Ioctl::{FSCTL_CREATE_USN_JOURNAL, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0};

use super::{bytes_of, i64_at, ioctl, name_from_utf16, u16_at, u32_at, u64_at, Handle};

pub const USN_REASON_DATA_OVERWRITE: u32 = 0x1;
pub const USN_REASON_DATA_EXTEND: u32 = 0x2;
pub const USN_REASON_DATA_TRUNCATION: u32 = 0x4;
pub const USN_REASON_FILE_CREATE: u32 = 0x100;
pub const USN_REASON_FILE_DELETE: u32 = 0x200;
pub const USN_REASON_RENAME_OLD_NAME: u32 = 0x1000;
pub const USN_REASON_RENAME_NEW_NAME: u32 = 0x2000;
pub const USN_REASON_BASIC_INFO_CHANGE: u32 = 0x8000;
pub const USN_REASON_HARD_LINK_CHANGE: u32 = 0x10000;
pub const USN_REASON_REPARSE_POINT_CHANGE: u32 = 0x100000;
pub const USN_REASON_CLOSE: u32 = 0x8000_0000;

/// The changes that can alter a name, a location, a size or a time.
/// Security, encryption, compression and similar changes are of no interest.
const REASONS: u32 = USN_REASON_DATA_OVERWRITE
    | USN_REASON_DATA_EXTEND
    | USN_REASON_DATA_TRUNCATION
    | USN_REASON_FILE_CREATE
    | USN_REASON_FILE_DELETE
    | USN_REASON_RENAME_OLD_NAME
    | USN_REASON_RENAME_NEW_NAME
    | USN_REASON_BASIC_INFO_CHANGE
    | USN_REASON_HARD_LINK_CHANGE
    | USN_REASON_REPARSE_POINT_CHANGE
    | USN_REASON_CLOSE;

const ERROR_JOURNAL_DELETE_IN_PROGRESS: i32 = 1178;
const ERROR_JOURNAL_NOT_ACTIVE: i32 = 1179;
const ERROR_JOURNAL_ENTRY_DELETED: i32 = 1181;

#[derive(Clone, Copy, Debug)]
pub struct Journal {
    pub id: u64,
    /// The oldest record still in the journal.
    pub first: i64,
    /// Where the next record will be written.
    pub next: i64,
}

/// The drive's journal, or `None` if it has none.
pub fn query(h: &Handle) -> io::Result<Option<Journal>> {
    let mut out = [0u8; 80];
    match ioctl(h, FSCTL_QUERY_USN_JOURNAL, &[], &mut out) {
        Ok(_) => Ok(Some(Journal { id: u64_at(&out, 0), first: i64_at(&out, 8), next: i64_at(&out, 16) })),
        Err(e) if matches!(e.raw_os_error(), Some(ERROR_JOURNAL_NOT_ACTIVE) | Some(ERROR_JOURNAL_DELETE_IN_PROGRESS)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Make sure the drive keeps a journal. Windows keeps one on the system
/// drive; other drives may need one created, as Windows Search and backup
/// tools also do. An existing journal is left as it is.
pub fn ensure(h: &Handle) -> io::Result<Journal> {
    if let Some(j) = query(h)? {
        return Ok(j);
    }
    // CREATE_USN_JOURNAL_DATA: maximum size, then allocation step.
    let mut input = [0u8; 16];
    input[..8].copy_from_slice(&(64u64 << 20).to_le_bytes());
    input[8..].copy_from_slice(&(8u64 << 20).to_le_bytes());
    ioctl(h, FSCTL_CREATE_USN_JOURNAL, &input, &mut [])?;
    query(h)?.ok_or_else(|| io::Error::other("the change journal could not be started"))
}

pub struct Change {
    pub frn: u64,
    pub parent: u64,
    pub name: String,
    pub reason: u32,
}

pub enum ReadError {
    /// The journal no longer reaches back to where reading should resume,
    /// or it was deleted or replaced: changes were lost.
    Lost,
    Io(io::Error),
}

/// Read the records from `start` on, without waiting for new ones. Returns
/// the position to continue from.
pub fn read(h: &Handle, journal: u64, start: i64, buf: &mut [u8], out: &mut Vec<Change>) -> Result<i64, ReadError> {
    let input = READ_USN_JOURNAL_DATA_V0 { StartUsn: start, ReasonMask: REASONS, ReturnOnlyOnClose: 0, Timeout: 0, BytesToWaitFor: 0, UsnJournalID: journal };
    let got = match ioctl(h, FSCTL_READ_USN_JOURNAL, bytes_of(&input), buf) {
        Ok(n) => n,
        Err(e) if matches!(e.raw_os_error(), Some(ERROR_JOURNAL_ENTRY_DELETED) | Some(ERROR_JOURNAL_NOT_ACTIVE) | Some(ERROR_JOURNAL_DELETE_IN_PROGRESS)) => {
            return Err(ReadError::Lost)
        }
        Err(e) => return Err(ReadError::Io(e)),
    };
    if got < 8 {
        return Ok(start);
    }
    let next = i64_at(buf, 0);
    let mut off = 8usize;
    while off + 60 <= got {
        let len = u32_at(buf, off) as usize;
        if len < 60 || off + len > got {
            break;
        }
        // Version 2 records carry 64-bit file references, which is what
        // NTFS uses; version 3 (128-bit, ReFS) is not read.
        if u16_at(buf, off + 4) == 2 {
            let (nlen, noff) = (u16_at(buf, off + 56) as usize, u16_at(buf, off + 58) as usize);
            if let Some(raw) = buf.get(off + noff..off + noff + nlen).filter(|_| noff >= 60 && noff + nlen <= len) {
                let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                out.push(Change { frn: u64_at(buf, off + 8), parent: u64_at(buf, off + 16), name: name_from_utf16(&units), reason: u32_at(buf, off + 40) });
            }
        }
        off += len;
    }
    Ok(next)
}
