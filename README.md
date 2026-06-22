# ddg-spotlight

A macOS-Spotlight-style **search launcher for the terminal**, built for
Omarchy / Hyprland. Hit a keybind and a small bordered search bar appears,
floating over your normal (un-dimmed, un-blurred) desktop. It has **three modes**,
chosen by a leading sigil:

- **(nothing)** — **web search** via DuckDuckGo: live suggestions as you type,
  then `Enter` grows the card into an instant answer + a Google-style ranked
  list of web links.
- **`@`** — **file search**: blazing-fast fuzzy search of files under your home.
- **`/`** — **folder search**: same, for directories.

It reads the **active Omarchy theme** at runtime
(`~/.config/omarchy/current/theme/colors.toml`), so it always matches the rest
of your TUIs and re-colors when you switch themes.

## Features

- **Spotlight overlay**: a thin-bordered card that floats over the undimmed,
  un-blurred desktop and **grows** as you type and search.
- **Three modes via sigils** (`@` files, `/` folders, otherwise web), shown as a
  mode icon in the search bar.
- **Fast web suggestions** — a single reused HTTPS client (no per-keystroke TLS
  setup) plus a short debounce.
- **Ranked web links** inline — real titles, domains and snippets from the
  DuckDuckGo HTML endpoint — alongside an **instant answer** block.
- **Blazing-fast local search** — an in-memory index of `$HOME` (built once with
  ripgrep's `ignore` walker, skipping hidden / `.gitignore`d / heavy dirs like
  `node_modules`, `target`, `site-packages`, caches) fuzzy-matched on every
  keystroke. ~90k entries index in ~0.1 s; filtering is then instant.
- **Open anything**: `Enter` (or `1`–`9` for web links) opens the highlighted
  result — web links in the browser, files/folders with `xdg-open`.

## Keys

| Key              | Action                                                       |
|------------------|--------------------------------------------------------------|
| *(type)*         | Edit the query. `@`/`/` at the start switch to file/folder mode |
| `↑` / `↓`        | Move through suggestions / web links / local results         |
| `Tab`            | Complete to the highlighted suggestion (web) / next result   |
| `Enter`          | Search (web) · open the highlighted link / file / folder     |
| `1`–`9`          | Open the matching **web** result directly                    |
| `Esc`            | Close web results (back to typing), or quit                  |
| `Ctrl-U`         | Clear the query                                              |
| `Ctrl-C`         | Quit                                                         |

## Install

Requires a Rust toolchain and `xdg-open` (both present on Omarchy).

```bash
cargo build --release
mkdir -p ~/.local/bin
cp target/release/ddg-spotlight ~/.local/bin/
```

Sanity checks without the TUI:

```bash
~/.local/bin/ddg-spotlight --query "rust programming language"   # web
~/.local/bin/ddg-spotlight --files cargo                         # files
~/.local/bin/ddg-spotlight --folders projects                    # folders
```

## Hyprland setup (the "Spotlight" part)

The card is drawn by the app; the floating, sharp, un-dimmed overlay is done
with Hyprland rules + an Alacritty config. Copy the snippet from
[`hypr-ddg-spotlight.conf`](./hypr-ddg-spotlight.conf):

- the keybind line → `~/.config/hypr/bindings.conf`
- the `windowrule` lines → `~/.config/hypr/looknfeel.conf` (or `hyprland.conf`)

How the look works:

- The window is a full-screen float (`size 2560 1440`, `move 0 0`) with
  **`no_blur on`** — so there's no Hyprland blur.
- Alacritty `window.opacity = 0` makes the *default* background fully
  transparent, so the desktop shows through the margins **undimmed and sharp**.
  The card paints an explicit background (`≠` the default), so it stays opaque.
  Want a slight dim behind the card? Raise `opacity` in
  `~/.config/alacritty/ddg-spotlight.toml` — the card stays opaque regardless.

Then reload Hyprland:

```bash
hyprctl reload
```

Press **`Super + S`** to summon the search. (`Super + Shift + S` is taken by
screenshots on Omarchy, so this uses plain `Super + S`.)

## How it works

```
main.rs    event loop, raw-mode RAII guard, mode-aware key handling, xdg-open
app.rs     pure state: Mode + sigil parsing, query, suggestions, links, local results
search.rs  background workers + debounce (web search, autocomplete, local search)
ddg.rs     shared client + Instant Answer API + autocomplete + HTML link scraping
local.rs   ignore-crate index of $HOME + SkimMatcherV2 fuzzy file/folder search
theme.rs   parse Omarchy colors.toml -> ratatui colors (with fallback)
ui.rs      transparent-margin overlay + centered growing card + per-mode rendering
```

A web search runs two requests concurrently (`ddg::fetch_all`): the Instant
Answer API for the abstract, and the no-JS HTML endpoint for the ranked links
(redirect URLs are decoded; ads / internal links dropped). Local search builds
the `$HOME` index lazily on the first `@`/`/` query, then filters it in memory.

Every web/local request is tagged with an incrementing *generation* id so
responses from out-of-date keystrokes are discarded.

## Notes / limitations

- The ranked web links come from scraping the DuckDuckGo **HTML** endpoint
  (no API key). If links ever stop appearing, the selectors in
  `ddg::parse_web_results` are the place to look.
- The local index is rebuilt per launch (lazily, on the first local query). To
  exclude more noise from local search, edit `DENY_DIRS` in `local.rs`.
- Network/HTTP errors are shown inline in the web results panel.
