//! Backend-independent capture policy.
//!
//! Deciding *whether* to store an incoming clipboard payload, and what else has
//! to happen when we do, lives here so both the X11 and (future) Wayland
//! backends share one policy and it can be tested without a display server.

use copycat_core::{NewClip, Store, StoreError, DEFAULT_HISTORY_CAP};

/// Decide whether an incoming text payload is worth storing.
///
/// M1 policy: skip empty / whitespace-only text. Duplicate collapsing is handled
/// by the store (by content hash). Password-manager detection and the per-app
/// exclude-list are not implemented — anything you copy is recorded.
pub fn should_store_text(text: &str) -> bool {
    !text.trim().is_empty()
}

/// What happened to an incoming payload.
#[derive(Debug, PartialEq, Eq)]
pub enum Captured {
    /// Stored, dropping `trimmed` old clips to stay under the cap.
    Stored { trimmed: usize },
    /// Rejected by [`should_store_text`].
    Skipped,
}

/// Store `text` if it is worth keeping, then trim the history back to `cap`.
///
/// Trimming belongs with the insert rather than on a timer: an insert is the
/// only thing that can push the history over the cap, so this is the only
/// moment it needs checking. Pinned clips are exempt from the cap, which is
/// what makes pinning a way to keep something indefinitely.
pub fn capture_text(store: &Store, text: &str, cap: usize) -> Result<Captured, StoreError> {
    if !should_store_text(text) {
        return Ok(Captured::Skipped);
    }
    store.insert(&NewClip::text(text))?;
    let trimmed = store.enforce_cap(cap)?;
    Ok(Captured::Stored { trimmed })
}

/// [`capture_text`] with the shipped history cap.
pub fn capture(store: &Store, text: &str) -> Result<Captured, StoreError> {
    capture_text(store, text, DEFAULT_HISTORY_CAP)
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

    fn store() -> Store {
        Store::open_in_memory().expect("in-memory store")
    }

    #[test]
    fn capturing_text_stores_it() {
        let store = store();
        assert_eq!(
            capture_text(&store, "hello", 10).unwrap(),
            Captured::Stored { trimmed: 0 }
        );
        assert_eq!(store.count().unwrap(), 1);
    }

    #[test]
    fn whitespace_is_not_stored_at_all() {
        let store = store();
        assert_eq!(capture_text(&store, "   ", 10).unwrap(), Captured::Skipped);
        assert_eq!(store.count().unwrap(), 0);
    }

    #[test]
    fn history_is_trimmed_to_the_cap() {
        let store = store();
        for i in 0..5 {
            capture_text(&store, &format!("clip {i}"), 3).unwrap();
        }
        // The cap is enforced on every insert, so it is never exceeded.
        assert_eq!(store.count().unwrap(), 3);
        // All five land in the same millisecond, so this relies on the store
        // breaking created_at ties by id.
        let kept: Vec<String> = store
            .recent(10)
            .unwrap()
            .iter()
            .map(|c| c.text().to_string())
            .collect();
        assert_eq!(kept, vec!["clip 4", "clip 3", "clip 2"], "newest survive");
    }

    #[test]
    fn the_trim_is_reported() {
        let store = store();
        for i in 0..3 {
            capture_text(&store, &format!("clip {i}"), 3).unwrap();
        }
        // The fourth insert pushes one out.
        assert_eq!(
            capture_text(&store, "clip 3", 3).unwrap(),
            Captured::Stored { trimmed: 1 }
        );
    }

    #[test]
    fn pinned_clips_are_exempt_from_the_cap() {
        let store = store();
        capture_text(&store, "keep me", 2).unwrap();
        let pinned_id = store.recent(1).unwrap()[0].id;
        store.set_pinned(pinned_id, true).unwrap();

        for i in 0..5 {
            capture_text(&store, &format!("filler {i}"), 2).unwrap();
        }

        let kept = store.recent(50).unwrap();
        assert!(
            kept.iter().any(|c| c.id == pinned_id),
            "a pinned clip must survive the cap - that is what pinning is for"
        );
        // Two unpinned plus the pin.
        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn re_copying_does_not_consume_cap_space() {
        let store = store();
        for _ in 0..5 {
            capture_text(&store, "same text", 3).unwrap();
        }
        assert_eq!(store.count().unwrap(), 1, "duplicates collapse");
    }
}
