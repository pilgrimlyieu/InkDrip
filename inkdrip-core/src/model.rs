use std::fmt::{self, Display};
use std::str::FromStr;

use bitflags::bitflags;
use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

use crate::util::generate_short_id;

// ─── Skip Days ──────────────────────────────────────────────────

bitflags! {
    /// Bitflags representing which days of the week to skip delivery.
    ///
    /// Each bit corresponds to a day: `MON=0x01`, `TUE=0x02`, …, `SUN=0x40`.
    /// Combine with `|`: `SkipDays::SAT | SkipDays::SUN` for weekends.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(transparent)]
    pub struct SkipDays: u8 {
        const MON = 0x01;
        const TUE = 0x02;
        const WED = 0x04;
        const THU = 0x08;
        const FRI = 0x10;
        const SAT = 0x20;
        const SUN = 0x40;

        /// Convenience: skip Saturday and Sunday.
        const WEEKENDS = Self::SAT.bits() | Self::SUN.bits();
    }
}

impl SkipDays {
    /// Check whether a given `chrono::Weekday` should be skipped.
    #[must_use]
    pub fn should_skip(self, weekday: chrono::Weekday) -> bool {
        let flag = match weekday {
            chrono::Weekday::Mon => Self::MON,
            chrono::Weekday::Tue => Self::TUE,
            chrono::Weekday::Wed => Self::WED,
            chrono::Weekday::Thu => Self::THU,
            chrono::Weekday::Fri => Self::FRI,
            chrono::Weekday::Sat => Self::SAT,
            chrono::Weekday::Sun => Self::SUN,
        };
        self.contains(flag)
    }
}

impl Default for SkipDays {
    fn default() -> Self {
        Self::empty()
    }
}

impl Display for SkipDays {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("none");
        }
        let names = [
            (Self::MON, "Mon"),
            (Self::TUE, "Tue"),
            (Self::WED, "Wed"),
            (Self::THU, "Thu"),
            (Self::FRI, "Fri"),
            (Self::SAT, "Sat"),
            (Self::SUN, "Sun"),
        ];
        let mut first = true;
        for (flag, name) in &names {
            if self.contains(*flag) {
                if !first {
                    f.write_str(",")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        Ok(())
    }
}

// ─── Book Format ────────────────────────────────────────────────

/// Supported book file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BookFormat {
    Epub,
    Txt,
    Markdown,
}

impl BookFormat {
    /// Infer format from file extension.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "epub" => Some(Self::Epub),
            "txt" | "text" => Some(Self::Txt),
            "md" | "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Txt => "txt",
            Self::Markdown => "markdown",
        }
    }
}

impl Display for BookFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─── Book ───────────────────────────────────────────────────────

/// A parsed and stored book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    pub format: BookFormat,
    pub file_hash: String,
    pub file_path: String,
    pub total_words: u32,
    pub total_segments: u32,
    pub created_at: DateTime<FixedOffset>,
}

impl Book {
    #[must_use]
    pub fn new(
        title: String,
        author: String,
        format: BookFormat,
        file_hash: String,
        file_path: String,
    ) -> Self {
        Self {
            id: generate_short_id(),
            title,
            author,
            format,
            file_hash,
            file_path,
            total_words: 0,
            total_segments: 0,
            created_at: Utc::now().into(),
        }
    }
}

// ─── Chapter (parsing intermediate) ────────────────────────────

/// A chapter extracted during parsing, before splitting into segments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub index: u32,
    pub title: String,
    /// Chapter content as sanitized HTML.
    pub content_html: String,
    pub word_count: u32,
}

// ─── Parsed Book ────────────────────────────────────────────────

/// Result of parsing a book file.
#[derive(Debug, Clone)]
pub struct ParsedBook {
    pub title: String,
    pub author: String,
    pub chapters: Vec<Chapter>,
    /// Extracted images: `(relative_path, image_bytes)`.
    pub images: Vec<(String, Vec<u8>)>,
}

impl ParsedBook {
    #[must_use]
    pub fn total_words(&self) -> u32 {
        self.chapters.iter().map(|c| c.word_count).sum()
    }
}

// ─── Segment ────────────────────────────────────────────────────

/// A segment is the atomic unit of content delivered via RSS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub book_id: String,
    /// Zero-based global index within the book.
    pub index: u32,
    /// Context title (e.g. "Chapter 3 (2/4)").
    pub title_context: String,
    /// Segment content as HTML.
    pub content_html: String,
    pub word_count: u32,
    /// Cumulative word count up to and including this segment.
    pub cumulative_words: u32,
}

impl Segment {
    #[must_use]
    pub fn new(
        book_id: String,
        index: u32,
        title_context: String,
        content_html: String,
        word_count: u32,
        cumulative_words: u32,
    ) -> Self {
        Self {
            id: generate_short_id(),
            book_id,
            index,
            title_context,
            content_html,
            word_count,
            cumulative_words,
        }
    }
}

// ─── Feed ───────────────────────────────────────────────────────

/// Budget enforcement mode for scheduling.
///
/// - `Strict`: Never exceed `words_per_day`; a segment is pushed to the next
///   day if it would cause the daily total to exceed the budget.
/// - `Flexible`: Allow a segment to be added if it brings the daily total
///   *closer* to `words_per_day`, even if it slightly overshoots. This mirrors
///   the "closer-to-target" logic used by the splitter and typically produces
///   daily totals with less variance from the budget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetMode {
    /// Never exceed `words_per_day` (default).
    #[default]
    Strict,
    /// Allow controlled overshoot when closer to the target.
    Flexible,
}

impl BudgetMode {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Flexible => "flexible",
        }
    }
}

impl Display for BudgetMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BudgetMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "strict" => Ok(Self::Strict),
            "flexible" => Ok(Self::Flexible),
            _ => Err(format!("Unknown budget mode: {s}")),
        }
    }
}

/// Feed status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedStatus {
    Active,
    Paused,
    Completed,
}

impl FeedStatus {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
        }
    }
}

impl Display for FeedStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FeedStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            _ => Err(format!("Unknown feed status: {s}")),
        }
    }
}

/// Schedule configuration for a feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    /// When to start releasing segments.
    pub start_at: DateTime<FixedOffset>,
    /// Target words to release per day.
    pub words_per_day: u32,
    /// Time of day to release (HH:MM).
    pub delivery_time: String,
    /// Days of the week to skip delivery.
    pub skip_days: SkipDays,
    /// IANA timezone string.
    pub timezone: String,
    /// Budget enforcement mode (strict or flexible).
    #[serde(default)]
    pub budget_mode: BudgetMode,
}

/// A feed subscription for a book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feed {
    pub id: String,
    pub book_id: String,
    /// URL-friendly slug for the feed endpoint.
    pub slug: String,
    pub schedule_config: ScheduleConfig,
    pub status: FeedStatus,
    pub created_at: DateTime<FixedOffset>,
}

impl Feed {
    #[must_use]
    pub fn new(book_id: String, slug: String, schedule_config: ScheduleConfig) -> Self {
        Self {
            id: generate_short_id(),
            book_id,
            slug,
            schedule_config,
            status: FeedStatus::Active,
            created_at: Utc::now().into(),
        }
    }
}

// ─── Segment Release ────────────────────────────────────────────

/// Pre-computed release timestamp for a segment in a specific feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentRelease {
    pub segment_id: String,
    pub feed_id: String,
    pub release_at: DateTime<FixedOffset>,
}

// ─── Aggregate Feed ─────────────────────────────────────────────

/// An aggregate feed that combines released segments from multiple feeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateFeed {
    pub id: String,
    /// URL-friendly slug (unique).
    pub slug: String,
    /// Human-readable title.
    pub title: String,
    /// Optional description.
    pub description: String,
    /// When true, all active feeds are included automatically.
    pub include_all: bool,
    pub created_at: DateTime<FixedOffset>,
}

impl AggregateFeed {
    #[must_use]
    pub fn new(slug: String, title: String, description: String, include_all: bool) -> Self {
        Self {
            id: generate_short_id(),
            slug,
            title,
            description,
            include_all,
            created_at: Utc::now().into(),
        }
    }
}
