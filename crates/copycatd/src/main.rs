//! copycatd — the clipboard capture daemon.
//!
//! Opens the history store, detects the display session, and runs the matching
//! capture backend. M1 implements the X11/XFIXES backend; the Wayland backend
//! (ext/wlr-data-control) follows.

mod backend;
mod capture;

use backend::SessionType;
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

    let store = match Store::open(&db_path) {
        Ok(s) => {
            eprintln!("copycatd: history store ready at {}", db_path.display());
            s
        }
        Err(e) => {
            eprintln!("copycatd: failed to open history store: {e}");
            std::process::exit(1);
        }
    };

    match SessionType::detect() {
        SessionType::X11 => {
            eprintln!("copycatd: X11 session detected");
            if let Err(e) = backend::x11::run(&store) {
                eprintln!("copycatd: X11 backend error: {e}");
                std::process::exit(1);
            }
        }
        SessionType::Wayland => {
            eprintln!(
                "copycatd: Wayland session detected — the Wayland backend is not implemented \
                 yet (X11 first). Exiting."
            );
        }
        SessionType::Unknown => {
            eprintln!(
                "copycatd: no display session detected (no DISPLAY/WAYLAND_DISPLAY). Exiting."
            );
        }
    }
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
