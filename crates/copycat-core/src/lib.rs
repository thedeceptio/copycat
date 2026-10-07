//! Shared models and storage for copycat.
//!
//! This crate holds everything that is independent of the UI toolkit or the
//! platform clipboard backend: the [`Clip`] data model and the SQLite-backed
//! [`Store`]. Both the daemon (`copycatd`) and the app (`copycat`) depend on it.

pub mod model;
pub mod paths;
pub mod search;
pub mod store;

pub use model::{content_hash, Clip, ClipKind, NewClip};
pub use paths::default_db_path;
pub use search::rank;
pub use store::{Store, StoreError};

use std::time::{SystemTime, UNIX_EPOCH};

/// Current wall-clock time as Unix milliseconds.
///
/// Used as the default timestamp for newly captured clips. Returns 0 if the
/// system clock is somehow before the Unix epoch (it never should be).
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
