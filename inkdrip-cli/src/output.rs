// CLI tool — stdout output is intentional.
#![expect(clippy::print_stdout, reason = "CLI tool outputs to user's terminal")]

use serde_json::Value;
use tabled::settings::Style;
use tabled::{Table, Tabled};

// ─── Table row types ────────────────────────────────────────────

#[derive(Tabled)]
struct BookRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Title")]
    title: String,
    #[tabled(rename = "Author")]
    author: String,
    #[tabled(rename = "Format")]
    format: String,
    #[tabled(rename = "Words")]
    words: String,
    #[tabled(rename = "Segments")]
    segments: String,
}

#[derive(Tabled)]
struct FeedRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Book")]
    book: String,
    #[tabled(rename = "Slug")]
    slug: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Progress")]
    progress: String,
}

#[derive(Tabled)]
struct SegmentRow {
    #[tabled(rename = "#")]
    index: String,
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Title")]
    title: String,
    #[tabled(rename = "Words")]
    words: String,
    #[tabled(rename = "Cumulative")]
    cumulative: String,
}

#[derive(Tabled)]
struct ReleaseRow {
    #[tabled(rename = "Segment")]
    segment_id: String,
    #[tabled(rename = "Release At")]
    release_at: String,
    #[tabled(rename = "Status")]
    status: String,
}

#[derive(Tabled)]
struct PreviewRow {
    #[tabled(rename = "#")]
    index: String,
    #[tabled(rename = "Title")]
    title: String,
    #[tabled(rename = "Words")]
    words: String,
    #[tabled(rename = "Release At")]
    release_at: String,
    #[tabled(rename = "Preview")]
    preview: String,
}

#[derive(Tabled)]
struct AggregateRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Slug")]
    slug: String,
    #[tabled(rename = "Title")]
    title: String,
    #[tabled(rename = "Include All")]
    include_all: String,
    #[tabled(rename = "Sources")]
    sources: String,
}

// ─── Helpers ────────────────────────────────────────────────────

fn str_val(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn u64_val(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

fn format_datetime(rfc3339: &str) -> String {
    // Show "YYYY-MM-DD HH:MM" in a compact form
    chrono::DateTime::parse_from_rfc3339(rfc3339).map_or_else(
        |_| rfc3339.to_owned(),
        |dt| dt.format("%Y-%m-%d %H:%M").to_string(),
    )
}

fn render_table<T: Tabled>(rows: Vec<T>) -> String {
    Table::new(rows).with(Style::rounded()).to_string()
}

// ─── Print functions ────────────────────────────────────────────

pub fn print_books_table(books: &[Value]) {
    if books.is_empty() {
        println!("No books found.");
        return;
    }

    let rows: Vec<BookRow> = books
        .iter()
        .map(|b| BookRow {
            id: str_val(b, "id"),
            title: truncate(&str_val(b, "title"), 30),
            author: truncate(&str_val(b, "author"), 20),
            format: str_val(b, "format"),
            words: u64_val(b, "total_words").to_string(),
            segments: u64_val(b, "total_segments").to_string(),
        })
        .collect();

    println!("{}", render_table(rows));
}

pub fn print_feeds_table(feeds: &[Value]) {
    if feeds.is_empty() {
        println!("No feeds found.");
        return;
    }

    let rows: Vec<FeedRow> = feeds
        .iter()
        .filter_map(|f| {
            let feed = f.get("feed")?;
            let released = u64_val(f, "released_segments");
            let total = u64_val(f, "total_segments");
            Some(FeedRow {
                id: str_val(feed, "id"),
                book: truncate(&str_val(f, "book_title"), 25),
                slug: str_val(feed, "slug"),
                status: str_val(feed, "status"),
                progress: format!("{released}/{total}"),
            })
        })
        .collect();

    println!("{}", render_table(rows));
}

pub fn print_segments_table(segments: &[Value]) {
    if segments.is_empty() {
        println!("No segments found.");
        return;
    }

    let rows: Vec<SegmentRow> = segments
        .iter()
        .map(|s| SegmentRow {
            index: u64_val(s, "index").to_string(),
            id: str_val(s, "id"),
            title: truncate(&str_val(s, "title_context"), 35),
            words: u64_val(s, "word_count").to_string(),
            cumulative: u64_val(s, "cumulative_words").to_string(),
        })
        .collect();

    println!("{}", render_table(rows));
}

pub fn print_releases_table(releases: &[Value]) {
    if releases.is_empty() {
        println!("No releases found.");
        return;
    }

    let rows: Vec<ReleaseRow> = releases
        .iter()
        .map(|r| {
            let is_released = r.get("released").and_then(Value::as_bool).unwrap_or(false);
            ReleaseRow {
                segment_id: str_val(r, "segment_id"),
                release_at: format_datetime(&str_val(r, "release_at")),
                status: if is_released {
                    "released".to_owned()
                } else {
                    "pending".to_owned()
                },
            }
        })
        .collect();

    println!("{}", render_table(rows));
}

pub fn print_preview(items: &[Value]) {
    if items.is_empty() {
        println!("No upcoming segments.");
        return;
    }

    let rows: Vec<PreviewRow> = items
        .iter()
        .map(|p| PreviewRow {
            index: u64_val(p, "index").to_string(),
            title: truncate(&str_val(p, "title_context"), 30),
            words: u64_val(p, "word_count").to_string(),
            release_at: format_datetime(&str_val(p, "release_at")),
            preview: truncate(&str_val(p, "content_preview"), 50),
        })
        .collect();

    println!("{}", render_table(rows));
}

pub fn print_upload_result(resp: &Value) {
    if let Some(book) = resp.get("book") {
        println!("Book uploaded successfully.");
        println!("  ID:       {}", str_val(book, "id"));
        println!("  Title:    {}", str_val(book, "title"));
        println!("  Author:   {}", str_val(book, "author"));
        println!("  Words:    {}", u64_val(book, "total_words"));
        println!("  Segments: {}", u64_val(resp, "segments_count"));
    }
}

pub fn print_feed_created(resp: &Value) {
    if let Some(feed) = resp.get("feed") {
        println!("Feed created successfully.");
        println!("  ID:    {}", str_val(feed, "id"));
        println!("  Slug:  {}", str_val(feed, "slug"));
        println!("  URL:   {}", str_val(resp, "feed_url"));
        println!("  Est.:  ~{} days", u64_val(resp, "estimated_days"));
    }
}

pub fn print_feed_status(resp: &Value) {
    if let Some(feed) = resp.get("feed") {
        println!("  ID:          {}", str_val(feed, "id"));
        println!("  Slug:        {}", str_val(feed, "slug"));
        println!("  Status:      {}", str_val(feed, "status"));
        println!("  Released:    {}", u64_val(resp, "released_segments"));
        println!("  Feed URL:    {}", str_val(resp, "feed_url"));

        if let Some(config) = feed.get("schedule_config") {
            println!("  Words/day:   {}", u64_val(config, "words_per_day"));
            println!("  Delivery:    {}", str_val(config, "delivery_time"));
            println!("  Timezone:    {}", str_val(config, "timezone"));
            println!("  Budget mode: {}", str_val(config, "budget_mode"));
        }
    }

    if let Some(book) = resp.get("book") {
        println!(
            "  Book:        {} ({})",
            str_val(book, "title"),
            str_val(book, "id")
        );
    }
}

pub fn print_feed_detail(resp: &Value) {
    if let Some(feed) = resp.get("feed") {
        println!("  ID:     {}", str_val(feed, "id"));
        println!("  Slug:   {}", str_val(feed, "slug"));
        println!("  Status: {}", str_val(feed, "status"));
    }
}

pub fn print_book_detail(resp: &Value) {
    if let Some(book) = resp.get("book") {
        println!("  ID:     {}", str_val(book, "id"));
        println!("  Title:  {}", str_val(book, "title"));
        println!("  Author: {}", str_val(book, "author"));
    }
}

pub fn print_resplit_result(resp: &Value) {
    println!("Book re-split completed.");
    println!(
        "  Released kept: {}",
        u64_val(resp, "released_segments_kept")
    );
    println!("  New segments:  {}", u64_val(resp, "new_segments"));
    println!("  Total:         {}", u64_val(resp, "total_segments"));
}

pub fn print_advance_result(resp: &Value) {
    let advanced = u64_val(resp, "advanced");
    let released = u64_val(resp, "total_released");
    let total = u64_val(resp, "total_segments");
    println!("Advanced {advanced} segment(s).");
    println!("  Released: {released}/{total}");
}

/// Strip HTML tags and display segment content as plain text.
pub fn print_segment_content(resp: &Value) {
    if let Some(seg) = resp.get("segment") {
        let title = str_val(resp, "book_title");
        let context = str_val(seg, "title_context");
        let words = u64_val(seg, "word_count");
        let index = u64_val(seg, "index");

        println!("── {title} / {context} (#{index}, {words} words) ──\n");

        let html = str_val(seg, "content_html");
        println!("{}", strip_html(&html));
    }
}

/// Simple HTML tag stripper for terminal display.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut last_was_newline = false;

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            if ch == '\n' {
                if !last_was_newline {
                    result.push(ch);
                    last_was_newline = true;
                }
            } else {
                result.push(ch);
                last_was_newline = false;
            }
        }
    }

    result.trim().to_owned()
}

// ─── Aggregate outputs ──────────────────────────────────────────

pub fn print_aggregate_created(resp: &Value) {
    if let Some(agg) = resp.get("aggregate") {
        println!("Aggregate feed created:");
        println!("  ID:          {}", str_val(agg, "id"));
        println!("  Slug:        {}", str_val(agg, "slug"));
        println!("  Title:       {}", str_val(agg, "title"));
        println!("  Include all: {}", bool_val(agg, "include_all"));
    }
}

pub fn print_aggregates_table(aggs: &[Value]) {
    if aggs.is_empty() {
        println!("No aggregate feeds.");
        return;
    }

    let rows: Vec<AggregateRow> = aggs
        .iter()
        .map(|item| {
            let agg = item.get("aggregate").unwrap_or(item);
            let sources = item
                .get("source_feed_ids")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            AggregateRow {
                id: str_val(agg, "id"),
                slug: str_val(agg, "slug"),
                title: truncate(&str_val(agg, "title"), 30),
                include_all: bool_val(agg, "include_all").to_string(),
                sources: sources.to_string(),
            }
        })
        .collect();

    println!("{}", render_table(rows));
}

fn bool_val(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

// ─── History / Undo / Redo ──────────────────────────────────────

#[derive(Tabled)]
struct HistoryRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Operation")]
    operation: String,
    #[tabled(rename = "Summary")]
    summary: String,
    #[tabled(rename = "Time")]
    time: String,
    #[tabled(rename = "")]
    marker: String,
}

pub fn print_history_table(entries: &[Value]) {
    if entries.is_empty() {
        println!("No history entries.");
        return;
    }

    let rows: Vec<HistoryRow> = entries
        .iter()
        .map(|e| {
            let marker = if bool_val(e, "is_current") {
                "<-".to_owned()
            } else {
                String::new()
            };
            HistoryRow {
                id: u64_val(e, "id").to_string(),
                operation: str_val(e, "operation"),
                summary: truncate(&str_val(e, "summary"), 50),
                time: str_val(e, "created_at"),
                marker,
            }
        })
        .collect();

    println!("{}", render_table(rows));
}

pub fn print_undo_result(resp: &Value) {
    println!(
        "Undone: {} — {}",
        str_val(resp, "undone"),
        str_val(resp, "summary"),
    );
}

pub fn print_redo_result(resp: &Value) {
    println!(
        "Redone: {} — {}",
        str_val(resp, "redone"),
        str_val(resp, "summary"),
    );
}
