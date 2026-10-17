//! Pager-specific text helpers. Tokenising, display width, clipping and
//! stripping of styled lines live in [`crate::style`] (shared with the
//! renderer) and are re-exported here.

use unicode_width::UnicodeWidthChar;

pub use crate::style::{clip, display_width, strip_ansi, tokens, Token};

/// Number of physical terminal rows `s` occupies at `term_width` columns.
pub fn physical_rows(s: &str, term_width: usize) -> usize {
    let w = display_width(s);
    if w == 0 || term_width == 0 {
        1
    } else {
        w.div_ceil(term_width)
    }
}

/// Shorten plain text `s` to `width` columns by replacing its beginning
/// with `…`, keeping the end (e.g. the file name of a long path).
pub fn elide_left(s: &str, width: usize) -> String {
    if display_width(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 1; // the ellipsis
    for c in s.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        tail.push(c);
    }
    let mut out = String::from("…");
    out.extend(tail.iter().rev());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_rows_counts_wrapping() {
        assert_eq!(physical_rows("", 10), 1);
        assert_eq!(physical_rows("0123456789", 10), 1);
        assert_eq!(physical_rows("0123456789a", 10), 2);
        assert_eq!(physical_rows("日本語", 4), 2);
    }

    #[test]
    fn elide_keeps_tail() {
        assert_eq!(elide_left("short.md", 20), "short.md");
        assert_eq!(elide_left("/very/long/path/file.md", 10), "…h/file.md");
        assert_eq!(
            display_width(&elide_left("/very/long/path/file.md", 10)),
            10
        );
        assert_eq!(elide_left("abc", 0), "");
    }
}
