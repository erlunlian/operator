# Operator

A native macOS code editor built from scratch in Rust with [GPUI](https://gpui.rs) — the GPU-accelerated UI framework from [Zed](https://zed.dev).

![macOS](https://img.shields.io/badge/platform-macOS-lightgrey)
![Rust](https://img.shields.io/badge/language-Rust-orange)
![License](https://img.shields.io/badge/license-MIT-blue)

## Features

- **Code editor** with syntax highlighting (tree-sitter), line numbers, search, undo/redo
- **Integrated terminal** with full PTY support, ANSI colors, and Mac keyboard shortcuts
- **Git diff panel** with staged/unstaged sections, stage/unstage/revert actions, and real-time filesystem watching
- **Split panes** with drag-to-split tabs and resizable dividers
- **Multi-workspace** support with a sidebar for switching between projects
- **Command palette** for quick actions
- **Session persistence** — remembers open workspaces, tabs, splits, and window state across restarts
- **Catppuccin Mocha** theme

## Requirements

- macOS 13+
- Rust toolchain (`rustup`)
- `cargo-watch` (for dev mode): `cargo install cargo-watch`

## Quick Start

```bash
git clone https://github.com/erlunlian/operator.git
cd operator
make dev
```

## Usage

| Command | Description |
|---------|-------------|
| `make dev` | Build and run with auto-reload on save |
| `make run` | Single build and run |
| `make release` | Build optimized `.app` bundle |
| `make open` | Build release and open the app |
| `make install` | Build release and copy to `/Applications` |
| `make clean` | Remove all build artifacts |

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Cmd+T` | New tab |
| `Cmd+W` | Close tab |
| `Cmd+N` | New workspace |
| `Cmd+P` | Command palette |
| `Cmd+B` | Toggle sidebar |
| `Cmd+G` | Toggle git diff panel |
| `Cmd+\` | Split pane |
| `Cmd+1-9` | Switch to tab N |
| `Ctrl+Tab` | Next tab |

## Architecture

```
src/
  app.rs              — Top-level app, layout, resize handles
  main.rs             — Entry point, window setup
  session.rs          — Session save/restore
  editor/             — File tree, tabbed file viewer, syntax highlighting
  terminal/           — PTY-backed terminal emulator, ANSI parsing
  git/                — Git diff panel, staging, file watching
  workspace/          — Workspace model, sidebar
  pane/               — Recursive split pane system, drag-to-split
  tab/                — Tab model and tab bar
  theme/              — Color palette (Catppuccin Mocha)
  settings/           — Settings panel
  command_center/     — Command palette
```

Built on:
- **[GPUI](https://gpui.rs)** — GPU-accelerated UI framework
- **[tree-sitter](https://tree-sitter.github.io)** — Syntax highlighting
- **[alacritty_terminal](https://github.com/alacritty/alacritty)** — Terminal emulation
- **[git2](https://github.com/rust-lang/git2-rs)** — Git operations
- **[notify](https://github.com/notify-rs/notify)** — Filesystem watching

## Contributing

Contributions welcome! Fork the repo and open a PR.

## License

MIT
