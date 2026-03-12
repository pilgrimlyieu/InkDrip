use rusqlite::Connection;

use inkdrip_core::error::{InkDripError, Result};

/// Current schema version.
const SCHEMA_VERSION: u32 = 3;

/// Run all pending database migrations.
pub fn run_migrations(conn: &Connection) -> Result<()> {
    let current_version = get_schema_version(conn)?;

    if current_version < 1 {
        migrate_v1(conn)?;
    }
    if current_version < 2 {
        migrate_v2(conn)?;
    }
    if current_version < 3 {
        migrate_v3(conn)?;
    }

    set_schema_version(conn, SCHEMA_VERSION)?;
    tracing::info!("Database schema at version {SCHEMA_VERSION}");
    Ok(())
}

fn get_schema_version(conn: &Connection) -> Result<u32> {
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| InkDripError::StorageError(format!("Failed to get schema version: {e}")))?;
    Ok(version)
}

fn set_schema_version(conn: &Connection, version: u32) -> Result<()> {
    conn.pragma_update(None, "user_version", version)
        .map_err(|e| InkDripError::StorageError(format!("Failed to set schema version: {e}")))?;
    Ok(())
}

/// Migration v1: Initial schema.
fn migrate_v1(conn: &Connection) -> Result<()> {
    tracing::info!("Running migration v1: initial schema");

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS books (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            author TEXT NOT NULL DEFAULT 'Unknown',
            format TEXT NOT NULL,
            file_hash TEXT NOT NULL,
            file_path TEXT NOT NULL,
            total_words INTEGER NOT NULL DEFAULT 0,
            total_segments INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_books_file_hash ON books(file_hash);

        CREATE TABLE IF NOT EXISTS segments (
            id TEXT PRIMARY KEY,
            book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
            idx INTEGER NOT NULL,
            title_context TEXT NOT NULL,
            content_html TEXT NOT NULL,
            word_count INTEGER NOT NULL,
            cumulative_words INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_segments_book_id ON segments(book_id);
        CREATE INDEX IF NOT EXISTS idx_segments_book_idx ON segments(book_id, idx);

        CREATE TABLE IF NOT EXISTS feeds (
            id TEXT PRIMARY KEY,
            book_id TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
            slug TEXT NOT NULL UNIQUE,
            schedule_config TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'active',
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_feeds_book_id ON feeds(book_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_feeds_slug ON feeds(slug);

        CREATE TABLE IF NOT EXISTS segment_releases (
            segment_id TEXT NOT NULL REFERENCES segments(id) ON DELETE CASCADE,
            feed_id TEXT NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
            release_at TEXT NOT NULL,
            PRIMARY KEY (segment_id, feed_id)
        );

        CREATE INDEX IF NOT EXISTS idx_releases_feed_time ON segment_releases(feed_id, release_at);

        CREATE TABLE IF NOT EXISTS aggregate_feeds (
            id TEXT PRIMARY KEY,
            slug TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            include_all INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_aggregate_feeds_slug ON aggregate_feeds(slug);

        CREATE TABLE IF NOT EXISTS aggregate_feed_sources (
            aggregate_id TEXT NOT NULL REFERENCES aggregate_feeds(id) ON DELETE CASCADE,
            feed_id TEXT NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
            PRIMARY KEY (aggregate_id, feed_id)
        );
        ",
    )
    .map_err(|e| InkDripError::StorageError(format!("Migration v1 failed: {e}")))?;

    Ok(())
}

/// Migration v2: Soft-delete support and undo/redo log.
fn migrate_v2(conn: &Connection) -> Result<()> {
    tracing::info!("Running migration v2: soft-delete + undo log");

    conn.execute_batch(
        "
        ALTER TABLE books ADD COLUMN deleted_at TEXT;
        ALTER TABLE feeds ADD COLUMN deleted_at TEXT;

        CREATE TABLE IF NOT EXISTS undo_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            operation TEXT NOT NULL,
            summary TEXT NOT NULL,
            created_at TEXT NOT NULL,
            payload TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS undo_cursor (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            current_id INTEGER NOT NULL DEFAULT 0
        );

        INSERT OR IGNORE INTO undo_cursor VALUES (1, 0);
        ",
    )
    .map_err(|e| InkDripError::StorageError(format!("Migration v2 failed: {e}")))?;

    Ok(())
}

/// Migration v3: Rebuild `feeds` table with a partial unique index on `slug`
/// restricted to non-deleted rows (`WHERE deleted_at IS NULL`).
///
/// The original schema had a global `UNIQUE` constraint on `slug` (both inline
/// and via an explicit index), which included soft-deleted rows.  This caused
/// `save_feed` to fail with a UNIQUE constraint violation whenever a
/// soft-deleted feed held the same slug as a new feed being created, even
/// though `get_feed_by_slug` (which filters `deleted_at IS NULL`) correctly
/// returned `None`.
///
/// `SQLite` does not support dropping inline column constraints, so the table
/// must be fully rebuilt.  Foreign-key enforcement is temporarily disabled
/// for the duration of the rebuild, as required by `SQLite`.
fn migrate_v3(conn: &Connection) -> Result<()> {
    tracing::info!("Running migration v3: partial unique index for feed slugs");

    // PRAGMA foreign_keys must be changed outside a transaction.
    conn.execute_batch("PRAGMA foreign_keys = OFF")
        .map_err(|e| InkDripError::StorageError(format!("Migration v3: {e}")))?;

    let result = conn.execute_batch(
        "
        BEGIN;

        -- Clean up any artifact from a previously failed migration attempt.
        DROP TABLE IF EXISTS feeds_new;

        CREATE TABLE feeds_new (
            id              TEXT PRIMARY KEY,
            book_id         TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
            slug            TEXT NOT NULL,
            schedule_config TEXT NOT NULL,
            status          TEXT NOT NULL DEFAULT 'active',
            created_at      TEXT NOT NULL,
            deleted_at      TEXT
        );

        INSERT INTO feeds_new
            SELECT id, book_id, slug, schedule_config, status, created_at, deleted_at
            FROM feeds;

        DROP TABLE feeds;

        ALTER TABLE feeds_new RENAME TO feeds;

        CREATE INDEX IF NOT EXISTS idx_feeds_book_id
            ON feeds(book_id);

        -- Partial index: only enforce slug uniqueness among non-deleted feeds.
        CREATE UNIQUE INDEX IF NOT EXISTS idx_feeds_slug
            ON feeds(slug) WHERE deleted_at IS NULL;

        COMMIT;
        ",
    );

    // Always restore FK enforcement, regardless of whether the migration succeeded.
    conn.execute_batch("PRAGMA foreign_keys = ON")
        .map_err(|e| InkDripError::StorageError(format!("Migration v3: restore FK: {e}")))?;

    result.map_err(|e| InkDripError::StorageError(format!("Migration v3 failed: {e}")))?;

    Ok(())
}
