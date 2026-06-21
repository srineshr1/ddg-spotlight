# AGENTS.md

## Commands

```
cargo build --release          # optimized build (opt-level=z, lto, stripped)
cargo test                     # all unit tests
ddg-spotlight --query "rust"   # CLI debug: fetch and print result, no TUI
```

## Architecture

Single binary crate — 6 source files, each purpose-specific:

| File | Role |
|------|------|
| `main.rs` | crossterm event loop, `Tui` RAII guard, key dispatch, `xdg-open` |
| `app.rs` | Pure state + transitions (testable without IO) |
| `search.rs` | Background worker thread + debouncer (generation-based staleness) |
| `ddg.rs` | DuckDuckGo Instant Answer API client + JSON types |
| `theme.rs` | Parse `~/.config/omarchy/current/theme/colors.toml` → ratatui colors |
| `ui.rs` | Centered card layout + rendering |

## Gotchas

- **Binary name is `ddg-spotlight`, not `search`.** The repo directory is `search` but the crate and binary are `ddg-spotlight`.
- **reqwest is blocking, not async.** Dependencies use `reqwest` with `features = ["blocking", …]`. The search worker spawns a thread that calls `reqwest::blocking::Client`. Do not introduce async code or tokio.
- **TLS is rustls.** No `native-tls` / OpenSSL dependency.
- **Release profile strips and size-optimizes.** `opt-level = "z"`, `lto = true`, `strip = true`.
- **`Tui` struct is a RAII guard.** It enables raw mode + alternate screen in `new()` and restores the terminal in `Drop`, even on panic. The `run()` function explicitly drops it before returning.
- **SearchWorker has a testable constructor.** Use `SearchWorker::spawn_with(fetch_fn)` in tests instead of real network calls.
- **Theme file is optional.** On any read/parse failure, `Theme::load()` silently falls back to a hardcoded default palette.
- **The `--query` / `-q` flag** exits early with a printed result, skipping the TUI entirely.
