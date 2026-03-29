use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::{HookEntryConfig, HooksConfig, TransformsConfig};
use crate::error::Result;
use crate::hooks;
use crate::model::Segment;

/// Context passed to each transform step.
pub struct TransformContext {
    /// Total number of segments in the book.
    pub total_segments: u32,
    /// Total word count for the book.
    pub total_words: u32,
    /// Base URL for generating links.
    pub base_url: String,
    /// Feed slug for generating links.
    pub feed_slug: String,
    /// Book ID for constructing image URLs.
    pub book_id: String,
}

/// A single content transformation step.
pub trait ContentTransform: Send + Sync {
    /// Apply the transform to a segment's `content_html` (in-place).
    ///
    /// # Errors
    ///
    /// Returns an error if the transform cannot be applied.
    fn transform(&self, segment: &mut Segment, ctx: &TransformContext) -> Result<()>;

    /// Human-readable name for logging.
    fn name(&self) -> &str;
}

/// Reading time and word count statistics appended to each segment.
///
/// Outputs something like: `≈ 5 min · ~1,234 words`
pub struct ReadingTimeTransform {
    /// Reading speed in words per minute.
    pub words_per_minute: u32,
}

impl ContentTransform for ReadingTimeTransform {
    fn name(&self) -> &'static str {
        "reading_time"
    }

    fn transform(&self, segment: &mut Segment, _ctx: &TransformContext) -> Result<()> {
        let minutes = estimate_reading_minutes(segment.word_count, self.words_per_minute);
        let stats = format!(
            r#"<p style="text-align:center;color:#888;font-size:0.85em;margin-top:1em">≈ {} min · ~{} words</p>"#,
            minutes,
            format_number(segment.word_count),
        );

        segment.content_html.push_str(&stats);
        Ok(())
    }
}

/// Estimate reading time in minutes, with a minimum of 1 minute.
fn estimate_reading_minutes(word_count: u32, words_per_minute: u32) -> u32 {
    if words_per_minute == 0 {
        return 1;
    }
    word_count.div_ceil(words_per_minute).max(1)
}

/// Format a number with thousand separators (e.g., 12345 -> "12,345").
#[expect(
    clippy::integer_division,
    reason = "estimating capacity, precision loss acceptable"
)]
fn format_number(n: u32) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(c);
    }
    result
}

/// Reading progress indicator appended to each segment.
///
/// Outputs something like: `[42% · 12/28]`
pub struct ReadingProgressTransform;

impl ContentTransform for ReadingProgressTransform {
    fn name(&self) -> &'static str {
        "reading_progress"
    }

    fn transform(&self, segment: &mut Segment, ctx: &TransformContext) -> Result<()> {
        let progress_pct = if ctx.total_words > 0 {
            (f64::from(segment.cumulative_words) / f64::from(ctx.total_words) * 100.0) as u32
        } else {
            0
        };

        let indicator = format!(
            r#"<p style="text-align:center;color:#888;font-size:0.85em;margin-top:1.5em;border-top:1px solid #eee;padding-top:0.5em">[{progress_pct}% · {}/{}]</p>"#,
            segment.index + 1,
            ctx.total_segments,
        );

        segment.content_html.push_str(&indicator);
        Ok(())
    }
}

/// Inject custom CSS into the segment's HTML content.
pub struct StyleTransform {
    pub css: String,
}

impl ContentTransform for StyleTransform {
    fn name(&self) -> &'static str {
        "style"
    }

    fn transform(&self, segment: &mut Segment, _ctx: &TransformContext) -> Result<()> {
        if self.css.is_empty() {
            return Ok(());
        }

        let style_tag = format!("<style>{}</style>\n", self.css);
        segment.content_html.insert_str(0, &style_tag);
        Ok(())
    }
}

/// Rewrite `<img src="...">` attributes to point to the server image endpoint.
///
/// EPUB images use internal paths (e.g. `../images/cover.jpg`). This transform
/// resolves them to `{base_url}/images/{book_id}/{basename}` so readers can
/// fetch them from the `InkDrip` server.
/// Regex for matching `<img src="...">` attributes.
#[expect(clippy::expect_used, reason = "constant regex pattern, infallible")]
static IMG_SRC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(<img\b[^>]*\bsrc=")([^"]+)("[^>]*>)"#)
        .expect("img src regex is a valid constant pattern")
});

pub struct ImageUrlTransform;

impl ContentTransform for ImageUrlTransform {
    fn name(&self) -> &'static str {
        "image_url"
    }

    fn transform(&self, segment: &mut Segment, ctx: &TransformContext) -> Result<()> {
        let base = format!("{}/images/{}", ctx.base_url, ctx.book_id);
        segment.content_html = IMG_SRC_RE
            .replace_all(&segment.content_html, |caps: &regex::Captures| {
                let original_src = &caps[2];
                // Extract basename from possibly nested path
                let basename = original_src.rsplit('/').next().unwrap_or(original_src);
                format!("{}{base}/{basename}{}", &caps[1], &caps[3])
            })
            .into_owned();
        Ok(())
    }
}

// ─── Hook-based external command transform ──────────────────────

/// JSON payload sent to the `segment_transform` hook on stdin.
#[derive(Serialize)]
struct SegmentTransformInput<'a> {
    hook: &'static str,
    segment_index: u32,
    title_context: &'a str,
    content_html: &'a str,
    word_count: u32,
    cumulative_words: u32,
    feed_slug: &'a str,
    base_url: &'a str,
    book_id: &'a str,
}

/// Expected JSON output from the `segment_transform` hook.
#[derive(Deserialize)]
struct SegmentTransformOutput {
    /// Replacement HTML.  If absent the original is kept.
    content_html: Option<String>,
}

/// Runs an external command for each segment at serve time.
pub struct ExternalCommandTransform {
    entry: HookEntryConfig,
    timeout_secs: u64,
}

impl ContentTransform for ExternalCommandTransform {
    fn name(&self) -> &'static str {
        "external_command"
    }

    fn transform(&self, segment: &mut Segment, ctx: &TransformContext) -> Result<()> {
        let input = SegmentTransformInput {
            hook: "segment_transform",
            segment_index: segment.index,
            title_context: &segment.title_context,
            content_html: &segment.content_html,
            word_count: segment.word_count,
            cumulative_words: segment.cumulative_words,
            feed_slug: &ctx.feed_slug,
            base_url: &ctx.base_url,
            book_id: &ctx.book_id,
        };

        if let Some(output) = hooks::run_hook::<_, SegmentTransformOutput>(
            "segment_transform",
            &self.entry,
            &input,
            self.timeout_secs,
        )? && let Some(html) = output.content_html
        {
            segment.content_html = html;
        }
        Ok(())
    }
}

/// Build the transform pipeline from configuration.
#[must_use]
pub fn build_pipeline(
    config: &TransformsConfig,
    hooks: &HooksConfig,
) -> Vec<Box<dyn ContentTransform>> {
    let mut transforms: Vec<Box<dyn ContentTransform>> = Vec::with_capacity(5);

    // Image URL rewrite must come first (before any content wrapping)
    transforms.push(Box::new(ImageUrlTransform));

    if !config.custom_css.is_empty() {
        transforms.push(Box::new(StyleTransform {
            css: config.custom_css.clone(),
        }));
    }

    if config.reading_time {
        transforms.push(Box::new(ReadingTimeTransform {
            words_per_minute: config.reading_speed,
        }));
    }

    if config.reading_progress {
        transforms.push(Box::new(ReadingProgressTransform));
    }

    // External command hook runs last so it can post-process all internal transforms
    if hooks.enabled && hooks.segment_transform.enabled {
        transforms.push(Box::new(ExternalCommandTransform {
            entry: hooks.segment_transform.clone(),
            timeout_secs: hooks.timeout_secs,
        }));
    }

    transforms
}

/// Apply all transforms in the pipeline to a segment.
///
/// # Errors
///
/// Returns an error if any individual transform fails.
pub fn apply_transforms(
    segment: &mut Segment,
    transforms: &[Box<dyn ContentTransform>],
    ctx: &TransformContext,
) -> Result<()> {
    for transform in transforms {
        tracing::trace!("Applying transform: {}", transform.name());
        transform.transform(segment, ctx)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_number_basic() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(1), "1");
        assert_eq!(format_number(12), "12");
        assert_eq!(format_number(123), "123");
        assert_eq!(format_number(1234), "1,234");
        assert_eq!(format_number(12345), "12,345");
        assert_eq!(format_number(123_456), "123,456");
        assert_eq!(format_number(1_234_567), "1,234,567");
        assert_eq!(format_number(42000), "42,000");
    }

    #[test]
    fn estimate_reading_minutes_basic() {
        // 300 words at 300 wpm = 1 min
        assert_eq!(estimate_reading_minutes(300, 300), 1);
        // 600 words at 300 wpm = 2 min
        assert_eq!(estimate_reading_minutes(600, 300), 2);
        // 450 words at 300 wpm = 2 min (rounds up)
        assert_eq!(estimate_reading_minutes(450, 300), 2);
        // 0 words = 1 min minimum
        assert_eq!(estimate_reading_minutes(0, 300), 1);
        // 0 wpm = 1 min (avoid division by zero)
        assert_eq!(estimate_reading_minutes(1000, 0), 1);
    }
}
