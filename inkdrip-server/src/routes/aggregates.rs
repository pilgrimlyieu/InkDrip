use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use chrono::Utc;
use serde::Deserialize;

use inkdrip_core::error::InkDripError;
use inkdrip_core::feed::{self, FeedFormat};
use inkdrip_core::model::AggregateFeed;

use super::check_auth;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ─── Management API ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateAggregateRequest {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub include_all: bool,
    /// Optional list of feed slugs to include as sources.
    #[serde(default)]
    pub feeds: Vec<String>,
}

/// `POST /api/aggregates` — Create an aggregate feed.
pub async fn create_aggregate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateAggregateRequest>,
) -> ApiResult<impl IntoResponse> {
    check_auth(&state, &headers)?;

    let agg = AggregateFeed::new(body.slug, body.title, body.description, body.include_all);
    state.store.save_aggregate_feed(&agg).await?;

    // Link explicit source feeds
    for feed_slug in &body.feeds {
        if let Some(feed) = state.store.get_feed_by_slug(feed_slug).await? {
            state.store.add_aggregate_source(&agg.id, &feed.id).await?;
        } else {
            tracing::warn!("Aggregate source feed not found: {feed_slug}");
        }
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "aggregate": agg })),
    ))
}

/// `GET /api/aggregates` — List all aggregate feeds.
pub async fn list_aggregates(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    check_auth(&state, &headers)?;
    let aggs = state.store.list_aggregate_feeds().await?;

    let mut results = Vec::with_capacity(aggs.len());
    for agg in aggs {
        let sources = state.store.list_aggregate_sources(&agg.id).await?;
        results.push(serde_json::json!({
            "aggregate": agg,
            "source_feed_ids": sources,
        }));
    }

    Ok(Json(results))
}

/// `GET /api/aggregates/:id` — Get aggregate feed details.
pub async fn get_aggregate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    check_auth(&state, &headers)?;

    let agg = state
        .store
        .get_aggregate_feed(&id)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(id)))?;

    let sources = state.store.list_aggregate_sources(&agg.id).await?;

    Ok(Json(serde_json::json!({
        "aggregate": agg,
        "source_feed_ids": sources,
    })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateAggregateRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub include_all: Option<bool>,
}

/// `PATCH /api/aggregates/:id` — Update an aggregate feed.
pub async fn update_aggregate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateAggregateRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    check_auth(&state, &headers)?;

    let agg = state
        .store
        .get_aggregate_feed(&id)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(id.clone())))?;

    let title = body.title.unwrap_or(agg.title);
    let description = body.description.unwrap_or(agg.description);
    let include_all = body.include_all.unwrap_or(agg.include_all);

    state
        .store
        .update_aggregate_feed(&id, &title, &description, include_all)
        .await?;

    let updated = state
        .store
        .get_aggregate_feed(&id)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(id)))?;

    Ok(Json(serde_json::json!({ "aggregate": updated })))
}

/// `DELETE /api/aggregates/:id` — Delete an aggregate feed.
pub async fn delete_aggregate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    check_auth(&state, &headers)?;
    state.store.delete_aggregate_feed(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/aggregates/:id/feeds/:feed_id` — Add a feed source to an aggregate.
pub async fn add_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, feed_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    check_auth(&state, &headers)?;
    state.store.add_aggregate_source(&id, &feed_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/aggregates/:id/feeds/:feed_id` — Remove a feed source from an aggregate.
pub async fn remove_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, feed_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    check_auth(&state, &headers)?;
    state.store.remove_aggregate_source(&id, &feed_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── Public serving endpoint ────────────────────────────────────

/// `GET /aggregates/:slug/:format` — Serve an aggregate feed.
pub async fn serve_aggregate(
    State(state): State<AppState>,
    Path((slug, format_name)): Path<(String, String)>,
) -> ApiResult<impl IntoResponse> {
    let format: FeedFormat = format_name
        .strip_suffix(".xml")
        .unwrap_or(&format_name)
        .parse()
        .unwrap_or(FeedFormat::Atom);

    let agg = state
        .store
        .get_aggregate_feed_by_slug(&slug)
        .await?
        .ok_or(ApiError(InkDripError::FeedNotFound(slug)))?;

    let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();
    let limit = state.config.feed.items_limit;

    let segments = state
        .store
        .get_aggregate_released_segments(&agg.id, agg.include_all, now, limit)
        .await?;

    let base_url = &state.config.server.base_url;
    let xml = match format {
        FeedFormat::Atom => feed::generate_aggregate_atom(&agg, &segments, base_url),
        FeedFormat::Rss => feed::generate_aggregate_rss(&agg, &segments, base_url),
    };

    Ok(([(header::CONTENT_TYPE, format.content_type())], xml))
}
