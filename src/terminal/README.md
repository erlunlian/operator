# terminal/

Terminal emulator. Spawns a real PTY process and renders ANSI-colored output.

## Files

| File | Purpose |
|---|---|
| `mod.rs` | Module exports. Re-exports `TerminalView`. |
| `terminal.rs` | `Terminal` — the backend. Spawns a PTY child process (via `portable_pty`), reads output in a background thread, and maintains a line buffer with ANSI color parsing. Handles input by writing to the PTY's writer. Parses SGR escape codes for foreground/background colors (16-color, 256-color, and true color). Key struct: `StyledChar { ch, fg, bg }` and `Terminal { lines, cursor_row, cursor_col, ... }`. |
| `terminal_view.rs` | `TerminalView` entity — the renderer. Wraps an `Entity<Terminal>` and renders its line buffer as styled text. Handles keyboard input (forwards to terminal), focus management, and scroll. Uses a fixed-size visible window and a scrollbar. |

## Patterns

- `Terminal` is an entity that owns the PTY process. It spawns a reader thread that pushes output into the line buffer via `cx.update()`.
- `TerminalView` observes `Terminal` and re-renders on changes. Keyboard events are captured and sent as bytes to the PTY writer.
- ANSI parsing is done inline during output processing — each character is pushed with its current style state (set by escape codes).
- The terminal detects Claude Code status by scanning output lines for known patterns (spinner, tool use, etc.), exposed via `claude_status()`.
