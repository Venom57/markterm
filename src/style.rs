//! ANSI/VT100 escape code helpers for terminal styling.

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
