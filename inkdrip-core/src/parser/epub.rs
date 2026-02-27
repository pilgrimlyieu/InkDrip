use std::io::Cursor;

use epub::doc::EpubDoc;

use crate::config::ParserConfig;
use crate::error::{InkDripError, Result};
use crate::model::{BookFormat, Chapter, ParsedBook};
use crate::word_count::{count_words_html, strip_html};

use super::BookParser;

/// EPUB format parser.
///
/// Uses the `epub` crate to navigate the EPUB spine and extract
/// chapter content as sanitized HTML.
pub struct EpubParser;

impl BookParser for EpubParser {
    fn format(&self) -> BookFormat {
        BookFormat::Epub
    }

    fn parse(&self, data: &[u8], _config: &ParserConfig) -> Result<ParsedBook> {
        let cursor = Cursor::new(data);
        let mut doc = EpubDoc::from_reader(cursor)
            .map_err(|e| InkDripError::ParseError(format!("Failed to open EPUB: {e}")))?;

        // Extract metadata
        let title = doc
            .mdata("title")
            .map_or_else(|| "Untitled".to_owned(), |m| m.value.clone());
        let author = doc
            .mdata("creator")
            .map_or_else(|| "Unknown".to_owned(), |m| m.value.clone());

        // Collect spine item IDs (reading order)
        let spine_ids: Vec<String> = doc.spine.iter().map(|s| s.idref.clone()).collect();

        let mut chapters = Vec::with_capacity(spine_ids.len());
        let mut images = Vec::new();

        // Iterate through spine items
        for (idx, spine_id) in spine_ids.iter().enumerate() {
            // Get the resource path for this spine ID, then read content
            let resource_path = match doc.resources.get(spine_id) {
                Some(res) => res.path.clone(),
                None => continue,
            };
            let Some(content) = doc.get_resource_str_by_path(&resource_path) else {
                continue;
            };

            // Sanitize HTML: keep semantic tags, strip scripts/styles
            let clean_html = sanitize_epub_html(&content);

            if clean_html.trim().is_empty() {
                continue;
            }

            let word_count = count_words_html(&clean_html);
            if word_count == 0 {
                continue;
            }

            // Try to extract a chapter title from the content
            let chapter_title = extract_title_from_html(&clean_html)
                .unwrap_or_else(|| format!("Chapter {}", idx + 1));

            chapters.push(Chapter {
                index: idx as u32,
                title: chapter_title,
                content_html: clean_html,
                word_count,
            });
        }

        // Collect image resource IDs first, then extract data
        let image_ids: Vec<(String, String)> = doc
            .resources
            .iter()
            .filter(|(_, res)| res.mime.starts_with("image/"))
            .map(|(id, res)| {
                let filename = res
                    .path
                    .file_name()
                    .map_or_else(|| id.clone(), |n| n.to_string_lossy().to_string());
                (id.clone(), filename)
            })
            .collect();

        for (id, filename) in image_ids {
            if let Some((data, _mime)) = doc.get_resource(&id) {
                images.push((filename, data));
            }
        }

        if chapters.is_empty() {
            return Err(InkDripError::ParseError(
                "No readable chapters found in EPUB".into(),
            ));
        }

        Ok(ParsedBook {
            title,
            author,
            chapters,
            images,
        })
    }
}

/// Sanitize EPUB XHTML content, keeping only semantic tags.
fn sanitize_epub_html(html: &str) -> String {
    // Extract body content if present
    let body_content = extract_body(html);

    ammonia::Builder::default()
        .tags(
            [
                "p",
                "br",
                "h1",
                "h2",
                "h3",
                "h4",
                "h5",
                "h6",
                "blockquote",
                "pre",
                "code",
                "em",
                "strong",
                "i",
                "b",
                "u",
                "s",
                "sub",
                "sup",
                "a",
                "img",
                "ul",
                "ol",
                "li",
                "dl",
                "dt",
                "dd",
                "table",
                "thead",
                "tbody",
                "tr",
                "th",
                "td",
                "figure",
                "figcaption",
                "div",
                "span",
                "hr",
                "ruby",
                "rt",
                "rp",
            ]
            .iter()
            .copied()
            .collect(),
        )
        .tag_attributes(
            [
                ("a", ["href"].iter().copied().collect()),
                ("img", ["src", "alt", "title"].iter().copied().collect()),
            ]
            .iter()
            .cloned()
            .collect(),
        )
        .clean(&body_content)
        .to_string()
}

/// Extract content within <body> tags, or return the whole string.
fn extract_body(html: &str) -> String {
    let lower = html.to_lowercase();
    if let Some(start) = lower.find("<body")
        && let Some(body_start) = html[start..].find('>')
    {
        let content_start = start + body_start + 1;
        if let Some(end) = lower[content_start..].find("</body>") {
            return html[content_start..content_start + end].to_string();
        }
        return html[content_start..].to_string();
    }
    html.to_owned()
}

/// Try to extract a heading from HTML to use as chapter title.
fn extract_title_from_html(html: &str) -> Option<String> {
    // Look for h1-h3 tags
    for tag in &["h1", "h2", "h3"] {
        let open = format!("<{tag}");
        if let Some(start) = html.to_lowercase().find(&open)
            && let Some(tag_end) = html[start..].find('>')
        {
            let content_start = start + tag_end + 1;
            let close = format!("</{tag}>");
            if let Some(end) = html[content_start..].to_lowercase().find(&close) {
                let title_html = &html[content_start..content_start + end];
                let title = strip_html(title_html).trim().to_owned();
                if !title.is_empty() {
                    return Some(title);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_body_with_tags() {
        let html = "<html><head><title>Test</title></head><body><p>Hello</p></body></html>";
        assert_eq!(extract_body(html), "<p>Hello</p>");
    }

    #[test]
    fn extract_body_no_body_tag() {
        let html = "<p>Hello</p>";
        assert_eq!(extract_body(html), "<p>Hello</p>");
    }

    #[test]
    fn extract_title() {
        let html = "<h1>Chapter 1: The Beginning</h1><p>Content here.</p>";
        assert_eq!(
            extract_title_from_html(html),
            Some("Chapter 1: The Beginning".to_owned())
        );
    }

    #[test]
    fn sanitize_removes_scripts() {
        let html = "<p>Hello</p><script>alert('xss')</script><p>World</p>";
        let clean = sanitize_epub_html(html);
        assert!(!clean.contains("script"));
        assert!(clean.contains("Hello"));
        assert!(clean.contains("World"));
    }
}
