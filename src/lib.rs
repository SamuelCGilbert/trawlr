//! Instant file-name search with a live index.
//!
//! `trawlrd` holds every file and folder name in memory, keeps the index
//! current from the operating system's change notifications, and answers
//! searches over a local socket (a Unix socket, or a named pipe on Windows).
//! `trawlr` and the graphical client are thin front ends to it.
//!
//! The index, the search language and the protocol are shared. How the disk
//! is read and watched is not: fanotify on Linux, the NTFS master file table
//! and change journal on Windows.

pub mod util;
pub mod config;
pub mod db;
pub mod index;
pub mod query;
pub mod server;

#[cfg(target_os = "linux")]
pub mod indexer;
#[cfg(target_os = "linux")]
pub mod mounts;
#[cfg(target_os = "linux")]
pub mod scan;
#[cfg(target_os = "linux")]
pub mod sys;
#[cfg(target_os = "linux")]
pub mod watch;
#[cfg(target_os = "linux")]
pub use mounts::SavedFs;

#[cfg(windows)]
pub mod win;
#[cfg(windows)]
pub use win::indexer;
#[cfg(windows)]
pub use win::volumes::SavedFs;
