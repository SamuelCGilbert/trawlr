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
    ($($a:tt)*) => { $crate::util::log(6, format_args!($($a)*)) };
}
#[macro_export]
macro_rules! warn {
    ($($a:tt)*) => { $crate::util::log(4, format_args!($($a)*)) };
}
#[macro_export]
macro_rules! debug {
    ($($a:tt)*) => { if $crate::util::debug_enabled() { $crate::util::log(7, format_args!($($a)*)) } };
}

static LOG_FILE: std::sync::OnceLock<parking_lot::Mutex<std::fs::File>> = std::sync::OnceLock::new();

/// Send log lines to a file from now on (a Windows service has no console).
/// The previous log is kept beside it as `.old`.
pub fn log_to_file(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::rename(path, path.with_extension("log.old"));
    let f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    let _ = LOG_FILE.set(parking_lot::Mutex::new(f));
    Ok(())
}

/// Size at which the log file starts again, so a misbehaving disk cannot
/// fill another one with messages.
const LOG_LIMIT: u64 = 5 << 20;

pub fn log(level: u8, args: std::fmt::Arguments) {
    let Some(file) = LOG_FILE.get() else {
        if cfg!(windows) {
            eprintln!("{args}");
        } else {
            eprintln!("<{level}>{args}");
        }
        return;
    };
    use std::io::Write;
    let tag = match level {
        0..=3 => "error",
        4 => "warning",
        7 => "debug",
        _ => "info",
    };
    let t = now_secs();
    let tm = crate::query::local_tm(t);
    let mut f = file.lock();
    if f.metadata().map(|m| m.len() > LOG_LIMIT).unwrap_or(false) {
        let _ = f.set_len(0);
    }
    let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {tag}: {args}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec);
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
