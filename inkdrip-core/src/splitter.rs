pub mod semantic;

use crate::error::Result;
use crate::model::{Chapter, Segment};

/// Configuration for text splitting.
#[derive(Debug, Clone)]
pub struct SplitConfig {
    /// Target word count per segment.
    pub target_words: u32,
    /// Maximum word count per segment (will try not to exceed).
    pub max_words: u32,
    /// Minimum word count per segment (avoid tiny trailing segments).
    pub min_words: u32,
}

impl SplitConfig {
    #[must_use]
    pub fn new(target: u32, max: u32, min: u32) -> Self {
        Self {
            target_words: target,
            max_words: max,
            min_words: min,
        }
    }
}

impl Default for SplitConfig {
    fn default() -> Self {
        Self {
            target_words: 1500,
            max_words: 2000,
            min_words: 500,
        }
    }
}

/// Trait for splitting parsed chapters into segments.
pub trait TextSplitter: Send + Sync {
    /// Split a book's chapters into segments suitable for RSS delivery.
    ///
    /// # Errors
    ///
    /// Returns an error if splitting fails (e.g. empty chapters or internal logic error).
    fn split(
        &self,
        book_id: &str,
        chapters: &[Chapter],
        config: &SplitConfig,
    ) -> Result<Vec<Segment>>;
}
