//! Shared utility functions used across `InkDrip` crates.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

/// FNV-1a 128-bit hash for lightweight file deduplication (not cryptographic).
#[must_use]
pub fn content_hash(data: &[u8]) -> u128 {
    let mut hash: u128 = 0xcbf2_9ce4_8422_2325;
    for &byte in data {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Format a content hash as a lowercase hex string.
#[must_use]
pub fn content_hash_hex(data: &[u8]) -> String {
    format!("{:x}", content_hash(data))
}

/// Generate a URL-friendly slug from a title.
///
/// - ASCII alphanumeric and non-ASCII alphanumeric (CJK etc.) characters are kept.
/// - Spaces, hyphens, underscores become `-`.
/// - Everything else becomes `-`.
/// - Consecutive hyphens are collapsed; trailing hyphens are trimmed.
#[must_use]
pub fn generate_slug(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_alphanumeric() {
                c
            } else {
                '-'
            }
        })
        .collect();

    let mut result = String::new();
    let mut prev_hyphen = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen && !result.is_empty() {
                result.push('-');
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }

    result.trim_end_matches('-').to_owned()
}

/// Escape HTML special characters in plain text.
#[must_use]
pub fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Escape XML special characters (superset of HTML escaping, includes `'`).
#[must_use]
pub fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Generate an 8-character alphanumeric short ID.
///
/// Uses the system random number generator. Collision probability is negligible
/// for typical single-user workloads (62^8 ≈ 2.18 × 10^14 combinations).
#[must_use]
pub fn generate_short_id() -> String {
    const CHARSET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    const ID_LEN: usize = 8;

    let mut id = String::with_capacity(ID_LEN);
    // Use two RandomState hashers to get enough entropy for 8 chars
    let s1 = RandomState::new();
    let s2 = RandomState::new();
    let mut h1 = s1.build_hasher();
    let mut h2 = s2.build_hasher();
    // Mix in a timestamp for additional entropy
    h1.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    h2.write_u64(process::id().into());

    let v1 = h1.finish();
    let v2 = h2.finish();

    for i in 0..ID_LEN {
        let byte = if i < 4 {
            ((v1 >> (i * 8)) & 0xFF) as u8
        } else {
            ((v2 >> ((i - 4) * 8)) & 0xFF) as u8
        };
        let idx = (byte as usize) % CHARSET.len();
        if let Some(&ch) = CHARSET.get(idx) {
            id.push(char::from(ch));
        }
    }

    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_deterministic() {
        let data = b"hello world";
        assert_eq!(content_hash(data), content_hash(data));
    }

    #[test]
    fn content_hash_different_data() {
        assert_ne!(content_hash(b"hello"), content_hash(b"world"));
    }

    #[test]
    fn slug_basic() {
        assert_eq!(generate_slug("Hello World"), "hello-world");
        assert_eq!(generate_slug("The Great Gatsby"), "the-great-gatsby");
        assert_eq!(generate_slug("三体"), "三体");
        assert_eq!(generate_slug("Test  Book!!!"), "test-book");
    }

    #[test]
    fn html_escape_basic() {
        assert_eq!(html_escape("<p>A & B</p>"), "&lt;p&gt;A &amp; B&lt;/p&gt;");
    }

    #[test]
    fn xml_escape_includes_apos() {
        assert!(xml_escape("it's").contains("&apos;"));
    }

    #[test]
    fn short_id_length_and_uniqueness() {
        let id1 = generate_short_id();
        let id2 = generate_short_id();
        assert_eq!(id1.len(), 8);
        assert_eq!(id2.len(), 8);
        assert_ne!(id1, id2);
    }

    #[test]
    fn short_id_charset() {
        let id = generate_short_id();
        for c in id.chars() {
            assert!(c.is_ascii_alphanumeric());
        }
    }
}
