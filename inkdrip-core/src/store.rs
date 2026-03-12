use async_trait::async_trait;
use chrono::{DateTime, FixedOffset};

use crate::error::Result;
use crate::model::{
    AggregateFeed, Book, Feed, FeedStatus, ScheduleConfig, Segment, SegmentRelease,
};
use crate::undo::UndoEntry;

/// Abstract storage backend for `InkDrip`.
///
/// Implementations handle persistence of books, segments, feeds, and release schedules.
#[async_trait]
pub trait BookStore: Send + Sync {
    // ─── Books ──────────────────────────────────────────────────

    async fn save_book(&self, book: &Book) -> Result<()>;
    async fn get_book(&self, id: &str) -> Result<Option<Book>>;
    async fn get_book_by_hash(&self, file_hash: &str) -> Result<Option<Book>>;
    async fn list_books(&self) -> Result<Vec<Book>>;
    async fn delete_book(&self, id: &str) -> Result<()>;
    async fn update_book_meta(&self, id: &str, title: &str, author: &str) -> Result<()>;

    /// Resolve a potentially abbreviated ID to its full book ID.
    /// Returns the full ID if exactly one book matches the prefix.
    async fn resolve_book_id(&self, prefix: &str) -> Result<String>;

    // ─── Segments ───────────────────────────────────────────────

    async fn save_segments(&self, segments: &[Segment]) -> Result<()>;
    async fn get_segments(&self, book_id: &str) -> Result<Vec<Segment>>;
    async fn get_segment(&self, id: &str) -> Result<Option<Segment>>;
    async fn delete_segments_for_book(&self, book_id: &str) -> Result<()>;

    // ─── Feeds ──────────────────────────────────────────────────

    async fn save_feed(&self, feed: &Feed) -> Result<()>;
    async fn get_feed(&self, id: &str) -> Result<Option<Feed>>;
    async fn get_feed_by_slug(&self, slug: &str) -> Result<Option<Feed>>;
    async fn list_feeds(&self) -> Result<Vec<Feed>>;
    async fn list_feeds_for_book(&self, book_id: &str) -> Result<Vec<Feed>>;
    async fn update_feed_status(&self, id: &str, status: FeedStatus) -> Result<()>;
    async fn update_feed_schedule(&self, id: &str, config: &ScheduleConfig) -> Result<()>;
    async fn update_feed_slug(&self, id: &str, slug: &str) -> Result<()>;
    async fn delete_feed(&self, id: &str) -> Result<()>;

    /// Resolve a potentially abbreviated ID to its full feed ID.
    /// Returns the full ID if exactly one feed matches the prefix.
    async fn resolve_feed_id(&self, prefix: &str) -> Result<String>;

    // ─── Segment Releases ───────────────────────────────────────

    async fn save_releases(&self, releases: &[SegmentRelease]) -> Result<()>;
    async fn delete_releases_for_feed(&self, feed_id: &str) -> Result<()>;
    async fn get_releases_for_feed(&self, feed_id: &str) -> Result<Vec<SegmentRelease>>;

    /// Delete only the unreleased segment releases for a feed (`release_at` > cutoff).
    async fn delete_future_releases_for_feed(
        &self,
        feed_id: &str,
        after: DateTime<FixedOffset>,
    ) -> Result<()>;

    /// Get unreleased segments for a feed (`release_at` > after), ordered by segment index.
    async fn get_unreleased_segments_for_feed(
        &self,
        feed_id: &str,
        after: DateTime<FixedOffset>,
        limit: u32,
    ) -> Result<Vec<(Segment, SegmentRelease)>>;

    /// Get the highest segment index that has been released in any feed for a book.
    async fn get_max_released_index_for_book(
        &self,
        book_id: &str,
        before: DateTime<FixedOffset>,
    ) -> Result<Option<u32>>;

    /// Delete segments (and their release records) with index >= `from_index`.
    async fn delete_segments_from_index(&self, book_id: &str, from_index: u32) -> Result<()>;

    /// Update `total_segments` on a book record.
    async fn update_book_segment_count(&self, id: &str, total_segments: u32) -> Result<()>;

    /// Get all segments released on or before the given time, for a specific feed.
    /// Results are ordered by segment index (ascending).
    async fn get_released_segments(
        &self,
        feed_id: &str,
        before: DateTime<FixedOffset>,
        limit: u32,
    ) -> Result<Vec<(Segment, SegmentRelease)>>;

    /// Count how many segments have been released for a feed.
    async fn count_released_segments(
        &self,
        feed_id: &str,
        before: DateTime<FixedOffset>,
    ) -> Result<u32>;

    /// Advance the next `count` unreleased segments for a feed by setting their
    /// `release_at` to the given timestamp (making them immediately available in RSS).
    /// Returns the number of segments actually advanced.
    async fn advance_releases(
        &self,
        feed_id: &str,
        count: u32,
        release_at: DateTime<FixedOffset>,
    ) -> Result<u32>;

    /// Get a single segment by book ID and zero-based index.
    async fn get_segment_by_index(&self, book_id: &str, index: u32) -> Result<Option<Segment>>;

    // ─── Aggregate Feeds ────────────────────────────────────────

    async fn save_aggregate_feed(&self, agg: &AggregateFeed) -> Result<()>;
    async fn get_aggregate_feed(&self, id: &str) -> Result<Option<AggregateFeed>>;
    async fn get_aggregate_feed_by_slug(&self, slug: &str) -> Result<Option<AggregateFeed>>;
    async fn list_aggregate_feeds(&self) -> Result<Vec<AggregateFeed>>;
    async fn update_aggregate_feed(
        &self,
        id: &str,
        title: &str,
        description: &str,
        include_all: bool,
    ) -> Result<()>;
    async fn delete_aggregate_feed(&self, id: &str) -> Result<()>;

    /// Upsert an aggregate feed by slug (for config.toml declarations).
    async fn upsert_aggregate_feed(&self, agg: &AggregateFeed) -> Result<()>;

    /// Add a feed to an aggregate's source list.
    async fn add_aggregate_source(&self, aggregate_id: &str, feed_id: &str) -> Result<()>;
    /// Remove a feed from an aggregate's source list.
    async fn remove_aggregate_source(&self, aggregate_id: &str, feed_id: &str) -> Result<()>;
    /// List feed IDs belonging to an aggregate.
    async fn list_aggregate_sources(&self, aggregate_id: &str) -> Result<Vec<String>>;

    /// Get released segments for an aggregate feed (union of all source feeds).
    /// When `include_all` is true, returns segments from all active feeds.
    async fn get_aggregate_released_segments(
        &self,
        aggregate_id: &str,
        include_all: bool,
        before: DateTime<FixedOffset>,
        limit: u32,
    ) -> Result<Vec<(Segment, SegmentRelease, Book)>>;

    // ─── Soft-Delete ────────────────────────────────────────────

    /// Mark a book as deleted (sets `deleted_at`), and cascade to its feeds.
    async fn soft_delete_book(&self, id: &str) -> Result<()>;
    /// Restore a soft-deleted book (clears `deleted_at`), and cascade to its feeds.
    async fn restore_book(&self, id: &str) -> Result<()>;
    /// Mark a feed as deleted (sets `deleted_at`).
    async fn soft_delete_feed(&self, id: &str) -> Result<()>;
    /// Restore a soft-deleted feed (clears `deleted_at`).
    async fn restore_feed(&self, id: &str) -> Result<()>;

    // ─── Undo/Redo Log ─────────────────────────────────────────

    /// Push a new undo entry: truncate redo chain, insert entry, advance cursor,
    /// prune oldest entries beyond `max_depth`. Returns the new entry ID.
    async fn push_undo_entry(
        &self,
        operation: &str,
        summary: &str,
        payload: &serde_json::Value,
        max_depth: u32,
    ) -> Result<i64>;

    /// Get the entry at the current undo cursor (the most recent undoable action).
    async fn get_undo_entry(&self) -> Result<Option<UndoEntry>>;

    /// Move the undo cursor backward (toward older entries).
    async fn retreat_undo_cursor(&self) -> Result<()>;

    /// Get the next redo entry (the entry immediately after the cursor).
    async fn get_redo_entry(&self) -> Result<Option<UndoEntry>>;

    /// Move the undo cursor forward to the given entry ID.
    async fn advance_undo_cursor(&self, id: i64) -> Result<()>;

    /// List recent undo history entries (newest first).
    async fn list_undo_history(&self, limit: u32) -> Result<Vec<UndoEntry>>;

    /// Clear all undo/redo history and hard-delete all soft-deleted resources.
    ///
    /// Soft-deleted books and feeds exist solely as undo/redo targets; once
    /// history is cleared they become unreachable orphans and must be purged.
    async fn clear_history(&self) -> Result<()>;
}
