//! Configuration file and the paths the daemon runs with.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[cfg(all(unix, not(target_os = "macos")))]
pub const SYSTEM_SOCKET: &str = "/run/trawlrd/trawlrd.sock";
#[cfg(all(unix, not(target_os = "macos")))]
pub const SYSTEM_DB: &str = "/var/lib/trawlrd/index.db";
#[cfg(all(unix, not(target_os = "macos")))]
pub const SYSTEM_CONFIG: &str = "/etc/trawlrd/config.toml";

#[cfg(target_os = "macos")]
pub const SYSTEM_SOCKET: &str = "/var/run/trawlrd/trawlrd.sock";
#[cfg(target_os = "macos")]
pub const SYSTEM_DB: &str = "/Library/Application Support/Trawlr/index.db";
#[cfg(target_os = "macos")]
pub const SYSTEM_CONFIG: &str = "/Library/Application Support/Trawlr/config.toml";

/// The service's named pipe. Clients reach it as `\\.\pipe\trawlrd`.
#[cfg(windows)]
pub const SYSTEM_SOCKET: &str = r"\\.\pipe\trawlrd";

/// `C:\ProgramData\Trawlr`, where the service keeps its configuration,
/// index and log.
#[cfg(windows)]
pub fn data_dir() -> PathBuf {
    std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData")).join("Trawlr")
}

/// Local, on-disk filesystems indexed without being asked. Virtual ones
/// (proc, sysfs, tmpfs, overlay, squashfs) and network mounts are left out
/// unless named in `extra_fs_types` or listed as a root.
#[cfg(all(unix, not(target_os = "macos")))]
const DEFAULT_FS_TYPES: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "zfs", "bcachefs", "jfs", "reiserfs", "nilfs2", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk",
    "hfsplus", "udf",
];
/// Local disks on a Mac (lifs is how recent macOS mounts FAT and exFAT).
#[cfg(target_os = "macos")]
const DEFAULT_FS_TYPES: &[&str] = &["apfs", "hfs", "msdos", "exfat", "lifs", "ntfs", "udf"];
/// Windows indexes NTFS volumes by reading their master file table.
#[cfg(windows)]
const DEFAULT_FS_TYPES: &[&str] = &["NTFS"];

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Directories to index. Default: `/` as a system service, `$HOME`
    /// otherwise. On Windows, drives (`"C:"`); default: every local NTFS drive.
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
    /// Groups (names or numeric ids) whose members may search it too. Groups
    /// that do not exist on this system are ignored.
    pub allowed_groups: Vec<String>,
    /// Index backup snapshots (Timeshift, Snapper). They repeat every file
    /// name on the system, often many times over, so they are left out.
    pub index_snapshots: bool,
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
    #[cfg(unix)]
    pub allowed_uids: HashSet<u32>,
    #[cfg(unix)]
    pub allowed_gids: Vec<u32>,
    /// Security identifiers of the users and groups allowed to search.
    #[cfg(windows)]
    pub allowed_sids: Vec<Vec<u8>>,
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
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(unix)]
    let var = "HOME";
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
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

#[cfg(unix)]
fn bytes_of(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    let b = p.as_os_str().as_bytes();
    if b.len() > 1 && b.ends_with(b"/") { b[..b.len() - 1].to_vec() } else { b.to_vec() }
}

/// A Windows path as the index spells it: `C:\Users\x`, no trailing `\`,
/// no `\\?\` prefix, either slash accepted, folded to lower case because
/// Windows compares names without case.
#[cfg(windows)]
fn bytes_of(p: &Path) -> Vec<u8> {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let s = s.trim_end_matches('\\');
    crate::util::to_lower(s.as_bytes())
}

#[cfg(unix)]
pub fn user_runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/tmp/trawlrd-{}", unsafe { libc::getuid() })))
}

#[cfg(unix)]
pub fn user_socket() -> PathBuf {
    user_runtime_dir().join("trawlrd/trawlrd.sock")
}

/// Where a client should look for the daemon: an explicit override, then the
/// system service, then this user's own instance.
pub fn client_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("TRAWLRD_SOCKET") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    return PathBuf::from(SYSTEM_SOCKET);
    #[cfg(unix)]
    {
        let sys = PathBuf::from(SYSTEM_SOCKET);
        if sys.exists() { sys } else { user_socket() }
    }
}

#[cfg(unix)]
fn unix_roots(root_strs: &[String]) -> Result<Vec<Vec<u8>>, String> {
    let mut roots: Vec<Vec<u8>> = Vec::new();
    for r in root_strs {
        let p = expand(r);
        let p = std::fs::canonicalize(&p).map_err(|e| format!("root {}: {e}", p.display()))?;
        roots.push(bytes_of(&p));
    }
    // A root inside another root is already covered by it.
    roots.sort();
    roots.dedup();
    let all = roots.clone();
    roots.retain(|r| !all.iter().any(|o| o != r && (o.as_slice() == b"/" || (r.starts_with(o) && r.get(o.len()) == Some(&b'/')))));
    // Inside the index the root `/` is the empty path, so that every
    // child path is simply `parent + "/" + name`.
    for r in roots.iter_mut() {
        if r.as_slice() == b"/" {
            r.clear();
        }
    }
    Ok(roots)
}

/// Windows indexes whole drives: `"C:"`, `"d:\\"` and `"D:/"` all name
/// drive `D:`. An empty list means every local NTFS drive.
#[cfg(windows)]
fn windows_roots(root_strs: &[String]) -> Result<Vec<Vec<u8>>, String> {
    let mut roots = Vec::new();
    for r in root_strs {
        let t = r.trim_end_matches(['\\', '/']);
        let b = t.as_bytes();
        if b.len() != 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
            return Err(format!("root {r:?}: on Windows a root is a whole drive, such as \"C:\" (use exclude to leave folders out)"));
        }
        roots.push(vec![b[0].to_ascii_uppercase(), b':']);
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

#[cfg(unix)]
fn gid_of(group: &str) -> Option<u32> {
    if let Ok(n) = group.parse::<u32>() {
        return Some(n);
    }
    let c = std::ffi::CString::new(group).ok()?;
    let gr = unsafe { libc::getgrnam(c.as_ptr()) };
    if gr.is_null() { None } else { Some(unsafe { (*gr).gr_gid }) }
}

/// Every group a user belongs to, read from the system's user database now,
/// so a user added to a group can search without anyone restarting trawlrd.
#[cfg(unix)]
fn groups_of(uid: u32) -> Vec<u32> {
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    let r = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut found) };
    if r != 0 || found.is_null() {
        return Vec::new();
    }
    let mut n: libc::c_int = 64;
    loop {
        let want = n;
        #[cfg(not(target_os = "macos"))]
        let (r, gids) = {
            let mut gids = vec![0 as libc::gid_t; n as usize];
            let r = unsafe { libc::getgrouplist(pw.pw_name, pw.pw_gid, gids.as_mut_ptr(), &mut n) };
            (r, gids)
        };
        // macOS takes and returns the ids as signed ints.
        #[cfg(target_os = "macos")]
        let (r, gids) = {
            let mut gids = vec![0 as libc::c_int; n as usize];
            let r = unsafe { libc::getgrouplist(pw.pw_name, pw.pw_gid as libc::c_int, gids.as_mut_ptr(), &mut n) };
            (r, gids.into_iter().map(|g| g as u32).collect::<Vec<u32>>())
        };
        if r >= 0 {
            let mut gids = gids;
            gids.truncate((n as usize).min(gids.len()));
            return gids;
        }
        // Too small: n now holds the size needed (glibc), or ask for more.
        if n <= want {
            n = want * 4;
        }
        if n > 1 << 16 {
            return Vec::new();
        }
    }
}

#[cfg(unix)]
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
        #[cfg(unix)]
        let system = unsafe { libc::geteuid() } == 0 && !ov.user_mode;
        // Reading a volume's file table needs administrator rights, so the
        // Windows daemon only exists as the machine-wide service.
        #[cfg(windows)]
        let system = true;
        let cfg_path = ov.config.clone().unwrap_or_else(|| {
            if system {
                #[cfg(windows)]
                return data_dir().join("config.toml");
                #[cfg(unix)]
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
        } else if cfg!(windows) {
            // Every local NTFS drive, found when indexing starts.
            Vec::new()
        } else if system {
            vec!["/".into()]
        } else {
            vec![home().to_string_lossy().into_owned()]
        };
        #[cfg(windows)]
        let roots = windows_roots(&root_strs)?;
        #[cfg(unix)]
        let roots = unix_roots(&root_strs)?;

        let socket = ov.socket.clone().unwrap_or_else(|| {
            #[cfg(unix)]
            if !system {
                return user_socket();
            }
            PathBuf::from(SYSTEM_SOCKET)
        });
        let db = ov.db.clone().unwrap_or_else(|| {
            #[cfg(windows)]
            return data_dir().join("index.db");
            #[cfg(unix)]
            if system {
                PathBuf::from(SYSTEM_DB)
            } else {
                std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache")).join("trawlrd/index.db")
            }
        });

        let allow_all = cfg.allowed_users.iter().any(|u| u == "*");
        #[cfg(unix)]
        let mut allowed_uids = HashSet::new();
        #[cfg(unix)]
        for u in cfg.allowed_users.iter().filter(|u| *u != "*") {
            if let Some(uid) = uid_of(u) {
                allowed_uids.insert(uid);
            } else {
                crate::warn!("allowed_users: unknown user {u:?}");
            }
        }
        #[cfg(unix)]
        let allowed_gids = cfg.allowed_groups.iter().filter_map(|g| gid_of(g)).collect();
        #[cfg(windows)]
        let allowed_sids = crate::win::security::resolve_accounts(cfg.allowed_users.iter().filter(|u| *u != "*"), cfg.allowed_groups.iter());

        let mut exclude_paths: HashSet<Vec<u8>> = cfg.exclude.iter().map(|e| bytes_of(&expand(e))).collect();
        // Never index our own database: every save would be a change to index.
        exclude_paths.insert(bytes_of(&db));
        exclude_paths.insert(bytes_of(&db.with_extension("db.tmp")));
        exclude_paths.insert(bytes_of(&db.with_extension("db.running")));
        // Our own log changes with every line written to it.
        #[cfg(windows)]
        exclude_paths.insert(bytes_of(&data_dir().join("trawlrd.log")));
        #[cfg(target_os = "macos")]
        for log in [crate::mac::LOG_FILE.to_string(), format!("{}.old", crate::mac::LOG_FILE)] {
            exclude_paths.insert(bytes_of(Path::new(&log)));
        }
        // Windows names compare without case.
        let exclude_names = cfg.exclude_names.iter().map(|n| if cfg!(windows) { crate::util::to_lower(n.as_bytes()) } else { n.as_bytes().to_vec() }).collect();
        let mut fs_types: HashSet<String> = DEFAULT_FS_TYPES.iter().map(|s| s.to_string()).collect();
        fs_types.extend(cfg.extra_fs_types.iter().cloned());

        Ok(Settings {
            cfg,
            roots,
            socket,
            db,
            system,
            watch: !ov.no_watch,
            allow_all,
            #[cfg(unix)]
            allowed_uids,
            #[cfg(unix)]
            allowed_gids,
            #[cfg(windows)]
            allowed_sids,
            exclude_paths,
            exclude_names,
            fs_types,
        })
    }

    /// Whether a user (other than the daemon's own) may search this index.
    #[cfg(unix)]
    pub fn may_search(&self, uid: u32) -> bool {
        if uid == 0 || self.allow_all || self.allowed_uids.contains(&uid) {
            return true;
        }
        !self.allowed_gids.is_empty() && groups_of(uid).iter().any(|g| self.allowed_gids.contains(g))
    }

    /// Whether a client with this user and these groups (security
    /// identifiers, binary) may search the index.
    #[cfg(windows)]
    pub fn may_search_sids(&self, user: &[u8], groups: &[Vec<u8>]) -> bool {
        self.allow_all || crate::win::security::is_system(user) || self.allowed_sids.iter().any(|a| a == user || groups.contains(a))
    }

    /// Whether a name is left out wherever it appears (`exclude_names`).
    /// On Windows `name` must be folded to lower case.
    pub fn is_excluded_name(&self, name: &[u8]) -> bool {
        self.exclude_names.iter().any(|p| wild_match(p, name))
    }

    /// The folders that hold an excluded path, so a scan only builds the
    /// full path of an entry to check it when it is in one of them.
    pub fn exclude_parents(&self) -> HashSet<Vec<u8>> {
        self.exclude_paths.iter().filter_map(|p| p.iter().rposition(|&b| b == crate::index::SEP).map(|i| p[..i].to_vec())).collect()
    }

    /// On Windows `path` and `name` must already be folded to lower case.
    pub fn is_excluded(&self, path: &[u8], name: &[u8]) -> bool {
        self.exclude_names.iter().any(|p| wild_match(p, name))
            || (!self.exclude_paths.is_empty() && self.exclude_paths.contains(path))
            || (!self.cfg.index_snapshots && is_snapshot_folder(path, name))
    }

    pub fn fs_allowed(&self, fstype: &str) -> bool {
        self.fs_types.contains(fstype)
    }

    /// Named in `extra_fs_types`: indexed even when it is not a local disk.
    pub fn fs_explicit(&self, fstype: &str) -> bool {
        self.cfg.extra_fs_types.iter().any(|t| t == fstype)
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
        let snap = if self.cfg.index_snapshots { "" } else { " no-snapshots" };
        format!("v1 roots={roots:?} exclude={ex:?} names={names:?} fs={fs:?}{snap}")
    }

    pub fn save_interval_secs(&self) -> u64 {
        self.cfg.save_interval_minutes.unwrap_or(360).max(1) * 60
    }

    pub fn scan_threads(&self) -> usize {
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        self.cfg.scan_threads.unwrap_or(cpus * 2).clamp(1, 64)
    }
}

/// Where backup tools keep their snapshots: Snapper's `.snapshots` and
/// Timeshift's `timeshift/snapshots` (rsync) and `timeshift-btrfs/snapshots`.
fn is_snapshot_folder(path: &[u8], name: &[u8]) -> bool {
    if name == b".snapshots" {
        return true;
    }
    if name != b"snapshots" {
        return false;
    }
    let parent = path.strip_suffix(name).unwrap_or(b"");
    parent.ends_with(b"/timeshift/") || parent.ends_with(b"/timeshift-btrfs/")
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
    fn snapshot_folders() {
        use super::is_snapshot_folder as snap;
        assert!(snap(b"/.snapshots", b".snapshots"));
        assert!(snap(b"/home/.snapshots", b".snapshots"));
        assert!(snap(b"/timeshift/snapshots", b"snapshots"));
        assert!(snap(b"/home/timeshift/snapshots", b"snapshots"));
        assert!(snap(b"/run/timeshift/backup/timeshift-btrfs/snapshots", b"snapshots"));
        assert!(!snap(b"/home/sal/snapshots", b"snapshots"));
        assert!(!snap(b"/home/sal/mytimeshift/snapshots", b"snapshots"));
        assert!(!snap(b"/timeshift", b"timeshift"));
        assert!(!snap(b"/src/__snapshots__", b"__snapshots__"));
    }

    #[test]
    fn globs() {
        assert!(wild_match(b"node_modules", b"node_modules"));
        assert!(wild_match(b"*.tmp", b"a.tmp"));
        assert!(wild_match(b"a?c*", b"abcdef"));
        assert!(!wild_match(b"*.tmp", b"a.tmpx"));
        assert!(wild_match(b"*", b""));
    }
}
