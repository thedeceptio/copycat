//! Backend-independent capture policy.
//!
//! Everything here is pure and unit-tested. Deciding *whether* to store an
//! incoming clipboard payload lives here so both the X11 and (future) Wayland
//! backends share one policy and it can be tested without a display server.

/// Decide whether an incoming text payload is worth storing.
///
/// M1 policy: skip empty / whitespace-only text. Duplicate collapsing is handled
/// by the store (by content hash). Password-manager detection and the per-app
/// exclude-list arrive in M4.
pub fn should_store_text(text: &str) -> bool {
    !text.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_ordinary_text() {
        assert!(should_store_text("hello"));
        assert!(should_store_text("  padded  "));
    }

    #[test]
    fn skips_empty_and_whitespace() {
        assert!(!should_store_text(""));
        assert!(!should_store_text("   "));
        assert!(!should_store_text("\n\t "));
    }
}
