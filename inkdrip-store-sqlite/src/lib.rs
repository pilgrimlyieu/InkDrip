mod migrations;

use std::fs;
use std::path::Path;
use std::result::Result as StdResult;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};
use rusqlite::Connection;
use tokio::sync::Mutex;

use inkdrip_core::error::{InkDripError, Result};
use inkdrip_core::model::{
    AggregateFeed, Book, BookFormat, Feed, FeedStatus, ScheduleConfig, Segment, SegmentRelease,
};
use inkdrip_core::store::BookStore;
use inkdrip_core::undo::UndoEntry;

/// SQLite-backed implementation of `BookStore`.
pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    /// Open (or create) a `SQLite` database at the given path.
    ///
    /// # Errors
    ///
    /// Returns `InkDripError::StorageError` if the database cannot be opened or
    /// the WAL pragmas cannot be set.
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure parent directories exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)
            .map_err(|e| InkDripError::StorageError(format!("Failed to open database: {e}")))?;

        // Enable WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| InkDripError::StorageError(format!("Failed to set pragmas: {e}")))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        Ok(store)
    }

    /// Run database migrations.
    ///
    /// # Errors
    ///
    /// Returns `InkDripError::StorageError` if a migration fails.
    pub async fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().await;
        migrations::run_migrations(&conn)
    }
}

#[async_trait]
impl BookStore for SqliteStore {
    // ─── Books ──────────────────────────────────────────────────

    async fn save_book(&self, book: &Book) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO books (id, title, author, format, file_hash, file_path, total_words, total_segments, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                book.id,
                book.title,
                book.author,
                book.format.as_str(),
                book.file_hash,
                book.file_path,
                book.total_words,
                book.total_segments,
                book.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| InkDripError::StorageError(format!("Failed to save book: {e}")))?;
        Ok(())
    }

    async fn get_book(&self, id: &str) -> Result<Option<Book>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT id, title, author, format, file_hash, file_path, total_words, total_segments, created_at FROM books WHERE id = ?1 AND deleted_at IS NULL")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let result = stmt
            .query_row(rusqlite::params![id], |row| Ok(row_to_book(row)))
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        match result {
            Some(book) => Ok(Some(book?)),
            None => Ok(None),
        }
    }

    async fn get_book_by_hash(&self, file_hash: &str) -> Result<Option<Book>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT id, title, author, format, file_hash, file_path, total_words, total_segments, created_at FROM books WHERE file_hash = ?1 AND deleted_at IS NULL")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let result = stmt
            .query_row(rusqlite::params![file_hash], |row| Ok(row_to_book(row)))
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        match result {
            Some(book) => Ok(Some(book?)),
            None => Ok(None),
        }
    }

    async fn list_books(&self) -> Result<Vec<Book>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT id, title, author, format, file_hash, file_path, total_words, total_segments, created_at FROM books WHERE deleted_at IS NULL ORDER BY created_at DESC")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let books = stmt
            .query_map([], |row| Ok(row_to_book(row)))
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        books.into_iter().collect::<Result<Vec<_>>>()
    }

    async fn delete_book(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        // CASCADE handles segments, feeds, and segment_releases automatically
        conn.execute("DELETE FROM books WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn update_book_meta(&self, id: &str, title: &str, author: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE books SET title = ?1, author = ?2 WHERE id = ?3",
            rusqlite::params![title, author, id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn resolve_book_id(&self, prefix: &str) -> Result<String> {
        let conn = self.conn.lock().await;
        let pattern = format!("{prefix}%");
        let mut stmt = conn
            .prepare("SELECT id FROM books WHERE id LIKE ?1 AND deleted_at IS NULL")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        let ids: Vec<String> = stmt
            .query_map(rusqlite::params![pattern], |row| row.get(0))
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        match ids.len() {
            0 => Err(InkDripError::BookNotFound(prefix.to_owned())),
            1 => Ok(ids.into_iter().next().unwrap_or_default()),
            _ => Err(InkDripError::AmbiguousId(prefix.to_owned())),
        }
    }

    // ─── Segments ───────────────────────────────────────────────

    async fn save_segments(&self, segments: &[Segment]) -> Result<()> {
        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO segments (id, book_id, idx, title_context, content_html, word_count, cumulative_words)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .map_err(|e| InkDripError::StorageError(e.to_string()))?;

            for seg in segments {
                stmt.execute(rusqlite::params![
                    seg.id,
                    seg.book_id,
                    seg.index,
                    seg.title_context,
                    seg.content_html,
                    seg.word_count,
                    seg.cumulative_words,
                ])
                .map_err(|e| InkDripError::StorageError(format!("Failed to save segment: {e}")))?;
            }
        }

        tx.commit()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn get_segments(&self, book_id: &str) -> Result<Vec<Segment>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, book_id, idx, title_context, content_html, word_count, cumulative_words
                 FROM segments WHERE book_id = ?1 ORDER BY idx",
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let segments = stmt
            .query_map(rusqlite::params![book_id], |row| {
                Ok(Segment {
                    id: row.get(0)?,
                    book_id: row.get(1)?,
                    index: row.get(2)?,
                    title_context: row.get(3)?,
                    content_html: row.get(4)?,
                    word_count: row.get(5)?,
                    cumulative_words: row.get(6)?,
                })
            })
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        Ok(segments)
    }

    async fn get_segment(&self, id: &str) -> Result<Option<Segment>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, book_id, idx, title_context, content_html, word_count, cumulative_words
                 FROM segments WHERE id = ?1",
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let result = stmt
            .query_row(rusqlite::params![id], |row| {
                Ok(Segment {
                    id: row.get(0)?,
                    book_id: row.get(1)?,
                    index: row.get(2)?,
                    title_context: row.get(3)?,
                    content_html: row.get(4)?,
                    word_count: row.get(5)?,
                    cumulative_words: row.get(6)?,
                })
            })
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        Ok(result)
    }

    async fn delete_segments_for_book(&self, book_id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM segments WHERE book_id = ?1",
            rusqlite::params![book_id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    // ─── Feeds ──────────────────────────────────────────────────

    async fn save_feed(&self, feed: &Feed) -> Result<()> {
        let conn = self.conn.lock().await;
        let schedule_json = serde_json::to_string(&feed.schedule_config)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        conn.execute(
            "INSERT INTO feeds (id, book_id, slug, schedule_config, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                feed.id,
                feed.book_id,
                feed.slug,
                schedule_json,
                feed.status.as_str(),
                feed.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| InkDripError::StorageError(format!("Failed to save feed: {e}")))?;
        Ok(())
    }

    async fn get_feed(&self, id: &str) -> Result<Option<Feed>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT id, book_id, slug, schedule_config, status, created_at FROM feeds WHERE id = ?1 AND deleted_at IS NULL")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let result = stmt
            .query_row(rusqlite::params![id], |row| Ok(row_to_feed(row)))
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        match result {
            Some(feed) => Ok(Some(feed?)),
            None => Ok(None),
        }
    }

    async fn get_feed_by_slug(&self, slug: &str) -> Result<Option<Feed>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT id, book_id, slug, schedule_config, status, created_at FROM feeds WHERE slug = ?1 AND deleted_at IS NULL")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let result = stmt
            .query_row(rusqlite::params![slug], |row| Ok(row_to_feed(row)))
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        match result {
            Some(feed) => Ok(Some(feed?)),
            None => Ok(None),
        }
    }

    async fn list_feeds(&self) -> Result<Vec<Feed>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT id, book_id, slug, schedule_config, status, created_at FROM feeds WHERE deleted_at IS NULL ORDER BY created_at DESC")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let feeds = stmt
            .query_map([], |row| Ok(row_to_feed(row)))
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        feeds.into_iter().collect::<Result<Vec<_>>>()
    }

    async fn list_feeds_for_book(&self, book_id: &str) -> Result<Vec<Feed>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT id, book_id, slug, schedule_config, status, created_at FROM feeds WHERE book_id = ?1 AND deleted_at IS NULL ORDER BY created_at DESC")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let feeds = stmt
            .query_map(rusqlite::params![book_id], |row| Ok(row_to_feed(row)))
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        feeds.into_iter().collect::<Result<Vec<_>>>()
    }

    async fn update_feed_status(&self, id: &str, status: FeedStatus) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE feeds SET status = ?1 WHERE id = ?2",
            rusqlite::params![status.as_str(), id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn update_feed_schedule(&self, id: &str, config: &ScheduleConfig) -> Result<()> {
        let conn = self.conn.lock().await;
        let json =
            serde_json::to_string(config).map_err(|e| InkDripError::StorageError(e.to_string()))?;
        conn.execute(
            "UPDATE feeds SET schedule_config = ?1 WHERE id = ?2",
            rusqlite::params![json, id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn delete_feed(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        // CASCADE handles segment_releases automatically
        conn.execute("DELETE FROM feeds WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn update_feed_slug(&self, id: &str, slug: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE feeds SET slug = ?1 WHERE id = ?2",
            rusqlite::params![slug, id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn resolve_feed_id(&self, prefix: &str) -> Result<String> {
        let conn = self.conn.lock().await;
        let pattern = format!("{prefix}%");
        let mut stmt = conn
            .prepare("SELECT id FROM feeds WHERE id LIKE ?1 AND deleted_at IS NULL")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        let ids: Vec<String> = stmt
            .query_map(rusqlite::params![pattern], |row| row.get(0))
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        match ids.len() {
            0 => Err(InkDripError::FeedNotFound(prefix.to_owned())),
            1 => Ok(ids.into_iter().next().unwrap_or_default()),
            _ => Err(InkDripError::AmbiguousId(prefix.to_owned())),
        }
    }

    // ─── Segment Releases ───────────────────────────────────────

    async fn save_releases(&self, releases: &[SegmentRelease]) -> Result<()> {
        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO segment_releases (segment_id, feed_id, release_at)
                     VALUES (?1, ?2, ?3)",
                )
                .map_err(|e| InkDripError::StorageError(e.to_string()))?;

            for r in releases {
                stmt.execute(rusqlite::params![
                    r.segment_id,
                    r.feed_id,
                    r.release_at.to_utc().to_rfc3339(),
                ])
                .map_err(|e| InkDripError::StorageError(format!("Failed to save release: {e}")))?;
            }
        }

        tx.commit()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn delete_releases_for_feed(&self, feed_id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM segment_releases WHERE feed_id = ?1",
            rusqlite::params![feed_id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn get_releases_for_feed(&self, feed_id: &str) -> Result<Vec<SegmentRelease>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT segment_id, feed_id, release_at FROM segment_releases WHERE feed_id = ?1 ORDER BY release_at",
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let results = stmt
            .query_map(rusqlite::params![feed_id], |row| {
                let release_at_str: String = row.get(2)?;
                Ok(SegmentRelease {
                    segment_id: row.get(0)?,
                    feed_id: row.get(1)?,
                    release_at: DateTime::parse_from_rfc3339(&release_at_str)
                        .unwrap_or_else(|_| chrono::Utc::now().into()),
                })
            })
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        Ok(results)
    }

    async fn delete_future_releases_for_feed(
        &self,
        feed_id: &str,
        after: DateTime<FixedOffset>,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM segment_releases WHERE feed_id = ?1 AND release_at > ?2",
            rusqlite::params![feed_id, after.to_utc().to_rfc3339()],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn get_unreleased_segments_for_feed(
        &self,
        feed_id: &str,
        after: DateTime<FixedOffset>,
        limit: u32,
    ) -> Result<Vec<(Segment, SegmentRelease)>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT s.id, s.book_id, s.idx, s.title_context, s.content_html, s.word_count, s.cumulative_words,
                        sr.segment_id, sr.feed_id, sr.release_at
                 FROM segment_releases sr
                 JOIN segments s ON sr.segment_id = s.id
                 WHERE sr.feed_id = ?1 AND sr.release_at > ?2
                 ORDER BY s.idx ASC
                 LIMIT ?3",
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let results = stmt
            .query_map(
                rusqlite::params![feed_id, after.to_utc().to_rfc3339(), limit],
                |row| {
                    let segment = Segment {
                        id: row.get(0)?,
                        book_id: row.get(1)?,
                        index: row.get(2)?,
                        title_context: row.get(3)?,
                        content_html: row.get(4)?,
                        word_count: row.get(5)?,
                        cumulative_words: row.get(6)?,
                    };
                    let release_at_str: String = row.get(9)?;
                    let release = SegmentRelease {
                        segment_id: row.get(7)?,
                        feed_id: row.get(8)?,
                        release_at: DateTime::parse_from_rfc3339(&release_at_str)
                            .unwrap_or_else(|_| chrono::Utc::now().into()),
                    };
                    Ok((segment, release))
                },
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        Ok(results)
    }

    async fn get_max_released_index_for_book(
        &self,
        book_id: &str,
        before: DateTime<FixedOffset>,
    ) -> Result<Option<u32>> {
        let conn = self.conn.lock().await;
        let result: Option<u32> = conn
            .query_row(
                "SELECT MAX(s.idx)
                 FROM segments s
                 JOIN segment_releases sr ON sr.segment_id = s.id
                 JOIN feeds f ON sr.feed_id = f.id
                 WHERE f.book_id = ?1 AND sr.release_at <= ?2",
                rusqlite::params![book_id, before.to_utc().to_rfc3339()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .flatten();
        Ok(result)
    }

    async fn delete_segments_from_index(&self, book_id: &str, from_index: u32) -> Result<()> {
        let conn = self.conn.lock().await;
        // CASCADE on segments handles segment_releases automatically
        conn.execute(
            "DELETE FROM segments WHERE book_id = ?1 AND idx >= ?2",
            rusqlite::params![book_id, from_index],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn update_book_segment_count(&self, id: &str, total_segments: u32) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE books SET total_segments = ?1 WHERE id = ?2",
            rusqlite::params![total_segments, id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn get_released_segments(
        &self,
        feed_id: &str,
        before: DateTime<FixedOffset>,
        limit: u32,
    ) -> Result<Vec<(Segment, SegmentRelease)>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT s.id, s.book_id, s.idx, s.title_context, s.content_html, s.word_count, s.cumulative_words,
                        sr.segment_id, sr.feed_id, sr.release_at
                 FROM segment_releases sr
                 JOIN segments s ON sr.segment_id = s.id
                 WHERE sr.feed_id = ?1 AND sr.release_at <= ?2
                 ORDER BY s.idx ASC
                 LIMIT ?3",
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let results = stmt
            .query_map(
                rusqlite::params![feed_id, before.to_utc().to_rfc3339(), limit],
                |row| {
                    let segment = Segment {
                        id: row.get(0)?,
                        book_id: row.get(1)?,
                        index: row.get(2)?,
                        title_context: row.get(3)?,
                        content_html: row.get(4)?,
                        word_count: row.get(5)?,
                        cumulative_words: row.get(6)?,
                    };
                    let release_at_str: String = row.get(9)?;
                    let release = SegmentRelease {
                        segment_id: row.get(7)?,
                        feed_id: row.get(8)?,
                        release_at: DateTime::parse_from_rfc3339(&release_at_str)
                            .unwrap_or_else(|_| chrono::Utc::now().into()),
                    };
                    Ok((segment, release))
                },
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        Ok(results)
    }

    async fn count_released_segments(
        &self,
        feed_id: &str,
        before: DateTime<FixedOffset>,
    ) -> Result<u32> {
        let conn = self.conn.lock().await;
        let count: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM segment_releases WHERE feed_id = ?1 AND release_at <= ?2",
                rusqlite::params![feed_id, before.to_utc().to_rfc3339()],
                |row| row.get(0),
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(count)
    }

    async fn advance_releases(
        &self,
        feed_id: &str,
        count: u32,
        release_at: DateTime<FixedOffset>,
    ) -> Result<u32> {
        let conn = self.conn.lock().await;
        let now_str = release_at.to_utc().to_rfc3339();
        let changed = conn
            .execute(
                "UPDATE segment_releases
                 SET release_at = ?3
                 WHERE rowid IN (
                     SELECT sr.rowid
                     FROM segment_releases sr
                     JOIN segments s ON sr.segment_id = s.id
                     WHERE sr.feed_id = ?1 AND sr.release_at > ?3
                     ORDER BY s.idx ASC
                     LIMIT ?2
                 )",
                rusqlite::params![feed_id, count, now_str],
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(u32::try_from(changed).unwrap_or(0))
    }

    async fn get_segment_by_index(&self, book_id: &str, index: u32) -> Result<Option<Segment>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, book_id, idx, title_context, content_html, word_count, cumulative_words
                 FROM segments WHERE book_id = ?1 AND idx = ?2",
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let result = stmt
            .query_row(rusqlite::params![book_id, index], |row| {
                Ok(Segment {
                    id: row.get(0)?,
                    book_id: row.get(1)?,
                    index: row.get(2)?,
                    title_context: row.get(3)?,
                    content_html: row.get(4)?,
                    word_count: row.get(5)?,
                    cumulative_words: row.get(6)?,
                })
            })
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        Ok(result)
    }

    // ─── Aggregate Feeds ────────────────────────────────────────

    async fn save_aggregate_feed(&self, agg: &AggregateFeed) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO aggregate_feeds (id, slug, title, description, include_all, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                agg.id,
                agg.slug,
                agg.title,
                agg.description,
                agg.include_all,
                agg.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn get_aggregate_feed(&self, id: &str) -> Result<Option<AggregateFeed>> {
        let conn = self.conn.lock().await;
        let result = conn
            .query_row(
                "SELECT id, slug, title, description, include_all, created_at
                 FROM aggregate_feeds WHERE id = ?1",
                [id],
                row_to_aggregate_feed,
            )
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(result)
    }

    async fn get_aggregate_feed_by_slug(&self, slug: &str) -> Result<Option<AggregateFeed>> {
        let conn = self.conn.lock().await;
        let result = conn
            .query_row(
                "SELECT id, slug, title, description, include_all, created_at
                 FROM aggregate_feeds WHERE slug = ?1",
                [slug],
                row_to_aggregate_feed,
            )
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(result)
    }

    async fn list_aggregate_feeds(&self) -> Result<Vec<AggregateFeed>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, slug, title, description, include_all, created_at
                 FROM aggregate_feeds ORDER BY created_at DESC",
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let rows = stmt
            .query_map([], row_to_aggregate_feed)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let mut feeds = Vec::new();
        for row in rows {
            feeds.push(row.map_err(|e| InkDripError::StorageError(e.to_string()))?);
        }
        Ok(feeds)
    }

    async fn update_aggregate_feed(
        &self,
        id: &str,
        title: &str,
        description: &str,
        include_all: bool,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE aggregate_feeds SET title = ?1, description = ?2, include_all = ?3 WHERE id = ?4",
            rusqlite::params![title, description, include_all, id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn delete_aggregate_feed(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM aggregate_feeds WHERE id = ?1", [id])
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn upsert_aggregate_feed(&self, agg: &AggregateFeed) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO aggregate_feeds (id, slug, title, description, include_all, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(slug) DO UPDATE SET title = excluded.title, description = excluded.description, include_all = excluded.include_all",
            rusqlite::params![
                agg.id,
                agg.slug,
                agg.title,
                agg.description,
                agg.include_all,
                agg.created_at.to_rfc3339(),
            ],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn add_aggregate_source(&self, aggregate_id: &str, feed_id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT OR IGNORE INTO aggregate_feed_sources (aggregate_id, feed_id) VALUES (?1, ?2)",
            rusqlite::params![aggregate_id, feed_id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn remove_aggregate_source(&self, aggregate_id: &str, feed_id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM aggregate_feed_sources WHERE aggregate_id = ?1 AND feed_id = ?2",
            rusqlite::params![aggregate_id, feed_id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn list_aggregate_sources(&self, aggregate_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare("SELECT feed_id FROM aggregate_feed_sources WHERE aggregate_id = ?1")
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let rows = stmt
            .query_map([aggregate_id], |row| row.get(0))
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| InkDripError::StorageError(e.to_string()))?);
        }
        Ok(ids)
    }

    async fn get_aggregate_released_segments(
        &self,
        aggregate_id: &str,
        include_all: bool,
        before: DateTime<FixedOffset>,
        limit: u32,
    ) -> Result<Vec<(Segment, SegmentRelease, Book)>> {
        let conn = self.conn.lock().await;
        let before_str = before.to_utc().to_rfc3339();

        let sql = if include_all {
            "SELECT s.id, s.book_id, s.idx, s.title_context, s.content_html, s.word_count, s.cumulative_words,
                    sr.segment_id, sr.feed_id, sr.release_at,
                    b.id, b.title, b.author, b.format, b.file_hash, b.file_path, b.total_words, b.total_segments, b.created_at
             FROM segment_releases sr
             JOIN segments s ON s.id = sr.segment_id
             JOIN feeds f ON f.id = sr.feed_id
             JOIN books b ON b.id = s.book_id
             WHERE sr.release_at <= ?1 AND f.status = 'active' AND f.deleted_at IS NULL AND b.deleted_at IS NULL
             ORDER BY sr.release_at DESC
             LIMIT ?2".to_owned()
        } else {
            "SELECT s.id, s.book_id, s.idx, s.title_context, s.content_html, s.word_count, s.cumulative_words,
                    sr.segment_id, sr.feed_id, sr.release_at,
                    b.id, b.title, b.author, b.format, b.file_hash, b.file_path, b.total_words, b.total_segments, b.created_at
             FROM segment_releases sr
             JOIN segments s ON s.id = sr.segment_id
             JOIN aggregate_feed_sources afs ON afs.feed_id = sr.feed_id AND afs.aggregate_id = ?3
             JOIN books b ON b.id = s.book_id
             WHERE sr.release_at <= ?1 AND b.deleted_at IS NULL
             ORDER BY sr.release_at DESC
             LIMIT ?2".to_owned()
        };

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let map_row = |row: &rusqlite::Row| -> rusqlite::Result<(Segment, SegmentRelease, Book)> {
            let segment = Segment {
                id: row.get(0)?,
                book_id: row.get(1)?,
                index: row.get(2)?,
                title_context: row.get(3)?,
                content_html: row.get(4)?,
                word_count: row.get(5)?,
                cumulative_words: row.get(6)?,
            };
            let release_at_str: String = row.get(9)?;
            let release = SegmentRelease {
                segment_id: row.get(7)?,
                feed_id: row.get(8)?,
                release_at: DateTime::parse_from_rfc3339(&release_at_str)
                    .unwrap_or_else(|_| chrono::Utc::now().into()),
            };
            let format_str: String = row.get(13)?;
            let format = match format_str.as_str() {
                "epub" => BookFormat::Epub,
                "txt" => BookFormat::Txt,
                _ => BookFormat::Markdown,
            };
            let book_created_str: String = row.get(18)?;
            let book = Book {
                id: row.get(10)?,
                title: row.get(11)?,
                author: row.get(12)?,
                format,
                file_hash: row.get(14)?,
                file_path: row.get(15)?,
                total_words: row.get(16)?,
                total_segments: row.get(17)?,
                created_at: DateTime::parse_from_rfc3339(&book_created_str)
                    .unwrap_or_else(|_| chrono::Utc::now().into()),
            };
            Ok((segment, release, book))
        };

        let rows = if include_all {
            stmt.query_map(rusqlite::params![before_str, limit], map_row)
        } else {
            stmt.query_map(rusqlite::params![before_str, limit, aggregate_id], map_row)
        }
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| InkDripError::StorageError(e.to_string()))?);
        }
        Ok(results)
    }

    // ─── Soft-Delete ────────────────────────────────────────────

    async fn soft_delete_book(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE books SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            rusqlite::params![now, id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        // Cascade to feeds belonging to this book
        conn.execute(
            "UPDATE feeds SET deleted_at = ?1 WHERE book_id = ?2 AND deleted_at IS NULL",
            rusqlite::params![now, id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn restore_book(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE books SET deleted_at = NULL WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        // Restore feeds belonging to this book
        conn.execute(
            "UPDATE feeds SET deleted_at = NULL WHERE book_id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn soft_delete_feed(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE feeds SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            rusqlite::params![now, id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn restore_feed(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE feeds SET deleted_at = NULL WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    // ─── Undo/Redo Log ─────────────────────────────────────────

    async fn push_undo_entry(
        &self,
        operation: &str,
        summary: &str,
        payload: &serde_json::Value,
        max_depth: u32,
    ) -> Result<i64> {
        let conn = self.conn.lock().await;
        let now = chrono::Utc::now().to_rfc3339();
        let payload_str = serde_json::to_string(payload)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        // Get current cursor position
        let cursor_id: i64 = tx
            .query_row("SELECT current_id FROM undo_cursor WHERE id = 1", [], |r| {
                r.get(0)
            })
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        // Collect entries beyond cursor (redo chain) for cleanup
        let orphan_payloads = collect_orphan_payloads(&tx, cursor_id)?;

        // Truncate redo chain: delete entries after cursor
        tx.execute(
            "DELETE FROM undo_log WHERE id > ?1",
            rusqlite::params![cursor_id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        // Hard-delete resources orphaned by redo chain truncation
        for op_payload in &orphan_payloads {
            hard_delete_orphaned_resource(&tx, op_payload)?;
        }

        // Insert new entry
        tx.execute(
            "INSERT INTO undo_log (operation, summary, created_at, payload) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![operation, summary, now, payload_str],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let new_id = tx.last_insert_rowid();

        // Advance cursor to the new entry
        tx.execute(
            "UPDATE undo_cursor SET current_id = ?1 WHERE id = 1",
            rusqlite::params![new_id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        // Prune oldest entries beyond max_depth
        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM undo_log", [], |r| r.get(0))
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        if count > i64::from(max_depth) {
            let excess = count - i64::from(max_depth);
            let pruned_payloads = collect_pruned_payloads(&tx, excess)?;

            tx.execute(
                "DELETE FROM undo_log WHERE id IN (SELECT id FROM undo_log ORDER BY id ASC LIMIT ?1)",
                rusqlite::params![excess],
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

            // Hard-delete resources from pruned entries (expired soft-deletes)
            for op_payload in &pruned_payloads {
                hard_delete_pruned_resource(&tx, op_payload)?;
            }
        }

        tx.commit()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(new_id)
    }

    async fn get_undo_entry(&self) -> Result<Option<UndoEntry>> {
        let conn = self.conn.lock().await;
        let cursor_id: i64 = conn
            .query_row("SELECT current_id FROM undo_cursor WHERE id = 1", [], |r| {
                r.get(0)
            })
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        if cursor_id == 0 {
            return Ok(None);
        }

        let entry = conn
            .query_row(
                "SELECT id, operation, summary, created_at, payload FROM undo_log WHERE id = ?1",
                rusqlite::params![cursor_id],
                row_to_undo_entry,
            )
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(entry)
    }

    async fn retreat_undo_cursor(&self) -> Result<()> {
        let conn = self.conn.lock().await;
        let cursor_id: i64 = conn
            .query_row("SELECT current_id FROM undo_cursor WHERE id = 1", [], |r| {
                r.get(0)
            })
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        // Find the previous entry (the one before cursor_id)
        let prev_id: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(id), 0) FROM undo_log WHERE id < ?1",
                rusqlite::params![cursor_id],
                |r| r.get(0),
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        conn.execute(
            "UPDATE undo_cursor SET current_id = ?1 WHERE id = 1",
            rusqlite::params![prev_id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn get_redo_entry(&self) -> Result<Option<UndoEntry>> {
        let conn = self.conn.lock().await;
        let cursor_id: i64 = conn
            .query_row("SELECT current_id FROM undo_cursor WHERE id = 1", [], |r| {
                r.get(0)
            })
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let entry = conn
            .query_row(
                "SELECT id, operation, summary, created_at, payload FROM undo_log WHERE id > ?1 ORDER BY id ASC LIMIT 1",
                rusqlite::params![cursor_id],
                row_to_undo_entry,
            )
            .optional()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(entry)
    }

    async fn advance_undo_cursor(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE undo_cursor SET current_id = ?1 WHERE id = 1",
            rusqlite::params![id],
        )
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn list_undo_history(&self, limit: u32) -> Result<Vec<UndoEntry>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT id, operation, summary, created_at, payload FROM undo_log ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        let entries = stmt
            .query_map(rusqlite::params![limit], row_to_undo_entry)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;
        Ok(entries)
    }

    async fn clear_history(&self) -> Result<()> {
        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| InkDripError::StorageError(e.to_string()))?;

        // Hard-delete soft-deleted feeds first (before books, to avoid FK cascade
        // from books removing feeds that we explicitly want to clean up here).
        tx.execute("DELETE FROM feeds WHERE deleted_at IS NOT NULL", [])
            .map_err(|e| InkDripError::StorageError(format!("clear_history: feeds: {e}")))?;

        // Hard-delete soft-deleted books (cascade removes their segments).
        tx.execute("DELETE FROM books WHERE deleted_at IS NOT NULL", [])
            .map_err(|e| InkDripError::StorageError(format!("clear_history: books: {e}")))?;

        // Wipe the undo log and reset the cursor.
        tx.execute("DELETE FROM undo_log", [])
            .map_err(|e| InkDripError::StorageError(format!("clear_history: undo_log: {e}")))?;

        tx.execute("UPDATE undo_cursor SET current_id = 0 WHERE id = 1", [])
            .map_err(|e| InkDripError::StorageError(format!("clear_history: cursor: {e}")))?;

        tx.commit()
            .map_err(|e| InkDripError::StorageError(format!("clear_history: commit: {e}")))?;

        Ok(())
    }
}

// ─── Helper functions ───────────────────────────────────────────

fn row_to_book(row: &rusqlite::Row) -> Result<Book> {
    let format_str: String = row
        .get(3)
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
    let format = match format_str.as_str() {
        "epub" => BookFormat::Epub,
        "txt" => BookFormat::Txt,
        "markdown" => BookFormat::Markdown,
        _ => {
            return Err(InkDripError::StorageError(format!(
                "Unknown format: {format_str}"
            )));
        }
    };

    let created_at_str: String = row
        .get(8)
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;

    Ok(Book {
        id: row
            .get(0)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        title: row
            .get(1)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        author: row
            .get(2)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        format,
        file_hash: row
            .get(4)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        file_path: row
            .get(5)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        total_words: row
            .get(6)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        total_segments: row
            .get(7)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        created_at: DateTime::parse_from_rfc3339(&created_at_str)
            .unwrap_or_else(|_| chrono::Utc::now().into()),
    })
}

fn row_to_feed(row: &rusqlite::Row) -> Result<Feed> {
    let schedule_json: String = row
        .get(3)
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
    let schedule_config: ScheduleConfig = serde_json::from_str(&schedule_json)
        .map_err(|e| InkDripError::StorageError(format!("Invalid schedule config: {e}")))?;

    let status_str: String = row
        .get(4)
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
    let status: FeedStatus = status_str
        .parse()
        .map_err(|e: String| InkDripError::StorageError(e))?;

    let created_at_str: String = row
        .get(5)
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;

    Ok(Feed {
        id: row
            .get(0)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        book_id: row
            .get(1)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        slug: row
            .get(2)
            .map_err(|e| InkDripError::StorageError(e.to_string()))?,
        schedule_config,
        status,
        created_at: DateTime::parse_from_rfc3339(&created_at_str)
            .unwrap_or_else(|_| chrono::Utc::now().into()),
    })
}

fn row_to_aggregate_feed(row: &rusqlite::Row) -> rusqlite::Result<AggregateFeed> {
    let created_at_str: String = row.get(5)?;
    Ok(AggregateFeed {
        id: row.get(0)?,
        slug: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        include_all: row.get(4)?,
        created_at: DateTime::parse_from_rfc3339(&created_at_str)
            .unwrap_or_else(|_| chrono::Utc::now().into()),
    })
}

fn row_to_undo_entry(row: &rusqlite::Row) -> rusqlite::Result<UndoEntry> {
    let created_at_str: String = row.get(3)?;
    let payload_str: String = row.get(4)?;
    Ok(UndoEntry {
        id: row.get(0)?,
        operation: row.get(1)?,
        summary: row.get(2)?,
        created_at: DateTime::parse_from_rfc3339(&created_at_str)
            .unwrap_or_else(|_| chrono::Utc::now().into()),
        payload: serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null),
    })
}

/// Collect payload strings for entries beyond the cursor (redo chain) before deletion.
fn collect_orphan_payloads(tx: &Connection, cursor_id: i64) -> Result<Vec<String>> {
    let mut stmt = tx
        .prepare("SELECT payload FROM undo_log WHERE id > ?1")
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![cursor_id], |row| row.get(0))
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
    let mut payloads = Vec::new();
    for row in rows {
        payloads.push(row.map_err(|e| InkDripError::StorageError(e.to_string()))?);
    }
    Ok(payloads)
}

/// Collect payload strings for the oldest `count` entries before pruning.
fn collect_pruned_payloads(tx: &Connection, count: i64) -> Result<Vec<String>> {
    let mut stmt = tx
        .prepare("SELECT payload FROM undo_log ORDER BY id ASC LIMIT ?1")
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
    let rows = stmt
        .query_map(rusqlite::params![count], |row| row.get(0))
        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
    let mut payloads = Vec::new();
    for row in rows {
        payloads.push(row.map_err(|e| InkDripError::StorageError(e.to_string()))?);
    }
    Ok(payloads)
}

/// When redo chain is truncated, hard-delete resources that were created and then
/// undone — their redo path is now unreachable.
fn hard_delete_orphaned_resource(tx: &Connection, payload_str: &str) -> Result<()> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload_str) {
        match val.get("op").and_then(|v| v.as_str()) {
            // CreateFeed was undone → feed is soft-deleted, redo is gone → hard-delete
            Some("CreateFeed") => {
                if let Some(id) = val.get("feed_id").and_then(|v| v.as_str()) {
                    tx.execute("DELETE FROM feeds WHERE id = ?1", rusqlite::params![id])
                        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
                }
            }
            // UploadBook was undone → book is soft-deleted, redo is gone → hard-delete
            Some("UploadBook") => {
                if let Some(id) = val.get("book_id").and_then(|v| v.as_str()) {
                    tx.execute("DELETE FROM books WHERE id = ?1", rusqlite::params![id])
                        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// When old entries are pruned, hard-delete resources that were deleted and are now
/// beyond the undoable window.
fn hard_delete_pruned_resource(tx: &Connection, payload_str: &str) -> Result<()> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload_str) {
        match val.get("op").and_then(|v| v.as_str()) {
            // DeleteBook was pruned → soft-deleted book is beyond undo horizon → hard-delete
            Some("DeleteBook") => {
                if let Some(id) = val.get("book_id").and_then(|v| v.as_str()) {
                    tx.execute("DELETE FROM books WHERE id = ?1", rusqlite::params![id])
                        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
                }
            }
            // DeleteFeed was pruned → soft-deleted feed is beyond undo horizon → hard-delete
            Some("DeleteFeed") => {
                if let Some(id) = val.get("feed_id").and_then(|v| v.as_str()) {
                    tx.execute("DELETE FROM feeds WHERE id = ?1", rusqlite::params![id])
                        .map_err(|e| InkDripError::StorageError(e.to_string()))?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Required by rusqlite for optional query results.
trait OptionalExt<T> {
    fn optional(self) -> StdResult<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for StdResult<T, rusqlite::Error> {
    fn optional(self) -> StdResult<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone, Utc};

    use inkdrip_core::model::{
        Book, BookFormat, BudgetMode, Feed, FeedStatus, ScheduleConfig, Segment, SegmentRelease,
        SkipDays,
    };
    use inkdrip_core::store::BookStore;

    use super::*;

    async fn setup_store() -> SqliteStore {
        let store = SqliteStore {
            conn: Arc::new(Mutex::new(Connection::open_in_memory().unwrap())),
        };
        store.migrate().await.unwrap();
        store
    }

    fn make_book(id: &str) -> Book {
        Book {
            id: id.to_owned(),
            title: "Test Book".to_owned(),
            author: "Author".to_owned(),
            format: BookFormat::Epub,
            file_hash: format!("hash_{id}"),
            file_path: format!("/data/{id}.epub"),
            total_words: 10_000,
            total_segments: 5,
            created_at: Utc::now().into(),
        }
    }

    fn make_segments(book_id: &str, count: u32) -> Vec<Segment> {
        let mut cumulative = 0u32;
        (0..count)
            .map(|i| {
                let wc = 1000;
                cumulative += wc;
                Segment {
                    id: format!("seg-{book_id}-{i}"),
                    book_id: book_id.to_owned(),
                    index: i,
                    title_context: format!("Chapter {}", i + 1),
                    content_html: format!("<p>Content of segment {i}</p>"),
                    word_count: wc,
                    cumulative_words: cumulative,
                }
            })
            .collect()
    }

    fn tz8() -> FixedOffset {
        FixedOffset::east_opt(8 * 3600).unwrap()
    }

    fn make_feed(id: &str, book_id: &str) -> Feed {
        Feed {
            id: id.to_owned(),
            book_id: book_id.to_owned(),
            slug: format!("test-{id}"),
            schedule_config: ScheduleConfig {
                start_at: tz8().with_ymd_and_hms(2026, 1, 1, 8, 0, 0).unwrap(),
                words_per_day: 2000,
                delivery_time: "08:00".to_owned(),
                skip_days: SkipDays::empty(),
                timezone: "UTC+8".to_owned(),
                budget_mode: BudgetMode::Strict,
            },
            status: FeedStatus::Active,
            created_at: Utc::now().into(),
        }
    }

    // ─── Book CRUD ──────────────────────────────────────────────

    #[tokio::test]
    async fn save_and_get_book() {
        let store = setup_store().await;
        let book = make_book("book1234");
        store.save_book(&book).await.unwrap();

        let got = store.get_book("book1234").await.unwrap().unwrap();
        assert_eq!(got.id, "book1234");
        assert_eq!(got.title, "Test Book");
    }

    #[tokio::test]
    async fn list_books_empty() {
        let store = setup_store().await;
        let books = store.list_books().await.unwrap();
        assert!(books.is_empty());
    }

    #[tokio::test]
    async fn delete_book_cascade() {
        let store = setup_store().await;
        let book = make_book("book1234");
        store.save_book(&book).await.unwrap();
        store.delete_book("book1234").await.unwrap();
        assert!(store.get_book("book1234").await.unwrap().is_none());
    }

    // ─── ID resolution ─────────────────────────────────────────

    #[tokio::test]
    async fn resolve_book_id_exact_match() {
        let store = setup_store().await;
        store.save_book(&make_book("abcd1234")).await.unwrap();
        let resolved = store.resolve_book_id("abcd1234").await.unwrap();
        assert_eq!(resolved, "abcd1234");
    }

    #[tokio::test]
    async fn resolve_book_id_prefix_match() {
        let store = setup_store().await;
        store.save_book(&make_book("abcd1234")).await.unwrap();
        let resolved = store.resolve_book_id("abcd").await.unwrap();
        assert_eq!(resolved, "abcd1234");
    }

    #[tokio::test]
    async fn resolve_book_id_ambiguous() {
        let store = setup_store().await;
        store.save_book(&make_book("abcd1111")).await.unwrap();
        store.save_book(&make_book("abcd2222")).await.unwrap();
        let err = store.resolve_book_id("abcd").await.unwrap_err();
        assert!(matches!(err, InkDripError::AmbiguousId(_)));
    }

    #[tokio::test]
    async fn resolve_book_id_not_found() {
        let store = setup_store().await;
        let err = store.resolve_book_id("nonexist").await.unwrap_err();
        assert!(matches!(err, InkDripError::BookNotFound(_)));
    }

    #[tokio::test]
    async fn resolve_feed_id_prefix() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();
        store
            .save_feed(&make_feed("feed5678", "book1234"))
            .await
            .unwrap();
        let resolved = store.resolve_feed_id("feed").await.unwrap();
        assert_eq!(resolved, "feed5678");
    }

    // ─── Segments ───────────────────────────────────────────────

    #[tokio::test]
    async fn save_and_get_segments() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();
        let segments = make_segments("book1234", 5);
        store.save_segments(&segments).await.unwrap();

        let got = store.get_segments("book1234").await.unwrap();
        assert_eq!(got.len(), 5);
        assert_eq!(got.first().unwrap().index, 0);
    }

    #[tokio::test]
    async fn get_segment_by_index_found() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();
        store
            .save_segments(&make_segments("book1234", 5))
            .await
            .unwrap();

        let seg = store
            .get_segment_by_index("book1234", 2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seg.index, 2);
        assert_eq!(seg.title_context, "Chapter 3");
    }

    #[tokio::test]
    async fn get_segment_by_index_not_found() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();
        store
            .save_segments(&make_segments("book1234", 3))
            .await
            .unwrap();

        let result = store.get_segment_by_index("book1234", 99).await.unwrap();
        assert!(result.is_none());
    }

    // ─── Releases & advance ────────────────────────────────────

    #[tokio::test]
    async fn advance_releases_basic() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();
        let segments = make_segments("book1234", 5);
        store.save_segments(&segments).await.unwrap();
        store
            .save_feed(&make_feed("feed0001", "book1234"))
            .await
            .unwrap();

        // All releases in the future
        let future = tz8().with_ymd_and_hms(2099, 1, 1, 8, 0, 0).unwrap();
        let releases: Vec<SegmentRelease> = segments
            .iter()
            .map(|s| SegmentRelease {
                segment_id: s.id.clone(),
                feed_id: "feed0001".to_owned(),
                release_at: future,
            })
            .collect();
        store.save_releases(&releases).await.unwrap();

        // Advance 2 segments to now
        let now: DateTime<FixedOffset> = Utc::now().into();
        let advanced = store.advance_releases("feed0001", 2, now).await.unwrap();
        assert_eq!(advanced, 2);

        // Verify: 2 released, 3 unreleased
        let released_count = store
            .count_released_segments("feed0001", now)
            .await
            .unwrap();
        assert_eq!(released_count, 2);
    }

    #[tokio::test]
    async fn advance_releases_more_than_available() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();
        let segments = make_segments("book1234", 3);
        store.save_segments(&segments).await.unwrap();
        store
            .save_feed(&make_feed("feed0001", "book1234"))
            .await
            .unwrap();

        let future = tz8().with_ymd_and_hms(2099, 1, 1, 8, 0, 0).unwrap();
        let releases: Vec<SegmentRelease> = segments
            .iter()
            .map(|s| SegmentRelease {
                segment_id: s.id.clone(),
                feed_id: "feed0001".to_owned(),
                release_at: future,
            })
            .collect();
        store.save_releases(&releases).await.unwrap();

        let now: DateTime<FixedOffset> = Utc::now().into();
        let advanced = store.advance_releases("feed0001", 10, now).await.unwrap();
        assert_eq!(advanced, 3); // Only 3 available
    }

    #[tokio::test]
    async fn count_released_segments_zero() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();
        store
            .save_feed(&make_feed("feed0001", "book1234"))
            .await
            .unwrap();

        let now: DateTime<FixedOffset> = Utc::now().into();
        let count = store
            .count_released_segments("feed0001", now)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    // ─── Feed operations ────────────────────────────────────────

    #[tokio::test]
    async fn update_feed_status() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();
        store
            .save_feed(&make_feed("feed0001", "book1234"))
            .await
            .unwrap();

        store
            .update_feed_status("feed0001", FeedStatus::Paused)
            .await
            .unwrap();

        let feed = store.get_feed("feed0001").await.unwrap().unwrap();
        assert_eq!(feed.status, FeedStatus::Paused);
    }

    #[tokio::test]
    async fn get_feed_by_slug() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();
        store
            .save_feed(&make_feed("feed0001", "book1234"))
            .await
            .unwrap();

        let feed = store
            .get_feed_by_slug("test-feed0001")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(feed.id, "feed0001");
    }

    #[tokio::test]
    async fn get_book_by_hash() {
        let store = setup_store().await;
        store.save_book(&make_book("book1234")).await.unwrap();

        let book = store
            .get_book_by_hash("hash_book1234")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(book.id, "book1234");

        assert!(
            store
                .get_book_by_hash("nonexistent")
                .await
                .unwrap()
                .is_none()
        );
    }
}
