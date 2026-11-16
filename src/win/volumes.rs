//! Which drives are indexed.

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};

use super::wide;

const DRIVE_FIXED: u32 = 3;

/// What the saved index remembers about a drive, to tell on the next start
/// whether it is still the same drive and where to pick up its journal.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedFs {
    /// `"C:"`.
    pub drive: String,
    pub serial: u64,
    pub journal_id: u64,
    /// The journal position the index is up to date with.
    pub next_usn: i64,
}

/// The file system on drive `letter` (`"NTFS"`, `"FAT32"`, `"exFAT"` ...),
/// and its serial number.
pub fn fs_of(letter: u8) -> Option<(String, u32)> {
    let root = wide(format!("{}:\\", letter as char));
    let mut name = [0u16; 64];
    let mut serial = 0u32;
    let ok = unsafe {
        GetVolumeInformationW(root.as_ptr(), std::ptr::null_mut(), 0, &mut serial, std::ptr::null_mut(), std::ptr::null_mut(), name.as_mut_ptr(), name.len() as u32)
    };
    if ok == 0 {
        return None;
    }
    let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Some((String::from_utf16_lossy(&name[..n]), serial))
}

/// Drive letters of the computer's own disks (not network drives, CDs or
/// card readers) formatted with NTFS.
pub fn local_ntfs_drives() -> Vec<u8> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| b'A' + i)
        .filter(|&l| {
            let root = wide(format!("{}:\\", l as char));
            unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_FIXED }
        })
        .filter(|&l| fs_of(l).is_some_and(|(fs, _)| fs == "NTFS"))
        .collect()
}
