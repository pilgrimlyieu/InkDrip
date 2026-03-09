use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::model::{FeedStatus, ScheduleConfig, SegmentRelease};

/// A single row in the undo log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    pub id: i64,
    pub operation: String,
    pub summary: String,
    pub created_at: DateTime<FixedOffset>,
    pub payload: serde_json::Value,
}

/// Snapshot of a feed's mutable state, used for undo/redo of updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedSnapshot {
    pub schedule_config: ScheduleConfig,
    pub status: FeedStatus,
    pub slug: String,
}

/// The typed payload stored inside each undo entry.
///
/// Serialized as JSON with a `"op"` discriminator tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum HistoryPayload {
    /// A new feed was created.
    CreateFeed { feed_id: String },
    /// A feed was deleted (soft-deleted).
    DeleteFeed { feed_id: String },
    /// A feed's schedule/config was updated.
    UpdateFeed {
        feed_id: String,
        old_state: FeedSnapshot,
        new_state: FeedSnapshot,
        /// Future releases before the update (for restoration).
        old_releases: Vec<SegmentRelease>,
        /// Future releases after the update (for redo).
        new_releases: Vec<SegmentRelease>,
    },
    /// Segments were advanced (released immediately).
    AdvanceFeed {
        feed_id: String,
        /// (`segment_id`, original `release_at`) pairs for the advanced segments.
        old_releases: Vec<(String, DateTime<FixedOffset>)>,
        /// (`segment_id`, new `release_at`) pairs after advance.
        new_releases: Vec<(String, DateTime<FixedOffset>)>,
        /// Future releases scheduled after the advance (for redo).
        post_advance_releases: Vec<SegmentRelease>,
        /// Future releases before the advance (for undo restoration).
        pre_advance_releases: Vec<SegmentRelease>,
    },
    /// A new book was uploaded.
    UploadBook { book_id: String },
    /// A book was deleted (soft-deleted).
    DeleteBook { book_id: String },
    /// A book's metadata was updated.
    UpdateBook {
        book_id: String,
        old_title: String,
        old_author: String,
        new_title: String,
        new_author: String,
    },
}

impl HistoryPayload {
    /// Short operation name for display and DB storage.
    #[must_use]
    pub fn operation_name(&self) -> &'static str {
        match self {
            Self::CreateFeed { .. } => "create_feed",
            Self::DeleteFeed { .. } => "delete_feed",
            Self::UpdateFeed { .. } => "update_feed",
            Self::AdvanceFeed { .. } => "advance_feed",
            Self::UploadBook { .. } => "upload_book",
            Self::DeleteBook { .. } => "delete_book",
            Self::UpdateBook { .. } => "update_book",
        }
    }
}
