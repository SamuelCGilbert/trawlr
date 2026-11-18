//! Reading an NTFS drive's master file table (MFT).
//!
//! Every file and folder on an NTFS drive has a record in the MFT: its names
//! (each with the record of the folder it is in), its size, its times. The
//! records are read straight off the drive in big sequential chunks, which
//! is how a drive with a million files is indexed in a few seconds. Single
//! records are fetched through `FSCTL_GET_NTFS_FILE_RECORD` when the change
//! journal reports that a file changed.
//!
//! Everything read here comes off the disk, so every offset and length is
//! checked before it is used: a damaged record is skipped, never trusted.

use std::io;

use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandleEx, OpenFileById, ReadFile, FileIdBothDirectoryInfo, FileIdType, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_BOTH_DIR_INFO,
    FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, SYNCHRONIZE,
};
use windows_sys::Win32::System::Ioctl::{FSCTL_GET_NTFS_FILE_RECORD, FSCTL_GET_NTFS_VOLUME_DATA};
use windows_sys::Win32::System::IO::OVERLAPPED;

use super::{filetime_to_unix, i64_at, ioctl, name_from_utf16, u16_at, u32_at, u64_at, wide, Handle};

/// The low 48 bits of a file reference are the record number; the top 16
/// are a sequence number, bumped each time the record is reused.
pub const REC_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

pub fn rec_of(frn: u64) -> u64 {
    frn & REC_MASK
}
pub fn seq_of(frn: u64) -> u16 {
    (frn >> 48) as u16
}

/// Record 5 is the drive's root folder.
pub const ROOT_REC: u64 = 5;
/// Records below this are NTFS's own files ($MFT, $LogFile, $Bitmap ...).
pub const FIRST_USER_REC: u64 = 16;
/// `$Extend`, the folder holding the change journal and other metadata.
pub const EXTEND_REC: u64 = 11;

const AT_STANDARD_INFORMATION: u32 = 0x10;
const AT_ATTRIBUTE_LIST: u32 = 0x20;
const AT_FILE_NAME: u32 = 0x30;
const AT_DATA: u32 = 0x80;
const AT_REPARSE_POINT: u32 = 0xC0;
const AT_END: u32 = 0xFFFF_FFFF;

const NS_DOS: u8 = 2;

pub const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
pub const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// A run of clusters: where it starts on the drive (`None` for a sparse
/// run, which has no clusters) and how many clusters long it is.
pub type Run = (Option<u64>, u64);

pub enum AttrList {
    Resident(Vec<u8>),
    NonResident { runs: Vec<Run>, size: u64 },
}

/// What one MFT record says.
#[derive(Default)]
pub struct Record {
    pub seq: u16,
    /// For an extension record, the base record it belongs to; else 0.
    pub base: u64,
    pub in_use: bool,
    pub is_dir: bool,
    /// Each name with the folder it is in. A file with hard links has
    /// several. Short 8.3 names are left out.
    pub names: Vec<(u64, String)>,
    /// Size of the main data stream, when this record holds its start.
    pub size: Option<u64>,
    /// Last write time, seconds since 1970.
    pub mtime: Option<i64>,
    pub attributes: Option<u32>,
    pub reparse_tag: Option<u32>,
    pub attr_list: Option<AttrList>,
    /// Runs of the unnamed data stream (used for $MFT itself).
    pub data_runs: Vec<(u64, Vec<Run>)>,
}

impl Record {
    /// A symbolic link or junction, as opposed to other reparse points such
    /// as OneDrive placeholders, which are ordinary files to the user.
    pub fn is_link(&self) -> bool {
        self.attributes.is_some_and(|a| a & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            && matches!(self.reparse_tag, Some(IO_REPARSE_TAG_SYMLINK) | Some(IO_REPARSE_TAG_MOUNT_POINT))
    }
}

/// Undo NTFS's torn-write protection: the last two bytes of every 512-byte
/// stride were swapped out for a check value when the record was written.
/// Returns false if the record does not check out (a torn or damaged write).
pub fn apply_fixups(rec: &mut [u8]) -> bool {
    let usa_ofs = u16_at(rec, 4) as usize;
    let usa_cnt = u16_at(rec, 6) as usize;
    if usa_cnt < 2 || usa_ofs + usa_cnt * 2 > rec.len() {
        return false;
    }
    let stride = rec.len() / (usa_cnt - 1);
    if stride < 2 || stride * (usa_cnt - 1) != rec.len() {
        return false;
    }
    let check = [rec[usa_ofs], rec[usa_ofs + 1]];
    if (1..usa_cnt).any(|i| rec[i * stride - 2..i * stride] != check) {
        return false;
    }
    for i in 1..usa_cnt {
        let end = i * stride - 2;
        rec[end] = rec[usa_ofs + 2 * i];
        rec[end + 1] = rec[usa_ofs + 2 * i + 1];
    }
    true
}

/// Decode a non-resident attribute's mapping pairs into cluster runs.
pub fn decode_runs(b: &[u8]) -> Vec<Run> {
    let mut runs = Vec::new();
    let (mut p, mut lcn) = (0usize, 0i64);
    while p < b.len() {
        let h = b[p];
        if h == 0 {
            break;
        }
        let (ln, lo) = ((h & 0xF) as usize, (h >> 4) as usize);
        p += 1;
        if ln == 0 || ln > 8 || lo > 8 || p + ln + lo > b.len() {
            break;
        }
        let mut len = 0u64;
        for i in 0..ln {
            len |= (b[p + i] as u64) << (8 * i);
        }
        p += ln;
        if lo == 0 {
            runs.push((None, len));
            continue;
        }
        let mut delta = 0i64;
        for i in 0..lo {
            delta |= (b[p + i] as i64) << (8 * i);
        }
        // Sign-extend from the top byte that was read.
        if lo < 8 && b[p + lo - 1] & 0x80 != 0 {
            delta |= -1i64 << (8 * lo);
        }
        p += lo;
        lcn = lcn.wrapping_add(delta);
        if lcn < 0 {
            break;
        }
        runs.push((Some(lcn as u64), len));
    }
    runs
}

/// Parse a record whose fixups have been applied. `None` if it is not a
/// file record at all.
pub fn parse_record(buf: &[u8]) -> Option<Record> {
    if buf.len() < 48 || &buf[0..4] != b"FILE" {
        return None;
    }
    let flags = u16_at(buf, 22);
    let mut r = Record { seq: u16_at(buf, 16), base: u64_at(buf, 32), in_use: flags & 1 != 0, is_dir: flags & 2 != 0, ..Default::default() };
    let used = (u32_at(buf, 24) as usize).min(buf.len());
    let mut off = u16_at(buf, 20) as usize;
    while off + 16 <= used {
        let t = u32_at(buf, off);
        if t == AT_END {
            break;
        }
        let len = u32_at(buf, off + 4) as usize;
        if len < 16 || off + len > used {
            break;
        }
        let a = &buf[off..off + len];
        let nonres = a[8] != 0;
        let named = a[9] != 0;
        let value = if nonres {
            None
        } else {
            let (vlen, voff) = (u32_at(a, 16) as usize, u16_at(a, 20) as usize);
            a.get(voff..voff.checked_add(vlen)?)
        };
        match t {
            AT_STANDARD_INFORMATION => {
                if let Some(v) = value.filter(|v| v.len() >= 36) {
                    r.mtime = Some(filetime_to_unix(i64_at(v, 8)));
                    r.attributes = Some(u32_at(v, 32));
                }
            }
            AT_ATTRIBUTE_LIST => {
                r.attr_list = if let Some(v) = value {
                    Some(AttrList::Resident(v.to_vec()))
                } else if a.len() >= 64 {
                    let ro = u16_at(a, 32) as usize;
                    a.get(ro..).map(|rl| AttrList::NonResident { runs: decode_runs(rl), size: u64_at(a, 48) })
                } else {
                    None
                };
            }
            AT_FILE_NAME => {
                if let Some(v) = value.filter(|v| v.len() >= 66) {
                    let (n, ns) = (v[64] as usize, v[65]);
                    if ns != NS_DOS {
                        if let Some(raw) = v.get(66..66 + 2 * n) {
                            let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                            r.names.push((u64_at(v, 0), name_from_utf16(&units)));
                        }
                    }
                    // The reparse tag is also kept here, for when the
                    // reparse attribute itself is not resident.
                    if r.reparse_tag.is_none() && u32_at(v, 56) & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        r.reparse_tag = Some(u32_at(v, 60));
                    }
                }
            }
            AT_DATA if !named => {
                if let Some(v) = value {
                    r.size = Some(v.len() as u64);
                } else if a.len() >= 64 {
                    let lowest_vcn = u64_at(a, 16);
                    // Only the first piece of a split attribute has the size.
                    if lowest_vcn == 0 {
                        r.size = Some(u64_at(a, 48));
                    }
                    let ro = u16_at(a, 32) as usize;
                    if let Some(rl) = a.get(ro..) {
                        r.data_runs.push((lowest_vcn, decode_runs(rl)));
                    }
                }
            }
            AT_REPARSE_POINT => {
                if let Some(v) = value.filter(|v| v.len() >= 4) {
                    r.reparse_tag = Some(u32_at(v, 0));
                }
            }
            _ => {}
        }
        off += len;
    }
    Some(r)
}

/// One entry of a folder listing read through the file system rather than
/// the MFT (for a folder that turned up without the journal describing its
/// contents).
pub struct DirEntry {
    pub frn: u64,
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub mtime: i64,
}

/// An open NTFS drive.
pub struct Ntfs {
    pub h: Handle,
    pub letter: u8,
    pub serial: u64,
    pub bytes_per_cluster: u64,
    pub record_size: usize,
    /// How much of the MFT holds records (the rest is preallocated space).
    pub mft_valid: u64,
    pub mft_runs: Vec<Run>,
}

/// Open drive `letter` (`b'C'`) itself for reading. Needs administrator
/// rights.
pub fn open_volume(letter: u8) -> io::Result<Handle> {
    let path = wide(format!(r"\\.\{}:", letter as char));
    Handle::new(unsafe {
        CreateFileW(path.as_ptr(), 0x8000_0000 /* GENERIC_READ */, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
    })
}

impl Ntfs {
    /// Open drive `letter` (`b'C'`) for reading. Needs administrator rights.
    pub fn open(letter: u8) -> io::Result<Ntfs> {
        let h = open_volume(letter)?;
        // NTFS_VOLUME_DATA_BUFFER, plus room for the extended part.
        let mut vd = [0u8; 128];
        ioctl(&h, FSCTL_GET_NTFS_VOLUME_DATA, &[], &mut vd)?;
        let mut v = Ntfs {
            h,
            letter,
            serial: u64_at(&vd, 0),
            bytes_per_cluster: u32_at(&vd, 44) as u64,
            record_size: u32_at(&vd, 48) as usize,
            mft_valid: u64_at(&vd, 56),
            mft_runs: Vec::new(),
        };
        if v.bytes_per_cluster == 0 || !(256..=65536).contains(&v.record_size) || !v.record_size.is_power_of_two() {
            return Err(io::Error::other("unexpected NTFS geometry"));
        }
        // Where the MFT itself lies: the data runs of record 0, $MFT.
        let rec0 = v.record(0)?.ok_or_else(|| io::Error::other("cannot read the MFT's own record"))?;
        let mut pieces = rec0.data_runs;
        if let Some(list) = rec0.attr_list {
            for seg in v.list_segments(&list, &[AT_DATA], 0)? {
                if let Some(ext) = v.record(seg)? {
                    pieces.extend(ext.data_runs);
                }
            }
        }
        pieces.sort_by_key(|(vcn, _)| *vcn);
        v.mft_runs = pieces.into_iter().flat_map(|(_, r)| r).collect();
        if v.mft_runs.is_empty() {
            return Err(io::Error::other("the MFT has no data runs"));
        }
        Ok(v)
    }

    /// Read `buf.len()` bytes at a byte offset on the drive. Offsets and
    /// lengths must be multiples of the sector size.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut done = 0usize;
        while done < buf.len() {
            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            let at = offset + done as u64;
            ov.Anonymous.Anonymous.Offset = at as u32;
            ov.Anonymous.Anonymous.OffsetHigh = (at >> 32) as u32;
            let want = (buf.len() - done).min(1 << 30) as u32;
            let mut got = 0u32;
            let ok = unsafe { ReadFile(self.h.raw(), buf[done..].as_mut_ptr(), want, &mut got, &mut ov) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            if got == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            done += got as usize;
        }
        Ok(())
    }

    /// Read bytes of a non-resident stream given its runs.
    fn read_runs(&self, runs: &[Run], start: u64, buf: &mut [u8]) -> io::Result<()> {
        let bpc = self.bytes_per_cluster;
        let (mut pos, mut filled) = (0u64, 0usize);
        for &(lcn, clusters) in runs {
            let run_len = clusters.saturating_mul(bpc);
            let want_from = start + filled as u64;
            if filled < buf.len() && want_from < pos + run_len {
                let within = want_from - pos;
                let n = ((run_len - within) as usize).min(buf.len() - filled);
                match lcn {
                    Some(l) => self.read_at(l * bpc + within, &mut buf[filled..filled + n])?,
                    None => buf[filled..filled + n].fill(0),
                }
                filled += n;
            }
            pos += run_len;
            if filled == buf.len() {
                return Ok(());
            }
        }
        Err(io::ErrorKind::UnexpectedEof.into())
    }

    /// Fetch and parse one record by number. `None` if NTFS has no record of
    /// that number in use (it then hands back a lower one instead).
    pub fn record(&self, rec: u64) -> io::Result<Option<Record>> {
        let mut out = vec![0u8; 12 + self.record_size];
        let input = (rec as i64).to_le_bytes();
        let got = ioctl(&self.h, FSCTL_GET_NTFS_FILE_RECORD, &input, &mut out)?;
        if got < 12 || rec_of(u64_at(&out, 0)) != rec {
            return Ok(None);
        }
        let len = (u32_at(&out, 8) as usize).min(got - 12);
        let buf = &mut out[12..12 + len];
        // The record comes back with its fixups usually applied already;
        // apply them only if they are still there.
        apply_fixups(buf);
        Ok(parse_record(buf))
    }

    /// The other records that hold attributes of the given types for the
    /// file with base record `base`, from its attribute list.
    fn list_segments(&self, list: &AttrList, types: &[u32], base: u64) -> io::Result<Vec<u64>> {
        let bytes = match list {
            AttrList::Resident(b) => b.clone(),
            AttrList::NonResident { runs, size } => {
                if *size > 4 << 20 {
                    return Ok(Vec::new());
                }
                // Reads must cover whole sectors; round up and trim after.
                let bpc = self.bytes_per_cluster as usize;
                let mut b = vec![0u8; (*size as usize).div_ceil(bpc) * bpc];
                self.read_runs(runs, 0, &mut b)?;
                b.truncate(*size as usize);
                b
            }
        };
        let mut segs = Vec::new();
        let mut off = 0usize;
        while off + 26 <= bytes.len() {
            let t = u32_at(&bytes, off);
            let len = u16_at(&bytes, off + 4) as usize;
            if len < 26 {
                break;
            }
            let seg = rec_of(u64_at(&bytes, off + 16));
            if types.contains(&t) && seg != base && !segs.contains(&seg) {
                segs.push(seg);
            }
            off += len;
        }
        Ok(segs)
    }

    /// Everything the MFT says about the file `frn` now, including what its
    /// extension records hold. `None` if it no longer exists (its record is
    /// free, or was reused for another file).
    pub fn file(&self, frn: u64) -> io::Result<Option<Record>> {
        let rec = rec_of(frn);
        let Some(mut r) = self.record(rec)? else { return Ok(None) };
        if !r.in_use || r.seq != seq_of(frn) || r.base != 0 {
            return Ok(None);
        }
        if let Some(list) = r.attr_list.take() {
            for seg in self.list_segments(&list, &[AT_FILE_NAME, AT_DATA], rec)? {
                if let Some(ext) = self.record(seg)? {
                    if ext.in_use && rec_of(ext.base) == rec {
                        r.names.extend(ext.names);
                        if r.size.is_none() {
                            r.size = ext.size;
                        }
                    }
                }
            }
        }
        Ok(Some(r))
    }

    /// Hand every in-use record of the MFT to `each`, in record order.
    pub fn scan(&self, cancel: &std::sync::atomic::AtomicBool, mut each: impl FnMut(u64, Record)) -> io::Result<()> {
        const CHUNK: usize = 4 << 20;
        let rs = self.record_size;
        let total = self.mft_valid / rs as u64;
        let mut buf = vec![0u8; CHUNK];
        let mut rec = 0u64;
        while rec < total {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let n = ((total - rec) as usize * rs).min(CHUNK);
            // Round up to whole clusters for the read; extra records past
            // the valid length are ignored.
            let bpc = self.bytes_per_cluster as usize;
            let read_len = n.div_ceil(bpc) * bpc;
            if read_len > buf.len() {
                buf.resize(read_len, 0);
            }
            self.read_runs(&self.mft_runs, rec * rs as u64, &mut buf[..read_len])?;
            for chunk in buf[..n].chunks_exact_mut(rs) {
                if apply_fixups(chunk) {
                    if let Some(r) = parse_record(chunk) {
                        if r.in_use {
                            each(rec, r);
                        }
                    }
                }
                rec += 1;
            }
        }
        Ok(())
    }

    /// List a folder through the file system, by its file reference.
    pub fn list_dir(&self, frn: u64) -> io::Result<Vec<DirEntry>> {
        let id = FILE_ID_DESCRIPTOR { dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32, Type: FileIdType, Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: frn as i64 } };
        let dir = Handle::new(unsafe {
            OpenFileById(
                self.h.raw(),
                &id,
                FILE_LIST_DIRECTORY | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                FILE_FLAG_BACKUP_SEMANTICS,
            )
        })?;
        let mut out = Vec::new();
        let mut buf = vec![0u64; 8192];
        let name_at = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
        loop {
            let ok = unsafe { GetFileInformationByHandleEx(dir.raw(), FileIdBothDirectoryInfo, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32) };
            if ok == 0 {
                let e = io::Error::last_os_error();
                // ERROR_NO_MORE_FILES
                if e.raw_os_error() == Some(18) {
                    break;
                }
                return Err(e);
            }
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len() * 8) };
            let mut off = 0usize;
            while let Some(e) = bytes.get(off..off + name_at) {
                // Copied out rather than referenced in place: nothing but the
                // documentation promises the entries are aligned.
                let info: FILE_ID_BOTH_DIR_INFO = unsafe { std::ptr::read_unaligned(e.as_ptr() as *const FILE_ID_BOTH_DIR_INFO) };
                let nlen = info.FileNameLength as usize;
                if let Some(raw) = bytes.get(off + name_at..off + name_at + nlen) {
                    let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    let name = name_from_utf16(&units);
                    if name != "." && name != ".." {
                        let attrs = info.FileAttributes;
                        let link = attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 && matches!(info.EaSize, IO_REPARSE_TAG_SYMLINK | IO_REPARSE_TAG_MOUNT_POINT);
                        out.push(DirEntry {
                            frn: info.FileId as u64,
                            name,
                            is_dir: attrs & 0x10 != 0,
                            is_link: link,
                            size: info.EndOfFile.max(0) as u64,
                            mtime: filetime_to_unix(info.LastWriteTime),
                        });
                    }
                }
                if info.NextEntryOffset == 0 {
                    break;
                }
                off += info.NextEntryOffset as usize;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs() {
        // 0x21: 1 length byte, 2 offset bytes. Second run goes backwards.
        let r = decode_runs(&[0x21, 0x10, 0x00, 0x01, 0x21, 0x08, 0x00, 0xFF, 0x01, 0x05, 0x00]);
        assert_eq!(r, vec![(Some(0x100), 0x10), (Some(0x100 - 0x100), 8), (None, 5)]);
    }

    #[test]
    fn fixups() {
        let mut rec = vec![0u8; 1024];
        rec[4] = 0x30; // update sequence array at 0x30
        rec[6] = 3; // three entries: check value + two strides
        rec[0x30] = 0xAB;
        rec[0x31] = 0xCD;
        rec[0x32] = 1;
        rec[0x33] = 2;
        rec[0x34] = 3;
        rec[0x35] = 4;
        rec[510] = 0xAB;
        rec[511] = 0xCD;
        rec[1022] = 0xAB;
        rec[1023] = 0xCD;
        assert!(apply_fixups(&mut rec));
        assert_eq!(&rec[510..512], &[1, 2]);
        assert_eq!(&rec[1022..1024], &[3, 4]);
        // Applying again finds no check values: refused, record unchanged.
        assert!(!apply_fixups(&mut rec));
    }
}
