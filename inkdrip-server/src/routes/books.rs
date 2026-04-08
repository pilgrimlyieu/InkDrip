use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;

use inkdrip_core::error::InkDripError;
use inkdrip_core::hooks;
use inkdrip_core::model::{Book, BookFormat, Chapter, ParsedBook, Segment};
use inkdrip_core::parser;
use inkdrip_core::scheduler;
use inkdrip_core::splitter::semantic::SemanticSplitter;
use inkdrip_core::splitter::{SplitConfig, TextSplitter};
use inkdrip_core::undo::HistoryPayload;
use inkdrip_core::util;

use super::check_auth;
use super::compute_next_delivery;
use super::history::push_history;
use super::replace_future_releases;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ─── Book endpoints ─────────────────────────────────────────────

/// POST /api/books — Upload and process a book file.
pub async fn upload_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> ApiResult<impl IntoResponse> {
    check_auth(&state, &headers)?;

    let (data, fname, title_override, author_override) =
        parse_upload_multipart(&mut multipart).await?;

    // Compute file hash for dedup
    let file_hash = util::content_hash_hex(&data);

    // Check for duplicate
    if let Some(existing) = state.store.get_book_by_hash(&file_hash).await? {
        return Err(ApiError(InkDripError::DuplicateBook(existing.id)));
    }

    // Parse the book
    let mut parsed = parser::parse_book(&data, &fname, &state.config.parser)?;

    // Run post_book_parse hook (may modify chapters)
    run_post_book_parse_hook(&state, &mut parsed);

    let title = title_override.unwrap_or_else(|| parsed.title.clone());
    let author = author_override.unwrap_or_else(|| parsed.author.clone());

    // Determine format
    let ext = fname.rsplit('.').next().unwrap_or("");
    let format = BookFormat::from_extension(ext)
        .ok_or(ApiError(InkDripError::UnsupportedFormat(ext.into())))?;

    // Save files and split into segments
    let (book, segments) = save_and_split_book(
        &state,
        &data,
        &parsed,
        BookMeta {
            title,
            author,
            format,
            file_hash,
            ext: ext.to_owned(),
        },
    )?;

    // Persist
    state.store.save_book(&book).await?;
    state.store.save_segments(&segments).await?;

    push_history(
        &state,
        HistoryPayload::UploadBook {
            book_id: book.id.clone(),
        },
        &format!("Upload book '{}'", book.title),
    )
    .await;

    tracing::info!(
        "Book '{}' imported: {} words, {} segments",
        book.title,
        book.total_words,
        book.total_segments
    );

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "book": book,
            "segments_count": segments.len(),
        })),
    ))
}

/// GET /api/books — List all books.
pub async fn list_books(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<Book>>> {
    check_auth(&state, &headers)?;
    let books = state.store.list_books().await?;
    Ok(Json(books))
}

/// GET /api/books/:id — Get book details.
pub async fn get_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_book_id(&id).await?;
    let book = state
        .store
        .get_book(&id)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(id.clone())))?;
    let segments = state.store.get_segments(&id).await?;
    let feeds = state.store.list_feeds_for_book(&id).await?;

    Ok(Json(serde_json::json!({
        "book": book,
        "segments": segments.iter().map(|s| serde_json::json!({
            "index": s.index,
            "title_context": s.title_context,
            "word_count": s.word_count,
        })).collect::<Vec<_>>(),
        "feeds": feeds,
    })))
}

/// DELETE /api/books/:id — Delete a book and its feeds.
pub async fn delete_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_book_id(&id).await?;

    let book = state
        .store
        .get_book(&id)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(id.clone())))?;

    // Delete file and images
    let _ = fs::remove_file(&book.file_path);
    let images_dir = PathBuf::from(&state.config.storage.data_dir)
        .join("images")
        .join(&book.id);
    let _ = fs::remove_dir_all(&images_dir);

    state.store.soft_delete_book(&id).await?;

    push_history(
        &state,
        HistoryPayload::DeleteBook {
            book_id: id.clone(),
        },
        &format!("Delete book '{}'", book.title),
    )
    .await;

    tracing::info!("Book '{}' deleted", book.title);

    Ok(StatusCode::NO_CONTENT)
}

// ─── Book edit & debug endpoints ────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct UpdateBookRequest {
    pub title: Option<String>,
    pub author: Option<String>,
}

/// PATCH /api/books/:id — Update book metadata.
pub async fn update_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateBookRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_book_id(&id).await?;

    let book = state
        .store
        .get_book(&id)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(id.clone())))?;

    let title = req.title.unwrap_or(book.title.clone());
    let author = req.author.unwrap_or(book.author.clone());

    state.store.update_book_meta(&id, &title, &author).await?;

    push_history(
        &state,
        HistoryPayload::UpdateBook {
            book_id: id.clone(),
            old_title: book.title,
            old_author: book.author,
            new_title: title,
            new_author: author,
        },
        &format!("Update book '{id}'"),
    )
    .await;

    let updated = state
        .store
        .get_book(&id)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(id)))?;
    Ok(Json(serde_json::json!({ "book": updated })))
}

#[derive(Debug, Deserialize)]
#[expect(clippy::struct_field_names)]
pub struct ResplitRequest {
    pub target_segment_words: Option<u32>,
    pub max_segment_words: Option<u32>,
    pub min_segment_words: Option<u32>,
}

/// POST /api/books/:id/resplit — Re-split a book, preserving released segments.
pub async fn resplit_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ResplitRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_book_id(&id).await?;

    let book = state
        .store
        .get_book(&id)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(id.clone())))?;

    // Re-parse book from file
    let data = fs::read(&book.file_path)?;
    let fname = book.file_path.rsplit('/').next().unwrap_or(&book.file_path);
    let parsed = parser::parse_book(&data, fname, &state.config.parser)?;

    // Build split config (use request values or defaults)
    let split_config = SplitConfig::new(
        req.target_segment_words
            .unwrap_or(state.config.defaults.target_segment_words),
        req.max_segment_words
            .unwrap_or(state.config.defaults.max_segment_words),
        req.min_segment_words
            .unwrap_or(state.config.defaults.min_segment_words),
    );

    // Find max released segment index across all feeds
    let now: chrono::DateTime<chrono::FixedOffset> = chrono::Utc::now().into();
    let max_released_idx = state
        .store
        .get_max_released_index_for_book(&id, now)
        .await?;
    let old_segments = state.store.get_segments(&id).await?;

    let released_cumulative = max_released_idx
        .and_then(|idx| old_segments.iter().find(|s| s.index == idx))
        .map_or(0, |s| s.cumulative_words);
    let start_index = max_released_idx.map_or(0, |i| i + 1);

    // Re-split the whole book
    let all_new = SemanticSplitter.split(&id, &parsed.chapters, &split_config)?;

    // Find where new segments overlap with released content
    let keep_from = if released_cumulative == 0 {
        0
    } else {
        all_new
            .iter()
            .position(|s| s.cumulative_words > released_cumulative)
            .unwrap_or(all_new.len())
    };

    // Build new segments with correct indices and cumulative words
    let mut cumulative = released_cumulative;
    let new_segments: Vec<Segment> = all_new
        .get(keep_from..)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            cumulative += seg.word_count;
            Segment {
                id: util::generate_short_id(),
                book_id: id.clone(),
                index: start_index + i as u32,
                title_context: seg.title_context.clone(),
                content_html: seg.content_html.clone(),
                word_count: seg.word_count,
                cumulative_words: cumulative,
            }
        })
        .collect();

    // Delete old unreleased segments (and their releases)
    state
        .store
        .delete_segments_from_index(&id, start_index)
        .await?;

    // Save new segments
    state.store.save_segments(&new_segments).await?;

    // Update book total segments
    let total = start_index + new_segments.len() as u32;
    state.store.update_book_segment_count(&id, total).await?;

    // Reschedule each feed for the new segments
    let feeds = state.store.list_feeds_for_book(&id).await?;
    for feed in &feeds {
        let releases = if new_segments.is_empty() {
            Vec::new()
        } else {
            let mut config = feed.schedule_config.clone();
            config.start_at = compute_next_delivery(&config);
            scheduler::compute_release_schedule(&new_segments, &config, &feed.id)
        };
        replace_future_releases(&state, &feed.id, now, &releases).await?;
    }

    tracing::info!(
        "Book '{}' re-split: kept {start_index} released, {} new segments",
        book.title,
        new_segments.len()
    );

    Ok(Json(serde_json::json!({
        "book_id": id,
        "released_segments_kept": start_index,
        "new_segments": new_segments.len(),
        "total_segments": total,
    })))
}

/// GET /api/books/:id/segments — List all segments for a book.
pub async fn list_segments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_book_id(&id).await?;

    state
        .store
        .get_book(&id)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(id.clone())))?;

    let segments = state.store.get_segments(&id).await?;
    let items: Vec<serde_json::Value> = segments
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "index": s.index,
                "title_context": s.title_context,
                "word_count": s.word_count,
                "cumulative_words": s.cumulative_words,
            })
        })
        .collect();

    Ok(Json(items))
}

/// GET /api/books/:id/segments/:index — Read a single segment by index.
pub async fn read_segment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, index)): Path<(String, u32)>,
) -> ApiResult<Json<serde_json::Value>> {
    check_auth(&state, &headers)?;
    let id = state.store.resolve_book_id(&id).await?;

    let book = state
        .store
        .get_book(&id)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(id.clone())))?;

    let segment = state
        .store
        .get_segment_by_index(&id, index)
        .await?
        .ok_or(ApiError(InkDripError::BookNotFound(format!(
            "segment #{index} in book {id}"
        ))))?;

    Ok(Json(serde_json::json!({
        "book_id": id,
        "book_title": book.title,
        "segment": {
            "id": segment.id,
            "index": segment.index,
            "title_context": segment.title_context,
            "word_count": segment.word_count,
            "cumulative_words": segment.cumulative_words,
            "content_html": segment.content_html,
        },
    })))
}

// ─── Helpers ────────────────────────────────────────────────────

/// Parse upload multipart form fields into (`data`, `filename`, `title_override`, `author_override`).
async fn parse_upload_multipart(
    multipart: &mut Multipart,
) -> ApiResult<(Vec<u8>, String, Option<String>, Option<String>)> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut title_override: Option<String> = None;
    let mut author_override: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError(InkDripError::ParseError(format!("Multipart error: {e}"))))?
    {
        let name = field.name().unwrap_or("").to_owned();
        match name.as_str() {
            "file" => {
                filename = field.file_name().map(ToOwned::to_owned);
                file_data = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| {
                            ApiError(InkDripError::ParseError(format!(
                                "Failed to read file: {e}"
                            )))
                        })?
                        .to_vec(),
                );
            }
            "title" => {
                title_override = Some(field.text().await.map_err(|e| {
                    ApiError(InkDripError::ParseError(format!(
                        "Failed to read title: {e}"
                    )))
                })?);
            }
            "author" => {
                author_override = Some(field.text().await.map_err(|e| {
                    ApiError(InkDripError::ParseError(format!(
                        "Failed to read author: {e}"
                    )))
                })?);
            }
            _ => {}
        }
    }

    let data = file_data.ok_or(ApiError(InkDripError::ParseError(
        "No file uploaded".into(),
    )))?;
    let fname = filename.ok_or(ApiError(InkDripError::ParseError(
        "No filename provided".into(),
    )))?;
    Ok((data, fname, title_override, author_override))
}

/// Metadata for saving a book derived from the upload.
struct BookMeta {
    title: String,
    author: String,
    format: BookFormat,
    file_hash: String,
    ext: String,
}

/// Save book file + images to disk, split into segments, and build the `Book` record.
fn save_and_split_book(
    state: &AppState,
    data: &[u8],
    parsed: &ParsedBook,
    meta: BookMeta,
) -> ApiResult<(Book, Vec<Segment>)> {
    let BookMeta {
        title,
        author,
        format,
        file_hash,
        ext,
    } = meta;
    let data_dir = &state.config.storage.data_dir;
    let book_id = util::generate_short_id();
    let books_dir = PathBuf::from(data_dir).join("books");
    fs::create_dir_all(&books_dir)?;
    let file_path = books_dir.join(format!("{book_id}.{ext}"));
    let mut file = File::create(&file_path)?;
    file.write_all(data)?;

    // Save extracted images
    if !parsed.images.is_empty() {
        let images_dir = PathBuf::from(data_dir).join("images").join(&book_id);
        fs::create_dir_all(&images_dir)?;
        for (img_name, img_data) in &parsed.images {
            let img_path = images_dir.join(img_name);
            if let Ok(mut f) = File::create(&img_path) {
                let _ = f.write_all(img_data);
            }
        }
    }

    // Split into segments
    let split_config = SplitConfig::new(
        state.config.defaults.target_segment_words,
        state.config.defaults.max_segment_words,
        state.config.defaults.min_segment_words,
    );
    let segments = SemanticSplitter
        .split(&book_id, &parsed.chapters, &split_config)
        .map_err(ApiError::from)?;

    let mut book = Book::new(
        title,
        author,
        format,
        file_hash,
        file_path.to_string_lossy().into_owned(),
    );
    book.id = book_id;
    book.total_words = parsed.total_words();
    book.total_segments = segments.len() as u32;

    Ok((book, segments))
}

// ─── Hook helpers ───────────────────────────────────────────────

/// JSON sent to the `post_book_parse` hook.
#[derive(serde::Serialize)]
struct PostBookParseInput<'a> {
    hook: &'static str,
    title: &'a str,
    author: &'a str,
    chapters: &'a [Chapter],
}

/// JSON expected back from the `post_book_parse` hook.
#[derive(serde::Deserialize)]
struct PostBookParseOutput {
    chapters: Option<Vec<Chapter>>,
}

/// Invoke the `post_book_parse` hook if configured.
///
/// On success the chapters in `parsed` are replaced with the hook's output.
/// On failure the original chapters are preserved.
fn run_post_book_parse_hook(state: &AppState, parsed: &mut ParsedBook) {
    let hooks_cfg = &state.config.hooks;
    if !hooks_cfg.enabled {
        return;
    }

    let input = PostBookParseInput {
        hook: "post_book_parse",
        title: &parsed.title,
        author: &parsed.author,
        chapters: &parsed.chapters,
    };

    match hooks::run_hook::<_, PostBookParseOutput>(
        "post_book_parse",
        &hooks_cfg.post_book_parse,
        &input,
        hooks_cfg.timeout_secs,
    ) {
        Ok(Some(output)) => {
            if let Some(chapters) = output.chapters {
                tracing::info!(
                    "post_book_parse hook replaced {} chapters with {}",
                    parsed.chapters.len(),
                    chapters.len()
                );
                parsed.chapters = chapters;
            }
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!("post_book_parse hook error (ignored): {e}");
        }
    }
}
