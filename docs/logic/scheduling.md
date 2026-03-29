<div align="right">

**[简体中文](scheduling.zh-CN.md)** | **[English](scheduling.md)**

</div>

# Scheduling Algorithm

InkDrip's scheduler computes release timestamps for all segments in a feed, distributing them across days based on a daily word budget. The implementation lives in [`inkdrip-core/src/scheduler.rs`](/inkdrip-core/src/scheduler.rs).

## Configuration

| Parameter     | Config Key               | Default           | Description                                              |
| ------------- | ------------------------ | ----------------- | -------------------------------------------------------- |
| Words per day | `defaults.words_per_day` | 3000              | Daily word budget                                        |
| Delivery time | `defaults.delivery_time` | `"08:00"`         | Fixed daily delivery time (HH:MM)                        |
| Timezone      | `defaults.timezone`      | `"Asia/Shanghai"` | IANA timezone name or `UTC±N`                            |
| Skip days     | `defaults.skip_days`     | `[]`              | Days of the week to skip (e.g. `["saturday", "sunday"]`) |
| Budget mode   | `defaults.budget_mode`   | `"strict"`        | Budget enforcement mode: `"strict"` or `"flexible"`      |

These defaults apply when creating a new feed and are **snapshotted** into the feed's `schedule_config`. Changing config.toml does not affect existing feeds — use `PATCH /api/feeds/:id` or `inkdrip edit feed` to update a live feed.

## Algorithm

The scheduler uses a **greedy budget allocation** approach:
1. Initialize `current_date` to the feed's `start_at` date and `daily_used` to 0.
2. For each segment (in order):
   - Check whether to advance to the next day based on `budget_mode`:
     - **Strict mode:** Advance if adding this segment would exceed `words_per_day` (and some content has already been scheduled for today).
     - **Flexible mode:** Advance if adding this segment would move the daily total *further* from `words_per_day` than the current total. This uses the "closer-to-target" heuristic (same as the splitter), allowing controlled overshoot when it results in a daily total closer to the budget.
   - Assign `release_at = current_date + delivery_time` (in the configured timezone).
   - Add the segment's `word_count` to `daily_used`.
3. When advancing dates, skip any day whose weekday appears in `skip_days`.

### Budget Modes

| Mode       | Behavior                                                                                                            |
| ---------- | ------------------------------------------------------------------------------------------------------------------- |
| `strict`   | Never exceed `words_per_day`. A segment is pushed to the next day if it would cause the total to exceed the budget. |
| `flexible` | Allow a segment if it brings the daily total closer to `words_per_day`, even if it slightly overshoots the budget.  |

**Example:** With `words_per_day = 3000` and two segments of 1550 and 1480 words (total 3030):
- **Strict mode:** Only the first segment (1550) is scheduled for day 1; the second (1480) goes to day 2.
- **Flexible mode:** Both segments are scheduled for day 1, since 3030 is closer to 3000 than 1550 alone.

### Key Behaviors

- **Multiple segments per day:** A day can hold multiple segments as long as the budget mode allows. Short segments naturally cluster together.
- **Same-day ordering (stagger):** When multiple segments land on the same day, they receive a small sub-second offset so that RSS readers always display them in reading order. Within a batch of N segments, segment k gets an offset of `(N-1-k)` seconds — the first segment in reading order has the highest timestamp and appears at the top in newest-first readers.
- **Oversized segments:** A segment larger than `words_per_day` is assigned to a fresh day on its own — it won't be split further at schedule time.
- **Skip days:** Weekend skipping (or any day combination) is supported. The scheduler advances past all skipped days when looking for the next valid date.

## Release Timing

All segments assigned to the same date share the same `release_at` timestamp — the configured `delivery_time` in the configured `timezone`. RSS readers polling after that time will see the new segments.

## Rescheduling

When a feed's configuration changes (e.g., `words_per_day`, `skip_days`, or `budget_mode` is updated), the scheduler recomputes all future releases. Already-released segments are not moved backward. The implementation:
1. Collects all segments for the book.
2. Recomputes the full schedule with the new config.
3. Updates only the `release_at` for segments not yet released.

## Estimation

The helper `estimate_days(total_words, words_per_day)` provides a rough day count: `ceil(total_words / words_per_day)`. This doesn't account for skip days and is used for UI display only.

## Data Flow

```
Feed creation request
        │
        ▼
  ScheduleConfig {
    start_at, words_per_day,
    delivery_time, timezone,
    skip_days, budget_mode
  }
        │
        ▼
  compute_release_schedule(segments, config, feed_id)
        │
        ▼
  Vec<SegmentRelease> {
    segment_id, feed_id, release_at
  }
        │
        ▼
  Stored in database
        │
        ▼
  serve_feed() queries: release_at ≤ now
```
