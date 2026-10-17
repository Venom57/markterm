# markterm — Analysis & Improvement Plan (17 OCT 2026)

A full review of the codebase covering performance, operation/robustness, and user
experience. Findings were produced by parallel analysis of the renderer
(`renderer.rs`, `style.rs`, `parser.rs`) and the pager/CLI/build
(`pager.rs`, `main.rs`, `terminal.rs`, `Cargo.toml`), then spot-checked by running
the binary against crafted markdown inputs.

**Legend:** ✅ = confirmed by running the binary · 📖 = from reading the code
(mostly interactive pager behaviour that can't be exercised non-interactively)

## Baseline (at commit `877401f`)

| Check | Result |
|-------|--------|
| Toolchain | Rust 1.99.1 stable installed via rustup (`~/.cargo`) |
| `cargo build` / `--release` | ✅ builds; release binary 1,017,416 bytes |
| `cargo test` | ✅ 16 passed, 0 failed |
| `cargo clippy -- -D warnings` | ❌ 8 errors (5× empty line after doc comment, needless range loop, redundant reference in `writeln!`, simplifiable `map_or`) |
| `cargo fmt --check` | ❌ 26 diff hunks across all source files |

---

## 1. Rendering correctness

### High

1. ✅ **Loose lists orphan their marker.** In a list with blank lines between items
   the bullet prints alone, then a blank line, then the text unindented. The
   `Tag::Paragraph` → `push_blank()` flushes the marker that `Tag::Item`
   (`renderer.rs:378`) placed in `current_line`.
   *Fix:* track a pending-marker flag; don't blank/flush before the first paragraph
   of an item.
2. ✅ **Nested-list parent drawn at child indent.** `Tag::List` (`renderer.rs:367`)
   changes `indent` before flushing the parent item's text in tight lists.
   *Fix:* `state.flush_wrapped()` at the top of `Tag::List`.
3. ✅ **Indent prefix applied twice.** `Tag::Item` writes `indent_prefix() + marker`
   into `current_line`, and `flush_wrapped` adds the prefix again. Inside a
   blockquote this shows a doubled border (`|   | • item`); under `--no-wrap` the
   list indent doubles.
   *Fix:* store the marker separately (`pending_marker: Option<String>`) and let
   `flush_wrapped` emit `prefix + marker` on the first line only.
4. ✅ **Blockquote nesting lost.** `in_blockquote: bool` (`renderer.rs:18`):
   `> >` renders at the same depth as its parent; closing the inner quote strips the
   border from the remainder of the outer quote; blank lines inside quotes have no
   `│`, breaking the bar.
   *Fix:* `blockquote_depth: usize`, one `│ ` per level; `push_blank` emits the bare
   prefix when depth > 0.
5. ✅ **Code-block blank lines.** `text.split('\n')` on `"line\n"` yields a trailing
   `""`: every block ends with an empty `│` row, and inside lists/quotes (one Text
   event per line) every line is followed by a blank (`renderer.rs:450`).
   *Fix:* buffer the block text; at `TagEnd::CodeBlock` iterate `buf.lines()`.
6. ✅ **Width = char count, not display width.** `style::visible_len`
   (`style.rs:41`) counts 1 per `char`. CJK and emoji overflow (a CJK paragraph at
   `-w 40` was ~110 columns), table columns misalign, combining marks over-count.
   *Fix:* add `unicode-width`; sum `UnicodeWidthChar::width(ch).unwrap_or(0)`.
   Apply in `truncate_styled` too.
7. ✅ **Long words/URLs never break.** A 93-char URL printed whole at width 38; URLs
   in table cells push the right border out. The spec requires a character-break
   fallback. *Fix:* split oversize segments by display width, keeping ANSI codes
   attached; allow breaks between CJK characters.
8. ✅ **Styles leak across wrapped lines.** Styles are opened once per text event and
   then split into words, so a wrapped bold/heading line ends with no reset and the
   next line has no opening code. Pages that start mid-span lose the style; an open
   style can bleed into the status bar; continuation lines inside quotes visibly lose
   bold (the quote prefix contains `RESET`); the link `DIM` code attaches to the
   previous word.
   *Fix:* close active SGR at every line break and re-open after the next prefix —
   ideally via the span model (see Plan, Phase 3).
9. ✅ **Links in table cells leak the URL below the table.** `TagEnd::Link`
   (`renderer.rs:474`) always appends to `current_line`, so `(https://…)` prints on
   its own line after the table. Cell bold/italic/code formatting is also dropped.
   *Fix:* route all inline output through one "current inline buffer" helper that
   targets `table_cell_buf` when in a cell.
10. ✅ **Terminal escape injection (security).** Raw control characters in input reach
    the terminal unchanged, even with `--theme none` — e.g. OSC 0 (window title) or
    OSC 52 (clipboard write). Raw `ESC` also corrupts `visible_len`.
    *Fix:* replace C0 controls (except `\t`) and `\x7f` in text, code and URLs before
    styling (U+FFFD or caret notation).

### Medium

11. ✅ **Table alignment ignored.** `table_alignments` (`renderer.rs:24`) is stored
    but never read; `:-:` and `-:` columns render left-aligned.
12. ✅ **Autolinks show the URL twice** (`https://a.io (https://a.io)`), and link text
    is not underlined as the spec requires. *Fix:* skip the parenthesised URL when
    text == URL or link type is Autolink/Email; add UNDERLINE to link text.
13. ✅ **YAML front matter renders as a horizontal rule plus a setext heading.**
    *Fix:* enable `ENABLE_YAML_STYLE_METADATA_BLOCKS`; skip or dim the block.
14. ✅ **Footnotes unsupported.** Without `ENABLE_FOOTNOTES`, `[^1]: note` parses as a
    link reference definition, rendering `^1 (note)` inline and losing the footnote.
    *Fix:* enable footnotes; render references as dim `[1]`, definitions with a dim
    `[1]:` prefix.
15. ✅ **HTML content silently dropped.** `Event::Html`/`InlineHtml` are ignored:
    `<br>` breaks, `<kbd>`, `<summary>`, centred README titles vanish. Passthrough
    is a non-goal, but losing the inner text is a bug. *Fix:* `<br>` → hard break;
    strip tags and keep inner text (dimmed for blocks).
16. ✅ **Images give no indication** — alt text renders as plain text.
    *Fix:* dim `[image: alt]`.
17. ✅ **Code blocks are never wrapped/truncated** (even with `--no-wrap`), tabs are
    emitted raw, and code inside lists/quotes ignores `indent_prefix()` (loses the
    quote border). *Fix:* expand tabs to stops, apply the indent prefix, truncate
    with `…` (or hard-wrap with `↪`).
18. ✅ **Inline code spacing/wrapping.** Internal double spaces collapse; background
    padding is asymmetric; multi-word spans can wrap mid-span with the background
    left open. *Fix:* treat inline code as one unbreakable segment.

### Low

19. ✅ No hanging indent — wrapped list text returns under the bullet.
20. ✅ Ordered-list numbers not right-aligned (`9.` vs `10.`).
21. ✅ Task items show both a bullet and a checkbox (`• [✓] done`).
22. ✅ Every document starts with a blank line (`push_blank` at `renderer.rs:66`,
    `map_or(true, …)` on an empty vec), wasting the first pager row.
23. ✅ Wide tables don't degrade: 8 columns at `-w 20` yields a 51-column table.
    *Fix:* fall back to a vertical `key: value` layout or truncate columns.

## 2. Operation & robustness

### High

24. ✅ **Broken pipe panics.** `markterm big.md | head -1` panics ("failed printing to
    stdout: Broken pipe") and exits 101 (`main.rs:75-77`, `println!`).
    *Fix:* locked `BufWriter`, `writeln!`, treat `BrokenPipe` as clean exit 0.
25. 📖 **Terminal not restored on panic or SIGTERM/SIGHUP.** The comment at
    `pager.rs:38` claims panic safety but nothing implements it; a panic or kill
    leaves the tty raw/no-echo. `disable_raw_mode()?` failure also drops the loop's
    result. *Fix:* RAII `TerminalGuard` (disable raw mode, leave alt screen, show
    cursor) + `panic::set_hook` + `signal-hook` for TERM/HUP/QUIT (exit 128+sig).
26. 📖 **No resize handling.** `Event::Resize` falls through to `_ => {}`
    (`pager.rs:188`). Page height and wrap width are fixed at startup; shrinking the
    terminal corrupts the display. *Fix:* on resize recompute height, re-render at
    the new width (unless `-w`), preserve relative position, clamp, redraw.
27. 📖 **Over-wide lines break page layout.** Lines wider than the terminal (code
    blocks, CJK, `-w` > actual width) physically wrap, so the page exceeds the screen,
    scrolls, and pushes the status line off. The "fits on one screen" check
    (`pager.rs:29`) has the same blind spot. *Fix:* clip each line in `draw_page`
    with an ANSI-aware truncate, or `DisableLineWrap` while paging.

### Medium

28. ✅ **Colour codes emitted when piped.** With the default `--theme auto`,
    redirected/piped output contains escape codes (`main.rs:59-65`).
    *Fix:* `auto` ⇒ no colour when stdout is not a TTY; add `--color=auto|always|never`.
29. ✅ **Invalid UTF-8 rejected outright** ("stream did not contain valid UTF-8",
    `main.rs:42,48`). *Fix:* read bytes, `String::from_utf8_lossy`.
30. ✅ **`NO_COLOR` off-spec.** `NO_COLOR=` (empty) disables colour; no-color.org says
    only a non-empty value should. A non-UTF-8 value is treated as unset.
    *Fix:* `var_os("NO_COLOR").is_some_and(|v| !v.is_empty())`.
31. 📖 **Status line inherits open styles** — it writes `REVERSE` with no leading
    `RESET` (`pager.rs:249`), and lines are not reset after drawing.
32. 📖 **Key events not filtered by kind.** On Windows or with keyboard enhancement,
    Press and Release both fire. *Fix:* check `key.kind == KeyEventKind::Press`.

## 3. Performance

33. 📖 **Redraw flicker / syscall count** (`pager.rs:196-261`): `execute!` flushes a
    full `Clear(All)` immediately (blank flash); unbuffered stdout makes each line its
    own `write()`; `terminal::size()` opens `/dev/tty` per redraw; no-op keys
    (Space at end, `g` at top) still redraw.
    *Fix:* build each frame in a `BufWriter` with `queue!`; per line `MoveTo` + line +
    `RESET` + `Clear(UntilNewLine)`; optional synchronized update; single flush; cache
    size; skip redraw when offset is unchanged.
34. ✅ **`opt-level = "z"` is ~55% slower.** On a 5.8 MB file: `z` ≈ 0.40 s vs `3` ≈
    0.26 s; binaries 1.02 MB vs 1.24 MB. *Fix:* `opt-level = 3` (or `"s"`),
    `codegen-units = 1`, and `panic = "abort"` once item 25 is done.
35. 📖 **Unneeded dependency features** (confirmed in `Cargo.lock`): pulldown-cmark
    defaults pull `getopts` + HTML escaper; clap `color`/`suggestions` pull anstream,
    anstyle-*, strsim; crossterm bracketed-paste. *Fix:* `default-features = false`
    with minimal feature sets.
36. 📖 **Allocation churn.** Per text event: style-prefix `Vec` + `String`, `format!`;
    per flush: rebuilt `indent_prefix()`, one `String` per word, re-scanned widths;
    `table_rows.clone()`/`table_row.clone()` instead of `mem::take`; `parse` collects
    all events into a `Vec` rather than streaming. Measured throughput is fine
    (~14 MB/s, no O(n²)), so this is low priority and mostly falls out of Phase 3.

## 4. User experience

37. ✅ **Themes are not implemented.** `auto`/`dark`/`light` produce identical output
    and `--theme bogus` is silently accepted. On light backgrounds the bright-white
    H1 is near-invisible and the `48;5;236` inline-code background is unreadable.
    *Fix:* clap `ValueEnum`; `Theme` struct of per-element styles; a light palette;
    `auto` via `COLORFGBG` / OSC 11 query, falling back to dark.
38. 📖 **No alternate screen.** The pager clears the user's shell output, leaves the
    last page and status bar behind on quit, and the mouse wheel scrolls the
    terminal's scrollback. *Fix:* `EnterAlternateScreen` + `cursor::Hide`; enable
    alternate-scroll mode (`\e[?1007h`) so the wheel sends Up/Down.
39. ✅ **No-colour mode loses structure** — headings look like body text.
    *Fix:* `#` prefixes, or `═`/`─` underlines for H1/H2.
40. ✅ Plain mode mixes ASCII `|` borders with Unicode corners/bullets.
41. 📖 Status line is never truncated; a long filename or narrow terminal wraps it.
42. 📖 No help screen (`?`/`h`); status bar lists only three keys; no `(END)` marker.
43. ✅ `cat x.md | markterm` fails ("required arguments were not provided: <FILE>").
    *Fix:* make FILE optional, default to `-` when stdin is not a TTY.
44. 📖 Ctrl-C exits 0 (convention is 130); Ctrl-Z does nothing (raw mode disables
    ISIG) — implement suspend/resume.
45. 📖 Ctrl-B/Ctrl-D/Ctrl-U work only by accident (`Char('b'|'d'|'u'), ..` matches
    any modifier) while the comments at `pager.rs:169,179` imply intent. Make them
    explicit.

## 5. Missing features

46. Search: `/`, `n`, `N` (spec stretch goal).
47. Extra pager keys: `f`/`z`/`w`, `Ctrl-L` redraw, `%`/`p` jumps, count prefixes.
48. Syntax highlighting, config file (spec M5).
49. Italic has no underline fallback (spec calls for `\e[4m` fallback).

## 6. Code quality, build & tests

50. ✅ Clippy (`-D warnings`) fails with 8 errors; `cargo fmt --check` fails. Module
    headers should use `//!` instead of `///`.
51. 📖 Dead code: `#[allow(dead_code)] mod style` hides unused items;
    `table_alignments` and `in_table_head` never read; `ListContext` `Clone` derive
    and depth field redundant; `~` padding in `draw_page` (`pager.rs:218`)
    unreachable because the offset is clamped.
52. ✅ **No snapshot/integration tests.** Unit tests only assert `contains`, which is
    why items 1–5 went unnoticed. CLAUDE.md calls for fixture snapshot tests.
53. 📖 Pager logic is untestable as written — factor key→action mapping and offset
    math into a pure state struct.

---

## Plan

### Phase 0 — Hygiene (unblocks CI-style checks)
- `cargo fmt`; fix the 8 clippy errors (#50); remove `#[allow(dead_code)]` and the
  dead code it hides (#51).
- Release profile: `opt-level = 3`, `codegen-units = 1` (#34). Trim dependency
  features (#35) and compare binary size.

### Phase 1 — Safety net
- Add exact-output snapshot tests (`--theme none`) under `tests/` using fixtures that
  reproduce items 1–5, 9, 11–14, 22 (#52). Tests should be written *failing* first
  where they capture a bug, then turned green in Phase 2.

#### Regression fixtures (`tests/fixtures/`)

Repro inputs used during this analysis. Render with `--theme none --no-pager` unless
noted.

| Fixture | Reproduces |
|---------|------------|
| `lists-quotes-code.md` | #1, #2, #3, #4, #5 (tabs: #17) |
| `loose-lists-autolinks.md` | #1, #3, #4, #5, #12, #22 |
| `wrapping-styles.md` | #6, #7, #8, #12, #19 — use `-w 40` |
| `extensions-tables-html.md` | #6, #7, #9, #11, #13, #14, #15, #16, #21 |
| `escape-injection.md` | #10 (contains raw ESC/BEL bytes on purpose) |
| `nowrap-indent.md` | #3 — use `--no-wrap` |
| `empty.md` | empty-input edge case |
| `narrow-table.md` | #23 — use `-w 20` |
| `code-in-list-ordered.md` | #5, #17, #20 — use `-w 40` |
| `code-in-quote-list.md` | #5, #17 |

Large performance inputs (5.8 MB `big.md`, 3.7 MB single-paragraph `onepara.md`)
were generated ad hoc and are not committed.

### Phase 2 — Quick, high-value fixes
- Renderer: #1, #2, #3, #4, #5, #9, #10, #12, #22.
- CLI/output: #24 (broken pipe), #28 (no colour when piped), #29 (lossy UTF-8),
  #30 (NO_COLOR), #43 (implicit stdin), clap `ValueEnum` for `--theme`.

### Phase 3 — Structural renderer refactor
- Replace ANSI-string `current_line` with a span model
  (`Vec<Span { style, text }>`), computing widths with `unicode-width` and emitting
  escape codes only when a line is finalised. Resolves #6, #7, #8, #18, #19 together
  and most of #36.
- Then: #11 (alignment), #17 (code blocks), #13–#16 (front matter, footnotes, HTML,
  images), #20, #21, #23.

### Phase 4 — Pager robustness & smoothness
- `TerminalGuard` + panic hook + signal handling (#25); alternate screen +
  alternate-scroll (#38); resize with re-render (#26); line clipping (#27);
  buffered single-flush redraws (#33); reset before status line (#31); key-kind
  filter (#32); status truncation (#41); explicit Ctrl bindings (#45).
- Extract a pure pager state struct and unit-test it (#53).

### Phase 5 — UX & features
- Real themes with a light palette and auto detection (#37); no-colour structure
  (#39, #40, #49); help screen and `(END)` marker (#42); exit codes and suspend
  (#44); search (#46); extra keys (#47); then M5 items (#48).
