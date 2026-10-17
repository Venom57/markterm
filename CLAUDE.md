# markterm

A lightweight, self-contained command-line tool that renders markdown files with VT100/XTerm terminal formatting and displays them in a built-in `more`-style pager.

## Project Overview

**markterm** fills a gap in the CLI markdown tool ecosystem: a single compiled binary with zero runtime dependencies that renders markdown beautifully using ANSI/VT100 escape codes and pages output like `more` (one screenful at a time, spacebar to continue).

### Goals
- Render markdown to formatted terminal output using VT100/ANSI escape sequences
- Built-in pager with `more`-style interaction (spacebar = next page, q = quit)
- Single static binary with no runtime dependencies
- Fast startup and rendering
- Respect terminal dimensions (columns and rows)
- Graceful degradation on limited terminals

### Non-Goals
- Full TUI application (no mouse capture, no file browser)
- Inline image rendering
- HTML passthrough rendering
- Competing with glow's feature set — focus on simplicity and speed

## Technology Stack

- **Language**: Rust (2021 edition)
- **Markdown parsing**: `pulldown-cmark` (CommonMark compliant; default features off)
- **Terminal interaction**: `crossterm` (only the `events` feature)
- **CLI argument parsing**: `clap` (derive-based; minimal features)
- **Display width**: `unicode-width`
- **Unix only**: `signal-hook` (terminal restore on signals), `libc` (background-colour query)
- **Build**: Cargo, targeting Linux primarily (macOS/BSDs as secondary)

## Architecture

```
src/
├── main.rs                # Entry point: CLI args, input reading, colour policy, theme resolution, output
├── parser.rs              # pulldown-cmark wrapper; returns an event iterator
├── renderer.rs            # Events → styled, display-width-wrapped lines (span model)
├── renderer/
│   ├── tests.rs           # Focused unit tests per element
│   └── snapshot_tests.rs  # Fixture snapshot tests + line invariant test
├── style.rs               # Style/Span model, SGR generation (StyledWriter), ANSI-aware width/clip/strip, sanitising
├── theme.rs               # Theme struct: style per element; dark / light / none palettes; italic fallback
├── terminal.rs            # Terminal size; background detection (COLORFGBG, OSC 11 + DA1 query)
└── pager/
    ├── mod.rs             # Pager I/O: event loop, buffered frame drawing, resize, suspend
    ├── state.rs           # Pure pager state: key → action mapping, offsets, counts, help, status line
    ├── search.rs          # Smart-case substring search and ANSI-aware match highlighting
    ├── guard.rs           # TerminalGuard (raw mode, alt screen, cursor), panic hook, signal handling
    └── text.rs            # Re-exports style.rs width/clip/strip helpers for the pager
tests/
├── cli.rs                 # Integration tests running the binary (colour policy, stdin, errors, broken pipe)
├── fixtures/*.md          # Sample/regression markdown inputs
└── snapshots/*.txt        # Expected renderer output (<name>.plain.txt / .color.txt / .light.txt)
docs/
└── 17OCT2026-PLAN.md      # Analysis findings (#1–#53), phased plan and implementation status
```

### Data Flow

```
File/stdin → pulldown-cmark event iterator → renderer + Theme (styled lines) → pager (page-at-a-time display)
```

1. **main.rs** parses CLI args, reads the input file (or stdin) as bytes (lossy UTF-8), decides whether colour is on, resolves `--theme auto` via `terminal::detect_background()`, and builds the `Theme`
2. **parser.rs** wraps pulldown-cmark (strikethrough, tables, task lists, footnotes, YAML metadata blocks enabled) and yields an event iterator
3. **renderer.rs** streams the events into inline `Run`s, wraps them into `Line`s of spans measured by display width, and emits a `Vec<String>`. Escape codes are written only when a line is finished, so **every line is self-contained**: it opens the styles it needs and ends in the plain state
4. **pager** displays the lines one terminal-page at a time. It receives a re-render closure so it can re-wrap on resize (unless `-w` was given). Non-TTY stdout or `--no-pager` prints directly through a buffered writer

## Markdown Rendering Spec

### Supported Elements & Formatting

Colours below are the `dark` theme; see [Themes](#themes) for `light` and `none`.

| Element | VT100 Rendering |
|---------|----------------|
| **H1** | Bold + Underline + bright white, preceded by blank line |
| **H2** | Bold + bright cyan, preceded by blank line |
| **H3** | Bold + bright yellow, preceded by blank line |
| **H4–H6** | Bold, preceded by blank line |
| **Bold** | `\e[1m` (bold/bright) |
| **Italic** | `\e[3m` (italic), or `\e[4m` (underline) on terminals without italics |
| **Strikethrough** | `\e[9m` (strikethrough) |
| **Inline code** | Grey background with one space of padding each side; never broken across lines unless longer than a line |
| **Code blocks** | Indented, dim box border, language label shown if present; tabs expanded to 4-column stops; long lines truncated with `…` (never wrapped) |
| **Block quotes** | `│` left border in dim per nesting level, indented text |
| **Unordered list** | `•` bullet, nested with `◦` and `▪`, indented per level, hanging indent for wrapped text |
| **Ordered list** | Numbered `1.`, `2.`, etc., right-aligned to the widest number in the list |
| **Horizontal rule** | `─` repeated across terminal width |
| **Links** | Text underlined, URL in dim parentheses after (omitted for autolinks/email or when text equals the URL) |
| **Tables** | Box-drawing borders, bold header row, column alignment honoured; falls back to stacked `header: value` rows when too wide |
| **Task lists** | `[✓]` / `[ ]` replaces the bullet, with color |
| **Footnotes** | Reference `[1]` (dim); definition rendered as `[1]: body` |
| **Images** | Dim placeholder `[image: alt]` |
| **HTML** | Not passed through: tags stripped, text kept (dim for blocks); `<br>` is a hard break |
| **YAML front matter** | Skipped |

Control characters in document content (C0 except tab/newline, DEL, C1) are replaced with U+FFFD, so a document can't inject terminal escapes.

### Word Wrapping
- Wrap to terminal width minus a small margin (2 columns)
- Respect indentation for nested elements (prefixes come from the block container stack)
- Measure by display width (`unicode-width`): CJK = 2 columns, combining marks/ZWJ = 0
- Break on word boundaries; CJK text may break between characters; words/URLs longer than a line start a new line and are broken by width
- `--no-wrap` truncates long lines with `…` instead

### Themes

| Theme | Description |
|-------|-------------|
| `auto` | Detects the background (`COLORFGBG`, then an OSC 11 query with a DA1 sentinel, 100 ms cap); falls back to `dark` |
| `dark` | The colours in the table above |
| `light` | H1 bold+underline in default colour, blue H2, magenta H3, light-grey inline-code background, blue list markers; no bright white/yellow |
| `none` | No colours or attributes |

Without colour, structure stays visible in plain text: H1 underlined with `═`, H2 with `─`, inline code keeps its backticks, and borders use the same `│`/box-drawing characters as colour mode.

## Pager Behavior

### Display
- Runs in the alternate screen with the cursor hidden; alternate-scroll mode makes the mouse wheel scroll the document (no mouse capture)
- Show one screenful of rendered lines at a time (terminal height - 1 for status line); each line is clipped to the terminal width
- Documents that fit on one screen are printed directly without starting the pager
- Status line at bottom: filename (elided to fit), line position, percentage through document, `(END)` at the bottom, `?` help hint
- Status line styled with reverse video
- Frames are built in a buffer and flushed once (no full-screen clear); redraw is skipped when nothing changed
- Terminal resize re-renders at the new width (unless `-w` was given) and keeps the reading position

### Key Bindings
| Key | Action |
|-----|--------|
| `Space` / `f` / `z` / `PageDown` / `Ctrl-F` | Next page |
| `b` / `w` / `PageUp` / `Ctrl-B` | Previous page |
| `Enter` / `j` / `e` / `Down` / `Ctrl-E` | Next line |
| `k` / `y` / `Up` / `Ctrl-Y` | Previous line |
| `d` / `Ctrl-D` | Half page down |
| `u` / `Ctrl-U` | Half page up |
| `g` / `<` / `Home` | Go to top (`<n>g`: go to line n) |
| `G` / `>` / `End` | Go to bottom (`<n>G`: go to line n) |
| `<n>%` / `<n>p` | Go to n percent |
| `/pattern` | Search forward (smart case; empty pattern repeats) |
| `n` / `N` | Next / previous match (wraps around) |
| `Ctrl-L` / `r` | Redraw |
| `Ctrl-Z` | Suspend (Unix) |
| `h` / `?` | Help screen |
| `q` / `Q` / `Esc` | Quit |
| `Ctrl-C` | Quit with exit status 130 |

A number typed before a movement key repeats it (e.g. `10j`). Only key presses are handled (releases ignored).

### Raw Mode & Terminal Restore
- Enter raw terminal mode on start; a `TerminalGuard` restores raw mode, the alternate screen and the cursor on exit
- A panic hook restores the terminal before printing the panic (release builds use `panic = "abort"`)
- SIGTERM/SIGHUP/SIGQUIT restore the terminal and exit with 128+signal (Unix)
- Disable line buffering and echo during pager operation

## CLI Interface

```
markterm [OPTIONS] [FILE]

Arguments:
  [FILE]    Markdown file to display (use - for stdin; defaults to stdin when it is piped)

Options:
  -w, --width <COLS>     Override terminal width
  -t, --theme <THEME>    Color theme: auto, dark, light, none (default: auto)
      --color <WHEN>     When to use colour: auto, always, never (default: auto)
      --no-pager         Dump rendered output to stdout without paging
      --no-wrap          Truncate long lines with ellipsis instead of wrapping
  -h, --help             Show help
  -V, --version          Show version
```

### Stdin Support
- `cat README.md | markterm`, `cat README.md | markterm -` or `markterm - < README.md`
- When reading from stdin, buffer all input before rendering; the pager reads keys from `/dev/tty`
- Input that is not valid UTF-8 is shown with replacement characters rather than rejected

### Colour & Pipe Detection
- If stdout is not a TTY (piped), output rendered text without pager and without interactive controls
- Colour is emitted only when `--theme` is not `none`, `NO_COLOR` is unset or empty, and either `--color always` or (`--color auto` and stdout is a TTY)
- A closed output pipe (e.g. `| head`) exits cleanly with status 0

### Exit Status
| Status | Meaning |
|--------|---------|
| `0` | Success (including a closed output pipe) |
| `1` | Input could not be read, or a terminal error |
| `2` | Invalid command-line usage |
| `130` | Quit with `Ctrl-C` |
| `128+N` | Terminated by signal N (terminal restored first) |

## Development

### Build & Run
```bash
cargo build                        # Debug build
cargo build --release              # Release build (opt-level "s", LTO, codegen-units 1, panic abort)
cargo run -- README.md             # Run on a file
cargo run -- --no-pager README.md  # Dump without paging
cargo install --locked --path .    # Install to ~/.cargo/bin
```

### Testing Strategy
- **Unit tests**: parser options, renderer output per element (`src/renderer/tests.rs`), word wrapping, style/width helpers, themes, background-detection parsing, pager state/keys/search
- **Integration tests**: `tests/cli.rs` runs the binary (colour policy, `NO_COLOR`, stdin, invalid UTF-8, errors, broken pipe)
- **Snapshot tests**: every `tests/fixtures/*.md` is rendered in plain and colour mode and compared to `tests/snapshots/`; control characters in colour snapshots are shown as `␛` etc. Regenerate intentionally with `UPDATE_SNAPSHOTS=1 cargo test` and review the diff
- **Line invariant test**: all fixtures × themes × widths × wrap modes — no line wider than the render width, no colour line leaves SGR state open
- Place sample markdown files in `tests/fixtures/` (per-fixture width/flags live in `settings_for()` in `snapshot_tests.rs`). Plain snapshots contain significant trailing spaces — don't strip them

### Linting & Formatting
```bash
cargo fmt                                   # Format code
cargo clippy --all-targets -- -D warnings   # Lint with warnings as errors
```

## Milestones

### M1: Foundation
- [x] Project scaffolding (Cargo.toml, module structure)
- [x] CLI argument parsing with clap
- [x] Basic markdown parsing with pulldown-cmark
- [x] Terminal dimension detection

### M2: Core Rendering
- [x] Headings, bold, italic, code (inline + block)
- [x] Lists (ordered, unordered, nested)
- [x] Block quotes
- [x] Horizontal rules
- [x] Word wrapping

### M3: Pager
- [x] Raw mode terminal control
- [x] Page-at-a-time display with spacebar
- [x] Status line
- [x] Navigation (up/down/page up/page down/home/end)
- [x] Clean exit on q/Esc and signal handling

### M4: Polish
- [x] Links and link display
- [x] Tables with box-drawing
- [x] Task lists
- [x] Pipe detection and NO_COLOR support
- [x] Stdin input
- [x] Theme support (dark/light/none)

### M5: Stretch
- [x] Search (/ and n)
- [ ] Syntax highlighting in code blocks (tree-sitter or syntect)
- [ ] Config file (~/.config/markterm/config.toml)
