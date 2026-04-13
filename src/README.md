# src/

Root of the Operator IDE source code, built on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) v0.2.2.

## Architecture

Operator follows an entity-based architecture. Each UI component is a GPUI `Entity<T>` created via `cx.new(|cx| T::new(cx))`. Entities communicate through:
- **Observers**: `cx.observe(&entity, |self, entity, cx| ...)` — react to entity changes
- **Callbacks**: `Rc<dyn Fn(...)>` — parent passes closures to children for event handling
- **Actions**: Global keybinding dispatch via `actions!()` macro and `on_action(cx.listener(...))`

## Key files

| File | Purpose |
|---|---|
| `main.rs` | Entry point. Creates the GPUI `Application`, registers global keybindings, opens the main window, restores session state, and wires global action handlers. |
| `app.rs` | `OperatorApp` — root entity. Owns workspaces, sidebar, diff panel, settings, and command center. Renders the main layout and handles top-level actions. |
| `actions.rs` | Defines all global GPUI actions (`NewTab`, `CloseTab`, `SplitPane`, etc.) using the `actions!()` macro. |
| `session.rs` | Session persistence. Captures/restores the full app state (workspaces, tabs, settings, window bounds) to `~/.config/operator/session.json`. Auto-saves every 5 seconds. |
| `recent_projects.rs` | Tracks recently opened project directories in `~/.config/operator/recent_projects.json`. |
| `text_input.rs` | `TextInput` — reusable single-line text input entity with cursor, selection, copy/cut/paste, word navigation. Used by the command center and file search bar. |

## Modules

| Directory | Purpose |
|---|---|
| `editor/` | File editor — tree browser, tabbed file viewer, syntax highlighting |
| `pane/` | Split pane system — recursive tree of tab groups with drag-to-split |
| `tab/` | Tab model and tab bar rendering |
| `terminal/` | Terminal emulator (PTY + ANSI parser + renderer) |
| `workspace/` | Workspace model (directory + layout) and sidebar |
| `git/` | Git diff panel (model + view) |
| `settings/` | App settings (global state + settings panel window) |
| `theme/` | Color palette (catppuccin mocha) |

## Data flow

```
OperatorApp
  ├── workspaces: Vec<Entity<Workspace>>
  │     └── layout: Entity<PaneGroup>
  │           └── root: SplitNode (recursive tree)
  │                 └── Leaf(TabGroup { tabs: Vec<Entity<Tab>> })
  │                       ├── TabContent::Terminal(Entity<TerminalView>)
  │                       └── TabContent::Editor(Entity<EditorView>)
  │                             ├── FileTree (inline struct)
  │                             └── open_files: Vec<OpenFile { viewer: Entity<FileViewer> }>
  ├── command_center: Entity<CommandCenter>
  ├── diff_panel: Entity<GitDiffPanel>
  └── settings_panel: Entity<SettingsPanel>
```
