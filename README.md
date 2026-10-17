# markterm

A lightweight command-line tool that renders markdown files with VT100/ANSI terminal formatting and displays them in a built-in `more`-style pager.

## Features

- **Markdown rendering** — headings, bold, italic, strikethrough, inline code, code blocks, lists, tables, block quotes, links, horizontal rules, task lists
- **Built-in pager** — page through documents with spacebar, scroll line-by-line, jump to top/bottom, search with `/`, built-in help with `?`
- **Resize aware** — re-wraps the document when the terminal is resized; uses the alternate screen so your shell output is left untouched
- **Word wrapping** — respects terminal width with proper line breaking
- **No-wrap mode** — truncate long lines with ellipsis instead of wrapping
- **Single binary** — no runtime dependencies, compiles to a standalone executable
- **Pipe-friendly** — auto-detects TTY; dumps plain, colour-free output when piped (override with `--color always`); honours [`NO_COLOR`](https://no-color.org)

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
./target/release/markterm README.md
```

### macOS PATH Setup

After installing with `cargo install`, the binary is placed in `~/.cargo/bin/`. If `markterm` isn't found, add Cargo's bin directory to your PATH:

```bash
source "$HOME/.cargo/env"
```

To make it permanent, add it to your shell profile:

```bash
# For zsh (default on macOS)
echo 'source "$HOME/.cargo/env"' >> ~/.zshrc

# For bash
echo 'source "$HOME/.cargo/env"' >> ~/.bash_profile
```

Then restart your terminal or run `source ~/.zshrc`.

## Usage

```
markterm [OPTIONS] [FILE]

Arguments:
  [FILE]    Markdown file to display (use - for stdin; defaults to stdin when it is piped)

Options:
  -w, --width <COLS>     Override terminal width (disables re-wrapping on resize)
  -t, --theme <THEME>    Color theme: auto, dark, light, none (default: auto)
      --color <WHEN>     When to use colour: auto, always, never (default: auto)
      --no-pager         Dump rendered output to stdout without paging
      --no-wrap          Truncate long lines with ellipsis instead of wrapping
  -h, --help             Show help
  -V, --version          Show version
```

### Colour

Colour and text attributes are emitted only when all of these hold:

- `--theme` is not `none`,
- the `NO_COLOR` environment variable is unset or empty, and
- `--color always` is given, or `--color auto` (the default) and stdout is a terminal.

So `markterm doc.md > out.txt` produces plain text, while
`markterm --color always doc.md | less -R` keeps the formatting.

Input that is not valid UTF-8 is displayed with replacement characters (`�`)
rather than rejected.

### Examples

```bash
# View a markdown file with paging
markterm README.md

# Pipe from stdin (the - is optional when stdin is piped)
cat README.md | markterm
cat README.md | markterm -

# Dump without pager (useful for piping)
markterm --no-pager README.md

# Keep colours when piping into another pager
markterm --color always README.md | less -R

# Override terminal width
markterm -w 60 README.md

# Truncate long lines instead of wrapping
markterm --no-wrap README.md
```

If the document fits on one screen it is printed directly, without starting the pager.

### Exit status

| Status | Meaning |
|--------|---------|
| `0` | Success (including when the output pipe is closed early, e.g. `\| head`) |
| `1` | The input could not be read, or a terminal error occurred |
| `2` | Invalid command-line usage |
| `130` | Quit with `Ctrl-C` |
| `128+N` | Terminated by signal N (e.g. `143` for SIGTERM); the terminal is restored first |

## Pager Controls

| Key | Action |
|-----|--------|
| `Space` / `f` / `z` / `PageDown` / `Ctrl-F` | Next page |
| `b` / `w` / `PageUp` / `Ctrl-B` | Previous page |
| `Enter` / `j` / `e` / `Down` / `Ctrl-E` | Next line |
| `k` / `y` / `Up` / `Ctrl-Y` | Previous line |
| `d` / `Ctrl-D` | Half page down |
| `u` / `Ctrl-U` | Half page up |
| `g` / `<` / `Home` | Go to top (`<n>g`: go to line *n*) |
| `G` / `>` / `End` | Go to bottom (`<n>G`: go to line *n*) |
| `<n>%` / `<n>p` | Go to *n* percent through the document |
| `/pattern` `Enter` | Search forward |
| `n` / `N` | Next / previous match (wraps around) |
| `h` / `?` | Show help (any key returns) |
| `Ctrl-L` / `r` | Redraw the screen |
| `Ctrl-Z` | Suspend to the shell (resume with `fg`) |
| `q` / `Q` / `Esc` | Quit |
| `Ctrl-C` | Quit with exit status 130 |

A number typed before a movement key repeats it, e.g. `10j` scrolls down ten
lines and `3 ` (3, Space) moves three pages. The mouse wheel scrolls the pager
in terminals that support alternate-scroll mode.

**Search** is a plain substring match on the displayed text, case-insensitive
unless the pattern contains an uppercase letter (smart case). Matches are
highlighted in reverse video. In the search prompt, `Backspace` edits, `Esc`
cancels, and an empty pattern repeats the previous search.

The status line shows the file name, the visible line range and the position
through the document, or `(END)` at the bottom.

## Rendering

markterm converts markdown elements to styled terminal output using ANSI/VT100 escape codes:

- **H1** — bold, underlined, bright white
- **H2** — bold, bright cyan
- **H3** — bold, bright yellow
- **H4–H6** — bold
- **Bold/Italic/Strikethrough** — native ANSI attributes
- **Inline code** — reverse video background
- **Code blocks** — bordered with box-drawing characters
- **Lists** — `•` `◦` `▪` bullets for unordered, numbered for ordered
- **Tables** — full box-drawing borders with bold headers
- **Block quotes** — `│` left border
- **Task lists** — `[✓]` / `[ ]` with color
- **Links** — text with URL shown in parentheses

## License

MIT
