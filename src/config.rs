//! Configuration file and the paths the daemon runs with.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const SYSTEM_SOCKET: &str = "/run/trawlrd/trawlrd.sock";
pub const SYSTEM_DB: &str = "/var/lib/trawlrd/index.db";
pub const SYSTEM_CONFIG: &str = "/etc/trawlrd/config.toml";

/// Local, on-disk filesystems indexed without being asked. Virtual ones
/// (proc, sysfs, tmpfs, overlay, squashfs) and network mounts are left out
/// unless named in `extra_fs_types` or listed as a root.
const DEFAULT_FS_TYPES: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "zfs", "bcachefs", "jfs", "reiserfs", "nilfs2", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk",
    "hfsplus", "udf",
];

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Directories to index. Default: `/` as a system service, `$HOME` otherwise.
    pub roots: Vec<String>,
    /// Absolute directories or files to leave out, with everything beneath them.
    pub exclude: Vec<String>,
    /// Names to leave out wherever they appear; `*` and `?` are wildcards.
    pub exclude_names: Vec<String>,
    /// Filesystem types to index in addition to local disks, e.g. `["tmpfs"]`.
    pub extra_fs_types: Vec<String>,
    /// Users (names or numeric ids) allowed to search the system-wide index.
    /// `"*"` allows everyone. root is always allowed.
    pub allowed_users: Vec<String>,
    /// How often the index is written to disk while it has unsaved changes.
    pub save_interval_minutes: Option<u64>,
    /// Threads used to walk the disk. Default: twice the CPU count.
    pub scan_threads: Option<usize>,
}

pub struct Settings {
    pub cfg: Config,
    pub roots: Vec<Vec<u8>>,
    pub socket: PathBuf,
    pub db: PathBuf,
    /// Serving a machine-wide index to other users, as opposed to one user's own.
    pub system: bool,
    pub watch: bool,
    pub allow_all: bool,
    pub allowed_uids: HashSet<u32>,
    exclude_paths: HashSet<Vec<u8>>,
    exclude_names: Vec<Vec<u8>>,
    fs_types: HashSet<String>,
}

#[derive(Default)]
pub struct Overrides {
    pub config: Option<PathBuf>,
    pub socket: Option<PathBuf>,
    pub db: Option<PathBuf>,
    pub roots: Vec<String>,
    pub no_watch: bool,
    pub user_mode: bool,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn expand(p: &str) -> PathBuf {
    if p == "~" {
        home()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(p)
    }
}

fn bytes_of(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let b = p.as_os_str().as_bytes();
    if b.len() > 1 && b.ends_with(b"/") { b[..b.len() - 1].to_vec() } else { b.to_vec() }
}

pub fn user_runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/tmp/trawlrd-{}", unsafe { libc::getuid() })))
}

pub fn user_socket() -> PathBuf {
    user_runtime_dir().join("trawlrd/trawlrd.sock")
}

/// Where a client should look for the daemon: an explicit override, then the
/// system service, then this user's own instance.
pub fn client_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("TRAWLRD_SOCKET") {
        return PathBuf::from(p);
    }
    let sys = PathBuf::from(SYSTEM_SOCKET);
    if sys.exists() { sys } else { user_socket() }
}

fn uid_of(user: &str) -> Option<u32> {
    if let Ok(n) = user.parse::<u32>() {
        return Some(n);
    }
    let c = std::ffi::CString::new(user).ok()?;
    let pw = unsafe { libc::getpwnam(c.as_ptr()) };
    if pw.is_null() { None } else { Some(unsafe { (*pw).pw_uid }) }
}

impl Settings {
    pub fn load(ov: Overrides) -> Result<Settings, String> {
        let system = unsafe { libc::geteuid() } == 0 && !ov.user_mode;
        let cfg_path = ov.config.clone().unwrap_or_else(|| {
            if system {
                PathBuf::from(SYSTEM_CONFIG)
            } else {
                std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("trawlrd/config.toml")
            }
        });
        let cfg: Config = match std::fs::read_to_string(&cfg_path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", cfg_path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && ov.config.is_none() => Config::default(),
            Err(e) => return Err(format!("{}: {e}", cfg_path.display())),
        };

        let root_strs: Vec<String> = if !ov.roots.is_empty() {
            ov.roots.clone()
        } else if !cfg.roots.is_empty() {
            cfg.roots.clone()
        } else if system {
            vec!["/".into()]
        } else {
            vec![home().to_string_lossy().into_owned()]
        };
        let mut roots: Vec<Vec<u8>> = Vec::new();
        for r in &root_strs {
            let p = expand(r);
            let p = std::fs::canonicalize(&p).map_err(|e| format!("root {}: {e}", p.display()))?;
            roots.push(bytes_of(&p));
        }
        // A root inside another root is already covered by it.
        roots.sort();
        roots.dedup();
        let all = roots.clone();
        roots.retain(|r| {
            !all.iter().any(|o| o != r && (o.as_slice() == b"/" || (r.starts_with(o) && r.get(o.len()) == Some(&b'/'))))
        });
        // Inside the index the root `/` is the empty path, so that every
        // child path is simply `parent + "/" + name`.
        for r in roots.iter_mut() {
            if r.as_slice() == b"/" {
                r.clear();
            }
        }

        let socket = ov.socket.clone().unwrap_or_else(|| if system { PathBuf::from(SYSTEM_SOCKET) } else { user_socket() });
        let db = ov.db.clone().unwrap_or_else(|| {
            if system {
                PathBuf::from(SYSTEM_DB)
            } else {
                std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache")).join("trawlrd/index.db")
            }
        });

        let mut allowed_uids = HashSet::new();
        let mut allow_all = false;
        for u in &cfg.allowed_users {
            if u == "*" {
                allow_all = true;
            } else if let Some(uid) = uid_of(u) {
                allowed_uids.insert(uid);
            } else {
                crate::warn!("allowed_users: unknown user {u:?}");
            }
        }

        let mut exclude_paths: HashSet<Vec<u8>> = cfg.exclude.iter().map(|e| bytes_of(&expand(e))).collect();
        // Never index our own database: every save would be a change to index.
        exclude_paths.insert(bytes_of(&db));
        exclude_paths.insert(bytes_of(&db.with_extension("db.tmp")));
        exclude_paths.insert(bytes_of(&db.with_extension("db.running")));
        let exclude_names = cfg.exclude_names.iter().map(|n| n.as_bytes().to_vec()).collect();
        let mut fs_types: HashSet<String> = DEFAULT_FS_TYPES.iter().map(|s| s.to_string()).collect();
        fs_types.extend(cfg.extra_fs_types.iter().cloned());

        Ok(Settings { cfg, roots, socket, db, system, watch: !ov.no_watch, allow_all, allowed_uids, exclude_paths, exclude_names, fs_types })
    }

    pub fn is_excluded(&self, path: &[u8], name: &[u8]) -> bool {
        self.exclude_names.iter().any(|p| wild_match(p, name)) || (!self.exclude_paths.is_empty() && self.exclude_paths.contains(path))
    }

    pub fn fs_allowed(&self, fstype: &str) -> bool {
        self.fs_types.contains(fstype)
    }

    pub fn is_root(&self, path: &[u8]) -> bool {
        self.roots.iter().any(|r| r == path)
    }

    /// Identifies what an index was built from. A saved index with another
    /// fingerprint describes a different set of files and is rebuilt.
    pub fn fingerprint(&self) -> String {
        let mut ex: Vec<String> = self.exclude_paths.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect();
        ex.sort();
        let mut fs: Vec<&String> = self.fs_types.iter().collect();
        fs.sort();
        let roots: Vec<String> = self.roots.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect();
        let names: Vec<String> = self.exclude_names.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect();
        format!("v1 roots={roots:?} exclude={ex:?} names={names:?} fs={fs:?}")
    }

    pub fn save_interval_secs(&self) -> u64 {
        self.cfg.save_interval_minutes.unwrap_or(360).max(1) * 60
    }

    pub fn scan_threads(&self) -> usize {
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        self.cfg.scan_threads.unwrap_or(cpus * 2).clamp(1, 64)
    }
}

/// Glob match with `*` (any run) and `?` (any one byte), whole-string.
pub fn wild_match(pat: &[u8], s: &[u8]) -> bool {
    let (mut p, mut i) = (0, 0);
    let (mut star, mut mark) = (usize::MAX, 0);
    while i < s.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == s[i]) {
            p += 1;
            i += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star = p;
            mark = i;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(wild_match(b"node_modules", b"node_modules"));
        assert!(wild_match(b"*.tmp", b"a.tmp"));
        assert!(wild_match(b"a?c*", b"abcdef"));
        assert!(!wild_match(b"*.tmp", b"a.tmpx"));
        assert!(wild_match(b"*", b""));
    }
}
