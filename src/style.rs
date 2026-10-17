//! ANSI/VT100 escape code helpers for terminal styling.

use std::borrow::Cow;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const ITALIC: &str = "\x1b[3m";
pub const UNDERLINE: &str = "\x1b[4m";
pub const REVERSE: &str = "\x1b[7m";
pub const STRIKETHROUGH: &str = "\x1b[9m";

// Foreground colors
pub const FG_GREEN: &str = "\x1b[32m";
pub const FG_CYAN: &str = "\x1b[36m";
pub const FG_BRIGHT_WHITE: &str = "\x1b[97m";
pub const FG_BRIGHT_CYAN: &str = "\x1b[96m";
pub const FG_BRIGHT_YELLOW: &str = "\x1b[93m";

// Background colors
pub const BG_GREY: &str = "\x1b[48;5;236m";

/// Build a style string from multiple codes.
pub fn combine(codes: &[&str]) -> String {
    codes.concat()
}

/// Wrap text with a style, appending RESET at the end.
pub fn styled(text: &str, codes: &[&str], use_color: bool) -> String {
    if !use_color {
        return text.to_string();
    }
    format!("{}{}{}", combine(codes), text, RESET)
}

/// True for control characters that must never reach the terminal from
/// document content: C0 controls other than tab and newline, DEL, and C1.
fn is_unsafe_control(c: char) -> bool {
    matches!(c, '\0'..='\x08' | '\x0b'..='\x1f' | '\x7f'..='\u{9f}')
}

/// Replace unsafe control characters (ESC, BEL, CR, DEL, C1, ...) with
/// U+FFFD so untrusted input cannot inject terminal escape sequences.
pub fn sanitize(s: &str) -> Cow<'_, str> {
    if !s.chars().any(is_unsafe_control) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(
        s.chars()
            .map(|c| if is_unsafe_control(c) { '\u{FFFD}' } else { c })
            .collect(),
    )
}

/// Calculate the display width of a string, ignoring ANSI escape sequences.
pub fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            len += 1;
        }
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visible_len_plain() {
        assert_eq!(visible_len("hello"), 5);
    }

    #[test]
    fn test_visible_len_with_ansi() {
        let s = format!("{}hello{}", BOLD, RESET);
        assert_eq!(visible_len(&s), 5);
    }

    #[test]
    fn test_sanitize() {
        assert!(matches!(sanitize("plain\ttext\n"), Cow::Borrowed(_)));
        assert_eq!(
            sanitize("a\x1b]0;t\x07b\x7f\r\u{9b}"),
            "a\u{FFFD}]0;t\u{FFFD}b\u{FFFD}\u{FFFD}\u{FFFD}"
        );
    }

    #[test]
    fn test_styled_no_color() {
        assert_eq!(styled("hi", &[BOLD], false), "hi");
    }

    #[test]
    fn test_styled_with_color() {
        let result = styled("hi", &[BOLD], true);
        assert!(result.starts_with("\x1b[1m"));
        assert!(result.ends_with("\x1b[0m"));
        assert!(result.contains("hi"));
    }
}
