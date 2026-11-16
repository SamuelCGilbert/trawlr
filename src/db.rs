//! Saving the index to disk and loading it back.
//!
//! The file is the index's arrays written verbatim behind a small JSON
//! header, zstd-compressed. Child lists and the handle lookup are rebuilt on
//! load rather than stored. The index must be compacted before saving, so the
//! file never contains tombstones.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::index::{DirInfo, Index, NONE};
use crate::SavedFs;

const MAGIC: &[u8; 8] = b"EVRYIDX1";
const VERSION: u32 = 1;

pub struct Meta {
    pub saved_at_ns: i64,
    /// Written on an orderly shutdown, after the last change was applied.
    pub clean: bool,
    pub boot_id: String,
    /// Roots, excludes and filesystem types the index was built with.
    pub fingerprint: String,
    pub fs: Vec<SavedFs>,
}

#[derive(Serialize, Deserialize)]
struct Header {
    version: u32,
    entries: u64,
    dirs: u64,
    lower_len: u64,
    orig_len: u64,
    orig_n: u64,
    handles_len: u64,
    perm_len: u64,
    sorted_upto: u32,
    roots: Vec<u32>,
    saved_at_ns: i64,
    clean: bool,
    boot_id: String,
    fingerprint: String,
    fs: Vec<SavedFs>,
}

trait Pod: Copy + Default {}
impl Pod for u8 {}
impl Pod for u16 {}
impl Pod for u32 {}
impl Pod for u64 {}
impl Pod for i64 {}

fn write_pod<T: Pod>(w: &mut impl Write, v: &[T]) -> io::Result<()> {
    let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) };
    w.write_all(bytes)
}

fn read_pod<T: Pod>(r: &mut impl Read, n: u64) -> io::Result<Vec<T>> {
    let n = usize::try_from(n).map_err(|_| bad("length"))?;
    if n.checked_mul(std::mem::size_of::<T>()).is_none_or(|b| b > 1 << 40) {
        return Err(bad("length"));
    }
    let mut v = vec![T::default(); n];
    let bytes = unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, n * std::mem::size_of::<T>()) };
    r.read_exact(bytes)?;
    Ok(v)
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("index file is damaged ({what})"))
}

/// Write the index atomically (temporary file, then rename). Returns the
/// size of the file in bytes.
pub fn save(path: &Path, idx: &Index, meta: &Meta) -> io::Result<u64> {
    assert!(!idx.needs_compaction(), "index must be compacted before saving");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("db.tmp");
    let header = Header {
        version: VERSION,
        entries: idx.len() as u64,
        dirs: idx.dirs.len() as u64,
        lower_len: idx.lower.len() as u64,
        orig_len: idx.orig.len() as u64,
        orig_n: idx.orig_id.len() as u64,
        handles_len: idx.handles.len() as u64,
        perm_len: idx.perm.len() as u64,
        sorted_upto: idx.sorted_upto,
        roots: idx.roots.clone(),
        saved_at_ns: meta.saved_at_ns,
        clean: meta.clean,
        boot_id: meta.boot_id.clone(),
        fingerprint: meta.fingerprint.clone(),
        fs: meta.fs.clone(),
    };
    let hjson = serde_json::to_vec(&header).map_err(io::Error::other)?;

    let mut file = File::create(&tmp)?;
    file.write_all(MAGIC)?;
    let mut w = zstd::stream::write::Encoder::new(file, 1)?;
    let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1).min(4);
    let _ = w.multithread(threads);
    w.write_all(&(hjson.len() as u32).to_le_bytes())?;
    w.write_all(&hjson)?;
    write_pod(&mut w, &idx.name_off)?;
    write_pod(&mut w, &idx.parent)?;
    write_pod(&mut w, &idx.size)?;
    write_pod(&mut w, &idx.mtime)?;
    write_pod(&mut w, &idx.flags)?;
    write_pod(&mut w, &idx.lower)?;
    write_pod(&mut w, &idx.orig)?;
    write_pod(&mut w, &idx.orig_id)?;
    write_pod(&mut w, &idx.orig_off)?;
    write_pod(&mut w, &idx.dirs.iter().map(|d| d.entry).collect::<Vec<u32>>())?;
    write_pod(&mut w, &idx.dirs.iter().map(|d| d.fs).collect::<Vec<u16>>())?;
    write_pod(&mut w, &idx.dirs.iter().map(|d| d.h_len).collect::<Vec<u8>>())?;
    write_pod(&mut w, &idx.dirs.iter().map(|d| d.h_off).collect::<Vec<u32>>())?;
    write_pod(&mut w, &idx.dirs.iter().map(|d| d.mtime_ns).collect::<Vec<i64>>())?;
    write_pod(&mut w, &idx.handles)?;
    write_pod(&mut w, &idx.perm)?;
    let file = w.finish()?;
    file.sync_all()?;
    let len = file.metadata()?.len();
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(len)
}

pub fn load(path: &Path) -> io::Result<(Index, Meta)> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(bad("not an index file"));
    }
    let mut r = zstd::stream::read::Decoder::new(file)?;
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let hlen = u32::from_le_bytes(len) as usize;
    if hlen > 16 << 20 {
        return Err(bad("header"));
    }
    let mut hjson = vec![0u8; hlen];
    r.read_exact(&mut hjson)?;
    let h: Header = serde_json::from_slice(&hjson).map_err(|_| bad("header"))?;
    if h.version != VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "index file is from another version"));
    }
    let n = h.entries;
    let mut idx = Index::new();
    idx.name_off = read_pod(&mut r, n + 1)?;
    idx.parent = read_pod(&mut r, n)?;
    idx.size = read_pod(&mut r, n)?;
    idx.mtime = read_pod(&mut r, n)?;
    idx.flags = read_pod(&mut r, n)?;
    idx.lower = read_pod(&mut r, h.lower_len)?;
    idx.orig = read_pod(&mut r, h.orig_len)?;
    idx.orig_id = read_pod(&mut r, h.orig_n)?;
    idx.orig_off = read_pod(&mut r, h.orig_n)?;
    let d_entry: Vec<u32> = read_pod(&mut r, h.dirs)?;
    let d_fs: Vec<u16> = read_pod(&mut r, h.dirs)?;
    let d_hlen: Vec<u8> = read_pod(&mut r, h.dirs)?;
    let d_hoff: Vec<u32> = read_pod(&mut r, h.dirs)?;
    let d_mtime: Vec<i64> = read_pod(&mut r, h.dirs)?;
    idx.handles = read_pod(&mut r, h.handles_len)?;
    idx.perm = read_pod(&mut r, h.perm_len)?;
    idx.sorted_upto = h.sorted_upto;
    idx.roots = h.roots;

    // Everything below indexes with these values, so reject a file whose
    // numbers do not add up instead of trusting it.
    let n = n as usize;
    let nd = d_entry.len();
    let fs_count = h.fs.len();
    let ok = idx.name_off.first() == Some(&0)
        && idx.name_off.last().copied() == Some(idx.lower.len() as u32)
        && idx.name_off.windows(2).all(|w| w[0] < w[1])
        && idx.name_off.windows(2).all(|w| idx.lower[w[1] as usize - 1] == 0)
        && (0..n).all(|i| {
            let p = idx.parent[i];
            let dir = idx.flags[i] & crate::index::F_DIR != 0;
            idx.flags[i] & crate::index::F_DEAD == 0
                && (p == NONE || ((p as usize) < n && idx.flags[p as usize] & crate::index::F_DIR != 0))
                && (!dir || ((idx.size[i] as usize) < nd && d_entry[idx.size[i] as usize] == i as u32))
        })
        && (0..nd).all(|s| (d_entry[s] as usize) < n && d_hoff[s] as usize + d_hlen[s] as usize <= idx.handles.len() && (d_fs[s] as usize) < fs_count.max(1))
        && idx.orig_id.windows(2).all(|w| w[0] < w[1])
        && idx.orig_id.iter().all(|&i| (i as usize) < n && idx.flags[i as usize] & crate::index::F_ORIG != 0)
        && idx.orig_off.iter().all(|&o| (o as usize) < idx.orig.len())
        && idx.orig.last().is_none_or(|&b| b == 0)
        && idx.flags.iter().filter(|&&f| f & crate::index::F_ORIG != 0).count() == idx.orig_id.len()
        && idx.perm.iter().all(|&i| (i as usize) < n)
        && idx.sorted_upto as usize <= n
        && idx.roots.iter().all(|&r| (r as usize) < n && idx.parent[r as usize] == NONE);
    if !ok {
        return Err(bad("inconsistent contents"));
    }

    idx.dirs = (0..nd).map(|s| DirInfo { entry: d_entry[s], fs: d_fs[s], h_len: d_hlen[s], h_off: d_hoff[s], mtime_ns: d_mtime[s], children: Vec::new() }).collect();
    idx.rebuild_derived();
    Ok((idx, Meta { saved_at_ns: h.saved_at_ns, clean: h.clean, boot_id: h.boot_id, fingerprint: h.fingerprint, fs: h.fs }))
}
