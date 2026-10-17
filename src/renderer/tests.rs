//! Unit tests for the renderer.

use super::*;
use crate::parser;

#[test]
fn test_heading_renders() {
    let events = parser::parse("# Hello World");
    let lines = render(events, 80, &Theme::none(), false);
    assert!(lines.iter().any(|l| l.contains("Hello World")));
}

#[test]
fn test_bold_renders() {
    let events = parser::parse("**bold text**");
    let lines = render(events, 80, &Theme::dark(true), false);
    let joined = lines.join("");
    assert!(joined.contains("bold text"));
}

#[test]
fn test_inline_formatting_stays_on_one_line() {
    let events = parser::parse("This is **bold** and *italic* text.");
    let lines = render(events, 80, &Theme::none(), false);
    let content_lines: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        content_lines.len(),
        1,
        "Expected 1 content line, got: {:?}",
        content_lines
    );
    assert!(content_lines[0].contains("bold"));
    assert!(content_lines[0].contains("italic"));
}

#[test]
fn test_list_renders() {
    let events = parser::parse("- item one\n- item two");
    let lines = render(events, 80, &Theme::none(), false);
    assert!(lines.iter().any(|l| l.contains("item one")));
    assert!(lines.iter().any(|l| l.contains("item two")));
}

#[test]
fn test_code_block_renders() {
    let events = parser::parse("```\nlet x = 1;\n```");
    let lines = render(events, 80, &Theme::none(), false);
    assert!(lines.iter().any(|l| l.contains("let x = 1;")));
}

#[test]
fn test_horizontal_rule() {
    let events = parser::parse("---");
    let lines = render(events, 40, &Theme::none(), false);
    assert!(lines.iter().any(|l| l.contains("─")));
}

#[test]
fn test_word_wrap() {
    let events = parser::parse("This is a very long line that should be wrapped when the terminal width is narrow enough to require it.");
    let lines = render(events, 40, &Theme::none(), false);
    let content_lines: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
    assert!(content_lines.len() > 1, "Long text should wrap");
    for line in &content_lines {
        assert!(
            style::display_width(line) <= 40,
            "No line should exceed width"
        );
    }
}

#[test]
fn test_no_wrap_truncates_with_ellipsis() {
    let events = parser::parse(
        "This is a very long line that should be truncated when no-wrap mode is enabled.",
    );
    let lines = render(events, 30, &Theme::none(), true);
    let content_lines: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
    assert_eq!(content_lines.len(), 1, "No-wrap should produce one line");
    assert!(
        content_lines[0].ends_with('…'),
        "Truncated line should end with ellipsis"
    );
    assert!(
        style::display_width(content_lines[0]) <= 30,
        "Line should not exceed width"
    );
}

#[test]
fn test_table_respects_terminal_width() {
    let md = "| Short | This column has a very long text that should cause wrapping when the terminal is narrow enough |\n|---|---|\n| a | More long text that definitely exceeds the terminal width for testing purposes |";
    let events = parser::parse(md);
    let lines = render(events, 50, &Theme::none(), false);
    for line in &lines {
        assert!(
            style::display_width(line) <= 50,
            "Table line exceeds width ({}): '{}'",
            style::display_width(line),
            line
        );
    }
}

#[test]
fn test_table_multiline_cells() {
    let md = "| A | B |\n|---|---|\n| short | This is a long cell that should definitely wrap to multiple lines within the table row |\n";
    let events = parser::parse(md);
    let lines = render(events, 40, &Theme::none(), false);
    // Count data row lines (lines with | between header separator and bottom border)
    let table_lines: Vec<&String> = lines.iter().filter(|l| l.contains('│')).collect();
    // Should have: header + at least 2 lines for the data row (since cell wraps)
    assert!(
        table_lines.len() > 3,
        "Expected multi-line row, got {} table lines: {:?}",
        table_lines.len(),
        table_lines
    );
}

#[test]
fn test_wrap_cell_text_basic() {
    let lines = wrap_plain("short text", 20);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], "short text");
}

#[test]
fn test_wrap_cell_text_wraps() {
    let lines = wrap_plain("this is a longer text that needs wrapping", 15);
    assert!(lines.len() > 1, "Should wrap: {:?}", lines);
    for line in &lines {
        assert!(
            style::display_width(line) <= 15,
            "Wrapped line too wide: '{}'",
            line
        );
    }
}

fn plain(md: &str, width: u16) -> Vec<String> {
    render(parser::parse(md), width, &Theme::none(), false)
}

fn color(md: &str, width: u16) -> Vec<String> {
    render(parser::parse(md), width, &Theme::dark(true), false)
}

/// Wrap one unstyled run and return the plain text of each line.
fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    let runs = [Run {
        style: Style::PLAIN,
        text: text.into(),
        atomic: false,
    }];
    wrap_runs(&runs, width, false)
        .iter()
        .map(|l| style::render_spans(&l.spans, false))
        .collect()
}

#[test]
fn long_word_breaks_by_width_on_its_own_line() {
    assert_eq!(
        wrap_plain("see abcdefghijklmnop end", 6),
        vec!["see", "abcdef", "ghijkl", "mnop", "end"]
    );
    assert_eq!(wrap_plain("abcdefgh x", 6), vec!["abcdef", "gh x"]);
}

#[test]
fn long_word_never_splits_wide_chars() {
    // Wide characters that do not fit move to the next line whole.
    let lines = wrap_plain("a🎉🎉🎉", 4);
    assert_eq!(lines, vec!["a🎉", "🎉🎉"]);
    // Combining marks stay with their base character.
    assert_eq!(
        wrap_plain("e\u{301}e\u{301}e\u{301}", 2),
        vec!["e\u{301}e\u{301}", "e\u{301}"]
    );
}

#[test]
fn cjk_breaks_between_characters() {
    let lines = wrap_plain("日本語のテキストです", 8);
    assert_eq!(lines, vec!["日本語の", "テキスト", "です"]);
    // No break before closing punctuation.
    assert_eq!(
        wrap_plain("日本語。日本語", 6),
        vec!["日本", "語。日", "本語"]
    );
    for l in wrap_plain("漢字と English が混ざる文章です", 10) {
        assert!(style::display_width(&l) <= 10, "{l:?}");
    }
}

#[test]
fn styles_reopen_on_every_wrapped_line() {
    let lines = color("**aaa bbb ccc**", 7);
    assert_eq!(
        lines,
        vec![
            "\x1b[1maaa\x1b[0m",
            "\x1b[1mbbb\x1b[0m",
            "\x1b[1mccc\x1b[0m"
        ]
    );
    let lines = color("> **aaa bbb**", 9);
    assert_eq!(
        lines,
        vec![
            "  \x1b[2m│ \x1b[0;1maaa\x1b[0m",
            "  \x1b[2m│ \x1b[0;1mbbb\x1b[0m"
        ]
    );
}

#[test]
fn link_url_is_dim_on_its_own_line() {
    let lines = color("[docs](https://example.com/x)", 20);
    assert_eq!(
        lines,
        vec![
            "\x1b[4mdocs\x1b[0m",
            "\x1b[2m(https://example.c\x1b[0m",
            "\x1b[2mom/x)\x1b[0m"
        ]
    );
}

#[test]
fn inline_code_is_one_unit_and_keeps_spaces() {
    assert_eq!(plain("x `a  b` y", 80), vec!["x `a  b` y"]);
    assert_eq!(plain("aaaa `b c`", 9), vec!["aaaa", "`b c`"]);
    assert_eq!(
        color("x `a  b` y", 80),
        vec!["x \x1b[48;5;236m a  b \x1b[0m y"]
    );
    // Longer than a line: broken by width.
    assert_eq!(plain("`abcdefghij`", 8), vec!["`abcde", "fghij`"]);
}

#[test]
fn every_color_line_is_self_contained() {
    let md = "# Heading that wraps over lines\n\n> *italic quote text that wraps*\n\n- **bold item that wraps around**";
    for line in color(md, 16) {
        if line.contains('\x1b') {
            assert!(line.ends_with("\x1b[0m"), "{line:?}");
        }
    }
}

#[test]
fn table_with_long_url_and_cjk_fits() {
    let md = "| a | b |\n|---|---|\n| 漢字漢字 | https://example.com/averyveryverylongurlwithoutspaces/abc |";
    let lines = plain(md, 40);
    for l in &lines {
        assert!(style::display_width(l) <= 38, "{l:?}");
    }
    assert_eq!(
        style::display_width(&lines[0]),
        style::display_width(&lines[3])
    );
}

#[test]
fn test_no_leading_blank_line() {
    assert_eq!(plain("hello", 80), vec!["hello"]);
}

#[test]
fn test_loose_list_marker_stays_with_text() {
    assert_eq!(plain("- one\n\n- two", 80), vec!["  • one", "", "  • two"]);
}

#[test]
fn test_nested_list_parent_at_parent_indent() {
    assert_eq!(
        plain("- parent\n  - child\n- next", 80),
        vec!["  • parent", "    ◦ child", "  • next"]
    );
}

#[test]
fn test_list_in_quote_single_border() {
    assert_eq!(plain("> - item", 80), vec!["  │   • item"]);
}

#[test]
fn test_no_wrap_list_indent_not_doubled() {
    let lines = render(
        parser::parse("- item\n  - nested"),
        80,
        &Theme::none(),
        true,
    );
    assert_eq!(lines, vec!["  • item", "    ◦ nested"]);
}

#[test]
fn test_nested_blockquotes() {
    assert_eq!(
        plain("> outer\n>\n> > inner\n>\n> back", 80),
        vec!["  │ outer", "  │", "  │ │ inner", "  │", "  │ back"]
    );
}

#[test]
fn test_wrapped_list_item_hanging_indent() {
    assert_eq!(
        plain("- aaaa bbbb cccc dddd", 15),
        vec!["  • aaaa bbbb", "    cccc dddd"]
    );
}

#[test]
fn test_code_block_no_trailing_or_doubled_blank_rows() {
    assert_eq!(
        plain("```\na\nb\n```", 80),
        vec!["  ╭───", "  │ a", "  │ b", "  ╰───"]
    );
    // Inside a list item (one Text event per line).
    assert_eq!(
        plain("- x\n\n      a\n      b", 80),
        vec![
            "  • x",
            "",
            "      ╭───",
            "      │ a",
            "      │ b",
            "      ╰───"
        ]
    );
}

#[test]
fn test_code_block_in_quote_keeps_border() {
    assert_eq!(
        plain("> ```\n> a\n> ```", 80),
        vec!["  │   ╭───", "  │   │ a", "  │   ╰───"]
    );
}

#[test]
fn test_code_block_expands_tabs_and_truncates() {
    assert_eq!(plain("```\n\tx\ty\n```", 80)[1], "  │     x   y");
    let lines = plain("```\n0123456789abcdef\n```", 16);
    assert_eq!(lines[1], "  │ 012345678…");
    assert!(lines.iter().all(|l| style::display_width(l) <= 14));
}

/// Every ESC in `s` must start a renderer-generated SGR sequence.
fn only_sgr_escapes(s: &str) -> bool {
    let mut rest = s;
    while let Some(i) = rest.find('\x1b') {
        rest = &rest[i + 1..];
        let Some(body) = rest.strip_prefix('[') else {
            return false;
        };
        let end = body.find(|c: char| !(c.is_ascii_digit() || c == ';'));
        match end {
            Some(e) if body[e..].starts_with('m') => rest = &body[e + 1..],
            _ => return false,
        }
    }
    true
}

#[test]
fn test_control_characters_are_neutralised() {
    let md = "Evil \x1b]0;title\x07 and \x1b[31mred\x1b[0m\r\x7f `\x1b[2J` \
              [x](http://a\x1b/) \u{9b}31m\n\n```\x1b]52\n\x1b[1mcode\x07\n```";
    let plain_out = render(parser::parse(md), 80, &Theme::none(), false).join("\n");
    assert!(
        !plain_out
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t'),
        "control chars leaked: {:?}",
        plain_out
    );
    let color_out = render(parser::parse(md), 80, &Theme::dark(true), false).join("\n");
    assert!(only_sgr_escapes(&color_out), "{:?}", color_out);
    assert!(!color_out.contains('\x07') && !color_out.contains('\u{9b}'));
}

#[test]
fn test_link_in_table_cell_stays_in_cell() {
    let md = "| A | B |\n|---|---|\n| [l](https://x.io) | `c` **b** |\n\nafter";
    let lines = plain(md, 80);
    assert!(lines.iter().any(|l| l.contains("│ l (https://x.io) │")));
    assert!(lines.iter().any(|l| l.contains("`c` b")));
    assert_eq!(lines.last().unwrap(), "after");
    assert!(!lines.iter().any(|l| l.trim() == "(https://x.io)"));
    let color = render(parser::parse(md), 80, &Theme::dark(true), false).join("\n");
    assert!(color.contains("\x1b[1mb\x1b[0m"), "{color:?}");
}

#[test]
fn test_table_alignment() {
    let md = "| Left | Center | Right |\n|:--|:-:|--:|\n| a | b | c |";
    let lines = plain(md, 80);
    assert_eq!(lines[1], "  │ Left │ Center │ Right │");
    assert_eq!(lines[3], "  │ a    │   b    │     c │");
}

#[test]
fn test_too_wide_table_falls_back_to_vertical() {
    let md = "| a | bb | c |\n|---|---|---|\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |";
    let lines = plain(md, 18);
    assert_eq!(
        lines,
        vec![
            "────────────────",
            "a:  1",
            "bb: 2",
            "c:  3",
            "────────────────",
            "a:  4",
            "bb: 5",
            "c:  6",
            "────────────────",
        ]
    );
    assert!(lines.iter().all(|l| style::display_width(l) <= 18));
}

#[test]
fn test_yaml_front_matter_skipped() {
    assert_eq!(
        plain("---\ntitle: x\n---\n\n# Head\n\nbody", 80),
        vec!["Head", "════", "", "body"]
    );
}

#[test]
fn test_footnotes() {
    assert_eq!(
        plain("Text[^1].\n\n[^1]: The body.", 80),
        vec!["Text[1].", "", "[1]: The body."]
    );
    let color = render(
        parser::parse("a[^n]\n\n[^n]: b"),
        80,
        &Theme::dark(true),
        false,
    );
    assert_eq!(color[0], "a\x1b[2m[n]\x1b[0m");
    assert_eq!(color[2], "\x1b[2m[n]: \x1b[0mb");
}

#[test]
fn test_inline_html() {
    assert_eq!(
        plain("a<br>b <BR/> c <kbd>Ctrl</kbd>+<kbd>C</kbd>", 80),
        vec!["a", "b", "c Ctrl+C"]
    );
}

#[test]
fn test_html_block_text_kept() {
    assert_eq!(
        plain(
            "<details>\n<summary>Click &amp; see</summary>\n\nbody\n</details>\n\n<!-- hidden -->",
            80
        ),
        vec!["Click & see", "", "body"]
    );
    assert_eq!(strip_html("a < b <i>c</i>"), "a < b c");
    let color = render(parser::parse("<p>x y</p>"), 80, &Theme::dark(true), false);
    assert_eq!(color, vec!["\x1b[2mx y\x1b[0m"]);
}

#[test]
fn test_images() {
    assert_eq!(
        plain("![Alt text](a.png) ![](b.png)", 80),
        vec!["[image: Alt text] [image]"]
    );
    let color = render(parser::parse("![a](a.png)"), 80, &Theme::dark(true), false);
    assert_eq!(color, vec!["\x1b[2m[image: a]\x1b[0m"]);
}

#[test]
fn test_ordered_numbers_right_aligned() {
    let md = "8. eight\n9. nine\n10. ten\n    - sub";
    assert_eq!(
        plain(md, 80),
        vec!["   8. eight", "   9. nine", "  10. ten", "      ◦ sub"]
    );
    assert_eq!(digits(0), 1);
    assert_eq!(digits(9), 1);
    assert_eq!(digits(10), 2);
    assert_eq!(digits(u64::MAX), 20);
}

#[test]
fn test_task_checkbox_replaces_bullet() {
    assert_eq!(
        plain("- [x] done\n- [ ] todo item that wraps", 20),
        vec!["  [✓] done", "  [ ] todo item", "      that wraps"]
    );
    assert_eq!(
        plain("- [x] loose\n\n- [ ] two", 80),
        vec!["  [✓] loose", "", "  [ ] two"]
    );
    let color = render(parser::parse("- [x] a"), 80, &Theme::dark(true), false);
    assert_eq!(color[0], "  \x1b[1;32m[✓] \x1b[0ma");
}

#[test]
fn test_autolink_url_not_repeated() {
    assert_eq!(
        plain("<https://a.io> <me@x.org> [https://b.io](https://b.io)", 80),
        vec!["https://a.io me@x.org https://b.io"]
    );
    assert_eq!(plain("[t](https://a.io)", 80), vec!["t (https://a.io)"]);
}

#[test]
fn test_link_text_underlined() {
    let lines = render(
        parser::parse("[t](https://a.io)"),
        80,
        &Theme::dark(true),
        false,
    );
    assert_eq!(lines[0], "\x1b[4mt\x1b[0;2m (https://a.io)\x1b[0m");
}

#[test]
fn test_empty_list_item_keeps_marker() {
    assert_eq!(plain("-\n- x", 80), vec!["  • ", "  • x"]);
}

#[test]
fn plain_headings_are_underlined() {
    assert_eq!(
        plain("# Title\n\n## Sub head\n\n### Three", 80),
        vec!["Title", "═════", "", "Sub head", "────────", "", "Three"]
    );
    // Sized to the widest wrapped line, in display columns.
    assert_eq!(plain("# 日本語", 80), vec!["日本語", "══════"]);
    assert_eq!(plain("# aaaa bbbb", 8), vec!["aaaa", "bbbb", "════"]);
    // Colour mode relies on the heading style instead.
    assert_eq!(color("# Title", 80), vec!["\x1b[1;4;97mTitle\x1b[0m"]);
}

#[test]
fn light_theme_styles() {
    let lines = render(
        parser::parse("# T\n\n## U\n\n`c` *i*"),
        80,
        &Theme::light(false),
        false,
    );
    assert_eq!(
        lines,
        vec![
            "\x1b[1;4mT\x1b[0m",
            "",
            "\x1b[1;34mU\x1b[0m",
            "",
            "\x1b[48;5;254m c \x1b[0m \x1b[4mi\x1b[0m"
        ]
    );
}

#[test]
fn plain_mode_uses_unicode_borders() {
    let out = plain("> q\n\n```\nx\n```\n\n| a |\n|---|\n| b |", 80).join("\n");
    assert!(!out.contains('|'), "{out}");
    assert!(out.contains("│ q") && out.contains("│ x") && out.contains("│ b   │"));
}
