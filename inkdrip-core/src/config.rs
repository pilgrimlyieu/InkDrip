use std::fmt::{self, Formatter};

use chrono::NaiveTime;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::model::SkipDays;

/// Application-wide configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub parser: ParserConfig,
    #[serde(default)]
    pub watch: WatchConfig,
    #[serde(default)]
    pub transforms: TransformsConfig,
    #[serde(default)]
    pub feed: FeedConfig,
    /// Static aggregate feed declarations, upserted on server start.
    #[serde(default)]
    pub aggregates: Vec<AggregateConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub base_url: String,
    pub api_token: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".into(),
            port: 8080,
            base_url: "http://localhost:8080".into(),
            api_token: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub data_dir: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dir: "./data".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    pub words_per_day: u32,
    pub target_segment_words: u32,
    pub max_segment_words: u32,
    pub min_segment_words: u32,
    /// Delivery time in HH:MM format.
    pub delivery_time: String,
    /// IANA timezone name (e.g. "Asia/Shanghai").
    pub timezone: String,
    /// Days of the week to skip delivery.
    ///
    /// Accepts a TOML array of day names (e.g. `["saturday", "sunday"]`) *or* the
    /// pipe-separated bitflag string produced by serialization (e.g. `"SAT | SUN"`).
    #[serde(default, deserialize_with = "deserialize_skip_days")]
    pub skip_days: SkipDays,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            words_per_day: 3000,
            target_segment_words: 1500,
            max_segment_words: 2000,
            min_segment_words: 500,
            delivery_time: "08:00".into(),
            timezone: "Asia/Shanghai".into(),
            skip_days: SkipDays::empty(),
        }
    }
}

impl DefaultsConfig {
    /// Parse the `delivery_time` string into a [`NaiveTime`].
    ///
    /// # Errors
    ///
    /// Returns an error if the string is not in `HH:MM` format.
    pub fn delivery_naive_time(&self) -> anyhow::Result<NaiveTime> {
        Ok(NaiveTime::parse_from_str(&self.delivery_time, "%H:%M")?)
    }

    /// Parse the `timezone` string into a `FixedOffset`.
    ///
    /// Supports `"UTC"`, `"UTC+N"` / `"UTC-N"` and common IANA names.
    ///
    /// # Panics
    ///
    /// Never panics; unknown timezones fall back to UTC with a warning.
    #[must_use]
    pub fn timezone_offset(&self) -> chrono::FixedOffset {
        parse_timezone_offset(&self.timezone)
    }
}

/// Parse a timezone string into a `FixedOffset`.
///
/// Supports `"UTC"`, `"GMT"`, `"UTC+N"`, `"UTC-N"`, and common IANA names.
/// Unknown values default to UTC with a tracing warning.
pub fn parse_timezone_offset(tz: &str) -> chrono::FixedOffset {
    // Common timezone name mappings
    let offset_hours: i32 = match tz {
        "UTC" | "GMT" | "Europe/London" => 0,
        "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Hong_Kong" | "Asia/Taipei" => 8,
        "Asia/Tokyo" | "Asia/Seoul" => 9,
        "Asia/Kolkata" => 5, // approximate
        "Europe/Berlin" | "Europe/Paris" => 1,
        "America/New_York" => -5,
        "America/Chicago" => -6,
        "America/Denver" => -7,
        "America/Los_Angeles" => -8,
        _ => {
            // Try parsing "UTC+N" or "UTC-N"
            if let Some(rest) = tz.strip_prefix("UTC") {
                rest.parse::<i32>().unwrap_or(0)
            } else {
                tracing::warn!("Unknown timezone '{tz}', defaulting to UTC");
                0
            }
        }
    };
    hours_to_offset(offset_hours)
}

/// Build a [`chrono::FixedOffset`] from a whole-hour offset, clamped to ±23 h.
///
/// `FixedOffset::east_opt` requires `|seconds| < 86400`. Clamping to ±23 h
/// gives at most ±82 800 s, always within the valid range.
fn hours_to_offset(hours: i32) -> chrono::FixedOffset {
    let secs = hours.clamp(-23, 23) * 3600;
    chrono::FixedOffset::east_opt(secs)
        .unwrap_or_else(|| unreachable!("secs {secs} should be valid"))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ParserConfig {
    /// TXT-specific parser settings.
    #[serde(default)]
    pub txt: TxtParserConfig,
}

/// Parser settings for plain-text (.txt) files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxtParserConfig {
    /// Regex pattern for chapter separators.
    pub chapter_separator: String,
    /// Paragraph separator pattern.
    pub paragraph_separator: String,
}

impl Default for TxtParserConfig {
    fn default() -> Self {
        Self {
            chapter_separator: "^={3,}$".into(),
            paragraph_separator: "\n\n".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchConfig {
    pub enabled: bool,
    pub dir: String,
    pub auto_create_feed: bool,
    pub scan_interval_secs: u64,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dir: "./books".into(),
            auto_create_feed: true,
            scan_interval_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformsConfig {
    pub reading_progress: bool,
    pub custom_css: String,
    pub external_command: Option<String>,
}

impl Default for TransformsConfig {
    fn default() -> Self {
        Self {
            reading_progress: true,
            custom_css: String::new(),
            external_command: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedConfig {
    /// Feed output format: "atom" or "rss".
    pub format: String,
    /// Maximum number of items returned per feed request.
    pub items_limit: u32,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            format: "atom".into(),
            items_limit: 50,
        }
    }
}

/// Static declaration of an aggregate feed in config.toml.
///
/// ```toml
/// [[aggregates]]
/// slug = "all"
/// title = "All Books"
/// include_all = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateConfig {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// When true, all active feeds are automatically included.
    #[serde(default)]
    pub include_all: bool,
    /// Explicit list of feed slugs to include (ignored when `include_all` is true).
    #[serde(default)]
    pub feeds: Vec<String>,
}

// ─── SkipDays config deserializer ───────────────────────────────

/// Deserialize `skip_days` from either a TOML array of day names
/// (`["saturday", "sunday"]`) or the pipe-separated bitflag string that
/// serialization produces (`"SAT | SUN"`).  Both forms are accepted so
/// that the config file, JSON round-trips, and human editing all work.
fn deserialize_skip_days<'de, D>(deserializer: D) -> Result<SkipDays, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct SkipDaysVisitor;

    impl<'de> Visitor<'de> for SkipDaysVisitor {
        type Value = SkipDays;

        fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
            formatter.write_str(
                r#"an array of day names (e.g. ["saturday", "sunday"]) or a "|"-separated string (e.g. "SAT | SUN")"#,
            )
        }

        /// Accept `"SAT | SUN"` or `""` (the round-tripped serialization form).
        fn visit_str<E: de::Error>(self, v: &str) -> Result<SkipDays, E> {
            if v.trim().is_empty() {
                return Ok(SkipDays::empty());
            }
            let mut flags = SkipDays::empty();
            for part in v.split('|') {
                let flag = parse_day_name(part.trim()).ok_or_else(|| {
                    de::Error::custom(format!("unknown day flag: `{}`", part.trim()))
                })?;
                flags |= flag;
            }
            Ok(flags)
        }

        /// Accept `["saturday", "sunday"]` (the human-readable TOML form).
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<SkipDays, A::Error> {
            let mut flags = SkipDays::empty();
            while let Some(day) = seq.next_element::<String>()? {
                let flag = parse_day_name(&day)
                    .ok_or_else(|| de::Error::custom(format!("unknown day name: `{day}`")))?;
                flags |= flag;
            }
            Ok(flags)
        }
    }

    deserializer.deserialize_any(SkipDaysVisitor)
}

/// Map a day name string (full name or uppercase abbreviation) to a [`SkipDays`] flag.
fn parse_day_name(name: &str) -> Option<SkipDays> {
    match name.to_lowercase().as_str() {
        "mon" | "monday" => Some(SkipDays::MON),
        "tue" | "tuesday" => Some(SkipDays::TUE),
        "wed" | "wednesday" => Some(SkipDays::WED),
        "thu" | "thursday" => Some(SkipDays::THU),
        "fri" | "friday" => Some(SkipDays::FRI),
        "sat" | "saturday" => Some(SkipDays::SAT),
        "sun" | "sunday" => Some(SkipDays::SUN),
        _ => None,
    }
}
