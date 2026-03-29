<div align="right">

**[简体中文](README.zh-CN.md)** | **[English](README.md)**

<a href="https://linux.do" alt="LINUX DO"><img src="https://shorturl.at/ggSqS" /></a>

</div>

# InkDrip


**Turn books into RSS feeds — drip-feed your reading.**

InkDrip is a self-hosted service that splits e-books into small segments and delivers them on a configurable schedule via standard Atom/RSS feeds. Subscribe with any RSS reader (FreshRSS, Miniflux, Inoreader, etc.) and read a little every day — no more, no less.

## Features

- **Multi-format support** — EPUB, plain text, and Markdown
- **Smart splitting** — Respects paragraph and sentence boundaries; avoids mid-thought breaks
- **Configurable schedule** — Set words per day, delivery time, timezone, skip specific days of the week
- **Standard feeds** — Atom and RSS 2.0 output at `/feeds/:slug/atom.xml` or `/feeds/:slug/rss.xml`
- **Multiple books** — One feed per book, manage many simultaneously
- **Aggregate feeds** — Combine multiple feeds into a single unified feed
- **OPML export** — Import all feeds into your reader at once
- **File watching** — Drop books into a directory for automatic import
- **Content transforms** — Reading progress indicator, custom CSS, navigation links
- **Hook system** — Run external commands at key pipeline stages via JSON stdin/stdout
- **Undo / redo** — Revert or replay recent operations on books and feeds
- **Tiny footprint** — Single binary, SQLite storage, <15MB Docker image, <10MB RAM

## Quick Start

### Docker (recommended)

```bash
# Pull and run
docker run -d \
  --name inkdrip \
  -p 8080:8080 \
  -v inkdrip-data:/data \
  -e INKDRIP__SERVER__BASE_URL=http://your-server:8080 \
  pilgrimlyieu/inkdrip:latest

# Upload a book
curl -F "file=@my-book.epub" http://localhost:8080/api/books

# Create a feed (use the book ID from the response)
curl -X POST http://localhost:8080/api/books/<BOOK_ID>/feeds \
  -H "Content-Type: application/json" \
  -d '{"words_per_day": 3000}'

# Subscribe in your RSS reader:
# http://localhost:8080/feeds/<slug>/atom.xml   (Atom format)
# http://localhost:8080/feeds/<slug>/rss.xml    (RSS 2.0 format)
```

> **Deployment note:** Set `INKDRIP__SERVER__BASE_URL` to your public URL.
> Using `localhost` or `127.0.0.1` will cause a startup warning and broken
> feed links for external readers.

### Docker Compose

See [docker-compose.yml](docker-compose.yml) for a complete setup with RSSHub and FreshRSS.

### Build from source

```bash
# Requires Rust 1.85+
cargo build --release

# Run the server
./target/release/inkdrip-server

# Or use the CLI
./target/release/inkdrip-cli --help
```

## CLI Usage

The CLI communicates with the running server via HTTP API.

**Global flags** available on all commands:

| Flag      | Env var         | Default                 | Description                  |
| --------- | --------------- | ----------------------- | ---------------------------- |
| `--url`   | `INKDRIP_URL`   | `http://localhost:8080` | Server URL                   |
| `--token` | `INKDRIP_TOKEN` | *(empty)*               | API token for authentication |
| `--json`  | —               | —                       | Output raw JSON              |

```bash
# Set server URL (or use --url flag)
export INKDRIP_URL=http://localhost:8080

# Upload a book
inkdrip add my-book.epub --title "My Book" --author "Author Name"

# List books / feeds
inkdrip list books
inkdrip list feeds

# Create a feed
inkdrip feed create <BOOK_ID> --words-per-day 3000 --delivery-time 08:00

# Pause / resume / check status
inkdrip feed pause <FEED_ID>
inkdrip feed resume <FEED_ID>
inkdrip feed status <FEED_ID>

# Advance N upcoming segments immediately (default: 1)
inkdrip feed advance <FEED_ID> --count 3

# Edit book metadata or feed configuration
inkdrip edit book <BOOK_ID> --title "New Title" --author "New Author"
inkdrip edit feed <FEED_ID> --words-per-day 2000 --delivery-time 09:00

# Re-split a book (preserves already-released segments)
inkdrip resplit <BOOK_ID> --target-words 1200

# Read a specific segment
inkdrip read <BOOK_ID> <SEGMENT_INDEX>

# Remove a book and all its feeds
inkdrip remove <BOOK_ID>

# Undo / redo / history
inkdrip history list            # Show recent operations
inkdrip history undo            # Undo the last operation
inkdrip history redo            # Redo the last undone operation
inkdrip history clear           # Clear history and purge soft-deleted resources

# Aggregate feeds
inkdrip aggregate create <SLUG> --title "Daily Reading" --feeds <SLUG_1> --feeds <SLUG_2>
inkdrip aggregate list
inkdrip aggregate delete <AGGREGATE_ID>

# Debug / inspect
inkdrip debug segments <BOOK_ID>         # List all segments
inkdrip debug releases <FEED_ID>         # List release schedule
inkdrip debug preview <FEED_ID> --limit 5  # Preview upcoming segments
```

## Configuration

Copy [config.example.toml](config.example.toml) to `config.toml` (or `data/config.toml` in Docker).

All settings can also be overridden via environment variables with the `INKDRIP__` prefix:

```bash
# Examples
INKDRIP__SERVER__PORT=9090
INKDRIP__DEFAULTS__WORDS_PER_DAY=2000
INKDRIP__DEFAULTS__TIMEZONE=America/New_York
INKDRIP__WATCH__ENABLED=true
```

### Key settings

| Setting                         | Default                 | Description                                                                    |
| ------------------------------- | ----------------------- | ------------------------------------------------------------------------------ |
| `server.host`                   | `0.0.0.0`               | Address to bind the HTTP server                                                |
| `server.port`                   | `8080`                  | Port to listen on                                                              |
| `server.base_url`               | `http://localhost:8080` | Public URL for feed links and images                                           |
| `server.api_token`              | *(empty)*               | Bearer token for API auth; empty = no auth                                     |
| `server.public_feeds`           | `true`                  | Allow feed/OPML/aggregate endpoints without auth; set `false` to require token |
| `server.max_upload_bytes`       | `52428800`              | Maximum upload size in bytes (50 MiB)                                          |
| `storage.data_dir`              | `./data`                | Directory for database, books, and images                                      |
| `defaults.words_per_day`        | `3000`                  | Default daily word budget                                                      |
| `defaults.target_segment_words` | `1500`                  | Target words per segment                                                       |
| `defaults.max_segment_words`    | `2000`                  | Maximum words per segment                                                      |
| `defaults.min_segment_words`    | `500`                   | Minimum words per segment                                                      |
| `defaults.delivery_time`        | `08:00`                 | Daily release time (HH:MM)                                                     |
| `defaults.timezone`             | `Asia/Shanghai`         | Timezone for scheduling                                                        |
| `defaults.skip_days`            | `[]`                    | Days to skip (see below)                                                       |
| `defaults.budget_mode`          | `strict`                | Budget enforcement: `strict` or `flexible` (see below)                         |
| `watch.enabled`                 | `false`                 | Auto-import books from a directory                                             |
| `watch.dir`                     | `./books`               | Directory to watch for new book files                                          |
| `watch.auto_create_feed`        | `true`                  | Auto-create a feed when a book is detected                                     |
| `watch.scan_interval_secs`      | `300`                   | How often to scan the directory (seconds)                                      |
| `feed.format`                   | `atom`                  | Default feed format (`atom` or `rss`)                                          |
| `feed.items_limit`              | `50`                    | Max items returned per feed request                                            |
| `history.stack_depth`           | `50`                    | Max undo operations retained                                                   |

See [config.example.toml](config.example.toml) for the full config reference including `[transforms]`, `[hooks]`, `[parser.txt]`, and `[[aggregates]]` sections.

### Skip Days

`skip_days` accepts an array of day names (full or abbreviated, case-insensitive):

| Full name   | Abbreviation | Day       |
| ----------- | ------------ | --------- |
| `monday`    | `mon`        | Monday    |
| `tuesday`   | `tue`        | Tuesday   |
| `wednesday` | `wed`        | Wednesday |
| `thursday`  | `thu`        | Thursday  |
| `friday`    | `fri`        | Friday    |
| `saturday`  | `sat`        | Saturday  |
| `sunday`    | `sun`        | Sunday    |

Example: `skip_days = ["saturday", "sunday"]` to skip weekends.

> **Note:** The JSON API accepts `skip_days` as a `u8` bitfield integer
> (`MON=1, TUE=2, WED=4, THU=8, FRI=16, SAT=32, SUN=64`).

### Budget Mode

`budget_mode` controls how strictly the daily word budget is enforced during scheduling:

| Mode       | Description                                                                                   |
| ---------- | --------------------------------------------------------------------------------------------- |
| `strict`   | Never exceed `words_per_day`. A segment is pushed to the next day if it would exceed budget.  |
| `flexible` | Allow a segment if it brings the daily total closer to `words_per_day`, even if overshooting. |

Example: With `words_per_day = 3000` and two segments of 1550 and 1480 words:
- **Strict:** Day 1 gets 1550 words; Day 2 gets 1480 words.
- **Flexible:** Day 1 gets both (3030 words), since 3030 is closer to 3000 than 1550.

## API Reference

### Books

| Method   | Endpoint                         | Description                                                 |
| -------- | -------------------------------- | ----------------------------------------------------------- |
| `POST`   | `/api/books`                     | Upload book (multipart: `file`, optional `title`, `author`) |
| `GET`    | `/api/books`                     | List all books                                              |
| `GET`    | `/api/books/:id`                 | Book details with segments and feeds                        |
| `PATCH`  | `/api/books/:id`                 | Update book metadata                                        |
| `DELETE` | `/api/books/:id`                 | Delete book and all associated feeds                        |
| `GET`    | `/api/books/:id/segments`        | List all segments for a book                                |
| `GET`    | `/api/books/:id/segments/:index` | Read a specific segment                                     |
| `POST`   | `/api/books/:id/resplit`         | Re-split book (preserves released segments)                 |

### Feeds

| Method   | Endpoint                  | Description                             |
| -------- | ------------------------- | --------------------------------------- |
| `POST`   | `/api/books/:id/feeds`    | Create feed for a book                  |
| `GET`    | `/api/feeds`              | List all feeds with progress            |
| `GET`    | `/api/feeds/:id`          | Feed details                            |
| `PATCH`  | `/api/feeds/:id`          | Update feed (status, schedule)          |
| `DELETE` | `/api/feeds/:id`          | Delete feed                             |
| `GET`    | `/api/feeds/:id/releases` | List release schedule                   |
| `GET`    | `/api/feeds/:id/preview`  | Preview upcoming unreleased segments    |
| `POST`   | `/api/feeds/:id/advance`  | Advance N upcoming segments immediately |

### Aggregates

| Method   | Endpoint                             | Description           |
| -------- | ------------------------------------ | --------------------- |
| `POST`   | `/api/aggregates`                    | Create aggregate feed |
| `GET`    | `/api/aggregates`                    | List all aggregates   |
| `GET`    | `/api/aggregates/:id`                | Aggregate details     |
| `PATCH`  | `/api/aggregates/:id`                | Update aggregate      |
| `DELETE` | `/api/aggregates/:id`                | Delete aggregate      |
| `POST`   | `/api/aggregates/:id/feeds/:feed_id` | Add source feed       |
| `DELETE` | `/api/aggregates/:id/feeds/:feed_id` | Remove source feed    |

### History

| Method   | Endpoint            | Description                               |
| -------- | ------------------- | ----------------------------------------- |
| `GET`    | `/api/history`      | List recent operations                    |
| `POST`   | `/api/history/undo` | Undo the last operation                   |
| `POST`   | `/api/history/redo` | Redo the last undone operation            |
| `DELETE` | `/api/history`      | Clear history and purge soft-deleted data |

### Public Endpoints

| Method | Endpoint                     | Description              |
| ------ | ---------------------------- | ------------------------ |
| `GET`  | `/feeds/:slug/atom.xml`      | Atom feed                |
| `GET`  | `/feeds/:slug/rss.xml`       | RSS 2.0 feed             |
| `GET`  | `/aggregates/:slug/atom.xml` | Aggregate atom feed      |
| `GET`  | `/aggregates/:slug/rss.xml`  | Aggregate RSS feed       |
| `GET`  | `/images/:book_id/:file`     | Book images              |
| `GET`  | `/opml`                      | OPML export of all feeds |
| `GET`  | `/health`                    | Health check             |

> **Auth note:** When `api_token` is set and `public_feeds = false`, the feed/OPML/aggregate endpoints require a `Bearer <token>` header. Images (`/images/`) and `/health` are always public.

### Create Feed Request Body

```json
{
  "words_per_day": 3000,
  "delivery_time": "08:00",
  "skip_days": 96,
  "timezone": "Asia/Shanghai",
  "slug": "my-custom-slug",
  "start_at": "2026-03-01T08:00:00+08:00"
}
```

All fields are optional; defaults from configuration are used.

`skip_days` is a `u8` bitfield: `MON=1, TUE=2, WED=4, THU=8, FRI=16, SAT=32, SUN=64`.
For weekends: `32 + 64 = 96`.

## How It Works

1. **Upload** — Book file is parsed into chapters (EPUB spine, TXT separators, Markdown headings)
2. **Split** — Chapters are split into segments at paragraph boundaries, targeting ~1500 words each
3. **Schedule** — When a feed is created, release timestamps are pre-computed based on words-per-day budget
4. **Serve** — RSS reader polls the feed endpoint; only segments with `release_at ≤ now` are returned
5. **Transform** — Before delivery, segments pass through a configurable pipeline (progress indicator, CSS, navigation)

No background scheduler needed — release timing is computed upfront and evaluated lazily on each request.

## Architecture

```
inkdrip-core/           Core library: parsing, splitting, scheduling, feed generation
inkdrip-store-sqlite/   SQLite storage backend
inkdrip-server/         HTTP server (axum)
inkdrip-cli/            CLI tool (clap + reqwest)
```

The workspace is split into independent crates for modularity. The storage layer is behind a trait (`BookStore`), allowing future alternative backends.

## Supported Formats

| Format     | Extension          | Chapter Detection                             |
| ---------- | ------------------ | --------------------------------------------- |
| EPUB       | `.epub`            | EPUB spine (reading order)                    |
| Plain Text | `.txt`, `.text`    | `===` separator lines or multiple blank lines |
| Markdown   | `.md`, `.markdown` | `#` and `##` headings                         |

## Documentation

- [Splitting Algorithm](./docs/logic/splitting.md) - Detailed explanation of the semantic splitting strategy used to break chapters into segments while preserving natural reading boundaries.
- [Transform Pipeline & Hooks](./docs/logic/pipeline.md) - Overview of the content transformation pipeline and how to use hooks for custom processing.
- [Scheduling Algorithm](./docs/logic/scheduling.md) - Explanation of how release timestamps are computed for segments based on the feed's scheduling configuration.

## License

[AGPL-3.0](LICENSE)
