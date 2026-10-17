//! ANSI-aware text helpers for the pager: tokenising styled lines, display
//! width, clipping to the screen width and stripping escape sequences.

use unicode_width::UnicodeWidthChar;

/// Tab stops used when expanding tabs for display.
pub const TAB_WIDTH: usize = 8;

/// One piece of a styled line: an escape sequence or a single character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token<'a> {
    Esc(&'a str),
    Char(char),
}

/// Iterator over the [`Token`]s of a string.
pub struct Tokens<'a> {
    s: &'a str,
    pos: usize,
}

/// Split `s` into escape sequences and characters.
pub fn tokens(s: &str) -> Tokens<'_> {
    Tokens { s, pos: 0 }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let rest = &self.s[self.pos..];
        let c = rest.chars().next()?;
        if c != '\x1b' {
            self.pos += c.len_utf8();
            return Some(Token::Char(c));
        }
        let len = escape_len(rest);
        self.pos += len;
        Some(Token::Esc(&rest[..len]))
    }
}

/// Byte length of the escape sequence at the start of `s` (which begins
/// with ESC). Handles CSI, OSC/DCS/APC/PM/SOS strings and two-byte escapes;
/// an unterminated sequence extends to the end of the string.
fn escape_len(s: &str) -> usize {
    let b = s.as_bytes();
    match b.get(1) {
        None => 1,
        Some(b'[') => {
            for (i, &c) in b.iter().enumerate().skip(2) {
                if (0x40..=0x7e).contains(&c) {
                    return i + 1;
                }
                if !(0x20..=0x3f).contains(&c) {
                    // Malformed: stop before the offending byte.
                    return i;
                }
            }
            b.len()
        }
        Some(b']' | b'P' | b'X' | b'^' | b'_') => {
            let mut i = 2;
            while i < b.len() {
                if b[i] == 0x07 {
                    return i + 1;
                }
                if b[i] == 0x1b && b.get(i + 1) == Some(&b'\\') {
                    return i + 2;
                }
                i += 1;
            }
            b.len()
        }
        Some(_) => 1 + s[1..].chars().next().map_or(0, char::len_utf8),
    }
}

/// Display width of `c` when printed at column `col`.
pub fn char_width(c: char, col: usize) -> usize {
    if c == '\t' {
        TAB_WIDTH - col % TAB_WIDTH
    } else {
        c.width().unwrap_or(0)
    }
}

/// Display width of a (possibly styled) string.
pub fn display_width(s: &str) -> usize {
    tokens(s).fold(0, |col, t| match t {
        Token::Char(c) => col + char_width(c, col),
        Token::Esc(_) => col,
    })
}

/// Clip a styled line to at most `width` display columns. Escape sequences
/// are kept intact (never cut), tabs are expanded to spaces, and a wide
/// character that would straddle the edge is dropped. The caller is
/// responsible for emitting a reset afterwards.
pub fn clip(s: &str, width: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut col = 0;
    for t in tokens(s) {
        match t {
            Token::Esc(e) => out.push_str(e),
            Token::Char(c) => {
                let w = char_width(c, col);
                if col + w > width {
                    if c == '\t' {
                        // Fill up to the edge with the part of the tab that fits.
                        out.extend(std::iter::repeat_n(' ', width - col));
                    }
                    break;
                }
                if c == '\t' {
                    out.extend(std::iter::repeat_n(' ', w));
                } else {
                    out.push(c);
                }
                col += w;
            }
        }
    }
    out
}

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
    fn tokenises_csi_and_chars() {
        let t: Vec<_> = tokens("\x1b[1mhi\x1b[0m").collect();
        assert_eq!(
            t,
            vec![
                Token::Esc("\x1b[1m"),
                Token::Char('h'),
                Token::Char('i'),
                Token::Esc("\x1b[0m"),
            ]
        );
    }

    #[test]
    fn tokenises_osc_with_bel_and_st() {
        let t: Vec<_> = tokens("\x1b]8;;http://x\x07a\x1b]8;;\x1b\\").collect();
        assert_eq!(
            t,
            vec![
                Token::Esc("\x1b]8;;http://x\x07"),
                Token::Char('a'),
                Token::Esc("\x1b]8;;\x1b\\"),
            ]
        );
    }

    #[test]
    fn tokenises_unterminated_and_lone_escape() {
        assert_eq!(
            tokens("\x1b[12").collect::<Vec<_>>(),
            [Token::Esc("\x1b[12")]
        );
        assert_eq!(tokens("\x1b").collect::<Vec<_>>(), [Token::Esc("\x1b")]);
        assert_eq!(
            tokens("\x1b(Bx").collect::<Vec<_>>(),
            [Token::Esc("\x1b("), Token::Char('B'), Token::Char('x')]
        );
    }

    #[test]
    fn malformed_csi_does_not_swallow_text() {
        let t: Vec<_> = tokens("\x1b[1é").collect();
        assert_eq!(t, vec![Token::Esc("\x1b[1"), Token::Char('é')]);
    }

    #[test]
    fn width_counts_wide_and_zero_width() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("\x1b[1m日本\x1b[0m"), 4);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(display_width("a\tb"), 9);
    }

    #[test]
    fn clip_short_line_unchanged() {
        assert_eq!(clip("\x1b[1mhi\x1b[0m", 10), "\x1b[1mhi\x1b[0m");
    }

    #[test]
    fn clip_never_cuts_escape() {
        let s = "\x1b[1mhello\x1b[0m world";
        let c = clip(s, 3);
        assert_eq!(c, "\x1b[1mhel");
        assert_eq!(display_width(&c), 3);
    }

    #[test]
    fn clip_drops_straddling_wide_char() {
        assert_eq!(clip("a日本", 2), "a");
        assert_eq!(clip("a日本", 3), "a日");
    }

    #[test]
    fn clip_expands_tabs() {
        assert_eq!(clip("a\tb", 20), "a       b");
        assert_eq!(clip("a\tb", 4), "a   ");
    }

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
