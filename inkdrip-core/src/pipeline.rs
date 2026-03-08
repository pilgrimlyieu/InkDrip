use std::fmt::Write as _;
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

/// Add navigation links between segments.
pub struct NavigationTransform;

impl ContentTransform for NavigationTransform {
    fn name(&self) -> &'static str {
        "navigation"
    }

    fn transform(&self, segment: &mut Segment, ctx: &TransformContext) -> Result<()> {
        let mut nav = String::new();
        nav.push_str(r#"<p style="text-align:center;font-size:0.85em;margin-top:1em">"#);

        if segment.index > 0 {
            let prev_url = format!(
                "{}/feeds/{}/atom.xml#segment-{}",
                ctx.base_url,
                ctx.feed_slug,
                segment.index - 1
            );
            write!(nav, r#"<a href="{prev_url}">← prev</a>"#).unwrap_or_default();
        }

        if segment.index > 0 && segment.index + 1 < ctx.total_segments {
            nav.push_str(" | ");
        }

        if segment.index + 1 < ctx.total_segments {
            let next_url = format!(
                "{}/feeds/{}/atom.xml#segment-{}",
                ctx.base_url,
                ctx.feed_slug,
                segment.index + 1
            );
            write!(nav, r#"<a href="{next_url}">next →</a>"#).unwrap_or_default();
        }

        nav.push_str("</p>");
        segment.content_html.push_str(&nav);
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

    // Navigation is always enabled (lightweight)
    transforms.push(Box::new(NavigationTransform));

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
