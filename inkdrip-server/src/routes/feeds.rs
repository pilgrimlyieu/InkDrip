use std::path::PathBuf;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use serde::Deserialize;
use tokio::fs;

use inkdrip_core::config::{parse_delivery_time, parse_timezone_offset};
use inkdrip_core::error::InkDripError;
use inkdrip_core::feed::{self, FeedFormat};
use inkdrip_core::model::{
    BudgetMode, Feed, FeedStatus, ScheduleConfig, Segment, SegmentRelease, SkipDays,
};
use inkdrip_core::pipeline;
use inkdrip_core::scheduler;
use inkdrip_core::undo::{FeedSnapshot, HistoryPayload};
use inkdrip_core::util;

use super::history::push_history;
use super::{check_auth, check_public_auth, compute_next_delivery, truncate_html};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ─── Feed API endpoints ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateFeedRequest {
    pub words_per_day: Option<u32>,
    pub delivery_time: Option<String>,
    pub skip_days: Option<u8>,
    pub timezone: Option<String>,
    pub slug: Option<String>,
    /// ISO 8601 datetime string for when to start releasing.
    pub start_at: Option<String>,
    /// Budget enforcement mode: "strict" or "flexible".
    pub budget_mode: Option<BudgetMode>,
}

/// POST /api/books/:id/feeds — Create a feed for a book.
pub async fn create_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(book_id): Path<String>,
    Json(req): Json<CreateFeedRequest>,
) -> ApiResult<impl IntoResponse> {
    check_auth(&state, &headers)?;
    let book_id = state.store.resolve_book_id(&book_id).await?;

    let book = state
        .store
        .get_book(&book_id)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(book_id.clone())))?;

    let defaults = &state.config.defaults;
    let tz_str = req.timezone.unwrap_or_else(|| defaults.timezone.clone());
    let tz = parse_timezone_offset(&tz_str);

    let start_at = if let Some(s) = &req.start_at {
        chrono::DateTime::parse_from_rfc3339(s)
            .map_err(|e| ApiError(InkDripError::ConfigError(format!("Invalid start_at: {e}"))))?
    } else {
        // Default: tomorrow at delivery_time
        let delivery = req
            .delivery_time
            .as_deref()
            .unwrap_or(&defaults.delivery_time);
        let time = parse_delivery_time(delivery);
        let now_local = Utc::now().with_timezone(&tz);
        let tomorrow = now_local.date_naive() + Duration::days(1);
        tz.from_local_datetime(&tomorrow.and_time(time))
            .single()
            .unwrap_or_else(|| Utc::now().into())
            .fixed_offset()
    };

    let schedule_config = ScheduleConfig {
        start_at,
        words_per_day: req.words_per_day.unwrap_or(defaults.words_per_day),
        delivery_time: req
            .delivery_time
            .unwrap_or_else(|| defaults.delivery_time.clone()),
        skip_days: req
            .skip_days
            .map_or(defaults.skip_days, SkipDays::from_bits_truncate),
        timezone: tz_str,
        budget_mode: req.budget_mode.unwrap_or(defaults.budget_mode),
    };

    let slug = req.slug.unwrap_or_else(|| util::generate_slug(&book.title));

    if state.store.get_feed_by_slug(&slug).await?.is_some() {
        return Err(ApiError(InkDripError::ConfigError(format!(
            "Feed slug '{slug}' already exists"
        ))));
    }

    let feed = Feed::new(book_id.clone(), slug, schedule_config.clone());

    let segments = state.store.get_segments(&book_id).await?;
    let releases = scheduler::compute_release_schedule(&segments, &schedule_config, &feed.id);

    let est_days = scheduler::estimate_days(book.total_words, schedule_config.words_per_day);

    state.store.save_feed(&feed).await?;
    state.store.save_releases(&releases).await?;

    push_history(
        &state,
        HistoryPayload::CreateFeed {
            feed_id: feed.id.clone(),
        },
        &format!("Create feed '{}' for '{}'", feed.slug, book.title),
    )
    .await;

    tracing::info!(
        "Feed '{}' created for '{}': ~{est_days} days",
        feed.slug,
        book.title,
    );

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "feed": feed,
            "estimated_days": est_days,
            "feed_url": format!("{}/feeds/{}/atom.xml", state.config.server.base_url, feed.slug),
        })),
    ))
}

/// GET /api/feeds — List all feeds.
pub async fn list_feeds(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    check_auth(&state, &headers)?;
    let feeds = state.store.list_feeds().await?;
    let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();

    let mut results = Vec::new();
    for feed in feeds {
        let book = state.store.get_book(&feed.book_id).await?;
        let released = state.store.count_released_segments(&feed.id, now).await?;
        let total = book.as_ref().map_or(0, |b| b.total_segments);

        results.push(serde_json::json!({
            "feed": feed,
            "book_title": book.map(|b| b.title).unwrap_or_default(),
            "released_segments": released,
            "total_segments": total,
            "feed_url": format!("{}/feeds/{}/atom.xml", state.config.server.base_url, feed.slug),
        }));
    }

    Ok(Json(results))
}

/// GET /api/feeds/:id — Get feed details.
pub async fn get_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_feed_id(&id).await?;

    let feed = state
        .store
        .get_feed(&id)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(id)))?;
    let book = state.store.get_book(&feed.book_id).await?;
    let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();
    let released = state.store.count_released_segments(&feed.id, now).await?;

    Ok(Json(serde_json::json!({
        "feed": feed,
        "book": book,
        "released_segments": released,
        "feed_url": format!("{}/feeds/{}/atom.xml", state.config.server.base_url, feed.slug),
    })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateFeedRequest {
    pub status: Option<String>,
    pub words_per_day: Option<u32>,
    pub delivery_time: Option<String>,
    pub skip_days: Option<u8>,
    pub timezone: Option<String>,
    pub slug: Option<String>,
    /// Budget enforcement mode: "strict" or "flexible".
    pub budget_mode: Option<BudgetMode>,
}

/// PATCH /api/feeds/:id — Update feed configuration.
///
/// Only unreleased segments are rescheduled; already-published entries are preserved.
pub async fn update_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateFeedRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_feed_id(&id).await?;

    let feed = state
        .store
        .get_feed(&id)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(id.clone())))?;

    // Capture pre-update state for undo
    let old_snapshot = FeedSnapshot {
        schedule_config: feed.schedule_config.clone(),
        status: feed.status,
        slug: feed.slug.clone(),
    };
    let old_releases = state.store.get_releases_for_feed(&id).await?;

    if let Some(status_str) = &req.status {
        let status: FeedStatus = status_str
            .parse()
            .map_err(|e: String| ApiError(InkDripError::ConfigError(e)))?;
        state.store.update_feed_status(&id, status).await?;
    }

    if let Some(slug) = &req.slug
        && slug != &feed.slug
    {
        if state.store.get_feed_by_slug(slug).await?.is_some() {
            return Err(ApiError(InkDripError::ConfigError(format!(
                "Feed slug '{slug}' already exists"
            ))));
        }
        state.store.update_feed_slug(&id, slug).await?;
    }

    let schedule_changed = req.words_per_day.is_some()
        || req.delivery_time.is_some()
        || req.skip_days.is_some()
        || req.timezone.is_some()
        || req.budget_mode.is_some();

    if schedule_changed {
        let mut new_config = feed.schedule_config.clone();
        if let Some(wpd) = req.words_per_day {
            new_config.words_per_day = wpd;
        }
        if let Some(dt) = &req.delivery_time {
            new_config.delivery_time.clone_from(dt);
        }
        if let Some(sd) = req.skip_days {
            new_config.skip_days = SkipDays::from_bits_truncate(sd);
        }
        if let Some(tz) = &req.timezone {
            new_config.timezone.clone_from(tz);
        }
        if let Some(bm) = req.budget_mode {
            new_config.budget_mode = bm;
        }

        let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();
        let released_count = state.store.count_released_segments(&id, now).await?;

        state
            .store
            .delete_future_releases_for_feed(&id, now)
            .await?;

        let all_segments = state.store.get_segments(&feed.book_id).await?;
        let unreleased: Vec<Segment> = all_segments
            .into_iter()
            .filter(|s| s.index >= released_count)
            .collect();

        if !unreleased.is_empty() {
            new_config.start_at = compute_next_delivery(&new_config);
            let releases = scheduler::compute_release_schedule(&unreleased, &new_config, &id);
            state.store.save_releases(&releases).await?;
        }

        state.store.update_feed_schedule(&id, &new_config).await?;
    }

    let updated = state.store.get_feed(&id).await?;
    let new_releases = state.store.get_releases_for_feed(&id).await?;

    if let Some(updated_feed) = &updated {
        let new_snapshot = FeedSnapshot {
            schedule_config: updated_feed.schedule_config.clone(),
            status: updated_feed.status,
            slug: updated_feed.slug.clone(),
        };
        push_history(
            &state,
            HistoryPayload::UpdateFeed {
                feed_id: id.clone(),
                old_state: old_snapshot,
                new_state: new_snapshot,
                old_releases,
                new_releases,
            },
            &format!("Update feed '{id}'"),
        )
        .await;
    }

    Ok(Json(serde_json::json!({ "feed": updated })))
}

/// DELETE /api/feeds/:id — Delete a feed.
pub async fn delete_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_feed_id(&id).await?;

    state
        .store
        .get_feed(&id)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(id.clone())))?;
    state.store.soft_delete_feed(&id).await?;

    push_history(
        &state,
        HistoryPayload::DeleteFeed {
            feed_id: id.clone(),
        },
        &format!("Delete feed '{id}'"),
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Feed debug endpoints ───────────────────────────────────────

/// GET /api/feeds/:id/releases — List all release entries for a feed.
pub async fn list_releases(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_feed_id(&id).await?;

    let feed = state
        .store
        .get_feed(&id)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(id.clone())))?;

    let releases = state.store.get_releases_for_feed(&id).await?;
    let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();
    let feed_tz = parse_timezone_offset(&feed.schedule_config.timezone);

    let items: Vec<serde_json::Value> = releases
        .iter()
        .map(|r| {
            serde_json::json!({
                "segment_id": r.segment_id,
                "release_at": r.release_at.with_timezone(&feed_tz).to_rfc3339(),
                "released": r.release_at <= now,
            })
        })
        .collect();

    Ok(Json(items))
}

#[derive(Debug, Deserialize)]
pub struct PreviewQuery {
    pub limit: Option<u32>,
}

/// Default number of segments to preview.
const DEFAULT_PREVIEW_LIMIT: u32 = 5;

/// GET /api/feeds/:id/preview — Preview upcoming unreleased segments.
pub async fn preview_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PreviewQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_feed_id(&id).await?;

    let feed = state
        .store
        .get_feed(&id)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(id.clone())))?;

    let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();
    let limit = query.limit.unwrap_or(DEFAULT_PREVIEW_LIMIT);
    let feed_tz = parse_timezone_offset(&feed.schedule_config.timezone);

    let upcoming = state
        .store
        .get_unreleased_segments_for_feed(&id, now, limit)
        .await?;

    let items: Vec<serde_json::Value> = upcoming
        .iter()
        .map(|(seg, rel)| {
            serde_json::json!({
                "segment_id": seg.id,
                "index": seg.index,
                "title_context": seg.title_context,
                "word_count": seg.word_count,
                "release_at": rel.release_at.with_timezone(&feed_tz).to_rfc3339(),
                "content_preview": truncate_html(&seg.content_html, 200),
            })
        })
        .collect();

    Ok(Json(items))
}

/// Default number of segments to advance.
const DEFAULT_ADVANCE_COUNT: u32 = 1;

#[derive(Debug, Deserialize)]
pub struct AdvanceFeedRequest {
    /// Number of upcoming segments to release immediately.
    pub count: Option<u32>,
}

/// POST /api/feeds/:id/advance — Advance the next N unreleased segments.
///
/// Sets their `release_at` to now, making them immediately visible in RSS.
pub async fn advance_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AdvanceFeedRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_feed_id(&id).await?;

    let feed = state
        .store
        .get_feed(&id)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(id.clone())))?;

    let count = body.count.unwrap_or(DEFAULT_ADVANCE_COUNT);
    let tz = parse_timezone_offset(&feed.schedule_config.timezone);
    let now = Utc::now().with_timezone(&tz);

    // Capture pre-advance state: the next `count` unreleased segments and full future releases
    let pre_advance_upcoming = state
        .store
        .get_unreleased_segments_for_feed(&id, now, count)
        .await?;
    let pre_advance_old: Vec<(String, DateTime<FixedOffset>)> = pre_advance_upcoming
        .iter()
        .map(|(_, r)| (r.segment_id.clone(), r.release_at))
        .collect();
    let pre_advance_releases = state.store.get_releases_for_feed(&id).await?;

    let advanced = state.store.advance_releases(&id, count, now).await?;

    // Reschedule all remaining future segments so that tomorrow's delivery
    // slot is filled — advancing today should not create a gap.
    let released_count = state.store.count_released_segments(&id, now).await?;
    state
        .store
        .delete_future_releases_for_feed(&id, now)
        .await?;

    let all_segments = state.store.get_segments(&feed.book_id).await?;
    let unreleased: Vec<Segment> = all_segments
        .into_iter()
        .filter(|s| s.index >= released_count)
        .collect();

    if !unreleased.is_empty() {
        let mut next_config = feed.schedule_config.clone();
        next_config.start_at = compute_next_delivery(&feed.schedule_config);
        let releases = scheduler::compute_release_schedule(&unreleased, &next_config, &id);
        state.store.save_releases(&releases).await?;
    }

    let book = state.store.get_book(&feed.book_id).await?;
    let total_segments = book.map_or(0, |b| b.total_segments);

    // Capture post-advance state for undo
    let new_releases_for_advanced: Vec<(String, chrono::DateTime<chrono::FixedOffset>)> =
        pre_advance_old
            .iter()
            .map(|(sid, _)| (sid.clone(), now))
            .collect();
    let post_advance_releases = state.store.get_releases_for_feed(&id).await?;

    push_history(
        &state,
        HistoryPayload::AdvanceFeed {
            feed_id: id.clone(),
            old_releases: pre_advance_old,
            new_releases: new_releases_for_advanced,
            pre_advance_releases,
            post_advance_releases,
        },
        &format!("Advance {advanced} segments for feed '{id}'"),
    )
    .await;

    Ok(Json(serde_json::json!({
        "feed_id": id,
        "advanced": advanced,
        "total_released": released_count,
        "total_segments": total_segments,
    })))
}

// ─── Public feed serving endpoints ──────────────────────────────

/// `GET /feeds/:slug/:format.xml` — Serve the feed for a book.
///
/// The `format` path segment selects the output: `"atom"` or `"rss"`.
/// This is the public endpoint that RSS readers poll.
/// It returns only segments whose `release_at` <= now.
/// Protected by `check_public_auth` when `server.public_feeds` is `false`.
pub async fn serve_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, format_name)): Path<(String, String)>,
) -> ApiResult<impl IntoResponse> {
    check_public_auth(&state, &headers)?;
    let format: FeedFormat = format_name
        .strip_suffix(".xml")
        .unwrap_or(&format_name)
        .parse()
        .unwrap_or(FeedFormat::Atom);
    let feed_record = state
        .store
        .get_feed_by_slug(&slug)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(slug.clone())))?;

    let book = state
        .store
        .get_book(&feed_record.book_id)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(
            feed_record.book_id.clone(),
        )))?;

    let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();
    let limit = state.config.feed.items_limit;

    let released = state
        .store
        .get_released_segments(&feed_record.id, now, limit)
        .await?;

    // Apply content transforms
    let transforms = pipeline::build_pipeline(&state.config.transforms, &state.config.hooks);
    let ctx = pipeline::TransformContext {
        total_segments: book.total_segments,
        total_words: book.total_words,
        base_url: state.config.server.base_url.clone(),
        feed_slug: slug.clone(),
        book_id: book.id.clone(),
    };

    let mut transformed: Vec<(Segment, SegmentRelease)> = released;
    for (segment, _) in &mut transformed {
        pipeline::apply_transforms(segment, &transforms, &ctx)?;
    }

    // Check if feed is completed (all segments released)
    if feed_record.status == FeedStatus::Active {
        let total_released = state
            .store
            .count_released_segments(&feed_record.id, now)
            .await?;
        if total_released >= book.total_segments {
            let _ = state
                .store
                .update_feed_status(&feed_record.id, FeedStatus::Completed)
                .await;
        }
    }

    let base_url = &state.config.server.base_url;

    let xml = match format {
        FeedFormat::Atom => feed::generate_atom_feed(&book, &feed_record, &transformed, base_url),
        FeedFormat::Rss => feed::generate_rss_feed(&book, &feed_record, &transformed, base_url),
    };

    Ok(([(header::CONTENT_TYPE, format.content_type())], xml))
}

/// `GET /images/:book_id/:filename` — Serve book images.
pub async fn serve_image(
    State(state): State<AppState>,
    Path((book_id, filename)): Path<(String, String)>,
) -> ApiResult<impl IntoResponse> {
    let images_dir = PathBuf::from(&state.config.storage.data_dir)
        .join("images")
        .join(&book_id);
    let file_path = images_dir.join(&filename);

    // Prevent path traversal
    if !file_path.starts_with(&images_dir) {
        return Err(ApiError(InkDripError::BookNotFound(
            "Invalid path".to_owned(),
        )));
    }

    let data = fs::read(&file_path).await.map_err(|e| {
        ApiError(InkDripError::BookNotFound(format!(
            "Image not found: {filename}: {e}"
        )))
    })?;

    let content_type = match file_path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };

    Ok(([(header::CONTENT_TYPE, content_type)], data))
}
