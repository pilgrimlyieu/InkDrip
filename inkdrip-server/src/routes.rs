pub mod aggregates;
pub mod books;
pub mod feeds;

use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;
use chrono::{Duration, TimeZone, Utc};

use inkdrip_core::config::parse_timezone_offset;
use inkdrip_core::error::InkDripError;
use inkdrip_core::feed;
use inkdrip_core::model::ScheduleConfig;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ─── Shared helpers ─────────────────────────────────────────────

/// Verify the API token from the `Authorization` header.
pub fn check_auth(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    let token = &state.config.server.api_token;
    if token.is_empty() {
        return Ok(());
    }

    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let expected = format!("Bearer {token}");
    if auth != expected {
        return Err(ApiError(InkDripError::Unauthorized));
    }
    Ok(())
}

/// Compute the next delivery datetime from a schedule config.
///
/// If today's `delivery_time` hasn't passed yet, use it; otherwise use tomorrow's.
pub fn compute_next_delivery(config: &ScheduleConfig) -> chrono::DateTime<chrono::FixedOffset> {
    let tz = parse_timezone_offset(&config.timezone);
    let delivery_time = chrono::NaiveTime::parse_from_str(&config.delivery_time, "%H:%M")
        .unwrap_or_else(|_| {
            chrono::NaiveTime::from_hms_opt(8, 0, 0)
                .unwrap_or_else(|| unreachable!("08:00:00 is always valid"))
        });

    let now = Utc::now().with_timezone(&tz);
    let today_at = tz
        .from_local_datetime(&now.date_naive().and_time(delivery_time))
        .single()
        .unwrap_or_else(|| Utc::now().into());

    if today_at > now {
        today_at.fixed_offset()
    } else {
        let tomorrow = now.date_naive() + Duration::days(1);
        tz.from_local_datetime(&tomorrow.and_time(delivery_time))
            .single()
            .unwrap_or_else(|| Utc::now().into())
            .fixed_offset()
    }
}

/// Truncate HTML content to approximately `max_chars` characters of plain text.
pub fn truncate_html(html: &str, max_chars: usize) -> String {
    let plain: String = html
        .chars()
        .fold((String::new(), false), |(mut acc, in_tag), ch| {
            if ch == '<' {
                (acc, true)
            } else if ch == '>' {
                (acc, false)
            } else if !in_tag {
                acc.push(ch);
                (acc, false)
            } else {
                (acc, true)
            }
        })
        .0;

    if plain.chars().count() <= max_chars {
        plain
    } else {
        let truncated: String = plain.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

/// GET /opml — Export all feeds as OPML.
pub async fn export_opml(State(state): State<AppState>) -> ApiResult<impl IntoResponse> {
    let feeds = state.store.list_feeds().await?;
    let mut feed_books = Vec::new();
    for f in feeds {
        if let Some(book) = state.store.get_book(&f.book_id).await? {
            feed_books.push((f, book));
        }
    }

    let opml = feed::generate_opml(&feed_books, &state.config.server.base_url);
    Ok((
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        opml,
    ))
}

/// GET /health — Health check.
pub async fn health_check() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use inkdrip_core::util;

    use super::*;

    #[test]
    fn generate_slug_basic() {
        assert_eq!(util::generate_slug("Hello World"), "hello-world");
        assert_eq!(util::generate_slug("The Great Gatsby"), "the-great-gatsby");
        assert_eq!(util::generate_slug("三体"), "三体");
        assert_eq!(util::generate_slug("Test  Book!!!"), "test-book");
    }

    #[test]
    fn truncate_html_short() {
        assert_eq!(truncate_html("<p>Hello</p>", 100), "Hello");
    }

    #[test]
    fn truncate_html_long() {
        let html = "<p>word </p>".repeat(50);
        let result = truncate_html(&html, 10);
        assert!(result.ends_with('…'));
        assert!(result.chars().count() <= 12); // 10 + "…"
    }

    #[test]
    fn truncate_html_strips_tags() {
        assert_eq!(
            truncate_html("<b>Bold</b> and <i>italic</i>", 100),
            "Bold and italic"
        );
    }
}
