//! copycatd — the clipboard capture daemon.
//!
//! M0 scaffold: opens (creating if needed) the history store and exits. The
//! Wayland `wlr-data-control` capture loop and X11/XFixes fallback land in M1.

use copycat_core::Store;
use std::path::PathBuf;

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("copycatd {version} starting");

    let db_path = match history_db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("copycatd: cannot determine data directory: {e}");
            std::process::exit(1);
        }
    };

    match Store::open(&db_path) {
        Ok(_store) => eprintln!("copycatd: history store ready at {}", db_path.display()),
        Err(e) => {
            eprintln!("copycatd: failed to open history store: {e}");
            std::process::exit(1);
        }
    }

    eprintln!("copycatd: M0 scaffold — capture loop lands in M1. Exiting cleanly.");
}

/// Resolve the history database path under the XDG data home, creating the
/// containing directory if needed. Falls back to `$HOME/.local/share`.
fn history_db_path() -> std::io::Result<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "neither XDG_DATA_HOME nor HOME is set",
            )
        })?;

    let dir = data_home.join("copycat");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("history.db"))
}
