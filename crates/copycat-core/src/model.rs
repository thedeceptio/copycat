//! The clip data model.

use std::fmt;
use std::str::FromStr;

/// Longest preview we keep for display/search, in characters.
const PREVIEW_MAX_CHARS: usize = 200;

/// The kind of content a clip holds.
///
/// MVP captures text now; the other variants are wired through storage ahead of
/// the M3 multi-format work so the schema does not need to change later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind {
    Text,
    Html,
    Image,
    Files,
}

impl ClipKind {
    /// Stable string used in the database and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            ClipKind::Text => "text",
            ClipKind::Html => "html",
            ClipKind::Image => "image",
            ClipKind::Files => "files",
        }
    }
}

impl fmt::Display for ClipKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a stored kind string is not recognised.
#[derive(Debug, thiserror::Error)]
#[error("unknown clip kind: {0}")]
pub struct ParseKindError(String);

impl FromStr for ClipKind {
    type Err = ParseKindError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "text" => Ok(ClipKind::Text),
            "html" => Ok(ClipKind::Html),
            "image" => Ok(ClipKind::Image),
            "files" => Ok(ClipKind::Files),
            other => Err(ParseKindError(other.to_string())),
        }
    }
}

/// A clip as stored in the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub id: i64,
    pub kind: ClipKind,
    /// Short, human-readable text used in the list and for searching.
    pub preview: String,
    /// Content hash used to detect duplicates.
    pub hash: String,
    /// Capture time, Unix milliseconds.
    pub created_at: i64,
    pub pinned: bool,
}

/// A clip about to be inserted, before the store assigns an id/timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewClip {
    pub kind: ClipKind,
    pub preview: String,
    pub hash: String,
}

impl NewClip {
    /// Build a text clip, deriving its preview and content hash from `text`.
    pub fn text(text: &str) -> Self {
        NewClip {
            kind: ClipKind::Text,
            preview: preview_of(text),
            hash: content_hash(ClipKind::Text, text.as_bytes()),
        }
    }
}

/// Trim a value down to a short single-purpose preview string.
pub fn preview_of(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() > PREVIEW_MAX_CHARS {
        let mut s: String = trimmed.chars().take(PREVIEW_MAX_CHARS).collect();
        s.push('…');
        s
    } else {
        trimmed.to_string()
    }
}

/// Content hash used for duplicate detection.
///
/// The kind is mixed in so that, e.g., identical bytes captured as text vs. an
/// image are treated as distinct clips.
pub fn content_hash(kind: ClipKind, bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(kind.as_str().as_bytes());
    hasher.update([0u8]);
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_roundtrips_through_string() {
        for kind in [
            ClipKind::Text,
            ClipKind::Html,
            ClipKind::Image,
            ClipKind::Files,
        ] {
            assert_eq!(ClipKind::from_str(kind.as_str()).unwrap(), kind);
        }
        assert!(ClipKind::from_str("nonsense").is_err());
    }

    #[test]
    fn preview_truncates_long_text_with_ellipsis() {
        let long = "a".repeat(PREVIEW_MAX_CHARS + 50);
        let p = preview_of(&long);
        assert_eq!(p.chars().count(), PREVIEW_MAX_CHARS + 1); // +1 for the ellipsis
        assert!(p.ends_with('…'));
    }

    #[test]
    fn preview_trims_whitespace() {
        assert_eq!(preview_of("   hello \n"), "hello");
    }

    #[test]
    fn identical_text_hashes_equal_different_text_differs() {
        assert_eq!(
            content_hash(ClipKind::Text, b"hello"),
            content_hash(ClipKind::Text, b"hello")
        );
        assert_ne!(
            content_hash(ClipKind::Text, b"hello"),
            content_hash(ClipKind::Text, b"world")
        );
    }

    #[test]
    fn same_bytes_different_kind_hash_differently() {
        assert_ne!(
            content_hash(ClipKind::Text, b"data"),
            content_hash(ClipKind::Image, b"data")
        );
    }
}
