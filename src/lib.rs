//! Instant file-name search with a live index.
//!
//! `trawlrd` holds every file and folder name in memory, keeps the index
//! current from kernel change notifications, and answers searches over a Unix
//! socket. `trawlr` and the graphical client are thin front ends to it.

pub mod util;
pub mod config;
pub mod db;
pub mod index;
pub mod indexer;
pub mod mounts;
pub mod query;
pub mod scan;
pub mod server;
pub mod sys;
pub mod watch;
