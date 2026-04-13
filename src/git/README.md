# git/

Git integration. Provides a diff panel that shows uncommitted changes.

## Files

| File | Purpose |
|---|---|
| `mod.rs` | Module exports. Re-exports `GitDiffPanel`. |
| `git_repo.rs` | `GitRepo` — helper for running git commands. Wraps `git diff`, `git status`, etc. Returns structured data (file list, diff hunks). |
| `diff_model.rs` | `DiffHunk` and `FileDiff` — data models for parsed git diffs. A `FileDiff` contains the file path and a list of `DiffHunk` entries with added/removed lines. |
| `diff_view.rs` | `GitDiffPanel` entity — renders the diff panel UI. Shows a list of changed files; clicking a file expands its diff hunks with color-coded added (green) / removed (red) lines. Has a `refresh()` method to re-run `git diff`. |

## Patterns

- `GitDiffPanel` is toggled via Cmd+Shift+G. When shown, it calls `refresh()` which runs git commands and parses output.
- The panel renders as a right-side column in the main app layout.
- Diff parsing is synchronous (runs `git diff` via `std::process::Command`).
