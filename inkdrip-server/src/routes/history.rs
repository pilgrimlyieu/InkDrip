use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

use inkdrip_core::error::InkDripError;
use inkdrip_core::model::SegmentRelease;
use inkdrip_core::undo::{FeedSnapshot, HistoryPayload};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

use super::{check_auth, replace_future_releases};

/// DELETE /api/history — Clear all undo/redo history and purge soft-deleted resources.
///
/// Hard-deletes every soft-deleted book and feed (which only exist as undo/redo
/// targets), wipes the undo log, and resets the cursor to zero.  Active books
/// and feeds are left untouched.
pub async fn clear_history(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<impl IntoResponse> {
    check_auth(&state, &headers)?;
    state.store.clear_history().await?;
    tracing::info!("History cleared");
    Ok((StatusCode::NO_CONTENT, ()))
}

/// GET /api/history — List recent undo log entries.
pub async fn list_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    query: Query<ListHistoryQuery>,
) -> ApiResult<Json<Vec<Value>>> {
    check_auth(&state, &headers)?;

    let limit = query.limit.unwrap_or(20);
    let entries = state.store.list_undo_history(limit).await?;
    let cursor_entry = state.store.get_undo_entry().await?;
    let cursor_id = cursor_entry.map_or(0, |e| e.id);

    let items: Vec<Value> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "operation": e.operation,
                "summary": e.summary,
                "created_at": e.created_at.to_rfc3339(),
                "is_current": e.id == cursor_id,
            })
        })
        .collect();

    Ok(Json(items))
}

#[derive(Debug, Deserialize)]
pub struct ListHistoryQuery {
    pub limit: Option<u32>,
}

/// POST /api/history/undo — Undo the action at the current cursor.
pub async fn undo(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    check_auth(&state, &headers)?;

    let entry = state
        .store
        .get_undo_entry()
        .await?
        .ok_or_else(|| ApiError(InkDripError::Other(anyhow::anyhow!("Nothing to undo"))))?;

    let payload: HistoryPayload = serde_json::from_value(entry.payload.clone()).map_err(|e| {
        ApiError(InkDripError::Other(anyhow::anyhow!(
            "Bad undo payload: {e}"
        )))
    })?;

    apply_undo(&state, &payload).await?;
    state.store.retreat_undo_cursor().await?;

    Ok(Json(serde_json::json!({
        "undone": entry.operation,
        "summary": entry.summary,
    })))
}

/// POST /api/history/redo — Redo the next action after the cursor.
pub async fn redo(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    check_auth(&state, &headers)?;

    let entry = state
        .store
        .get_redo_entry()
        .await?
        .ok_or_else(|| ApiError(InkDripError::Other(anyhow::anyhow!("Nothing to redo"))))?;

    let payload: HistoryPayload = serde_json::from_value(entry.payload.clone()).map_err(|e| {
        ApiError(InkDripError::Other(anyhow::anyhow!(
            "Bad redo payload: {e}"
        )))
    })?;

    apply_redo(&state, &payload).await?;
    state.store.advance_undo_cursor(entry.id).await?;

    Ok(Json(serde_json::json!({
        "redone": entry.operation,
        "summary": entry.summary,
    })))
}

// ─── Undo dispatch ──────────────────────────────────────────────

async fn apply_undo(state: &AppState, payload: &HistoryPayload) -> ApiResult<()> {
    match payload {
        HistoryPayload::CreateFeed { feed_id } => {
            state.store.soft_delete_feed(feed_id).await?;
        }
        HistoryPayload::DeleteFeed { feed_id } => {
            state.store.restore_feed(feed_id).await?;
        }
        HistoryPayload::UpdateFeed {
            feed_id,
            old_state,
            old_releases,
            ..
        } => {
            restore_feed_state(state, feed_id, old_state).await?;
            // Restore the pre-update release schedule
            state.store.delete_releases_for_feed(feed_id).await?;
            state.store.save_releases(old_releases).await?;
        }
        HistoryPayload::AdvanceFeed {
            feed_id,
            old_releases,
            pre_advance_releases,
            ..
        } => {
            // Restore original timestamps for the advanced segments
            for (seg_id, original_at) in old_releases {
                update_single_release(state, seg_id, feed_id, original_at).await?;
            }
            // Restore future releases (the post-advance reschedule undone)
            restore_future_releases(state, feed_id, pre_advance_releases).await?;
        }
        HistoryPayload::UploadBook { book_id } => {
            state.store.soft_delete_book(book_id).await?;
        }
        HistoryPayload::DeleteBook { book_id } => {
            state.store.restore_book(book_id).await?;
        }
        HistoryPayload::UpdateBook {
            book_id,
            old_title,
            old_author,
            ..
        } => {
            state
                .store
                .update_book_meta(book_id, old_title, old_author)
                .await?;
        }
    }
    Ok(())
}

// ─── Redo dispatch ──────────────────────────────────────────────

async fn apply_redo(state: &AppState, payload: &HistoryPayload) -> ApiResult<()> {
    match payload {
        HistoryPayload::CreateFeed { feed_id } => {
            state.store.restore_feed(feed_id).await?;
        }
        HistoryPayload::DeleteFeed { feed_id } => {
            state.store.soft_delete_feed(feed_id).await?;
        }
        HistoryPayload::UpdateFeed {
            feed_id,
            new_state,
            new_releases,
            ..
        } => {
            restore_feed_state(state, feed_id, new_state).await?;
            state.store.delete_releases_for_feed(feed_id).await?;
            state.store.save_releases(new_releases).await?;
        }
        HistoryPayload::AdvanceFeed {
            feed_id,
            new_releases,
            post_advance_releases,
            ..
        } => {
            for (seg_id, new_at) in new_releases {
                update_single_release(state, seg_id, feed_id, new_at).await?;
            }
            restore_future_releases(state, feed_id, post_advance_releases).await?;
        }
        HistoryPayload::UploadBook { book_id } => {
            state.store.restore_book(book_id).await?;
        }
        HistoryPayload::DeleteBook { book_id } => {
            state.store.soft_delete_book(book_id).await?;
        }
        HistoryPayload::UpdateBook {
            book_id,
            new_title,
            new_author,
            ..
        } => {
            state
                .store
                .update_book_meta(book_id, new_title, new_author)
                .await?;
        }
    }
    Ok(())
}

// ─── Shared helpers ─────────────────────────────────────────────

async fn restore_feed_state(
    state: &AppState,
    feed_id: &str,
    snapshot: &FeedSnapshot,
) -> ApiResult<()> {
    state
        .store
        .update_feed_schedule(feed_id, &snapshot.schedule_config)
        .await?;
    state
        .store
        .update_feed_status(feed_id, snapshot.status)
        .await?;
    state
        .store
        .update_feed_slug(feed_id, &snapshot.slug)
        .await?;
    Ok(())
}

async fn update_single_release(
    state: &AppState,
    segment_id: &str,
    feed_id: &str,
    release_at: &chrono::DateTime<chrono::FixedOffset>,
) -> ApiResult<()> {
    // Update by deleting + re-inserting the single release
    let release = SegmentRelease {
        segment_id: segment_id.to_owned(),
        feed_id: feed_id.to_owned(),
        release_at: *release_at,
    };
    // We need to delete and re-save; use existing store methods
    // For single release updates, we save as a batch of 1
    state.store.save_releases(&[release]).await?;
    Ok(())
}

async fn restore_future_releases(
    state: &AppState,
    feed_id: &str,
    releases: &[SegmentRelease],
) -> ApiResult<()> {
    let now: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now().into();

    // Restore only future rows from the snapshot.
    let future: Vec<_> = releases
        .iter()
        .filter(|r| r.release_at > now)
        .cloned()
        .collect();
    replace_future_releases(state, feed_id, now, &future).await
}

/// Push an undo entry, logging a warning on failure rather than propagating.
pub async fn push_history(state: &AppState, payload: HistoryPayload, summary: &str) {
    let op = payload.operation_name();
    let json = match serde_json::to_value(&payload) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to serialize undo payload for {op}: {e}");
            return;
        }
    };

    let max_depth = state.config.history.stack_depth;
    if let Err(e) = state
        .store
        .push_undo_entry(op, summary, &json, max_depth)
        .await
    {
        warn!("Failed to push undo entry for {op}: {e}");
    }
}
