//! Colour themes: the style of every rendered element.
//!
//! A [`Theme`] is chosen once from `--theme` (with `auto` already resolved
//! to the detected terminal background) and handed to the renderer. The
//! `none` theme has no styles at all and switches the renderer to its
//! plain-text structure (backticks around inline code, underlined H1/H2).

use crate::style::{Color, Style};

/// Style of each element the renderer draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Whether escape codes are emitted at all. When false every style is
    /// ignored and plain-text fallbacks are used instead.
    pub color: bool,
    /// H1–H6.
    pub heading: [Style; 6],
    pub bold: Style,
    pub italic: Style,
    pub strike: Style,
    pub inline_code: Style,
    /// Fenced code block border (`│`, `╰───`).
    pub code_border: Style,
    /// Fenced code block top line with the language name (`╭─ rust`).
    pub code_label: Style,
    pub quote_border: Style,
    /// Bullets by nesting depth (`•`, `◦`, `▪`).
    pub bullets: [Style; 3],
    pub ordered_marker: Style,
    pub link_text: Style,
    pub link_url: Style,
    pub task_done: Style,
    pub task_todo: Style,
    pub table_border: Style,
    pub table_header: Style,
    pub rule: Style,
    /// Footnote references and definition labels.
    pub footnote: Style,
    /// Text kept from HTML blocks.
    pub html: Style,
    /// Image placeholders (`[image: alt]`).
    pub image: Style,
}

const PLAIN: Style = Style::PLAIN;
const BOLD: Style = PLAIN.bold();
const DIM: Style = PLAIN.dim();

impl Theme {
    /// The palette for dark backgrounds.
    pub fn dark(italic: bool) -> Theme {
        let marker = PLAIN.fg(Color::Ansi(6)); // cyan
        Theme {
            color: true,
            heading: [
                BOLD.underline().fg(Color::Ansi(15)), // bright white
                BOLD.fg(Color::Ansi(14)),             // bright cyan
                BOLD.fg(Color::Ansi(11)),             // bright yellow
                BOLD,
                BOLD,
                BOLD,
            ],
            bold: BOLD,
            italic: italic_style(italic),
            strike: PLAIN.strike(),
            inline_code: PLAIN.bg(Color::Indexed(236)),
            code_border: DIM,
            code_label: DIM,
            quote_border: DIM,
            bullets: [marker; 3],
            ordered_marker: marker,
            link_text: PLAIN.underline(),
            link_url: DIM,
            task_done: BOLD.fg(Color::Ansi(2)),
            task_todo: DIM,
            table_border: DIM,
            table_header: BOLD,
            rule: DIM,
            footnote: DIM,
            html: DIM,
            image: DIM,
        }
    }

    /// The palette for light backgrounds: default foreground for H1, blue
    /// and magenta headings, a light-grey code background, and no bright
    /// white or yellow (unreadable on white).
    pub fn light(italic: bool) -> Theme {
        let marker = PLAIN.fg(Color::Ansi(4)); // blue
        Theme {
            heading: [
                BOLD.underline(),
                BOLD.fg(Color::Ansi(4)), // blue
                BOLD.fg(Color::Ansi(5)), // magenta
                BOLD,
                BOLD,
                BOLD,
            ],
            inline_code: PLAIN.bg(Color::Indexed(254)),
            bullets: [marker; 3],
            ordered_marker: marker,
            task_done: BOLD.fg(Color::Ansi(2)),
            ..Theme::dark(italic)
        }
    }

    /// No colours or attributes.
    pub fn none() -> Theme {
        Theme {
            color: false,
            heading: [PLAIN; 6],
            bold: PLAIN,
            italic: PLAIN,
            strike: PLAIN,
            inline_code: PLAIN,
            code_border: PLAIN,
            code_label: PLAIN,
            quote_border: PLAIN,
            bullets: [PLAIN; 3],
            ordered_marker: PLAIN,
            link_text: PLAIN,
            link_url: PLAIN,
            task_done: PLAIN,
            task_todo: PLAIN,
            table_border: PLAIN,
            table_header: PLAIN,
            rule: PLAIN,
            footnote: PLAIN,
            html: PLAIN,
            image: PLAIN,
        }
    }
}

/// Italic where supported, underline otherwise (the spec's fallback).
fn italic_style(italic: bool) -> Style {
    if italic {
        PLAIN.italic()
    } else {
        PLAIN.underline()
    }
}

/// Whether the terminal named by `TERM` can show italics. The Linux
/// console, DEC VT terminals, `dumb` and similar consoles cannot; they get
/// underline instead. Unknown or unset terminals are assumed capable.
pub fn supports_italic(term: Option<&str>) -> bool {
    let Some(term) = term else {
        return true;
    };
    let base = term.split('-').next().unwrap_or(term);
    !(matches!(base, "linux" | "dumb" | "ansi" | "cons25" | "vt52")
        || (base.starts_with("vt") && base[2..].chars().all(|c| c.is_ascii_digit())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn italic_fallback_for_limited_consoles() {
        for term in [
            "linux",
            "vt100",
            "vt220",
            "vt102-am",
            "dumb",
            "linux-16color",
        ] {
            assert!(!supports_italic(Some(term)), "{term}");
        }
        for term in ["xterm-256color", "alacritty", "tmux-256color", "vte"] {
            assert!(supports_italic(Some(term)), "{term}");
        }
        assert!(supports_italic(None));
        assert_eq!(Theme::dark(false).italic, PLAIN.underline());
        assert_eq!(Theme::dark(true).italic, PLAIN.italic());
    }

    #[test]
    fn light_palette_avoids_bright_white_and_yellow() {
        let light = Theme::light(true);
        let bright = [Color::Ansi(11), Color::Ansi(15)];
        for s in light.heading {
            for c in bright {
                assert_ne!(s.patch(PLAIN), s.patch(PLAIN.fg(c)), "{s:?}");
            }
        }
        assert_ne!(light.inline_code, Theme::dark(true).inline_code);
    }

    #[test]
    fn none_has_no_styles() {
        let t = Theme::none();
        assert!(!t.color);
        assert!(t.heading.iter().all(|s| s.is_plain()));
        assert!(t.bold.is_plain() && t.inline_code.is_plain() && t.rule.is_plain());
    }
}
