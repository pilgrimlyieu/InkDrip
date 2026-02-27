use std::string::FromUtf8Error;

use regex::Regex;

use crate::config::ParserConfig;
use crate::error::{InkDripError, Result};
use crate::model::{BookFormat, Chapter, ParsedBook};
use crate::util::html_escape;
use crate::word_count::count_words;

use super::BookParser;

/// Plain text file parser.
///
/// Splits content into chapters using a configurable separator pattern
/// (default: lines of 3+ `=` characters). Paragraphs are separated by blank lines.
pub struct TxtParser;

impl BookParser for TxtParser {
    fn format(&self) -> BookFormat {
        BookFormat::Txt
    }

    fn parse(&self, data: &[u8], config: &ParserConfig) -> Result<ParsedBook> {
        let text = String::from_utf8(data.to_vec())
            .or_else(|_| {
                // Try common CJK encodings - fallback to lossy UTF-8
                Ok::<String, FromUtf8Error>(String::from_utf8_lossy(data).to_string())
            })
            .map_err(|e| InkDripError::ParseError(format!("Failed to decode text: {e}")))?;

        let text = text.trim();
        if text.is_empty() {
            return Err(InkDripError::ParseError("Empty text file".into()));
        }

        let chapter_sep = Regex::new(&config.txt.chapter_separator).map_err(|e| {
            InkDripError::ConfigError(format!("Invalid chapter separator regex: {e}"))
        })?;

        let raw_chapters = split_into_chapters(text, &chapter_sep);

        let mut chapters = Vec::with_capacity(raw_chapters.len());
        for (idx, raw) in raw_chapters.iter().enumerate() {
            let (title, body) = extract_chapter_title(raw, idx);
            let content_html = text_to_html(body);
            let word_count = count_words(body);

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
                "No readable content found in text file".into(),
            ));
        }

        // Try to extract a book title from the first non-empty line
        let title = text
            .lines()
            .find(|l| !l.trim().is_empty())
            .map_or_else(|| "Untitled".to_owned(), |l| l.trim().to_owned());

        Ok(ParsedBook {
            title,
            author: "Unknown".to_owned(),
            chapters,
            images: Vec::new(),
        })
    }
}

/// Split text into chapters using the separator regex.
/// If no separator is found, tries splitting by multiple blank lines (4+).
/// Treat the entire text as one chapter if no splits are found.
fn split_into_chapters<'a>(text: &'a str, separator: &Regex) -> Vec<&'a str> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chapter_starts: Vec<usize> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if separator.is_match(line.trim()) {
            chapter_starts.push(i);
        }
    }

    if chapter_starts.is_empty() {
        // No separators found: try splitting by multiple blank lines (4+).
        let parts: Vec<&str> = match Regex::new(r"\n{4,}") {
            Ok(re) => re.split(text).collect(),
            Err(_) => vec![text],
        };
        if parts.len() > 1 {
            return parts;
        }
        return vec![text];
    }

    let mut chapters = Vec::new();
    let line_offsets: Vec<usize> = {
        let mut offsets = vec![0usize];
        for line in text.lines() {
            let last = offsets.last().copied().unwrap_or(0);
            // +1 for the newline character
            offsets.push(last + line.len() + 1);
        }
        offsets
    };

    // Content before first separator
    if let Some(&first_sep) = chapter_starts.first()
        && first_sep > 0
    {
        let end = line_offsets
            .get(first_sep)
            .copied()
            .unwrap_or(text.len())
            .min(text.len());
        let chunk = &text[..end];
        if !chunk.trim().is_empty() {
            chapters.push(chunk.trim());
        }
    }

    // Content between separators
    for (i, &sep_line) in chapter_starts.iter().enumerate() {
        let start = if sep_line + 1 < line_offsets.len() {
            line_offsets
                .get(sep_line + 1)
                .copied()
                .unwrap_or(text.len())
        } else {
            text.len()
        };

        let end = if i + 1 < chapter_starts.len() {
            chapter_starts
                .get(i + 1)
                .and_then(|&next| line_offsets.get(next))
                .copied()
                .unwrap_or(text.len())
                .min(text.len())
        } else {
            text.len()
        };

        if start < end {
            let chunk = &text[start.min(text.len())..end];
            if !chunk.trim().is_empty() {
                chapters.push(chunk.trim());
            }
        }
    }

    if chapters.is_empty() {
        vec![text]
    } else {
        chapters
    }
}

/// Extract a title from the beginning of a chapter's text.
/// Returns `(title, remaining_body)`.
fn extract_chapter_title(text: &str, fallback_index: usize) -> (String, &str) {
    let first_line = text.lines().next().unwrap_or("").trim();

    // Heuristic: if the first line is short (likely a title) and followed by a blank line
    if !first_line.is_empty()
        && first_line.len() <= 80
        && let Some(rest_start) = text.find('\n')
    {
        let after_first = &text[rest_start..];
        // Check if there's a blank line after the title
        if after_first.starts_with("\n\n") || after_first.trim_start().starts_with('\n') {
            return (first_line.to_owned(), after_first.trim_start_matches('\n'));
        }
    }

    // Fallback: use "Part N" as title
    (format!("Part {}", fallback_index + 1), text)
}

/// Convert plain text to basic HTML, wrapping paragraphs in <p> tags.
#[expect(
    clippy::integer_division,
    reason = "capacity estimate, precision not needed"
)]
fn text_to_html(text: &str) -> String {
    let mut html = String::with_capacity(text.len() + text.len() / 10);
    let paragraphs: Vec<&str> = text.split("\n\n").collect();

    for para in paragraphs {
        let trimmed = para.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Escape HTML special characters
        let escaped = html_escape(trimmed);
        // Preserve single line breaks within a paragraph as <br>
        let with_breaks = escaped.replace('\n', "<br>\n");
        html.push_str("<p>");
        html.push_str(&with_breaks);
        html.push_str("</p>\n");
    }

    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ParserConfig;

    #[test]
    fn text_to_html_basic() {
        let text = "Hello world.\n\nSecond paragraph.";
        let html = text_to_html(text);
        assert!(html.contains("<p>Hello world.</p>"));
        assert!(html.contains("<p>Second paragraph.</p>"));
    }

    #[test]
    fn split_by_equals_separator() {
        let config = ParserConfig::default();
        let sep = Regex::new(&config.txt.chapter_separator).unwrap();
        let text = "Intro content\n===\nChapter 1\n\nContent of chapter 1.\n===\nChapter 2\n\nContent of chapter 2.";
        let chapters = split_into_chapters(text, &sep);
        assert_eq!(chapters.len(), 3);
    }

    #[test]
    fn parse_simple_txt() {
        let config = ParserConfig::default();
        let parser = TxtParser;
        let text = b"My Book Title\n\nSome content here.\n\nMore content.";
        let result = parser.parse(text, &config).unwrap();
        assert_eq!(result.title, "My Book Title");
        assert!(!result.chapters.is_empty());
    }
}
