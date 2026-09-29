//! Clipboard capture backends.
//!
//! The daemon selects a backend from the running session. X11 is implemented
//! first (the dev machine's session); the Wayland backend (ext/wlr-data-control)
//! follows.

pub mod x11;

/// Which display server session the daemon is running under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionType {
    X11,
    Wayland,
    Unknown,
}

impl SessionType {
    /// Detect the session from the process environment.
    pub fn detect() -> Self {
        Self::from_env(
            std::env::var_os("WAYLAND_DISPLAY").is_some(),
            std::env::var_os("DISPLAY").is_some(),
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        )
    }

    /// Pure resolver used by [`SessionType::detect`] and tests.
    ///
    /// `XDG_SESSION_TYPE` is authoritative when it names a known type; otherwise
    /// we infer from which display sockets are advertised, preferring Wayland.
    pub fn from_env(has_wayland: bool, has_display: bool, session_type: Option<&str>) -> Self {
        match session_type {
            Some("wayland") => SessionType::Wayland,
            Some("x11") => SessionType::X11,
            _ if has_wayland => SessionType::Wayland,
            _ if has_display => SessionType::X11,
            _ => SessionType::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_type_env_var_is_authoritative() {
        assert_eq!(
            SessionType::from_env(true, true, Some("x11")),
            SessionType::X11
        );
        assert_eq!(
            SessionType::from_env(false, false, Some("wayland")),
            SessionType::Wayland
        );
    }

    #[test]
    fn falls_back_to_sockets_when_session_type_unknown() {
        assert_eq!(
            SessionType::from_env(true, true, None),
            SessionType::Wayland,
            "prefer Wayland when both sockets exist"
        );
        assert_eq!(
            SessionType::from_env(false, true, Some("tty")),
            SessionType::X11
        );
        assert_eq!(
            SessionType::from_env(false, false, None),
            SessionType::Unknown
        );
    }
}
