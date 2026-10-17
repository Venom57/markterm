//! Exact-output snapshot tests for every fixture in `tests/fixtures/`.
//!
//! Each fixture is rendered in plain (`use_color = false`) and colour mode and
//! compared against `tests/snapshots/<name>.plain.txt` / `<name>.color.txt`.
//! In colour snapshots, control characters (including ESC) are replaced by
//! their Unicode "control picture" (ESC → `␛`) so diffs stay readable.
//!
//! Run with `UPDATE_SNAPSHOTS=1 cargo test` to (re)write the snapshot files.

use std::fs;
use std::path::{Path, PathBuf};

use super::render;
use crate::parser;

/// Per-fixture render settings: (width, no_wrap).
fn settings_for(name: &str) -> (u16, bool) {
    match name {
        "wrapping-styles" | "code-in-list-ordered" | "unicode-width" => (40, false),
        "long-words" => (30, false),
        "narrow-table" => (20, false),
        "nowrap-indent" => (80, true),
        _ => (80, false),
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn render_fixture(content: &str, width: u16, use_color: bool, no_wrap: bool) -> String {
    let events = parser::parse(content);
    let lines = render(events, width, use_color, no_wrap);
    let mut out = String::new();
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Make control characters visible (ESC → `␛`, BEL → `␇`, …), keeping newlines.
fn visible_controls(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\n' => '\n',
            '\x7f' => '␡',
            c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32).unwrap_or(c),
            c => c,
        })
        .collect()
}

/// A minimal LCS-based line diff, formatted like a unified diff body.
fn line_diff(expected: &str, actual: &str) -> String {
    let a: Vec<&str> = expected.lines().collect();
    let b: Vec<&str> = actual.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push_str(&format!("  {}\n", a[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push_str(&format!("- {}\n", a[i]));
            i += 1;
        } else {
            out.push_str(&format!("+ {}\n", b[j]));
            j += 1;
        }
    }
    out
}

fn check_snapshot(path: &Path, actual: &str, update: bool) -> Result<(), String> {
    if update {
        fs::write(path, actual).map_err(|e| format!("{}: {}", path.display(), e))?;
        return Ok(());
    }
    let expected = fs::read_to_string(path).map_err(|e| {
        format!(
            "{}: {} (run with UPDATE_SNAPSHOTS=1 to create it)",
            path.display(),
            e
        )
    })?;
    if expected == actual {
        Ok(())
    } else {
        Err(format!(
            "snapshot mismatch: {}\n--- expected\n+++ actual\n{}",
            path.display(),
            line_diff(&expected, actual)
        ))
    }
}

/// All `tests/fixtures/*.md` paths, sorted.
fn fixture_paths() -> Vec<PathBuf> {
    let fixtures = manifest_dir().join("tests/fixtures");
    let mut entries: Vec<PathBuf> = fs::read_dir(&fixtures)
        .expect("read tests/fixtures")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty(), "no fixtures found");
    entries
}

/// True when every SGR sequence opened in `line` is closed by its end:
/// the last escape sequence (if any) is a reset and the line ends with it.
fn sgr_closed(line: &str) -> bool {
    use crate::style::{tokens, Token};
    let mut open = false;
    for t in tokens(line) {
        if let Token::Esc(e) = t {
            open = !(e == "\x1b[0m" || e == "\x1b[m");
        }
    }
    !open
}

/// Every rendered line, in both modes and at several widths, is no wider
/// than the render width and (in colour) leaves no SGR state open, so the
/// pager can start a page at any line.
#[test]
fn fixtures_satisfy_line_invariants() {
    let mut failures = Vec::new();
    for path in fixture_paths() {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let bytes = fs::read(path).expect("read fixture");
        let content = String::from_utf8_lossy(&bytes);
        let (snap_width, no_wrap) = settings_for(&name);
        for width in [snap_width, 24, 33, 50, 80] {
            for nw in [no_wrap, !no_wrap] {
                for color in [false, true] {
                    let lines = render(parser::parse(&content), width, color, nw);
                    for (i, line) in lines.iter().enumerate() {
                        let w = crate::style::display_width(line);
                        if w > width as usize {
                            failures.push(format!(
                                "{name} w={width} no_wrap={nw} color={color} line {i}: width {w}: {line:?}"
                            ));
                        }
                        if color && !sgr_closed(line) {
                            failures.push(format!(
                                "{name} w={width} no_wrap={nw} line {i}: open SGR: {line:?}"
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn sgr_closed_detects_open_state() {
    assert!(sgr_closed("plain"));
    assert!(sgr_closed("\x1b[1mx\x1b[0m"));
    assert!(!sgr_closed("\x1b[1mx"));
    assert!(!sgr_closed("\x1b[1mx\x1b[0m\x1b[2my"));
}

#[test]
fn fixtures_match_snapshots() {
    let root = manifest_dir();
    let snapshots = root.join("tests/snapshots");
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some_and(|v| v == "1");
    if update {
        fs::create_dir_all(&snapshots).expect("create snapshots dir");
    }

    let entries = fixture_paths();

    let mut failures = Vec::new();
    for path in &entries {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let bytes = fs::read(path).expect("read fixture");
        let content = String::from_utf8_lossy(&bytes);
        let (width, no_wrap) = settings_for(&name);

        let plain = render_fixture(&content, width, false, no_wrap);
        let color = visible_controls(&render_fixture(&content, width, true, no_wrap));

        for (suffix, actual) in [("plain", plain), ("color", color)] {
            let snap = snapshots.join(format!("{}.{}.txt", name, suffix));
            if let Err(e) = check_snapshot(&snap, &actual, update) {
                failures.push(e);
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} snapshot(s) failed:\n\n{}\nIf the change is intended, rerun with UPDATE_SNAPSHOTS=1.",
            failures.len(),
            failures.join("\n")
        );
    }
}

#[test]
fn line_diff_marks_changes() {
    let d = line_diff("a\nb\nc\n", "a\nx\nc\n");
    assert_eq!(d, "  a\n- b\n+ x\n  c\n");
}
