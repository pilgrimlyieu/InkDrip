mod error;
mod routes;
mod state;

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, patch, post};
use chrono::{NaiveTime, TimeZone, Utc};
use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use tokio::net::TcpListener;
use tokio::time::sleep;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use inkdrip_core::config::{AppConfig, DefaultsConfig, parse_timezone_offset};
use inkdrip_core::model::AggregateFeed;
use inkdrip_core::model::{Book, BookFormat, Feed, ScheduleConfig, Segment};
use inkdrip_core::parser;
use inkdrip_core::scheduler;
use inkdrip_core::splitter::semantic::SemanticSplitter;
use inkdrip_core::splitter::{SplitConfig, TextSplitter};
use inkdrip_core::util;
use inkdrip_store_sqlite::SqliteStore;

use crate::state::AppState;

/// Default config file paths to search (in order).
const CONFIG_SEARCH_PATHS: &[&str] = &["./config.toml", "./data/config.toml", "/data/config.toml"];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "inkdrip_server=info,inkdrip_core=info,tower_http=info".into()),
        )
        .init();

    // Load configuration
    let config = load_config()?;
    tracing::info!(
        "InkDrip server starting on {}:{}",
        config.server.host,
        config.server.port
    );

    if config.server.base_url.contains("localhost") || config.server.base_url.contains("127.0.0.1")
    {
        tracing::warn!(
            "base_url is set to \"{}\". Feed links and images will not be reachable from remote clients. \
             Set server.base_url to your public address for production use.",
            config.server.base_url
        );
    }

    // Ensure data directories exist
    let data_dir = Path::new(&config.storage.data_dir);
    fs::create_dir_all(data_dir.join("books"))?;
    fs::create_dir_all(data_dir.join("images"))?;

    // Initialize database
    let db_path = data_dir.join("inkdrip.db");
    let store = SqliteStore::open(&db_path)?;
    store.migrate().await?;

    // Build application state
    let state = AppState {
        config: Arc::new(config.clone()),
        store: Arc::new(store),
    };

    // Upsert aggregate feeds declared in config.toml
    for agg_cfg in &config.aggregates {
        let agg = AggregateFeed::new(
            agg_cfg.slug.clone(),
            agg_cfg.title.clone(),
            agg_cfg.description.clone(),
            agg_cfg.include_all,
        );
        state.store.upsert_aggregate_feed(&agg).await?;

        // Resolve and link explicit feed slugs
        if !agg_cfg.include_all {
            // Get the upserted aggregate (may have existing ID from prior run)
            if let Some(existing) = state
                .store
                .get_aggregate_feed_by_slug(&agg_cfg.slug)
                .await?
            {
                for feed_slug in &agg_cfg.feeds {
                    if let Some(feed) = state.store.get_feed_by_slug(feed_slug).await? {
                        state
                            .store
                            .add_aggregate_source(&existing.id, &feed.id)
                            .await?;
                    } else {
                        tracing::warn!(
                            "Aggregate '{}': source feed '{feed_slug}' not found, skipping",
                            agg_cfg.slug,
                        );
                    }
                }
            }
        }
        tracing::info!("Aggregate feed '{}' synced from config", agg_cfg.slug);
    }

    // Set up file watcher if enabled
    if config.watch.enabled {
        let watch_state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = run_file_watcher(watch_state).await {
                tracing::error!("File watcher error: {e}");
            }
        });
    }

    // Build router
    let app = build_router(state);

    // Start server
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("Listening on {addr}");
    axum::serve(listener, app).await?;

    Ok(())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        // Book API
        .route(
            "/api/books",
            post(routes::books::upload_book)
                .route_layer(DefaultBodyLimit::max(state.config.server.max_upload_bytes)),
        )
        .route("/api/books", get(routes::books::list_books))
        .route("/api/books/{id}", get(routes::books::get_book))
        .route("/api/books/{id}", patch(routes::books::update_book))
        .route("/api/books/{id}", delete(routes::books::delete_book))
        .route("/api/books/{id}/segments", get(routes::books::list_segments))
        .route(
            "/api/books/{book_id}/segments/{index}",
            get(routes::books::read_segment),
        )
        .route("/api/books/{id}/resplit", post(routes::books::resplit_book))
        // Feed API
        .route("/api/books/{id}/feeds", post(routes::feeds::create_feed))
        .route("/api/feeds", get(routes::feeds::list_feeds))
        .route("/api/feeds/{id}", get(routes::feeds::get_feed))
        .route("/api/feeds/{id}", patch(routes::feeds::update_feed))
        .route("/api/feeds/{id}", delete(routes::feeds::delete_feed))
        .route("/api/feeds/{id}/releases", get(routes::feeds::list_releases))
        .route("/api/feeds/{id}/preview", get(routes::feeds::preview_feed))
        .route("/api/feeds/{id}/advance", post(routes::feeds::advance_feed))
        // Aggregate feed API
        .route("/api/aggregates", post(routes::aggregates::create_aggregate))
        .route("/api/aggregates", get(routes::aggregates::list_aggregates))
        .route("/api/aggregates/{id}", get(routes::aggregates::get_aggregate))
        .route("/api/aggregates/{id}", patch(routes::aggregates::update_aggregate))
        .route("/api/aggregates/{id}", delete(routes::aggregates::delete_aggregate))
        .route("/api/aggregates/{id}/feeds/{feed_id}", post(routes::aggregates::add_source))
        .route("/api/aggregates/{id}/feeds/{feed_id}", delete(routes::aggregates::remove_source))
        // Public feed serving
        .route("/feeds/{slug}/{format}", get(routes::feeds::serve_feed))
        .route("/aggregates/{slug}/{format}", get(routes::aggregates::serve_aggregate))
        .route("/images/{book_id}/{filename}", get(routes::feeds::serve_image))
        // OPML & health
        .route("/opml", get(routes::export_opml))
        .route("/health", get(routes::health_check))
        // Middleware
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

fn load_config() -> anyhow::Result<AppConfig> {
    let mut figment = Figment::new().merge(Serialized::defaults(AppConfig::default()));

    // Merge config file if found
    let config_path = env::var("INKDRIP_CONFIG").ok().or_else(|| {
        CONFIG_SEARCH_PATHS
            .iter()
            .find(|p| Path::new(p).exists())
            .map(ToString::to_string)
    });

    if let Some(path) = &config_path {
        tracing::info!("Loading config from: {path}");
        figment = figment.merge(Toml::file(path));
    }

    // Environment variable overrides (INKDRIP__SERVER__PORT etc.)
    figment = figment.merge(Env::prefixed("INKDRIP__").split("__"));

    let config: AppConfig = figment.extract()?;
    Ok(config)
}

/// Periodically scan the watch directory for new book files.
async fn run_file_watcher(state: AppState) -> anyhow::Result<()> {
    let watch_dir = PathBuf::from(&state.config.watch.dir);
    let interval = Duration::from_secs(state.config.watch.scan_interval_secs);

    tracing::info!("File watcher started on {watch_dir:?} with interval {interval:?}");

    loop {
        sleep(interval).await;

        if !watch_dir.exists() {
            continue;
        }

        // Get known file hashes
        let books = state.store.list_books().await.unwrap_or_default();
        let known_hashes: HashSet<String> = books.iter().map(|b| b.file_hash.clone()).collect();

        // Scan directory
        let entries = match fs::read_dir(&watch_dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Failed to read watch directory: {e}");
                continue;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            process_watch_entry(&state, &path, &known_hashes).await;
        }
    }
}

/// Process a single file found in the watch directory.
///
/// Returns early (silently) if the file extension is unsupported or already imported.
async fn process_watch_entry(state: &AppState, path: &Path, known_hashes: &HashSet<String>) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_owned();

    if BookFormat::from_extension(&ext).is_none() {
        return;
    }

    let Ok(data) = fs::read(path) else { return };
    let hash = util::content_hash_hex(&data);
    if known_hashes.contains(&hash) {
        return;
    }

    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    tracing::info!("Auto-importing book: {filename}");
    if let Err(e) = import_book(state, path, &data, &filename, &hash, &ext).await {
        tracing::warn!("Failed to import {filename}: {e}");
    }
}

/// Import a book from raw bytes into storage and optionally create a feed.
async fn import_book(
    state: &AppState,
    path: &Path,
    data: &[u8],
    filename: &str,
    hash: &str,
    ext: &str,
) -> anyhow::Result<()> {
    let parsed =
        parser::parse_book(data, filename, &state.config.parser).context("parse failed")?;
    let format = BookFormat::from_extension(ext)
        .ok_or_else(|| anyhow::anyhow!("unsupported extension: {ext}"))?;
    let book_id = util::generate_short_id();

    let dest = PathBuf::from(&state.config.storage.data_dir)
        .join("books")
        .join(format!("{book_id}.{ext}"));
    fs::copy(path, &dest).context("copy failed")?;

    let split_config = SplitConfig::new(
        state.config.defaults.target_segment_words,
        state.config.defaults.max_segment_words,
        state.config.defaults.min_segment_words,
    );
    let segments = SemanticSplitter
        .split(&book_id, &parsed.chapters, &split_config)
        .context("split failed")?;

    let mut book = Book::new(
        parsed.title.clone(),
        parsed.author.clone(),
        format,
        hash.to_owned(),
        dest.to_string_lossy().into_owned(),
    );
    book.id.clone_from(&book_id);
    book.total_words = parsed.total_words();
    book.total_segments = segments.len() as u32;

    state.store.save_book(&book).await.context("save book")?;
    state
        .store
        .save_segments(&segments)
        .await
        .context("save segments")?;

    if state.config.watch.auto_create_feed {
        create_auto_feed(state, &book_id, &parsed.title, &segments).await;
    }
    Ok(())
}

/// Compute the delivery start datetime from defaults config.
fn compute_delivery_start(defaults: &DefaultsConfig) -> chrono::DateTime<chrono::FixedOffset> {
    let tz = parse_timezone_offset(&defaults.timezone);
    let delivery_time =
        NaiveTime::parse_from_str(&defaults.delivery_time, "%H:%M").unwrap_or_else(|_| {
            NaiveTime::from_hms_opt(8, 0, 0)
                .unwrap_or_else(|| unreachable!("08:00:00 is always a valid NaiveTime"))
        });
    let tomorrow = (Utc::now() + chrono::Duration::days(1)).date_naive();
    tz.from_local_datetime(&tomorrow.and_time(delivery_time))
        .single()
        .unwrap_or_else(|| Utc::now().into())
        .fixed_offset()
}

/// Create a feed automatically for a newly imported book.
async fn create_auto_feed(state: &AppState, book_id: &str, title: &str, segments: &[Segment]) {
    let slug = util::generate_slug(title);
    if let Err(e) = try_persist_auto_feed(state, book_id, title, &slug, segments).await {
        tracing::warn!("Failed to create auto-feed '{}': {}", slug, e);
    } else {
        tracing::info!(
            "Auto-created feed '{slug}' for '{title}' ({} segments)",
            segments.len()
        );
    }
}

/// Build and persist the auto-feed and its release schedule.
async fn try_persist_auto_feed(
    state: &AppState,
    book_id: &str,
    _title: &str,
    slug: &str,
    segments: &[Segment],
) -> anyhow::Result<()> {
    let defaults = &state.config.defaults;
    let schedule = ScheduleConfig {
        start_at: compute_delivery_start(defaults),
        words_per_day: defaults.words_per_day,
        delivery_time: defaults.delivery_time.clone(),
        skip_days: defaults.skip_days,
        timezone: defaults.timezone.clone(),
    };

    let feed = Feed::new(book_id.to_owned(), slug.to_owned(), schedule.clone());
    let mut releases = scheduler::compute_release_schedule(segments, &schedule);
    for r in &mut releases {
        r.feed_id.clone_from(&feed.id);
    }

    state.store.save_feed(&feed).await.context("save feed")?;
    state
        .store
        .save_releases(&releases)
        .await
        .context("save releases")?;
    Ok(())
}
