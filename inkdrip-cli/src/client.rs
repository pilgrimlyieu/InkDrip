use std::path::Path;

use anyhow::Result;
use reqwest::{Client, Method, multipart};
use serde_json::Value;
use tokio::fs;

/// HTTP API client for `InkDrip`.
pub struct ApiClient {
    client: Client,
    base_url: String,
    token: String,
}

/// Parameters for updating a feed.
#[derive(Default)]
pub struct UpdateFeedParams {
    pub status: Option<String>,
    pub words_per_day: Option<u32>,
    pub delivery_time: Option<String>,
    pub skip_days: Option<u8>,
    pub timezone: Option<String>,
    pub slug: Option<String>,
}

impl ApiClient {
    pub fn new(base_url: &str, token: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
        }
    }

    fn auth_header(&self) -> Option<String> {
        if self.token.is_empty() {
            None
        } else {
            Some(format!("Bearer {}", self.token))
        }
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{path}", self.base_url);
        let mut req = self.client.request(method, &url);
        if let Some(auth) = self.auth_header() {
            req = req.header("Authorization", auth);
        }
        req
    }

    pub async fn upload_book(
        &self,
        file_path: &Path,
        title: Option<String>,
        author: Option<String>,
    ) -> Result<Value> {
        let file_name = file_path
            .file_name()
            .map_or_else(|| "book".to_owned(), |n| n.to_string_lossy().into_owned());

        let file_bytes = fs::read(file_path).await?;

        let mut form = multipart::Form::new().part(
            "file",
            multipart::Part::bytes(file_bytes).file_name(file_name),
        );

        if let Some(t) = title {
            form = form.text("title", t);
        }
        if let Some(a) = author {
            form = form.text("author", a);
        }

        let resp = self
            .request(Method::POST, "/api/books")
            .multipart(form)
            .send()
            .await?;

        handle_response(resp).await
    }

    pub async fn list_books(&self) -> Result<Vec<Value>> {
        let resp = self.request(Method::GET, "/api/books").send().await?;
        let val = handle_response(resp).await?;
        Ok(val.as_array().cloned().unwrap_or_default())
    }

    pub async fn list_feeds(&self) -> Result<Vec<Value>> {
        let resp = self.request(Method::GET, "/api/feeds").send().await?;
        let val = handle_response(resp).await?;
        Ok(val.as_array().cloned().unwrap_or_default())
    }

    pub async fn create_feed(
        &self,
        book_id: &str,
        words_per_day: Option<u32>,
        delivery_time: Option<String>,
        slug: Option<String>,
        skip_days: Option<u8>,
        start_at: Option<String>,
    ) -> Result<Value> {
        let mut obj = serde_json::Map::new();
        if let Some(w) = words_per_day {
            obj.insert("words_per_day".to_owned(), serde_json::json!(w));
        }
        if let Some(d) = delivery_time {
            obj.insert("delivery_time".to_owned(), serde_json::json!(d));
        }
        if let Some(s) = skip_days {
            obj.insert("skip_days".to_owned(), serde_json::json!(s));
        }
        if let Some(s) = slug {
            obj.insert("slug".to_owned(), serde_json::json!(s));
        }
        if let Some(s) = start_at {
            obj.insert("start_at".to_owned(), serde_json::json!(s));
        }
        let body = serde_json::Value::Object(obj);

        let resp = self
            .request(Method::POST, &format!("/api/books/{book_id}/feeds"))
            .json(&body)
            .send()
            .await?;

        handle_response(resp).await
    }

    pub async fn get_feed(&self, feed_id: &str) -> Result<Value> {
        let resp = self
            .request(Method::GET, &format!("/api/feeds/{feed_id}"))
            .send()
            .await?;
        handle_response(resp).await
    }

    pub async fn update_feed_status(&self, feed_id: &str, status: &str) -> Result<Value> {
        let body = serde_json::json!({ "status": status });
        let resp = self
            .request(Method::PATCH, &format!("/api/feeds/{feed_id}"))
            .json(&body)
            .send()
            .await?;
        handle_response(resp).await
    }

    pub async fn update_book(
        &self,
        book_id: &str,
        title: Option<String>,
        author: Option<String>,
    ) -> Result<Value> {
        let mut obj = serde_json::Map::new();
        if let Some(t) = title {
            obj.insert("title".to_owned(), serde_json::json!(t));
        }
        if let Some(a) = author {
            obj.insert("author".to_owned(), serde_json::json!(a));
        }
        let body = serde_json::Value::Object(obj);

        let resp = self
            .request(Method::PATCH, &format!("/api/books/{book_id}"))
            .json(&body)
            .send()
            .await?;
        handle_response(resp).await
    }

    pub async fn update_feed(&self, feed_id: &str, params: &UpdateFeedParams) -> Result<Value> {
        let mut obj = serde_json::Map::new();
        if let Some(s) = &params.status {
            obj.insert("status".to_owned(), serde_json::json!(s));
        }
        if let Some(w) = params.words_per_day {
            obj.insert("words_per_day".to_owned(), serde_json::json!(w));
        }
        if let Some(d) = &params.delivery_time {
            obj.insert("delivery_time".to_owned(), serde_json::json!(d));
        }
        if let Some(s) = params.skip_days {
            obj.insert("skip_days".to_owned(), serde_json::json!(s));
        }
        if let Some(t) = &params.timezone {
            obj.insert("timezone".to_owned(), serde_json::json!(t));
        }
        if let Some(s) = &params.slug {
            obj.insert("slug".to_owned(), serde_json::json!(s));
        }
        let body = serde_json::Value::Object(obj);

        let resp = self
            .request(Method::PATCH, &format!("/api/feeds/{feed_id}"))
            .json(&body)
            .send()
            .await?;
        handle_response(resp).await
    }

    pub async fn resplit_book(
        &self,
        book_id: &str,
        target_words: Option<u32>,
        max_words: Option<u32>,
        min_words: Option<u32>,
    ) -> Result<Value> {
        let mut obj = serde_json::Map::new();
        if let Some(t) = target_words {
            obj.insert("target_segment_words".to_owned(), serde_json::json!(t));
        }
        if let Some(m) = max_words {
            obj.insert("max_segment_words".to_owned(), serde_json::json!(m));
        }
        if let Some(m) = min_words {
            obj.insert("min_segment_words".to_owned(), serde_json::json!(m));
        }
        let body = serde_json::Value::Object(obj);

        let resp = self
            .request(Method::POST, &format!("/api/books/{book_id}/resplit"))
            .json(&body)
            .send()
            .await?;
        handle_response(resp).await
    }

    pub async fn list_segments(&self, book_id: &str) -> Result<Vec<Value>> {
        let resp = self
            .request(Method::GET, &format!("/api/books/{book_id}/segments"))
            .send()
            .await?;
        let val = handle_response(resp).await?;
        Ok(val.as_array().cloned().unwrap_or_default())
    }

    pub async fn list_releases(&self, feed_id: &str) -> Result<Vec<Value>> {
        let resp = self
            .request(Method::GET, &format!("/api/feeds/{feed_id}/releases"))
            .send()
            .await?;
        let val = handle_response(resp).await?;
        Ok(val.as_array().cloned().unwrap_or_default())
    }

    pub async fn preview_feed(&self, feed_id: &str, limit: Option<u32>) -> Result<Vec<Value>> {
        let mut path = format!("/api/feeds/{feed_id}/preview");
        if let Some(n) = limit {
            use std::fmt::Write;
            let _ = write!(path, "?limit={n}");
        }
        let resp = self.request(Method::GET, &path).send().await?;
        let val = handle_response(resp).await?;
        Ok(val.as_array().cloned().unwrap_or_default())
    }

    pub async fn delete_book(&self, book_id: &str) -> Result<()> {
        let resp = self
            .request(Method::DELETE, &format!("/api/books/{book_id}"))
            .send()
            .await?;
        handle_response(resp).await?;
        Ok(())
    }

    pub async fn advance_feed(&self, feed_id: &str, count: u32) -> Result<Value> {
        let body = serde_json::json!({ "count": count });
        let resp = self
            .request(Method::POST, &format!("/api/feeds/{feed_id}/advance"))
            .json(&body)
            .send()
            .await?;
        handle_response(resp).await
    }

    pub async fn read_segment(&self, book_id: &str, index: u32) -> Result<Value> {
        let resp = self
            .request(
                Method::GET,
                &format!("/api/books/{book_id}/segments/{index}"),
            )
            .send()
            .await?;
        handle_response(resp).await
    }

    // ─── Aggregates ─────────────────────────────────────────────

    pub async fn create_aggregate(
        &self,
        slug: &str,
        title: &str,
        description: &str,
        include_all: bool,
        feeds: &[String],
    ) -> Result<Value> {
        let body = serde_json::json!({
            "slug": slug,
            "title": title,
            "description": description,
            "include_all": include_all,
            "feeds": feeds,
        });
        let resp = self
            .request(Method::POST, "/api/aggregates")
            .json(&body)
            .send()
            .await?;
        handle_response(resp).await
    }

    pub async fn list_aggregates(&self) -> Result<Vec<Value>> {
        let resp = self.request(Method::GET, "/api/aggregates").send().await?;
        let val = handle_response(resp).await?;
        Ok(val.as_array().cloned().unwrap_or_default())
    }

    pub async fn delete_aggregate(&self, id: &str) -> Result<()> {
        let resp = self
            .request(Method::DELETE, &format!("/api/aggregates/{id}"))
            .send()
            .await?;
        handle_response(resp).await?;
        Ok(())
    }

    // ─── History / Undo / Redo ──────────────────────────────────

    pub async fn list_history(&self, limit: Option<u32>) -> Result<Vec<Value>> {
        let mut path = "/api/history".to_owned();
        if let Some(l) = limit {
            path = format!("{path}?limit={l}");
        }
        let resp = self.request(Method::GET, &path).send().await?;
        let val = handle_response(resp).await?;
        Ok(val.as_array().cloned().unwrap_or_default())
    }

    pub async fn undo(&self) -> Result<Value> {
        let resp = self
            .request(Method::POST, "/api/history/undo")
            .send()
            .await?;
        handle_response(resp).await
    }

    pub async fn redo(&self) -> Result<Value> {
        let resp = self
            .request(Method::POST, "/api/history/redo")
            .send()
            .await?;
        handle_response(resp).await
    }

    pub async fn clear_history(&self) -> Result<()> {
        let resp = self.request(Method::DELETE, "/api/history").send().await?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("{}", extract_error_message(&body))
        }
    }
}

/// Extract a human-readable error message from an API JSON error body.
///
/// The server returns `{"error": "..."}` on failures. This function extracts the
/// `"error"` field for a clean message, falling back to the raw body if parsing fails.
fn extract_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| body.to_owned())
}

async fn handle_response(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status.is_success() {
        let val: Value = serde_json::from_str(&body).unwrap_or(Value::String(body));
        Ok(val)
    } else {
        let message = extract_error_message(&body);
        anyhow::bail!("{message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_error_field() {
        let body = r#"{"error":"Book already exists (ID: abcd1234)"}"#;
        assert_eq!(
            extract_error_message(body),
            "Book already exists (ID: abcd1234)"
        );
    }

    #[test]
    fn extract_fallback_plain_text() {
        let body = "Internal Server Error";
        assert_eq!(extract_error_message(body), "Internal Server Error");
    }

    #[test]
    fn extract_json_without_error_field() {
        let body = r#"{"message":"something went wrong"}"#;
        assert_eq!(extract_error_message(body), body);
    }
}
