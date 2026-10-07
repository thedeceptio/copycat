//! Fuzzy ranking for the search popup.
//!
//! The store returns candidate clips (recency/pinned ordered, or a `LIKE`
//! prefilter); this ranks them against the user's query so the best matches
//! surface first. Kept here, pure and unit-tested, so the UI layer stays thin.

use crate::model::Clip;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

/// Rank `clips` against `query`, best match first.
///
/// An empty (or whitespace-only) query returns the clips unchanged, preserving
/// the store's pinned-then-recency order. Otherwise only clips that fuzzily
/// match are kept, sorted by match score, with pinned status and then recency
/// as tie-breakers.
pub fn rank(query: &str, clips: Vec<Clip>) -> Vec<Clip> {
    let query = query.trim();
    if query.is_empty() {
        return clips;
    }

    let matcher = SkimMatcherV2::default();
    let mut scored: Vec<(i64, Clip)> = clips
        .into_iter()
        .filter_map(|clip| matcher.fuzzy_match(&clip.content, query).map(|s| (s, clip)))
        .collect();

    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.pinned.cmp(&a.1.pinned))
            .then(b.1.created_at.cmp(&a.1.created_at))
    });

    scored.into_iter().map(|(_, clip)| clip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClipKind, NewClip};

    /// Build a stored-shape clip from text for ranking tests.
    fn clip(id: i64, text: &str, created_at: i64, pinned: bool) -> Clip {
        let n = NewClip::text(text);
        Clip {
            id,
            kind: ClipKind::Text,
            content: n.content,
            preview: n.preview,
            hash: n.hash,
            created_at,
            pinned,
        }
    }

    #[test]
    fn empty_query_preserves_order() {
        let input = vec![
            clip(1, "alpha", 300, false),
            clip(2, "beta", 200, false),
            clip(3, "gamma", 100, false),
        ];
        let out = rank("  ", input.clone());
        assert_eq!(
            out.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "whitespace query returns input untouched"
        );
    }

    #[test]
    fn filters_out_non_matches() {
        let input = vec![
            clip(1, "hello world", 100, false),
            clip(2, "goodbye", 200, false),
        ];
        let out = rank("xyz", input);
        assert!(out.is_empty(), "nothing fuzzy-matches 'xyz'");
    }

    #[test]
    fn matches_subsequence() {
        let input = vec![
            clip(1, "hello world", 100, false),
            clip(2, "nope", 200, false),
        ];
        let out = rank("hw", input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, 1, "'hw' is a subsequence of 'hello world'");
    }

    #[test]
    fn contiguous_match_ranks_above_scattered() {
        let input = vec![
            clip(1, "cut a tomato", 200, false), // scattered c..a..t
            clip(2, "category", 100, false),     // contiguous "cat"
        ];
        let out = rank("cat", input);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].id, 2,
            "the contiguous 'cat' in 'category' outranks the scattered match"
        );
    }

    #[test]
    fn pinned_breaks_score_ties() {
        // Identical content → identical score; the pinned one should come first.
        let input = vec![
            clip(1, "same text", 100, false),
            clip(2, "same text", 100, true),
        ];
        let out = rank("same", input);
        assert_eq!(out[0].id, 2, "pinned wins an otherwise exact tie");
    }
}
