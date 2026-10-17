//! Plain-substring search over the visible (ANSI-stripped) text of lines,
//! and ANSI-aware highlighting of the matches.

use super::text::{self, Token};

/// Highlight on/off: reverse video, switched off without a full reset so
/// the line's own styling survives around the match.
const HL_ON: &str = "\x1b[7m";
const HL_OFF: &str = "\x1b[27m";

/// A compiled search pattern with smart-case matching: case-sensitive only
/// when the pattern contains an uppercase letter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    pattern: String,
    needle: Vec<char>,
    case_sensitive: bool,
}

impl Search {
    /// Compile `pattern`; `None` for an empty pattern.
    pub fn new(pattern: &str) -> Option<Self> {
        if pattern.is_empty() {
            return None;
        }
        let case_sensitive = pattern.chars().any(char::is_uppercase);
        let mut s = Search {
            pattern: pattern.to_string(),
            needle: Vec::new(),
            case_sensitive,
        };
        s.needle = pattern.chars().map(|c| s.fold(c)).collect();
        Some(s)
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    fn fold(&self, c: char) -> char {
        if self.case_sensitive {
            c
        } else {
            c.to_lowercase().next().unwrap_or(c)
        }
    }

    fn folded(&self, plain: &str) -> Vec<char> {
        plain.chars().map(|c| self.fold(c)).collect()
    }

    /// Non-overlapping matches in `plain`, as `[start, end)` char indices.
    pub fn find(&self, plain: &str) -> Vec<(usize, usize)> {
        let hay = self.folded(plain);
        let n = self.needle.len();
        let mut out = Vec::new();
        let mut i = 0;
        while i + n <= hay.len() {
            if hay[i..i + n] == self.needle[..] {
                out.push((i, i + n));
                i += n;
            } else {
                i += 1;
            }
        }
        out
    }

    /// Whether the visible text of styled `line` contains a match.
    pub fn matches_line(&self, line: &str) -> bool {
        let hay = self.folded(&text::strip_ansi(line));
        hay.windows(self.needle.len())
            .any(|w| w == &self.needle[..])
    }

    /// Wrap every match in styled `line` with reverse video. Escape sequences
    /// are never split; highlighting is re-asserted after any escape inside a
    /// match (e.g. a reset) so it covers the whole match.
    pub fn highlight(&self, line: &str) -> String {
        let ranges = self.find(&text::strip_ansi(line));
        if ranges.is_empty() {
            return line.to_string();
        }
        let mut out = String::with_capacity(line.len() + ranges.len() * 10);
        let mut idx = 0;
        let mut r = 0;
        let mut inside = false;
        for tok in text::tokens(line) {
            match tok {
                Token::Esc(e) => {
                    out.push_str(e);
                    if inside {
                        out.push_str(HL_ON);
                    }
                }
                Token::Char(c) => {
                    if !inside && r < ranges.len() && idx == ranges[r].0 {
                        out.push_str(HL_ON);
                        inside = true;
                    }
                    out.push(c);
                    idx += 1;
                    if inside && idx == ranges[r].1 {
                        out.push_str(HL_OFF);
                        inside = false;
                        r += 1;
                    }
                }
            }
        }
        if inside {
            out.push_str(HL_OFF);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pattern_is_rejected() {
        assert!(Search::new("").is_none());
    }

    #[test]
    fn smart_case() {
        let lower = Search::new("hello").unwrap();
        assert_eq!(
            lower.find("Hello HELLO hello"),
            vec![(0, 5), (6, 11), (12, 17)]
        );
        let upper = Search::new("Hello").unwrap();
        assert_eq!(upper.find("Hello HELLO hello"), vec![(0, 5)]);
    }

    #[test]
    fn matches_are_non_overlapping_char_indices() {
        let s = Search::new("aa").unwrap();
        assert_eq!(s.find("aaaa"), vec![(0, 2), (2, 4)]);
        let s = Search::new("本").unwrap();
        assert_eq!(s.find("日本語本"), vec![(1, 2), (3, 4)]);
    }

    #[test]
    fn matches_ignore_escape_codes() {
        let s = Search::new("bold text").unwrap();
        assert!(s.matches_line("some \x1b[1mbold\x1b[0m text"));
        assert!(!s.matches_line("\x1b[1mbold\x1b[0m"));
    }

    #[test]
    fn highlight_plain_line() {
        let s = Search::new("cat").unwrap();
        assert_eq!(
            s.highlight("a cat and a Cat"),
            "a \x1b[7mcat\x1b[27m and a \x1b[7mCat\x1b[27m"
        );
    }

    #[test]
    fn highlight_across_styles_reasserts_after_escapes() {
        let s = Search::new("ab").unwrap();
        assert_eq!(
            s.highlight("x\x1b[1ma\x1b[0mb"),
            "x\x1b[1m\x1b[7ma\x1b[0m\x1b[7mb\x1b[27m"
        );
        assert_eq!(text::strip_ansi(&s.highlight("x\x1b[1ma\x1b[0mb")), "xab");
    }

    #[test]
    fn highlight_without_match_is_identity() {
        let s = Search::new("zzz").unwrap();
        assert_eq!(s.highlight("\x1b[1mabc\x1b[0m"), "\x1b[1mabc\x1b[0m");
    }
}
