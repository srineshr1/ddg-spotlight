# AGENTS.md

## Commands

```
cargo build --release            # optimized build (opt-level=z, lto, stripped)
cargo test                       # all unit tests (61)
cargo clippy --all-targets       # lints
ddg-spotlight --query "rust"     # CLI debug: web answer + links, no TUI
ddg-spotlight --files cargo      # CLI debug: local file search, no TUI
ddg-spotlight --folders projects # CLI debug: local folder search, no TUI
```

> If crate downloads fail with an HTTP/2 framing error, prefix cargo with
> `CARGO_HTTP_MULTIPLEXING=false`.

## Architecture

Single binary crate — 7 source files, each purpose-specific:

| File | Role |
|------|------|
| `main.rs` | crossterm event loop, `Tui` RAII guard, mode-aware key dispatch, `xdg_open` |
| `app.rs` | Pure state + transitions: `Mode` (Web/Folder/File), sigil parsing, web + local results |
| `search.rs` | Background workers: `SearchWorker` (web `fetch_all`) + `SuggestionWorker` (autocomplete) + `LocalSearchWorker` (file/folder) + debouncer |
| `ddg.rs` | Shared `Client` + Instant Answer API + autocomplete (`/ac/`) + HTML link scraping (`scraper`) + `fetch_all` |
| `local.rs` | `ignore`-crate index of `$HOME` + `SkimMatcherV2` fuzzy search (File/Folder modes) |
| `theme.rs` | Parse `~/.config/omarchy/current/theme/colors.toml` → ratatui colors |
| `ui.rs` | Transparent-margin overlay + centered growing card (per-mode: suggestions / answer+links / local results) |

## Modes (leading sigil)

- **(no sigil)** → **Web**: DuckDuckGo autocomplete → Enter runs `fetch_all`
  (answer + ranked links). `1`–`9` open a link; the sigil-less query is the term.
- **`@`** → **File**: live fuzzy search of files under `$HOME`. Enter opens the
  highlighted file with `xdg-open`. Digits are part of the term (no quick-open).
- **`/`** → **Folder**: same, for directories.
- The sigil is shown as a **mode icon**, not as typed text (`App::term()` strips it).

## Search flow

- **Typing** is debounced (`DEBOUNCE` = 120 ms). On fire, `main` dispatches by
  mode: web → `SuggestionWorker`; local → `LocalSearchWorker`.
- **Up/Down/Tab** navigate the active list (web suggestions, web links, or local
  results); **Enter** searches (web) or opens (local / selected web link).
- Generation ids tag every web/local request so stale responses are discarded.

## Gotchas

- **Binary name is `ddg-spotlight`, not `search`.** The repo dir is `search` but the crate/binary are `ddg-spotlight`.
- **One shared HTTP client.** `ddg::build_client()` is built once per worker and reused (reqwest pools connections). Do **not** build a client per request — that TLS/cert setup was the old cause of slow suggestions. Per-request timeouts via `.timeout()`.
- **reqwest is blocking, not async.** rustls TLS. No tokio.
- **HTML scraping uses `scraper`.** `parse_web_results` selects `div.result`, `a.result__a`, `.result__snippet`; skips `result--ad` + internal `duckduckgo.com` links and decodes the `uddg` redirect (`decode_ddg_href` + `percent_decode`). Needs a browser `User-Agent` (`BROWSER_UA`).
- **Local index = `ignore::WalkBuilder` (parallel) over `$HOME`.** Skips hidden + `.gitignore`d files and a `DENY_DIRS` denylist (node_modules, target, dist, build, venv, __pycache__, vendor, .git, .cache, site-packages). `max_depth 14`, `MAX_INDEX 200k`. Built **lazily on the first local query** in `LocalSearchWorker`, then filtered in-memory (instant). To prune more noise, edit `DENY_DIRS`.
- **Fuzzy ranking** = `SkimMatcherV2` on the entry *name*, sorted by score → shallower `path_depth` → shorter name.
- **`fetch_all` only errors if *both* requests fail.** Missing answer still yields links and vice-versa.
- **Release profile strips + size-optimizes.** `opt-level="z"`, `lto=true`, `strip=true`. ~5M with `scraper` + `ignore`.
- **`Tui` is a RAII guard** (raw mode + alt screen in `new`, restored in `Drop`).
- **Workers have testable constructors** (`spawn_with`). Tests pass fake fetch fns.
- **Theme file is optional**; `Theme::load()` falls back to a built-in palette.

## The overlay (Spotlight look)

- **Full-screen, fully transparent overlay window.** The Hyprland rules in
  `hypr-ddg-spotlight.conf` size the window to the monitor (`size 2560 1440`,
  `move 0 0`) with **`no_blur on`** and `border_size 0`.
- **Alacritty `window.opacity = 0` + `colors.primary.background = "#000000"`.**
  Opacity only affects the *default* background, so the margins are fully
  transparent (the desktop shows through, undimmed and sharp) while the card —
  which paints an explicit `#1f1f28` background ≠ `#000000` — stays **opaque**.
  Raising `opacity` adds a dim; the card stays opaque regardless.
- **Card geometry is pure + tested.** `compute_card_rect` / `card_width` /
  `inner_height` size the card from the state (small bar → suggestions → results
  → local list) and keep it on-screen.
- Reload Hyprland rules with `hyprctl reload`; the Alacritty config applies on
  next window launch.
