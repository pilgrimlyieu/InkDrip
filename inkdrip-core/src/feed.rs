use std::convert::Infallible;
use std::fmt::Write as _;
use std::str::FromStr;

use atom_syndication::{Content, Entry, Feed as AtomFeed, Link, Person, Text};
use chrono::Utc;
use rss::{Channel, Guid, Item};

use crate::model::{AggregateFeed, Book, Feed, Segment, SegmentRelease};
use crate::util::xml_escape;

/// Feed output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedFormat {
    Atom,
    Rss,
}

impl FeedFormat {
    #[must_use]
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Atom => "application/atom+xml; charset=utf-8",
            Self::Rss => "application/rss+xml; charset=utf-8",
        }
    }
}

impl FromStr for FeedFormat {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "rss" => Self::Rss,
            _ => Self::Atom,
        })
    }
}

/// Generate an Atom feed XML string for the given released segments.
#[must_use]
pub fn generate_atom_feed(
    book: &Book,
    feed: &Feed,
    segments: &[(Segment, SegmentRelease)],
    base_url: &str,
) -> String {
    let feed_url = format!("{base_url}/feeds/{}/atom.xml", feed.slug);

    let entries: Vec<Entry> = segments
        .iter()
        .map(|(segment, release)| {
            let mut entry = Entry::default();
            entry.set_id(format!("{feed_url}/segments/{}", segment.id));
            entry.set_title(Text::plain(&segment.title_context));
            entry.set_updated(release.release_at);
            entry.set_published(Some(release.release_at));

            let mut content = Content::default();
            content.set_content_type(Some("html".to_owned()));
            content.set_value(Some(segment.content_html.clone()));
            entry.set_content(Some(content));

            let link = Link {
                href: format!("{feed_url}/segments/{}", segment.id),
                rel: "alternate".to_owned(),
                ..Default::default()
            };
            entry.set_links(vec![link]);

            entry
        })
        .collect();

    let mut atom_feed = AtomFeed::default();
    atom_feed.set_id(&feed_url);
    atom_feed.set_title(Text::plain(format!("{} — InkDrip", book.title)));

    if !book.author.is_empty() && book.author != "Unknown" {
        atom_feed.set_authors(vec![Person {
            name: book.author.clone(),
            ..Default::default()
        }]);
    }

    let self_link = Link {
        href: feed_url.clone(),
        rel: "self".to_owned(),
        mime_type: Some("application/atom+xml".to_owned()),
        ..Default::default()
    };
    atom_feed.set_links(vec![self_link]);

    atom_feed.set_updated(
        segments
            .last()
            .map_or_else(Utc::now, |(_, r)| r.release_at.with_timezone(&Utc)),
    );

    atom_feed.set_entries(entries);
    atom_feed.to_string()
}

/// Generate an RSS 2.0 feed XML string for the given released segments.
#[must_use]
pub fn generate_rss_feed(
    book: &Book,
    feed: &Feed,
    segments: &[(Segment, SegmentRelease)],
    base_url: &str,
) -> String {
    let feed_url = format!("{base_url}/feeds/{}/rss.xml", feed.slug);

    let items: Vec<Item> = segments
        .iter()
        .map(|(segment, release)| {
            let mut item = Item::default();
            item.set_title(Some(segment.title_context.clone()));
            item.set_description(Some(segment.content_html.clone()));
            item.set_pub_date(Some(release.release_at.to_rfc2822()));
            item.set_link(Some(format!("{feed_url}/segments/{}", segment.id)));

            let mut guid = Guid::default();
            guid.set_value(format!("{feed_url}/segments/{}", segment.id));
            guid.set_permalink(false);
            item.set_guid(Some(guid));

            item
        })
        .collect();

    let mut channel = Channel::default();
    channel.set_title(format!("{} — InkDrip", book.title));
    channel.set_link(&feed_url);
    channel.set_description(format!(
        "Drip-feed reading of {} by {}",
        book.title, book.author
    ));
    channel.set_items(items);

    channel.to_string()
}

/// Generate an OPML document listing all feeds.
#[must_use]
pub fn generate_opml(feeds: &[(Feed, Book)], base_url: &str) -> String {
    let mut outlines = String::new();
    for (feed, book) in feeds {
        let feed_url = format!("{base_url}/feeds/{}/atom.xml", feed.slug);
        let _ = write!(
            outlines,
            r#"      <outline type="rss" text="{}" title="{}" xmlUrl="{}" />{newline}"#,
            xml_escape(&book.title),
            xml_escape(&book.title),
            xml_escape(&feed_url),
            newline = '\n',
        );
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head>
    <title>InkDrip Feeds</title>
  </head>
  <body>
    <outline text="InkDrip" title="InkDrip">
{outlines}    </outline>
  </body>
</opml>"#
    )
}

/// Generate an Atom feed XML for an aggregate feed.
#[must_use]
pub fn generate_aggregate_atom(
    agg: &AggregateFeed,
    segments: &[(Segment, SegmentRelease, Book)],
    base_url: &str,
) -> String {
    let feed_url = format!("{base_url}/aggregates/{}/atom.xml", agg.slug);

    let entries: Vec<Entry> = segments
        .iter()
        .map(|(segment, release, book)| {
            let mut entry = Entry::default();
            entry.set_id(format!("{}:{}:{}", agg.slug, book.id, segment.id));
            entry.set_title(Text::plain(format!(
                "{} — {}",
                book.title, segment.title_context
            )));
            entry.set_updated(release.release_at);
            entry.set_published(Some(release.release_at));

            let mut content = Content::default();
            content.set_content_type(Some("html".to_owned()));
            content.set_value(Some(segment.content_html.clone()));
            entry.set_content(Some(content));

            entry
        })
        .collect();

    let mut atom_feed = AtomFeed::default();
    atom_feed.set_id(&feed_url);
    atom_feed.set_title(Text::plain(format!("{} — InkDrip", agg.title)));

    let self_link = Link {
        href: feed_url,
        rel: "self".to_owned(),
        mime_type: Some("application/atom+xml".to_owned()),
        ..Default::default()
    };
    atom_feed.set_links(vec![self_link]);

    atom_feed.set_updated(
        segments
            .last()
            .map_or_else(Utc::now, |(_, r, _)| r.release_at.with_timezone(&Utc)),
    );

    atom_feed.set_entries(entries);
    atom_feed.to_string()
}

/// Generate an RSS 2.0 feed XML for an aggregate feed.
#[must_use]
pub fn generate_aggregate_rss(
    agg: &AggregateFeed,
    segments: &[(Segment, SegmentRelease, Book)],
    base_url: &str,
) -> String {
    let feed_url = format!("{base_url}/aggregates/{}/rss.xml", agg.slug);

    let items: Vec<Item> = segments
        .iter()
        .map(|(segment, release, book)| {
            let mut item = Item::default();
            item.set_title(Some(format!("{} — {}", book.title, segment.title_context)));
            item.set_description(Some(segment.content_html.clone()));
            item.set_pub_date(Some(release.release_at.to_rfc2822()));

            let mut guid = Guid::default();
            guid.set_value(format!("{}:{}:{}", agg.slug, book.id, segment.id));
            guid.set_permalink(false);
            item.set_guid(Some(guid));

            item
        })
        .collect();

    let mut channel = Channel::default();
    channel.set_title(format!("{} — InkDrip", agg.title));
    channel.set_link(&feed_url);
    channel.set_description(if agg.description.is_empty() {
        format!("Aggregate feed: {}", agg.title)
    } else {
        agg.description.clone()
    });
    channel.set_items(items);

    channel.to_string()
}
