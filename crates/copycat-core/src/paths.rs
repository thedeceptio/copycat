//! Shared filesystem locations, so the daemon and app open the same database.

use std::path::{Path, PathBuf};

/// The copycat data directory under a given XDG data home (pure, no I/O).
pub fn data_dir_in(data_home: &Path) -> PathBuf {
    data_home.join("copycat")
}

/// The history database path under a given XDG data home (pure, no I/O).
pub fn db_path_in(data_home: &Path) -> PathBuf {
    data_dir_in(data_home).join("history.db")
}

/// Resolve the history database path from the environment, creating the
/// containing directory. Uses `XDG_DATA_HOME`, falling back to
/// `$HOME/.local/share`.
pub fn default_db_path() -> std::io::Result<PathBuf> {
    let data_home = resolve_data_home()?;
    let dir = data_dir_in(&data_home);
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("history.db"))
}

fn resolve_data_home() -> std::io::Result<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "neither XDG_DATA_HOME nor HOME is set",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_under_the_data_home() {
        let home = Path::new("/home/user/.local/share");
        assert_eq!(
            data_dir_in(home),
            Path::new("/home/user/.local/share/copycat")
        );
        assert_eq!(
            db_path_in(home),
            Path::new("/home/user/.local/share/copycat/history.db")
        );
    }
}
