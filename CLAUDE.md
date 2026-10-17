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
- Full TUI application (no mouse support, no file browser)
- Inline image rendering
- HTML passthrough rendering
- Competing with glow's feature set — focus on simplicity and speed

## Technology Stack

- **Language**: Rust (2021 edition)
- **Markdown parsing**: `pulldown-cmark` (CommonMark compliant)
- **Terminal interaction**: `crossterm` (cross-platform terminal manipulation)
- **CLI argument parsing**: `clap` (derive-based)
- **Build**: Cargo, targeting Linux primarily (macOS/BSDs as secondary)

## Architecture

```
src/
├── main.rs           # Entry point, CLI arg parsing, orchestration
├── parser.rs         # Markdown parsing wrapper around pulldown-cmark
├── renderer.rs       # Converts parsed markdown events to ANSI-formatted lines
├── pager.rs          # more-style pager: raw mode, input handling, page display
├── style.rs          # ANSI/VT100 escape code definitions and style management
└── terminal.rs       # Terminal capability detection and dimension queries
```

### Data Flow

```
File → pulldown-cmark events → renderer (styled lines) → pager (page-at-a-time display)
```

1. **main.rs** parses CLI args, reads the input file (or stdin), passes content to the parser
2. **parser.rs** wraps pulldown-cmark, producing a stream of markdown events
3. **renderer.rs** consumes events and produces a `Vec<String>` of ANSI-styled, word-wrapped lines
4. **pager.rs** takes the rendered lines and displays them one terminal-page at a time, handling user input (spacebar, enter, q, arrows, etc.)

## Markdown Rendering Spec

### Supported Elements & Formatting

| Element | VT100 Rendering |
|---------|----------------|
| **H1** | Bold + Underline + bright white, preceded by blank line |
| **H2** | Bold + bright cyan, preceded by blank line |
| **H3** | Bold + yellow, preceded by blank line |
| **H4–H6** | Bold, preceded by blank line |
| **Bold** | `\e[1m` (bold/bright) |
| **Italic** | `\e[3m` (italic) or `\e[4m` (underline) as fallback |
| **Strikethrough** | `\e[9m` (strikethrough) |
| **Inline code** | `\e[7m` (reverse video) or colored background |
| **Code blocks** | Indented, with dim border, syntax name shown if present |
| **Block quotes** | `│` left border in dim/gray, indented text |
| **Unordered list** | `•` bullet, nested with `◦` and `▪`, indented per level |
| **Ordered list** | Numbered `1.`, `2.`, etc., indented per level |
| **Horizontal rule** | `─` repeated across terminal width |
| **Links** | Text shown with underline, URL in dim parentheses after |
| **Tables** | Box-drawing characters for borders, header row bold |
| **Task lists** | `[✓]` / `[ ]` with color |

### Word Wrapping
- Wrap to terminal width minus a small margin (2 columns)
- Respect indentation for nested elements
- Break on word boundaries; fall back to character break for long words/URLs

## Pager Behavior

### Display
- Show one screenful of rendered lines at a time (terminal height - 1 for status line)
- Status line at bottom: filename, line position, percentage through document
- Status line styled with reverse video

### Key Bindings
| Key | Action |
|-----|--------|
| `Space` | Next page |
| `Enter` / `Down` | Next line |
| `b` / `PageUp` | Previous page |
| `Up` | Previous line |
| `g` / `Home` | Go to top |
| `G` / `End` | Go to bottom |
| `q` / `Esc` | Quit |
| `/` | Search forward (stretch goal) |
| `n` | Next search match (stretch goal) |

### Raw Mode
- Enter raw terminal mode on start, restore on exit (including on SIGINT/SIGTERM)
- Disable line buffering and echo during pager operation

## CLI Interface

```
markterm [OPTIONS] <FILE>

Arguments:
  <FILE>    Markdown file to display (use - for stdin)

Options:
  -w, --width <COLS>     Override terminal width
  -t, --theme <THEME>    Color theme: auto, dark, light, none (default: auto)
      --no-pager         Dump rendered output to stdout without paging
  -h, --help             Show help
  -V, --version          Show version
```

### Stdin Support
- `cat README.md | markterm -` or `markterm - < README.md`
- When reading from stdin, buffer all input before rendering

### Pipe Detection
- If stdout is not a TTY (piped), output rendered text without pager and without interactive controls
- Strip ANSI codes when piped if `--theme none` or `NO_COLOR` env var is set

## Development

### Build & Run
```bash
cargo build                        # Debug build
cargo build --release              # Release build
cargo run -- README.md             # Run on a file
cargo run -- --no-pager README.md  # Dump without paging
```

### Testing Strategy
- **Unit tests**: Parser event handling, renderer output for each element type, word wrapping
- **Integration tests**: Full pipeline from markdown string to rendered output (non-interactive)
- **Snapshot tests**: Compare rendered output against expected baselines for sample .md files
- Place sample markdown files in `tests/fixtures/`

### Linting & Formatting
```bash
cargo fmt                  # Format code
cargo clippy -- -D warnings  # Lint with warnings as errors
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
