//! SQLite-backed clip history.
//!
//! The store is the single source of truth for clip history. It handles
//! insertion with duplicate collapsing, recency/pinned ordering, search,
//! pinning, deletion, and enforcing the history cap.

use crate::model::{Clip, ClipKind, NewClip};
use crate::now_millis;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;
use std::str::FromStr;

/// Errors the store can return.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Convenience alias for store results.
pub type Result<T> = std::result::Result<T, StoreError>;

/// The clip history database.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) a store at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_conn(Connection::open(path)?)
    }

    /// Open an in-memory store. Used by tests and ephemeral runs.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS clips (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 kind       TEXT    NOT NULL,
                 content    TEXT    NOT NULL DEFAULT '',
                 preview    TEXT    NOT NULL,
                 hash       TEXT    NOT NULL UNIQUE,
                 created_at INTEGER NOT NULL,
                 pinned     INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_clips_created ON clips(created_at);",
        )?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Insert a clip captured now. See [`Store::insert_at`].
    pub fn insert(&self, new: &NewClip) -> Result<i64> {
        self.insert_at(new, now_millis())
    }

    /// Insert a clip with an explicit capture time.
    ///
    /// Duplicate collapsing (M1 #11): if a clip with the same content hash
    /// already exists, its timestamp is bumped to `created_at` (moving it back
    /// to the top of the history) instead of inserting a second copy. Returns
    /// the row id of the new or existing clip.
    pub fn insert_at(&self, new: &NewClip, created_at: i64) -> Result<i64> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM clips WHERE hash = ?1",
                params![new.hash],
                |r| r.get(0),
            )
            .optional()?;

        if let Some(id) = existing {
            self.conn.execute(
                "UPDATE clips SET created_at = ?1 WHERE id = ?2",
                params![created_at, id],
            )?;
            return Ok(id);
        }

        self.conn.execute(
            "INSERT INTO clips (kind, content, preview, hash, created_at, pinned)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![
                new.kind.as_str(),
                new.content,
                new.preview,
                new.hash,
                created_at
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The most recent clips, pinned first then newest-first, capped at `limit`.
    pub fn recent(&self, limit: usize) -> Result<Vec<Clip>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, content, preview, hash, created_at, pinned FROM clips
             ORDER BY pinned DESC, created_at DESC, id DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_clip)?;
        collect(rows)
    }

    /// Clips whose full content contains `query` (case-insensitive), pinned
    /// first then newest-first, capped at `limit`.
    ///
    /// Ties on `created_at` are broken by id. Timestamps have millisecond
    /// resolution, so two clips captured in the same millisecond would
    /// otherwise come back in whatever order SQLite felt like.
    ///
    /// This is the candidate filter; fuzzy ranking lives in [`crate::search`].
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Clip>> {
        let like = format!("%{}%", escape_like(query));
        let mut stmt = self.conn.prepare(
            // Legacy rows (migrated before content was stored) have an empty
            // content; fall back to their preview so they stay findable.
            "SELECT id, kind, content, preview, hash, created_at, pinned FROM clips
             WHERE (CASE WHEN content = '' THEN preview ELSE content END)
                   LIKE ?1 ESCAPE '\\'
             ORDER BY pinned DESC, created_at DESC, id DESC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![like, limit as i64], row_to_clip)?;
        collect(rows)
    }

    /// Pin or unpin a clip. Returns true if a row was affected.
    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE clips SET pinned = ?1 WHERE id = ?2",
            params![pinned as i64, id],
        )?;
        Ok(n > 0)
    }

    /// Delete a single clip. Returns true if a row was removed.
    pub fn delete(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM clips WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Delete every clip. Returns the number removed.
    pub fn clear(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM clips", [])?)
    }

    /// Delete every unpinned clip, keeping pins. Returns the number removed.
    pub fn clear_unpinned(&self) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM clips WHERE pinned = 0", [])?)
    }

    /// Total number of clips.
    pub fn count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM clips", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// A cheap fingerprint of the history's state: `(row count, newest
    /// timestamp)`.
    ///
    /// Comparing this between polls lets a reader notice new, removed, or
    /// bumped clips without pulling every row.
    pub fn revision(&self) -> Result<(usize, i64)> {
        let newest: i64 =
            self.conn
                .query_row("SELECT COALESCE(MAX(created_at), 0) FROM clips", [], |r| {
                    r.get(0)
                })?;
        Ok((self.count()?, newest))
    }

    /// Enforce the history cap: keep the newest `max` unpinned clips and delete
    /// older unpinned ones. Pinned clips are always kept and never counted
    /// against the cap. Returns the number removed.
    pub fn enforce_cap(&self, max: usize) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM clips
             WHERE pinned = 0
               AND id NOT IN (
                   SELECT id FROM clips WHERE pinned = 0
                   ORDER BY created_at DESC, id DESC
                   LIMIT ?1
               )",
            params![max as i64],
        )?)
    }
}

/// Map a SQL row into a [`Clip`].
fn row_to_clip(row: &Row<'_>) -> rusqlite::Result<Clip> {
    let kind_str: String = row.get(1)?;
    let kind = ClipKind::from_str(&kind_str).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown kind {kind_str:?}"),
            )),
        )
    })?;
    Ok(Clip {
        id: row.get(0)?,
        kind,
        content: row.get(2)?,
        preview: row.get(3)?,
        hash: row.get(4)?,
        created_at: row.get(5)?,
        pinned: row.get::<_, i64>(6)? != 0,
    })
}

/// Apply schema migrations to an existing database.
///
/// New databases are created with the current schema above; this brings
/// pre-existing ones (e.g. M1 databases without a `content` column) up to date.
fn migrate(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "clips", "content")? {
        conn.execute(
            "ALTER TABLE clips ADD COLUMN content TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

/// Whether `table` has a column named `column`.
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    // `table` is an internal literal, never user input.
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn collect(
    rows: rusqlite::MappedRows<'_, impl FnMut(&Row<'_>) -> rusqlite::Result<Clip>>,
) -> Result<Vec<Clip>> {
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Escape LIKE wildcards in user input so a search for "50%" is literal.
fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewClip;

    fn store() -> Store {
        Store::open_in_memory().expect("open in-memory store")
    }

    #[test]
    fn insert_then_count_and_recent() {
        let s = store();
        s.insert_at(&NewClip::text("first"), 100).unwrap();
        s.insert_at(&NewClip::text("second"), 200).unwrap();
        assert_eq!(s.count().unwrap(), 2);
        let recent = s.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].preview, "second"); // newest first
        assert_eq!(recent[1].preview, "first");
    }

    #[test]
    fn duplicate_is_collapsed_and_bumped() {
        let s = store();
        s.insert_at(&NewClip::text("dup"), 100).unwrap();
        s.insert_at(&NewClip::text("other"), 150).unwrap();
        s.insert_at(&NewClip::text("dup"), 200).unwrap(); // re-copy the first

        assert_eq!(s.count().unwrap(), 2, "no duplicate row created");
        let recent = s.recent(10).unwrap();
        assert_eq!(recent[0].preview, "dup", "re-copied clip moves to the top");
        assert_eq!(recent[0].created_at, 200);
    }

    #[test]
    fn pinned_clips_sort_first() {
        let s = store();
        let old = s.insert_at(&NewClip::text("old"), 100).unwrap();
        s.insert_at(&NewClip::text("new"), 200).unwrap();
        assert!(s.set_pinned(old, true).unwrap());

        let recent = s.recent(10).unwrap();
        assert_eq!(
            recent[0].preview, "old",
            "pinned clip is first despite being older"
        );
        assert!(recent[0].pinned);
    }

    #[test]
    fn search_filters_by_content_and_is_literal_for_wildcards() {
        let s = store();
        s.insert_at(&NewClip::text("hello world"), 100).unwrap();
        s.insert_at(&NewClip::text("goodbye"), 200).unwrap();
        s.insert_at(&NewClip::text("100% sure"), 300).unwrap();

        assert_eq!(s.search("hello", 10).unwrap().len(), 1);
        // "hello world" and "goodbye" contain 'o'; "100% sure" does not.
        assert_eq!(s.search("o", 10).unwrap().len(), 2);
        // "%" must be treated literally, not as SQL "match anything".
        let pct = s.search("100%", 10).unwrap();
        assert_eq!(pct.len(), 1);
        assert_eq!(pct[0].preview, "100% sure");
    }

    #[test]
    fn stores_and_returns_full_content_beyond_preview_length() {
        let s = store();
        // Longer than the preview cap, with a needle only in the tail.
        let long = format!("{}NEEDLE_TAIL", "x".repeat(500));
        s.insert_at(&NewClip::text(&long), 100).unwrap();

        let recent = s.recent(10).unwrap();
        assert_eq!(recent[0].content, long, "full content is preserved");
        assert!(
            recent[0].preview.chars().count() <= 201,
            "preview stays truncated"
        );
        // Search matches content past the preview cutoff.
        assert_eq!(s.search("NEEDLE_TAIL", 10).unwrap().len(), 1);
    }

    #[test]
    fn revision_changes_when_history_changes() {
        let s = store();
        assert_eq!(s.revision().unwrap(), (0, 0), "empty history");

        s.insert_at(&NewClip::text("one"), 100).unwrap();
        assert_eq!(s.revision().unwrap(), (1, 100));

        let id = s.insert_at(&NewClip::text("two"), 200).unwrap();
        assert_eq!(s.revision().unwrap(), (2, 200), "new clip moves the mark");

        // A re-copy bumps an existing clip: count is unchanged, newest moves.
        s.insert_at(&NewClip::text("one"), 300).unwrap();
        assert_eq!(s.revision().unwrap(), (2, 300), "bump is still a change");

        s.delete(id).unwrap();
        assert_eq!(s.revision().unwrap(), (1, 300), "deletion is a change");
    }

    #[test]
    fn migrates_pre_content_database() {
        // Simulate an M1-era database: clips table without a `content` column.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE clips (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 kind TEXT NOT NULL,
                 preview TEXT NOT NULL,
                 hash TEXT NOT NULL UNIQUE,
                 created_at INTEGER NOT NULL,
                 pinned INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO clips (kind, preview, hash, created_at, pinned)
             VALUES ('text', 'legacy', 'h1', 100, 0);",
        )
        .unwrap();
        assert!(!column_exists(&conn, "clips", "content").unwrap());

        // Opening through the store runs the migration.
        let s = Store::from_conn(conn).unwrap();
        assert!(column_exists(&s.conn, "clips", "content").unwrap());

        let recent = s.recent(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].preview, "legacy");
        assert_eq!(recent[0].content, "", "legacy rows get an empty content");
    }

    #[test]
    fn delete_and_clear() {
        let s = store();
        let a = s.insert_at(&NewClip::text("a"), 100).unwrap();
        s.insert_at(&NewClip::text("b"), 200).unwrap();
        assert!(s.delete(a).unwrap());
        assert!(!s.delete(a).unwrap(), "deleting again is a no-op");
        assert_eq!(s.count().unwrap(), 1);
        assert_eq!(s.clear().unwrap(), 1);
        assert_eq!(s.count().unwrap(), 0);
    }

    #[test]
    fn clear_unpinned_keeps_pins() {
        let s = store();
        let keep = s.insert_at(&NewClip::text("keep"), 100).unwrap();
        s.insert_at(&NewClip::text("drop"), 200).unwrap();
        s.set_pinned(keep, true).unwrap();

        assert_eq!(s.clear_unpinned().unwrap(), 1);
        let remaining = s.recent(10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].preview, "keep");
    }

    #[test]
    fn clips_from_the_same_millisecond_still_order_newest_first() {
        // Timestamps are millisecond-resolution, so a burst of copies can share
        // one. Without a tiebreaker the order would be whatever SQLite returns.
        let store = Store::open_in_memory().unwrap();
        let at = 1_700_000_000_000;
        for text in ["first", "second", "third"] {
            store.insert_at(&NewClip::text(text), at).unwrap();
        }
        let order: Vec<String> = store
            .recent(10)
            .unwrap()
            .iter()
            .map(|c| c.text().to_string())
            .collect();
        assert_eq!(order, vec!["third", "second", "first"]);
    }

    #[test]
    fn enforce_cap_keeps_newest_unpinned_plus_all_pins() {
        let s = store();
        // 5 unpinned clips at increasing timestamps
        for i in 0..5 {
            s.insert_at(&NewClip::text(&format!("clip{i}")), 100 + i as i64)
                .unwrap();
        }
        // pin the oldest
        let oldest = s.search("clip0", 1).unwrap()[0].id;
        s.set_pinned(oldest, true).unwrap();

        // cap unpinned to 2
        let removed = s.enforce_cap(2).unwrap();
        assert_eq!(
            removed, 2,
            "5 unpinned, keep newest 2 -> remove 2 (pin excluded)"
        );

        let remaining = s.recent(10).unwrap();
        // pinned clip0 + newest two unpinned (clip4, clip3)
        let previews: Vec<_> = remaining.iter().map(|c| c.preview.as_str()).collect();
        assert!(previews.contains(&"clip0"), "pinned clip survives the cap");
        assert!(previews.contains(&"clip4"));
        assert!(previews.contains(&"clip3"));
        assert_eq!(remaining.len(), 3);
    }
}
