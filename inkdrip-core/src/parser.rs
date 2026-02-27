pub mod epub;
pub mod markdown;
pub mod txt;

use crate::config::ParserConfig;
use crate::error::{InkDripError, Result};
use crate::model::{BookFormat, ParsedBook};

/// Trait for book format parsers.
///
/// Each parser takes raw file bytes and produces a structured `ParsedBook`.
pub trait BookParser: Send + Sync {
    /// Parse a book from raw bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is malformed or the format is unsupported.
    fn parse(&self, data: &[u8], config: &ParserConfig) -> Result<ParsedBook>;

    /// The format this parser handles.
    fn format(&self) -> BookFormat;
}

/// Select the appropriate parser for a given format.
#[must_use]
pub fn parser_for_format(format: BookFormat) -> Box<dyn BookParser> {
    match format {
        BookFormat::Epub => Box::new(epub::EpubParser),
        BookFormat::Txt => Box::new(txt::TxtParser),
        BookFormat::Markdown => Box::new(markdown::MarkdownParser),
    }
}

/// Detect book format from file extension and parse the content.
///
/// # Errors
///
/// Returns an error if the file has no extension, the format is unsupported,
/// or parsing fails.
pub fn parse_book(data: &[u8], filename: &str, config: &ParserConfig) -> Result<ParsedBook> {
    let ext = filename
        .rsplit('.')
        .next()
        .ok_or_else(|| InkDripError::UnsupportedFormat("no file extension".into()))?;

    let format = BookFormat::from_extension(ext)
        .ok_or_else(|| InkDripError::UnsupportedFormat(ext.to_owned()))?;

    let parser = parser_for_format(format);
    parser.parse(data, config)
}
