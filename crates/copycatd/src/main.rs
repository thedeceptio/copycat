//! copycatd — the clipboard capture daemon.
//!
//! Opens the history store, detects the display session, and runs the matching
//! capture backend. M1 implements the X11/XFIXES backend; the Wayland backend
//! (ext/wlr-data-control) follows.

mod backend;
mod capture;

use backend::SessionType;
use copycat_core::{default_db_path, Store};

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("copycatd {version} starting");

    let db_path = match default_db_path() {
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
