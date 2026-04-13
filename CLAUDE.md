# CLAUDE.md

## Build & Run

- `cargo build` to build
- `cargo run` to run
- This is a GPUI-based macOS desktop app (Rust)

## Code Conventions

- Don't invent new patterns. Use existing vetted Rust crates when possible.
- Don't hand-roll logic that a dependency already provides. If a library exposes an iterator, renderer, or data structure for a task, use it instead of reimplementing the same thing with lower-level access. Custom code on top of a library should be minimal glue, not a parallel implementation.
- If no crate exists, check how https://github.com/zed-industries/zed handles it — they use the same GPUI framework and Alacritty terminal backend. Replicate their approach rather than rolling a custom solution.
- Terminal emulation uses `alacritty_terminal`. For mouse/scroll/input handling, follow Zed's `crates/terminal/` and `crates/terminal_view/` as reference implementations.
