/// Count "words" in a text string.
///
/// For CJK (Chinese, Japanese, Korean) characters, each character counts as one word.
/// For Latin/alphabetic text, words are separated by whitespace.
/// This gives a reasonable approximation for mixed-language content.
#[must_use]
pub fn count_words(text: &str) -> u32 {
    let mut count: u32 = 0;
    let mut in_latin_word = false;

    for ch in text.chars() {
        if is_cjk_char(ch) {
            // Each CJK character counts as one word
            if in_latin_word {
                count += 1;
                in_latin_word = false;
            }
            count += 1;
        } else if ch.is_alphanumeric() {
            // Start or continue a Latin word
            in_latin_word = true;
        } else {
            // Whitespace or punctuation
            if in_latin_word {
                count += 1;
                in_latin_word = false;
            }
        }
    }

    // Don't forget the last Latin word
    if in_latin_word {
        count += 1;
    }

    count
}

/// Check if a character is in the CJK Unified Ideographs range.
fn is_cjk_char(ch: char) -> bool {
    matches!(ch,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{2E80}'..='\u{2EFF}' // CJK Radicals Supplement
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and Fullwidth Forms
    )
}

/// Strip HTML tags and return plain text for word counting.
#[must_use]
pub fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut last_was_space = true;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                // Add a space after tags to separate adjacent text nodes
                if !last_was_space {
                    result.push(' ');
                    last_was_space = true;
                }
            }
            _ if !in_tag => {
                result.push(ch);
                last_was_space = ch.is_whitespace();
            }
            _ => {}
        }
    }

    result
}

/// Count words in an HTML string (strips tags first).
#[must_use]
pub fn count_words_html(html: &str) -> u32 {
    count_words(&strip_html(html))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_english_words() {
        assert_eq!(count_words("Hello world"), 2);
        assert_eq!(count_words("The quick brown fox jumps"), 5);
    }

    #[test]
    fn count_chinese_chars() {
        assert_eq!(count_words("你好世界"), 4);
        assert_eq!(count_words("这是一个测试"), 6);
    }

    #[test]
    fn count_mixed() {
        // "Hello" (1 word) + "世界" (2 chars) = 3
        assert_eq!(count_words("Hello世界"), 3);
        // "Hello" (1) + " " + "世界" (2) + " " + "test" (1) = 4
        assert_eq!(count_words("Hello 世界 test"), 4);
    }

    #[test]
    fn count_empty() {
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("   "), 0);
    }

    #[test]
    fn strip_html_works() {
        assert_eq!(strip_html("<p>Hello</p>").trim(), "Hello");
        assert_eq!(count_words_html("<p>Hello <b>world</b></p>"), 2);
    }
}
