use std::string::FromUtf8Error;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

use crate::config::ParserConfig;
use crate::error::{InkDripError, Result};
use crate::model::{BookFormat, Chapter, ParsedBook};
use crate::word_count::count_words_html;

use super::BookParser;

/// Markdown file parser.
///
/// Uses pulldown-cmark event stream to detect heading boundaries for chapter
/// splitting, avoiding fragile line-based regex matching.
pub struct MarkdownParser;

/// Maximum heading level treated as a chapter boundary.
const CHAPTER_HEADING_LEVEL: HeadingLevel = HeadingLevel::H2;

impl BookParser for MarkdownParser {
    fn format(&self) -> BookFormat {
        BookFormat::Markdown
    }

    fn parse(&self, data: &[u8], _config: &ParserConfig) -> Result<ParsedBook> {
        let text = String::from_utf8(data.to_vec())
            .or_else(|_| Ok::<_, FromUtf8Error>(String::from_utf8_lossy(data).to_string()))
            .map_err(|e| InkDripError::ParseError(format!("Failed to decode markdown: {e}")))?;

        let text = text.trim();
        if text.is_empty() {
            return Err(InkDripError::ParseError("Empty markdown file".into()));
        }

        let raw_chapters = split_by_headings(text);

        let mut chapters = Vec::new();
        for (idx, (title, body)) in raw_chapters.into_iter().enumerate() {
            let content_html = render_markdown_to_html(&body);
            let word_count = count_words_html(&content_html);

            if word_count == 0 {
                continue;
            }

            chapters.push(Chapter {
                index: idx as u32,
                title,
                content_html,
                word_count,
            });
        }

        if chapters.is_empty() {
            return Err(InkDripError::ParseError(
                "No readable content found in markdown file".into(),
            ));
        }

        let title = chapters
            .first()
            .map_or_else(|| "Untitled".to_owned(), |c| c.title.clone());

        Ok(ParsedBook {
            title,
            author: "Unknown".to_owned(),
            chapters,
            images: Vec::new(),
        })
    }
}

/// Split markdown text into chapters using pulldown-cmark event stream.
///
/// Heading levels up to [`CHAPTER_HEADING_LEVEL`] (h1, h2) mark chapter
/// boundaries. Each chapter retains its original markdown source for
/// independent rendering.
fn split_by_headings(text: &str) -> Vec<(String, String)> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;

    // Collect heading boundary offsets and titles from the event stream.
    let mut boundaries: Vec<(usize, String)> = Vec::new();
    let mut in_heading = false;
    let mut heading_title = String::new();

    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) if level <= CHAPTER_HEADING_LEVEL => {
                in_heading = true;
                heading_title.clear();
                boundaries.push((range.start, String::new()));
            }
            Event::End(TagEnd::Heading(level)) if level <= CHAPTER_HEADING_LEVEL => {
                in_heading = false;
                if let Some(last) = boundaries.last_mut() {
                    heading_title.trim().clone_into(&mut last.1);
                }
                heading_title.clear();
            }
            Event::Text(t) | Event::Code(t) if in_heading => {
                heading_title.push_str(&t);
            }
            _ => {}
        }
    }

    // Build sections from the byte-offset boundaries.
    let Some(&(first_offset, _)) = boundaries.first() else {
        return vec![("Part 1".to_owned(), text.to_owned())];
    };

    let mut sections: Vec<(String, String)> = Vec::with_capacity(boundaries.len() + 1);

    // Content before the first heading (if any) becomes a preamble section.
    if first_offset > 0 {
        let preamble = text[..first_offset].trim();
        if !preamble.is_empty() {
            sections.push((format!("Part {}", sections.len() + 1), preamble.to_owned()));
        }
    }

    for (i, (start, title)) in boundaries.iter().enumerate() {
        let end = boundaries
            .get(i + 1)
            .map_or(text.len(), |(next_start, _)| *next_start);
        let body = text[*start..end].trim_end();

        let display_title = if title.is_empty() {
            format!("Part {}", sections.len() + 1)
        } else {
            title.clone()
        };

        sections.push((display_title, body.to_owned()));
    }

    if sections.is_empty() {
        sections.push(("Part 1".to_owned(), text.to_owned()));
    }

    sections
}

/// Render markdown text to HTML using pulldown-cmark.
#[expect(
    clippy::integer_division,
    reason = "capacity estimate, precision not needed"
)]
fn render_markdown_to_html(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;

    let parser = Parser::new_ext(markdown, options);
    let mut html = String::with_capacity(markdown.len() + markdown.len() / 4_usize);
    html::push_html(&mut html, parser);
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ParserConfig;

    #[test]
    fn split_by_headings_basic() {
        let md = "# Chapter 1\n\nContent 1.\n\n# Chapter 2\n\nContent 2.\n";
        let sections = split_by_headings(md);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, "Chapter 1");
        assert_eq!(sections[1].0, "Chapter 2");
    }

    #[test]
    fn split_by_headings_no_headings() {
        let md = "Just some text without any headings.\n\nAnother paragraph.";
        let sections = split_by_headings(md);
        assert_eq!(sections.len(), 1);
    }

    #[test]
    fn heading_in_code_block_ignored() {
        let md = "# Real Chapter\n\nSome text.\n\n```\n# Not a heading\n```\n";
        let sections = split_by_headings(md);
        assert_eq!(sections.len(), 1, "code-block heading should not split");
        assert_eq!(sections[0].0, "Real Chapter");
    }

    #[test]
    fn preamble_before_first_heading() {
        let md = "Some preamble text.\n\n# Chapter 1\n\nContent.";
        let sections = split_by_headings(md);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, "Part 1"); // preamble
        assert_eq!(sections[1].0, "Chapter 1");
    }

    #[test]
    fn render_markdown_to_html_basic() {
        let md = "Hello **world**";
        let html = render_markdown_to_html(md);
        assert!(html.contains("<strong>world</strong>"));
    }

    #[test]
    fn parse_markdown_basic() {
        let parser = MarkdownParser;
        let config = ParserConfig::default();
        let md = b"# My Book\n\nFirst chapter content.\n\n## Chapter 2\n\nSecond chapter content.";
        let result = parser.parse(md, &config).unwrap();
        assert_eq!(result.title, "My Book");
        assert!(result.chapters.len() >= 2);
    }
}
