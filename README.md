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
- **Tiny footprint** — Single binary, SQLite storage, ~20MB Docker image, <50MB RAM

## Quick Start

### Docker (recommended)

```bash
# Pull and run
docker run -d \
  --name inkdrip \
  -p 8080:8080 \
  -v inkdrip-data:/data \
  -e INKDRIP__SERVER__BASE_URL=http://your-server:8080 \
  inkdrip:latest

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

```bash
# Set server URL (or use --url flag)
export INKDRIP_URL=http://localhost:8080

# Upload a book
inkdrip add my-book.epub --title "My Book" --author "Author Name"

# List books
inkdrip list books

# Create a feed
inkdrip feed create <BOOK_ID> --words-per-day 3000 --delivery-time 08:00

# List feeds with progress
inkdrip list feeds

# Pause / resume a feed
inkdrip feed pause <FEED_ID>
inkdrip feed resume <FEED_ID>

# Check feed status
inkdrip feed status <FEED_ID>

# Remove a book
inkdrip remove <BOOK_ID>

# Aggregate feeds
inkdrip aggregate create --title "Daily Reading" --feeds <FEED_ID_1>,<FEED_ID_2>
inkdrip aggregate list
inkdrip aggregate delete <AGGREGATE_ID>
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

| Setting                         | Default                 | Description                                |
| ------------------------------- | ----------------------- | ------------------------------------------ |
| `server.base_url`               | `http://localhost:8080` | Public URL for feed links and images       |
| `server.api_token`              | *(empty)*               | Bearer token for API auth; empty = no auth |
| `defaults.words_per_day`        | `3000`                  | Default daily word budget                  |
| `defaults.target_segment_words` | `1500`                  | Target words per segment                   |
| `defaults.delivery_time`        | `08:00`                 | Daily release time (HH:MM)                 |
| `defaults.timezone`             | `Asia/Shanghai`         | Timezone for scheduling                    |
| `defaults.skip_days`            | `[]`                    | Days to skip (see below)                   |
| `watch.enabled`                 | `false`                 | Auto-import books from a directory         |

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

## API Reference

### Books

| Method   | Endpoint         | Description                                                 |
| -------- | ---------------- | ----------------------------------------------------------- |
| `POST`   | `/api/books`     | Upload book (multipart: `file`, optional `title`, `author`) |
| `GET`    | `/api/books`     | List all books                                              |
| `GET`    | `/api/books/:id` | Book details with segments and feeds                        |
| `DELETE` | `/api/books/:id` | Delete book and all associated feeds                        |

### Feeds

| Method   | Endpoint               | Description                    |
| -------- | ---------------------- | ------------------------------ |
| `POST`   | `/api/books/:id/feeds` | Create feed for a book         |
| `GET`    | `/api/feeds`           | List all feeds with progress   |
| `GET`    | `/api/feeds/:id`       | Feed details                   |
| `PATCH`  | `/api/feeds/:id`       | Update feed (status, schedule) |
| `DELETE` | `/api/feeds/:id`       | Delete feed                    |

### Aggregates

| Method   | Endpoint                               | Description           |
| -------- | -------------------------------------- | --------------------- |
| `POST`   | `/api/aggregates`                      | Create aggregate feed |
| `GET`    | `/api/aggregates`                      | List all aggregates   |
| `GET`    | `/api/aggregates/:id`                  | Aggregate details     |
| `PATCH`  | `/api/aggregates/:id`                  | Update aggregate      |
| `DELETE` | `/api/aggregates/:id`                  | Delete aggregate      |
| `POST`   | `/api/aggregates/:id/sources/:feed_id` | Add source feed       |
| `DELETE` | `/api/aggregates/:id/sources/:feed_id` | Remove source feed    |

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

| Format     | Extension | Chapter Detection                             |
| ---------- | --------- | --------------------------------------------- |
| EPUB       | `.epub`   | EPUB spine (reading order)                    |
| Plain Text | `.txt`    | `===` separator lines or multiple blank lines |
| Markdown   | `.md`     | `#` and `##` headings                         |

## License

MIT
