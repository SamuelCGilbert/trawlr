//! macOS: the folders are read with `getattrlistbulk` and kept current with
//! FSEvents, whose event ids also let a restarted daemon pick up where the
//! saved index left off.

pub mod fsevents;
pub mod indexer;
pub mod scan;
pub mod service;
pub mod volumes;

use serde::{Deserialize, Serialize};

/// What the saved index remembers about the disks, to tell on the next start
/// whether FSEvents can replay what happened while the daemon was stopped.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedFs {
    /// The FSEvents identity of the volumes the roots are on. Event ids
    /// mean nothing on a volume with another identity.
    pub volumes: String,
    /// The last event the index has applied.
    pub event_id: u64,
}

/// Where the service keeps its configuration and index.
pub const DATA_DIR: &str = "/Library/Application Support/Trawlr";
/// The service's log, rotated by the daemon itself.
pub const LOG_FILE: &str = "/Library/Logs/Trawlr/trawlrd.log";
