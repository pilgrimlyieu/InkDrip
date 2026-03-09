use chrono::{Datelike, Duration, TimeZone};

use crate::config::{parse_delivery_time, parse_timezone_offset};
use crate::model::{ScheduleConfig, Segment, SegmentRelease, SkipDays};

/// Compute release timestamps for all segments in a feed.
///
/// The algorithm distributes segments across days based on `words_per_day`.
/// Each day's budget is `words_per_day`; segments are assigned to the earliest
/// day that still has remaining budget.
///
/// When multiple segments fall on the same day, a small per-second stagger is
/// applied so that segment reading order is preserved across all RSS readers
/// (which may sort by timestamp). The first segment in reading order receives
/// the largest timestamp within the batch, ensuring correct top-to-bottom
/// order in newest-first readers.
#[must_use]
pub fn compute_release_schedule(
    segments: &[Segment],
    config: &ScheduleConfig,
    feed_id: &str,
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
            feed_id: feed_id.to_owned(),
            release_at,
        });

        daily_remaining -= word_count;
    }

    // Stagger same-day releases so reading order is deterministic.
    // Within each batch sharing a timestamp, segment k (0-indexed in reading
    // order) gets an offset of (N-1-k) seconds. This makes the first segment
    // have the largest timestamp, placing it at the top in newest-first readers.
    stagger_same_day_releases(&mut releases);

    releases
}

/// Add per-second offsets to releases that share the same base timestamp.
fn stagger_same_day_releases(releases: &mut [SegmentRelease]) {
    let mut i = 0;
    while i < releases.len() {
        // Find the end of the batch sharing the same release_at
        let Some(base_release) = releases.get(i) else {
            break;
        };
        let base = base_release.release_at;
        let mut j = i + 1;
        while releases.get(j).is_some_and(|r| r.release_at == base) {
            j += 1;
        }
        let batch_size = j - i;
        if batch_size > 1
            && let Some(batch) = releases.get_mut(i..j)
        {
            for (k, release) in batch.iter_mut().enumerate() {
                let offset_secs = (batch_size - 1 - k) as i64;
                release.release_at += Duration::seconds(offset_secs);
            }
        }
        i = j;
    }
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

        let releases = compute_release_schedule(&segments, &config, "feed-1");
        assert_eq!(releases.len(), 4);

        // All releases should have the correct feed_id
        for r in &releases {
            assert_eq!(r.feed_id, "feed-1");
        }

        // First two segments should be on day 1, next two on day 2
        assert_eq!(
            releases[0].release_at.date_naive(),
            releases[1].release_at.date_naive()
        );
        assert_ne!(
            releases[1].release_at.date_naive(),
            releases[2].release_at.date_naive()
        );

        // Stagger: first segment in batch has higher timestamp
        assert!(releases[0].release_at > releases[1].release_at);
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

        let releases = compute_release_schedule(&segments, &config, "feed-1");
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
