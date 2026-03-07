use chrono::{Datelike, TimeZone};

use crate::config::{parse_delivery_time, parse_timezone_offset};
use crate::model::{ScheduleConfig, Segment, SegmentRelease, SkipDays};

/// Compute release timestamps for all segments in a feed.
///
/// The algorithm distributes segments across days based on `words_per_day`.
/// Each day's budget is `words_per_day`; segments are assigned to the earliest
/// day that still has remaining budget. All segments on a given day share
/// the same `delivery_time`.
#[must_use]
pub fn compute_release_schedule(
    segments: &[Segment],
    config: &ScheduleConfig,
) -> Vec<SegmentRelease> {
    let tz = parse_timezone_offset(&config.timezone);
    let delivery_time = parse_delivery_time(&config.delivery_time);

    let mut releases = Vec::with_capacity(segments.len());
    let mut current_date = config.start_at.with_timezone(&tz).date_naive();
    let mut daily_remaining = i64::from(config.words_per_day);

    for segment in segments {
        let word_count = i64::from(segment.word_count);

        // If this segment doesn't fit in current day's budget, advance to next day
        if daily_remaining < word_count && daily_remaining < i64::from(config.words_per_day) {
            current_date = advance_date(current_date, config.skip_days);
            daily_remaining = i64::from(config.words_per_day);
        }

        // Assign release time
        let release_at = tz
            .from_local_datetime(&current_date.and_time(delivery_time))
            .single()
            .unwrap_or_else(|| {
                // FixedOffset has no DST; single() is always Some.
                // Keep earliest() as a safeguard for potential future DST-aware tz support.
                tz.from_local_datetime(&current_date.and_time(delivery_time))
                    .earliest()
                    .unwrap_or_else(|| tz.from_utc_datetime(&current_date.and_time(delivery_time)))
            })
            .fixed_offset();

        releases.push(SegmentRelease {
            segment_id: segment.id.clone(),
            feed_id: String::new(), // Caller sets this
            release_at,
        });

        daily_remaining -= word_count;
    }

    releases
}

/// Advance a date by one day, skipping any days in `skip_days`.
#[expect(
    clippy::expect_used,
    reason = "NaiveDate should never overflow when adding one day, and we want to fail fast if it does"
)]
fn advance_date(date: chrono::NaiveDate, skip_days: SkipDays) -> chrono::NaiveDate {
    let mut next = date
        .succ_opt()
        .expect("NaiveDate should never overflow when adding one day");
    while skip_days.should_skip(next.weekday()) {
        next = next
            .succ_opt()
            .expect("NaiveDate should never overflow when adding one day");
    }
    next
}

/// Estimate the number of days needed to finish a book with given settings.
#[must_use]
pub fn estimate_days(total_words: u32, words_per_day: u32) -> u32 {
    if words_per_day == 0 {
        return 0;
    }
    total_words.div_ceil(words_per_day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Segment, SkipDays};

    fn make_segments(word_counts: &[u32]) -> Vec<Segment> {
        let mut cumulative = 0u32;
        word_counts
            .iter()
            .enumerate()
            .map(|(i, &wc)| {
                cumulative += wc;
                Segment::new(
                    "book-1".into(),
                    i as u32,
                    format!("Segment {}", i + 1),
                    "<p>content</p>".into(),
                    wc,
                    cumulative,
                )
            })
            .collect()
    }

    #[test]
    fn basic_schedule() {
        let segments = make_segments(&[1000, 1000, 1000, 1000]);
        let config = ScheduleConfig {
            start_at: chrono::FixedOffset::east_opt(8 * 3600)
                .unwrap()
                .with_ymd_and_hms(2025, 9, 1, 8, 0, 0)
                .unwrap(),
            words_per_day: 2000,
            delivery_time: "08:00".to_owned(),
            skip_days: SkipDays::empty(),
            timezone: "UTC+8".to_owned(),
        };

        let releases = compute_release_schedule(&segments, &config);
        assert_eq!(releases.len(), 4);

        // First two segments should be on day 1, next two on day 2
        assert_eq!(
            releases[0].release_at.date_naive(),
            releases[1].release_at.date_naive()
        );
        assert_ne!(
            releases[1].release_at.date_naive(),
            releases[2].release_at.date_naive()
        );
    }

    #[test]
    fn skip_weekends() {
        // Start on a Friday
        let segments = make_segments(&[3000, 3000, 3000]);
        let config = ScheduleConfig {
            start_at: chrono::FixedOffset::east_opt(0)
                .unwrap()
                .with_ymd_and_hms(2025, 9, 5, 8, 0, 0) // Friday
                .unwrap(),
            words_per_day: 3000,
            delivery_time: "08:00".to_owned(),
            skip_days: SkipDays::WEEKENDS,
            timezone: "UTC".to_owned(),
        };

        let releases = compute_release_schedule(&segments, &config);
        assert_eq!(releases.len(), 3);

        // Day 1: Friday, Day 2: Monday (skipping Sat/Sun), Day 3: Tuesday
        let d1 = releases[0].release_at.date_naive();
        let d2 = releases[1].release_at.date_naive();
        let d3 = releases[2].release_at.date_naive();
        assert_eq!(d1.weekday(), chrono::Weekday::Fri);
        assert_eq!(d2.weekday(), chrono::Weekday::Mon);
        assert_eq!(d3.weekday(), chrono::Weekday::Tue);
    }

    #[test]
    fn estimate_days_works() {
        assert_eq!(estimate_days(10000, 3000), 4);
        assert_eq!(estimate_days(3000, 3000), 1);
        assert_eq!(estimate_days(0, 3000), 0);
    }
}
