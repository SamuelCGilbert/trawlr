//! Small shared helpers: case folding, logging, time.

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn debug_enabled() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

/// Log lines carry a syslog priority prefix so journald files them at the
/// right level when the daemon runs under systemd.
#[macro_export]
macro_rules! info {
    ($($a:tt)*) => { eprintln!("<6>{}", format_args!($($a)*)) };
}
#[macro_export]
macro_rules! warn {
    ($($a:tt)*) => { eprintln!("<4>{}", format_args!($($a)*)) };
}
#[macro_export]
macro_rules! debug {
    ($($a:tt)*) => { if $crate::util::debug_enabled() { eprintln!("<7>{}", format_args!($($a)*)) } };
}

/// Append the search form of `name` to `out`.
///
/// Every name is stored twice at most: this folded form, which all
/// case-insensitive matching and sorting runs on, and the original bytes when
/// they differ. Names that are not valid UTF-8 only get their ASCII folded.
pub fn lower_into(name: &[u8], out: &mut Vec<u8>) {
    if name.is_ascii() {
        out.extend(name.iter().map(|b| b.to_ascii_lowercase()));
        return;
    }
    match std::str::from_utf8(name) {
        Ok(s) => {
            let mut buf = [0u8; 4];
            for ch in s.chars() {
                if ch.is_ascii() {
                    out.push((ch as u8).to_ascii_lowercase());
                } else {
                    for l in ch.to_lowercase() {
                        out.extend_from_slice(l.encode_utf8(&mut buf).as_bytes());
                    }
                }
            }
        }
        Err(_) => out.extend(name.iter().map(|b| b.to_ascii_lowercase())),
    }
}

pub fn to_lower(name: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(name.len());
    lower_into(name, &mut v);
    v
}

/// Paths never contain NUL, but a hostile name from the wire could; map those
/// to an unopenable path instead of panicking.
pub fn cstr(bytes: &[u8]) -> CString {
    CString::new(bytes).unwrap_or_else(|_| CString::new("/\u{1}invalid").unwrap())
}

pub fn now_ns() -> i64 {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    d.as_secs() as i64 * 1_000_000_000 + d.subsec_nanos() as i64
}

pub fn now_secs() -> i64 {
    now_ns() / 1_000_000_000
}

/// Seconds since the epoch squeezed into the 32 bits the index stores.
pub fn clamp_mtime(secs: i64) -> u32 {
    secs.clamp(0, u32::MAX as i64) as u32
}

pub fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A cheap, well-mixed hash for short byte strings (file handles).
pub fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut h = seed ^ (bytes.len() as u64).wrapping_mul(K);
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let v = u64::from_le_bytes(c.try_into().unwrap());
        h = (h ^ v).wrapping_mul(K);
        h ^= h >> 29;
    }
    let rem = chunks.remainder();
    if !rem.is_empty() {
        let mut b = [0u8; 8];
        b[..rem.len()].copy_from_slice(rem);
        h = (h ^ u64::from_le_bytes(b)).wrapping_mul(K);
        h ^= h >> 29;
    }
    h = (h ^ (h >> 32)).wrapping_mul(K);
    h ^ (h >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_ascii_and_unicode() {
        assert_eq!(to_lower(b"ReadMe.TXT"), b"readme.txt");
        assert_eq!(to_lower("ÄRGER.Pdf".as_bytes()), "ärger.pdf".as_bytes());
        // Invalid UTF-8 keeps its bytes, folding only ASCII.
        assert_eq!(to_lower(b"A\xffB"), b"a\xffb");
    }
}
