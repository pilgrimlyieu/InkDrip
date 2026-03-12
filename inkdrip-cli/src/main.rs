// CLI tool — stdout output is intentional.
#![expect(clippy::print_stdout, reason = "CLI tool outputs to user's terminal")]

mod client;
mod output;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "inkdrip",
    about = "InkDrip — Turn books into RSS feeds",
    version
)]
struct Cli {
    /// Server URL (default: `http://localhost:8080`)
    #[arg(long, env = "INKDRIP_URL", default_value = "http://localhost:8080")]
    url: String,

    /// API token for authentication
    #[arg(long, env = "INKDRIP_TOKEN", default_value = "", global = true)]
    token: String,

    /// Output raw JSON instead of formatted tables
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Upload a book file
    Add {
        /// Path to the book file (epub, txt, md)
        file: PathBuf,
        /// Override book title
        #[arg(long)]
        title: Option<String>,
        /// Override book author
        #[arg(long)]
        author: Option<String>,
    },
    /// List resources
    List {
        #[command(subcommand)]
        resource: ListResource,
    },
    /// Manage feeds
    Feed {
        #[command(subcommand)]
        action: FeedAction,
    },
    /// Edit book or feed configuration
    Edit {
        #[command(subcommand)]
        target: EditTarget,
    },
    /// Re-split a book (preserves released segments)
    Resplit {
        /// Book ID (prefix match supported)
        book_id: String,
        /// Target words per segment
        #[arg(long)]
        target_words: Option<u32>,
        /// Maximum words per segment
        #[arg(long)]
        max_words: Option<u32>,
        /// Minimum words per segment
        #[arg(long)]
        min_words: Option<u32>,
    },
    /// Debug and inspect internal data
    Debug {
        #[command(subcommand)]
        action: DebugAction,
    },
    /// Read a specific segment of a book
    Read {
        /// Book ID (prefix match supported)
        book_id: String,
        /// Segment index (zero-based)
        index: u32,
    },
    /// Remove a book and its feeds
    Remove {
        /// Book ID
        book_id: String,
    },
    /// Manage aggregate feeds
    Aggregate {
        #[command(subcommand)]
        action: AggregateAction,
    },
    /// View and manage undo/redo history
    History {
        #[command(subcommand)]
        action: HistoryAction,
    },
}

#[derive(Subcommand)]
enum ListResource {
    /// List all books
    Books,
    /// List all feeds
    Feeds,
}

#[derive(Subcommand)]
enum FeedAction {
    /// Create a new feed for a book
    Create {
        /// Book ID
        book_id: String,
        /// Words to release per day (uses server default from config.toml if omitted)
        #[arg(long)]
        words_per_day: Option<u32>,
        /// Delivery time in HH:MM format (uses server default from config.toml if omitted)
        #[arg(long)]
        delivery_time: Option<String>,
        /// Custom slug for the feed URL
        #[arg(long)]
        slug: Option<String>,
        /// Days to skip delivery (bitflags: Mon=1,Tue=2,Wed=4,Thu=8,Fri=16,Sat=32,Sun=64; weekends=96)
        /// Uses server default from config.toml if omitted.
        #[arg(long)]
        skip_days: Option<u8>,
        /// Start date (ISO 8601, default: tomorrow)
        #[arg(long)]
        start: Option<String>,
    },
    /// Pause a feed
    Pause {
        /// Feed ID
        feed_id: String,
    },
    /// Resume a paused feed
    Resume {
        /// Feed ID
        feed_id: String,
    },
    /// Show feed status
    Status {
        /// Feed ID
        feed_id: String,
    },
    /// Advance N upcoming segments (release them immediately)
    Advance {
        /// Feed ID
        feed_id: String,
        /// Number of segments to advance (default: 1)
        #[arg(long, short, default_value = "1")]
        count: u32,
    },
}

#[derive(Subcommand)]
enum EditTarget {
    /// Edit book metadata
    Book {
        /// Book ID (prefix match supported)
        book_id: String,
        /// New title
        #[arg(long)]
        title: Option<String>,
        /// New author
        #[arg(long)]
        author: Option<String>,
    },
    /// Edit feed configuration
    Feed {
        /// Feed ID (prefix match supported)
        feed_id: String,
        /// New slug
        #[arg(long)]
        slug: Option<String>,
        /// Words per day
        #[arg(long)]
        words_per_day: Option<u32>,
        /// Delivery time (HH:MM)
        #[arg(long)]
        delivery_time: Option<String>,
        /// Days to skip delivery (bitflags: Mon=1,Tue=2,Wed=4,Thu=8,Fri=16,Sat=32,Sun=64)
        #[arg(long)]
        skip_days: Option<u8>,
        /// Timezone (e.g. "Asia/Shanghai")
        #[arg(long)]
        timezone: Option<String>,
        /// Status (active, paused, completed)
        #[arg(long)]
        status: Option<String>,
    },
}

#[derive(Subcommand)]
enum DebugAction {
    /// List segments for a book
    Segments {
        /// Book ID
        book_id: String,
    },
    /// List release schedule for a feed
    Releases {
        /// Feed ID
        feed_id: String,
    },
    /// Preview upcoming unreleased segments for a feed
    Preview {
        /// Feed ID
        feed_id: String,
        /// Number of segments to preview
        #[arg(long, default_value = "5")]
        limit: u32,
    },
}

#[derive(Subcommand)]
enum AggregateAction {
    /// Create an aggregate feed
    Create {
        /// URL slug for the aggregate
        slug: String,
        /// Display title
        #[arg(long)]
        title: String,
        /// Description
        #[arg(long, default_value = "")]
        description: String,
        /// Include all active feeds automatically
        #[arg(long)]
        include_all: bool,
        /// Feed slugs to include (when not using --include-all)
        #[arg(long)]
        feeds: Vec<String>,
    },
    /// List aggregate feeds
    List,
    /// Delete an aggregate feed
    Delete {
        /// Aggregate feed ID
        id: String,
    },
}

#[derive(Subcommand)]
enum HistoryAction {
    /// Undo the most recent action
    Undo,
    /// Redo the last undone action
    Redo,
    /// List recent history entries
    List {
        /// Maximum entries to show
        #[arg(long, default_value = "20")]
        limit: u32,
    },
}

/// Print API response as JSON or formatted output.
fn print_output(json_mode: bool, data: &impl Serialize, formatter: impl FnOnce()) {
    if json_mode {
        if let Ok(json) = serde_json::to_string_pretty(data) {
            println!("{json}");
        }
    } else {
        formatter();
    }
}

#[tokio::main]
#[expect(clippy::too_many_lines, reason = "CLI dispatch match arm")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = client::ApiClient::new(&cli.url, &cli.token);
    let json_mode = cli.json;

    match cli.command {
        Commands::Add {
            file,
            title,
            author,
        } => {
            let resp = client.upload_book(&file, title, author).await?;
            print_output(json_mode, &resp, || output::print_upload_result(&resp));
        }
        Commands::List { resource } => match resource {
            ListResource::Books => {
                let books = client.list_books().await?;
                print_output(json_mode, &books, || output::print_books_table(&books));
            }
            ListResource::Feeds => {
                let feeds = client.list_feeds().await?;
                print_output(json_mode, &feeds, || output::print_feeds_table(&feeds));
            }
        },
        Commands::Feed { action } => match action {
            FeedAction::Create {
                book_id,
                words_per_day,
                delivery_time,
                slug,
                skip_days,
                start,
            } => {
                let resp = client
                    .create_feed(
                        &book_id,
                        words_per_day,
                        delivery_time,
                        slug,
                        skip_days,
                        start,
                    )
                    .await?;
                print_output(json_mode, &resp, || output::print_feed_created(&resp));
            }
            FeedAction::Pause { feed_id } => {
                let resp = client.update_feed_status(&feed_id, "paused").await?;
                print_output(json_mode, &resp, || {
                    println!("Feed paused.");
                    output::print_feed_detail(&resp);
                });
            }
            FeedAction::Resume { feed_id } => {
                let resp = client.update_feed_status(&feed_id, "active").await?;
                print_output(json_mode, &resp, || {
                    println!("Feed resumed.");
                    output::print_feed_detail(&resp);
                });
            }
            FeedAction::Status { feed_id } => {
                let resp = client.get_feed(&feed_id).await?;
                print_output(json_mode, &resp, || output::print_feed_status(&resp));
            }
            FeedAction::Advance { feed_id, count } => {
                let resp = client.advance_feed(&feed_id, count).await?;
                print_output(json_mode, &resp, || output::print_advance_result(&resp));
            }
        },
        Commands::Edit { target } => match target {
            EditTarget::Book {
                book_id,
                title,
                author,
            } => {
                let resp = client.update_book(&book_id, title, author).await?;
                print_output(json_mode, &resp, || {
                    println!("Book updated.");
                    output::print_book_detail(&resp);
                });
            }
            EditTarget::Feed {
                feed_id,
                slug,
                words_per_day,
                delivery_time,
                skip_days,
                timezone,
                status,
            } => {
                let params = client::UpdateFeedParams {
                    status,
                    words_per_day,
                    delivery_time,
                    skip_days,
                    timezone,
                    slug,
                };
                let resp = client.update_feed(&feed_id, &params).await?;
                print_output(json_mode, &resp, || {
                    println!("Feed updated.");
                    output::print_feed_detail(&resp);
                });
            }
        },
        Commands::Resplit {
            book_id,
            target_words,
            max_words,
            min_words,
        } => {
            let resp = client
                .resplit_book(&book_id, target_words, max_words, min_words)
                .await?;
            print_output(json_mode, &resp, || output::print_resplit_result(&resp));
        }
        Commands::Debug { action } => match action {
            DebugAction::Segments { book_id } => {
                let segments = client.list_segments(&book_id).await?;
                print_output(json_mode, &segments, || {
                    output::print_segments_table(&segments);
                });
            }
            DebugAction::Releases { feed_id } => {
                let releases = client.list_releases(&feed_id).await?;
                print_output(json_mode, &releases, || {
                    output::print_releases_table(&releases);
                });
            }
            DebugAction::Preview { feed_id, limit } => {
                let items = client.preview_feed(&feed_id, Some(limit)).await?;
                print_output(json_mode, &items, || output::print_preview(&items));
            }
        },
        Commands::Read { book_id, index } => {
            let resp = client.read_segment(&book_id, index).await?;
            print_output(json_mode, &resp, || output::print_segment_content(&resp));
        }
        Commands::Remove { book_id } => {
            client.delete_book(&book_id).await?;
            println!("Book {book_id} deleted.");
        }
        Commands::Aggregate { action } => match action {
            AggregateAction::Create {
                slug,
                title,
                description,
                include_all,
                feeds,
            } => {
                let resp = client
                    .create_aggregate(&slug, &title, &description, include_all, &feeds)
                    .await?;
                print_output(json_mode, &resp, || {
                    output::print_aggregate_created(&resp);
                });
            }
            AggregateAction::List => {
                let aggs = client.list_aggregates().await?;
                print_output(json_mode, &aggs, || {
                    output::print_aggregates_table(&aggs);
                });
            }
            AggregateAction::Delete { id } => {
                client.delete_aggregate(&id).await?;
                println!("Aggregate feed {id} deleted.");
            }
        },
        Commands::History { action } => match action {
            HistoryAction::Undo => {
                let resp = client.undo().await?;
                print_output(json_mode, &resp, || output::print_undo_result(&resp));
            }
            HistoryAction::Redo => {
                let resp = client.redo().await?;
                print_output(json_mode, &resp, || output::print_redo_result(&resp));
            }
            HistoryAction::List { limit } => {
                let entries = client.list_history(Some(limit)).await?;
                print_output(json_mode, &entries, || {
                    output::print_history_table(&entries);
                });
            }
        },
    }

    Ok(())
}
