use chrono::{Datelike, Duration, TimeZone};

use crate::config::{parse_delivery_time, parse_timezone_offset};
use crate::model::{BudgetMode, ScheduleConfig, Segment, SegmentRelease, SkipDays};

/// Compute release timestamps for all segments in a feed.
///
/// The algorithm distributes segments across days based on `words_per_day`.
/// Each day's budget is `words_per_day`; segments are assigned to the earliest
/// day that still has remaining budget.
///
/// The `budget_mode` field in config controls how strictly the budget is enforced:
/// - `Strict`: Never exceed `words_per_day`; a segment is pushed to the next day
///   if adding it would exceed the budget.
/// - `Flexible`: Allow a segment if it brings the daily total closer to the budget,
///   even if it slightly overshoots (mirroring the splitter's "closer-to-target" logic).
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
    let budget = config.words_per_day;
    let mut daily_used: u32 = 0;

    for segment in segments {
        let word_count = segment.word_count;

        // Determine if we should advance to the next day.
        // - In Strict mode: advance if adding this segment would exceed the budget
        //   (unless nothing has been scheduled today, which forces this segment in).
        // - In Flexible mode: advance if adding this segment would move us further
        //   from the budget target (using the "closer-to-target" heuristic).
        let should_advance = daily_used > 0
            && match config.budget_mode {
                BudgetMode::Strict => daily_used.saturating_add(word_count) > budget,
                BudgetMode::Flexible => !is_closer_to_budget(daily_used, word_count, budget),
            };

        if should_advance {
            current_date = advance_date(current_date, config.skip_days);
            daily_used = 0;
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

        daily_used = daily_used.saturating_add(word_count);
    }

    // Stagger same-day releases so reading order is deterministic.
    // Within each batch sharing a timestamp, segment k (0-indexed in reading
    // order) gets an offset of (N-1-k) seconds. This makes the first segment
    // have the largest timestamp, placing it at the top in newest-first readers.
    stagger_same_day_releases(&mut releases);

    releases
}

/// Check whether adding `unit_words` to a buffer of `current_words` gets
/// strictly closer to `target` than stopping now.
///
/// Returns `true` when `|current + unit - target| < |current - target|`,
/// i.e., the combined total is at least as close to the target as the current
/// total alone. This allows controlled overshoot when the overshoot would be
/// smaller than the current undershoot.
fn is_closer_to_budget(current_words: u32, unit_words: u32, target: u32) -> bool {
    unit_words < target.saturating_sub(current_words).saturating_mul(2)
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
    use crate::model::{BudgetMode, Segment, SkipDays};

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

    fn make_config(words_per_day: u32, budget_mode: BudgetMode) -> ScheduleConfig {
        ScheduleConfig {
            start_at: chrono::FixedOffset::east_opt(8 * 3600)
                .unwrap()
                .with_ymd_and_hms(2025, 9, 1, 8, 0, 0)
                .unwrap(),
            words_per_day,
            delivery_time: "08:00".to_owned(),
            skip_days: SkipDays::empty(),
            timezone: "UTC+8".to_owned(),
            budget_mode,
        }
    }

    #[test]
    fn basic_schedule_strict() {
        let segments = make_segments(&[1000, 1000, 1000, 1000]);
        let config = make_config(2000, BudgetMode::Strict);

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
    fn strict_mode_does_not_exceed_budget() {
        // Two segments that together slightly exceed the budget.
        // Strict mode should split them across two days.
        let segments = make_segments(&[1550, 1480]); // sum = 3030 > 3000
        let config = make_config(3000, BudgetMode::Strict);

        let releases = compute_release_schedule(&segments, &config, "feed-1");
        assert_eq!(releases.len(), 2);

        // They should be on different days
        assert_ne!(
            releases[0].release_at.date_naive(),
            releases[1].release_at.date_naive()
        );
    }

    #[test]
    fn flexible_mode_allows_closer_overshoot() {
        // Two segments that together slightly exceed the budget.
        // Flexible mode should group them because 3030 is closer to 3000 than 1550.
        let segments = make_segments(&[1550, 1480]); // sum = 3030 > 3000
        let config = make_config(3000, BudgetMode::Flexible);

        let releases = compute_release_schedule(&segments, &config, "feed-1");
        assert_eq!(releases.len(), 2);

        // They should be on the SAME day (flexible allows overshoot closer to target)
        assert_eq!(
            releases[0].release_at.date_naive(),
            releases[1].release_at.date_naive()
        );
    }

    #[test]
    fn flexible_mode_does_not_add_when_further() {
        // First segment uses most of budget; second would move us much further away.
        // 2800 + 1200 = 4000, which is further from 3000 than 2800 alone.
        // (distance from 2800 to 3000 is 200, distance from 4000 to 3000 is 1000)
        let segments = make_segments(&[2800, 1200]);
        let config = make_config(3000, BudgetMode::Flexible);

        let releases = compute_release_schedule(&segments, &config, "feed-1");
        assert_eq!(releases.len(), 2);

        // They should be on DIFFERENT days (1200 > 2*(3000-2800) = 400)
        assert_ne!(
            releases[0].release_at.date_naive(),
            releases[1].release_at.date_naive()
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
            budget_mode: BudgetMode::Strict,
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

    /// Stagger must give the FIRST segment in reading order
    /// the LARGEST timestamp within a same-day batch.
    #[test]
    fn stagger_gives_first_segment_largest_timestamp() {
        // 3 small segments that fit in one day
        let segments = make_segments(&[500, 500, 500]);
        let config = make_config(3000, BudgetMode::Strict);

        let releases = compute_release_schedule(&segments, &config, "feed-1");
        assert_eq!(releases.len(), 3);

        // All on same day
        assert_eq!(
            releases[0].release_at.date_naive(),
            releases[1].release_at.date_naive()
        );
        assert_eq!(
            releases[1].release_at.date_naive(),
            releases[2].release_at.date_naive()
        );

        // First segment has the LARGEST timestamp
        assert!(
            releases[0].release_at > releases[1].release_at,
            "seg0 must have larger timestamp than seg1"
        );
        assert!(
            releases[1].release_at > releases[2].release_at,
            "seg1 must have larger timestamp than seg2"
        );

        // Verify specific offsets: seg0 = +2s, seg1 = +1s, seg2 = +0s
        let base = releases[2].release_at; // seg2 has base time (no offset)
        assert_eq!(
            (releases[0].release_at - base).num_seconds(),
            2,
            "seg0 should be 2 seconds after base"
        );
        assert_eq!(
            (releases[1].release_at - base).num_seconds(),
            1,
            "seg1 should be 1 second after base"
        );
    }

    /// Test that segments on different days don't interfere with each other's stagger.
    #[test]
    fn stagger_only_affects_same_day_segments() {
        // 4 segments: 2 fit on day 1, 2 fit on day 2
        let segments = make_segments(&[1500, 1400, 1500, 1400]);
        let config = make_config(3000, BudgetMode::Strict);

        let releases = compute_release_schedule(&segments, &config, "feed-1");
        assert_eq!(releases.len(), 4);

        // Day 1: seg0, seg1
        let day1 = releases[0].release_at.date_naive();
        assert_eq!(releases[1].release_at.date_naive(), day1);

        // Day 2: seg2, seg3
        let day2 = releases[2].release_at.date_naive();
        assert_eq!(releases[3].release_at.date_naive(), day2);
        assert_ne!(day1, day2);

        // Stagger within day 1: seg0 > seg1
        assert!(releases[0].release_at > releases[1].release_at);

        // Stagger within day 2: seg2 > seg3
        assert!(releases[2].release_at > releases[3].release_at);
    }

    /// Sorting by `release_at` DESC should give correct reading order.
    #[test]
    fn stagger_produces_correct_order_when_sorted_desc() {
        use std::collections::HashMap;

        let segments = make_segments(&[500, 500, 500, 500]);
        let config = make_config(3000, BudgetMode::Strict);

        let releases = compute_release_schedule(&segments, &config, "feed-1");

        // Map segment_id to index for verification
        let id_to_index: HashMap<String, usize> = segments
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), i))
            .collect();

        // Sort by release_at DESC (what the database does)
        let mut sorted_releases = releases.clone();
        sorted_releases.sort_by(|a, b| b.release_at.cmp(&a.release_at));

        // After DESC sort, releases should be in reading order (seg0, seg1, seg2, seg3)
        // because stagger gave seg0 the largest timestamp
        for (position, release) in sorted_releases.iter().enumerate() {
            let segment_index = id_to_index.get(&release.segment_id).unwrap();
            assert_eq!(
                *segment_index, position,
                "After DESC sort, position {position} should contain segment index {position}, but got {segment_index}"
            );
        }
    }
}
