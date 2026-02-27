use std::mem;

use crate::error::Result;
use crate::model::{Chapter, Segment};
use crate::util::html_escape;
use crate::word_count::{count_words, count_words_html, strip_html};

use super::{SplitConfig, TextSplitter};

/// Semantic text splitter that respects paragraph and sentence boundaries.
///
/// Strategy:
/// 1. Short chapters (≤ `max_words`) → single segment
/// 2. Long chapters → split at paragraph boundaries (<p> tags)
/// 3. Oversized paragraphs → split at sentence boundaries
/// 4. Oversized sentences → hard split at word boundary (last resort)
pub struct SemanticSplitter;

impl TextSplitter for SemanticSplitter {
    fn split(
        &self,
        book_id: &str,
        chapters: &[Chapter],
        config: &SplitConfig,
    ) -> Result<Vec<Segment>> {
        let mut segments = Vec::with_capacity(chapters.len());
        let mut global_index: u32 = 0;
        let mut cumulative_words: u32 = 0;

        for chapter in chapters {
            let chapter_segments = split_chapter(chapter, config);
            let total_parts = chapter_segments.len();

            for (part_idx, (content_html, word_count)) in chapter_segments.into_iter().enumerate() {
                cumulative_words += word_count;

                let title_context = if total_parts == 1 {
                    chapter.title.clone()
                } else {
                    format!("{} ({}/{total_parts})", chapter.title, part_idx + 1)
                };

                segments.push(Segment::new(
                    book_id.to_owned(),
                    global_index,
                    title_context,
                    content_html,
                    word_count,
                    cumulative_words,
                ));

                global_index += 1;
            }
        }

        // Merge trailing tiny segments into the previous one
        merge_tiny_trailing(&mut segments, config.min_words);

        Ok(segments)
    }
}

/// Split a single chapter into segment-sized chunks.
/// Returns Vec of `(content_html, word_count)`.
fn split_chapter(chapter: &Chapter, config: &SplitConfig) -> Vec<(String, u32)> {
    // Short chapter: return as-is
    if chapter.word_count <= config.max_words {
        return vec![(chapter.content_html.clone(), chapter.word_count)];
    }

    // Split into paragraphs
    let paragraphs = split_html_paragraphs(&chapter.content_html);

    let mut segments: Vec<(String, u32)> = Vec::with_capacity(paragraphs.len());
    let mut current_html = String::new();
    let mut current_words: u32 = 0;

    for para in &paragraphs {
        let para_words = count_words_html(para);

        // If a single paragraph exceeds `max_words`, split it at sentence boundaries
        if para_words > config.max_words {
            // Flush current buffer
            if current_words > 0 {
                segments.push((mem::take(&mut current_html), current_words));
                current_words = 0;
            }
            // Split the oversized paragraph
            let sub_parts = split_paragraph_by_sentences(para, config);
            segments.extend(sub_parts);
            continue;
        }

        // Would adding this paragraph exceed target? If so, flush.
        if current_words > 0 && current_words + para_words > config.target_words {
            segments.push((mem::take(&mut current_html), current_words));
            current_words = 0;
        }

        current_html.push_str(para);
        current_html.push('\n');
        current_words += para_words;
    }

    // Flush remaining
    if current_words > 0 {
        // If the remainder is too small and we have a previous segment, merge
        if current_words < config.min_words {
            if let Some((prev_html, prev_words)) = segments.last_mut() {
                prev_html.push_str(&current_html);
                *prev_words += current_words;
            } else {
                segments.push((current_html, current_words));
            }
        } else {
            segments.push((current_html, current_words));
        }
    }

    if segments.is_empty() {
        vec![(chapter.content_html.clone(), chapter.word_count)]
    } else {
        segments
    }
}

/// Split HTML content into individual paragraph blocks.
/// Handles <p>...</p>, <h1>-<h6>, <blockquote>, etc.
fn split_html_paragraphs(html: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;

    // Simple state machine to split at top-level block elements
    let block_tags = [
        "p",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "blockquote",
        "pre",
        "ul",
        "ol",
        "li",
        "div",
        "figure",
        "hr",
        "table",
    ];

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let Some(&lch) = lower_chars.get(i) else {
            break;
        };
        let Some(&ch) = chars.get(i) else { break };

        if lch == '<' {
            // Check if it's a block-level opening or closing tag
            let is_closing = lower_chars.get(i + 1).copied() == Some('/');
            let tag_start = if is_closing { i + 2 } else { i + 1 };

            let mut tag_name = String::new();
            let mut j = tag_start;
            while let Some(&jch) = lower_chars.get(j)
                && jch.is_alphanumeric()
            {
                tag_name.push(jch);
                j += 1;
            }

            let is_block = block_tags.contains(&tag_name.as_str());

            if is_block && !is_closing && depth == 0 {
                // Start of a new block element at top level
                let trimmed = current.trim().to_owned();
                if !trimmed.is_empty() {
                    paragraphs.push(trimmed);
                }
                current.clear();
            }

            if is_block {
                if is_closing {
                    depth -= 1;
                } else {
                    depth += 1;
                }
            }

            // Add character to current
            current.push(ch);
            i += 1;

            if is_block && is_closing && depth <= 0 {
                // End of block element at top level, flush
                // Find the closing >
                while let Some(&inner_ch) = chars.get(i) {
                    current.push(inner_ch);
                    i += 1;
                    if inner_ch == '>' {
                        break;
                    }
                }
                let trimmed = current.trim().to_owned();
                if !trimmed.is_empty() {
                    paragraphs.push(trimmed);
                }
                current.clear();
                depth = 0;
            }
        } else {
            current.push(ch);
            i += 1;
        }
    }

    // Flush remaining
    let trimmed = current.trim().to_owned();
    if !trimmed.is_empty() {
        paragraphs.push(trimmed);
    }

    // If we couldn't split at all, fall back to splitting by newlines
    if paragraphs.len() <= 1 && html.len() > 100 {
        let lines: Vec<String> = html
            .split('\n')
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() > 1 {
            return lines;
        }
    }

    paragraphs
}

/// Split an oversized paragraph at sentence boundaries.
fn split_paragraph_by_sentences(html: &str, config: &SplitConfig) -> Vec<(String, u32)> {
    let plain = strip_html(html);
    let sentences = split_sentences(&plain);

    let mut segments: Vec<(String, u32)> = Vec::new();
    let mut current_text = String::new();
    let mut current_words: u32 = 0;

    for sentence in &sentences {
        let s_words = count_words(sentence);

        // If adding this sentence would exceed target, flush current buffer
        if current_words > 0 && current_words + s_words > config.target_words {
            segments.push((
                format!("<p>{}</p>", html_escape(&current_text)),
                current_words,
            ));
            current_text.clear();
            current_words = 0;
        }

        if !current_text.is_empty() {
            current_text.push(' ');
        }
        current_text.push_str(sentence);
        current_words += s_words;
    }

    if current_words > 0 {
        segments.push((
            format!("<p>{}</p>", html_escape(&current_text)),
            current_words,
        ));
    }

    if segments.is_empty() {
        vec![(html.to_owned(), count_words_html(html))]
    } else {
        segments
    }
}

/// Split text into sentences using punctuation-based heuristics.
/// Handles both CJK sentence-ending punctuation and Latin period/question/exclamation marks.
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);

        // Sentence-ending punctuation
        let is_sentence_end = matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '；' | '…');

        if is_sentence_end && !current.trim().is_empty() {
            sentences.push(mem::take(&mut current).trim().to_owned());
        }
    }

    // Remaining text
    let remaining = current.trim().to_owned();
    if !remaining.is_empty() {
        sentences.push(remaining);
    }

    sentences
}

/// Merge tiny trailing segments (below `min_words`) into the previous segment.
fn merge_tiny_trailing(segments: &mut Vec<Segment>, min_words: u32) {
    loop {
        if segments.len() < 2 {
            return;
        }
        let is_tiny = segments.last().is_some_and(|s| s.word_count < min_words);
        if !is_tiny {
            return;
        }
        let Some(last) = segments.pop() else { break };
        if let Some(prev) = segments.last_mut() {
            prev.content_html.push_str(&last.content_html);
            prev.word_count += last.word_count;
            prev.cumulative_words = last.cumulative_words;
        }
    }

    // Re-index after merging
    for (idx, seg) in segments.iter_mut().enumerate() {
        seg.index = idx as u32;
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;
    use crate::model::Chapter;

    fn make_chapter(title: &str, html: &str) -> Chapter {
        Chapter {
            index: 0,
            title: title.to_owned(),
            content_html: html.to_owned(),
            word_count: count_words_html(html),
        }
    }

    #[test]
    fn short_chapter_no_split() {
        let config = SplitConfig {
            target_words: 1500,
            max_words: 2000,
            min_words: 500,
        };
        let chapter = make_chapter("Ch1", "<p>Hello world.</p>");
        let parts = split_chapter(&chapter, &config);
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn long_chapter_splits() {
        let config = SplitConfig {
            target_words: 10,
            max_words: 15,
            min_words: 3,
        };
        // Create a chapter with many paragraphs
        let mut html = String::new();
        for i in 0..20 {
            writeln!(html, "<p>This is paragraph number {i}.</p>").unwrap();
        }
        let chapter = make_chapter("Long Chapter", &html);
        let parts = split_chapter(&chapter, &config);
        assert!(
            parts.len() > 1,
            "Expected multiple parts, got {}",
            parts.len()
        );
    }

    #[test]
    fn split_sentences_works() {
        let text = "Hello world. This is a test. Another sentence!";
        let sentences = split_sentences(text);
        assert_eq!(sentences.len(), 3);
    }

    #[test]
    fn split_sentences_chinese() {
        let text = "你好世界。这是测试。另一个句子！";
        let sentences = split_sentences(text);
        assert_eq!(sentences.len(), 3);
    }

    #[test]
    fn semantic_splitter() {
        let config = SplitConfig {
            target_words: 10,
            max_words: 15,
            min_words: 3,
        };
        let splitter = SemanticSplitter;
        let chapters = vec![
            make_chapter("Ch1", "<p>Short chapter.</p>"),
            make_chapter(
                "Ch2",
                "<p>Para one with some words.</p><p>Para two with some more words.</p><p>Para three even more.</p>",
            ),
        ];
        let segments = splitter.split("test-book", &chapters, &config).unwrap();
        assert!(!segments.is_empty());
        // Verify cumulative words are non-decreasing
        let mut prev = 0u32;
        for seg in &segments {
            assert!(seg.cumulative_words >= prev);
            prev = seg.cumulative_words;
        }
    }
}
