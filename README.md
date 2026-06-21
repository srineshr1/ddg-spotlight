# ddg-spotlight

A macOS-Spotlight-style **web search launcher for the terminal**, built for
Omarchy / Hyprland. Hit a keybind, a small floating box appears in the centre of
your screen, you type, and you get live DuckDuckGo instant answers — press
`Enter` to open the full results (or a selected topic) in your browser.

It reads the **active Omarchy theme** at runtime
(`~/.config/omarchy/current/theme/colors.toml`), so it always matches the rest
of your TUIs and automatically re-colors when you switch themes.

## Features

- **Live, debounced search** as you type (DuckDuckGo Instant Answer API — no
  scraping, ToS-friendly).
- **Inline preview**: heading, abstract/definition, source, and a scrollable
  list of related topics.
- **Browser hand-off**: `Enter` opens the selected topic, or the full
  `duckduckgo.com` results page for your query, via `xdg-open`.
- **Omarchy theming** at runtime, with a built-in fallback palette.
- **Spotlight window** via a Hyprland float rule + keybind.

## Keys

| Key            | Action                                              |
|----------------|-----------------------------------------------------|
| *(type)*       | Edit the query (live search after a short pause)    |
| `↑` / `↓`      | Select a related topic                              |
| `Enter`        | Open selected topic, or full DDG results, in browser|
| `Ctrl-U`       | Clear the query                                     |
| `Esc` / `Ctrl-C` | Quit                                              |

## Install

Requires a Rust toolchain and `xdg-open` (both present on Omarchy).

```bash
# from the project directory
cargo build --release
mkdir -p ~/.local/bin
cp target/release/ddg-spotlight ~/.local/bin/

# make sure ~/.local/bin is on your PATH (Omarchy default shells already do this)
```

Quick sanity check without the TUI:

```bash
~/.local/bin/ddg-spotlight --query "rust programming language"
```

## Hyprland setup (the "Spotlight" part)

The compact box is drawn by the app; the *floating, centered, dimmed* window is
done with Hyprland rules. Copy the snippet from
[`hypr-ddg-spotlight.conf`](./hypr-ddg-spotlight.conf):

- the keybind line → `~/.config/hypr/bindings.conf`
- the `windowrule` lines → `~/.config/hypr/looknfeel.conf`

Then reload:

```bash
hyprctl reload
```

Now press **`Super + S`** anywhere to summon the search. (`Super + Shift + S` is
already taken by screenshots on Omarchy, so this uses plain `Super + S` — change
it in the snippet if you prefer another combo.)

> The keybind launches `alacritty --class ddg-spotlight`, and the window rules
> match `class:^(ddg-spotlight)$`. If you'd rather use Ghostty, swap the exec
> line for `ghostty --class=ddg-spotlight -e ~/.local/bin/ddg-spotlight`.

## How it works

```
main.rs    event loop, terminal raw-mode RAII guard, key handling, xdg-open
app.rs     pure state: query, generation counter, selection, status
search.rs  background worker thread + debounce (latest-wins via generation ids)
ddg.rs     Instant Answer API client + JSON types + URL helpers
theme.rs   parse Omarchy colors.toml -> ratatui colors (with fallback)
ui.rs      compact centered "card" layout + rendering
```

Searches run on a worker thread; each query is tagged with an incrementing
*generation* id so that responses from out-of-date keystrokes are discarded —
you only ever see results for what's currently in the box.

## Notes / limitations

- The DuckDuckGo Instant Answer API returns abstracts, definitions and related
  topics, **not** a ranked list of web links. That's why full web results open
  in the browser. Many queries (especially navigational ones) have no instant
  answer — in that case the box just says so and `Enter` takes you to DDG.
- Network/HTTP errors are shown inline in the status area.
