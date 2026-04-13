# editor/

The file editor module. Provides a composite editor tab with a file tree sidebar and tabbed file viewers.

## Files

| File | Purpose |
|---|---|
| `mod.rs` | Module exports. Re-exports `EditorView` and `FileViewer`. |
| `editor_view.rs` | `EditorView` entity — the main editor container. Renders a split layout: collapsible file tree on the left, sub-tab bar + file content on the right. Manages open files as `OpenFile` structs (each wrapping an `Entity<FileViewer>`). Handles file open/close, tab switching, tree resize via mouse drag. |
| `file_tree.rs` | `FileTree` struct (not an entity — owned inline by `EditorView`). Walks the filesystem lazily (only expanded dirs). Maintains `expanded_dirs: HashSet<PathBuf>` for collapse/expand. Renders indented tree with chevron icons. Filters out `.git`, `target`, `node_modules`, etc. |
| `file_viewer.rs` | `FileViewer` entity — the core text editor. Handles: line-based text buffer, cursor movement, selection, undo/redo, syntax highlighting, line numbers, virtual scrolling (`uniform_list`). Also provides: Cmd+F search with match highlighting, Cmd+click go-to-definition, double-click word highlight, copy/cut/paste. This is the largest file (~2100 lines) since GPUI has no built-in editor widget. |
| `syntax.rs` | Syntax highlighting via tree-sitter. `highlight_source(code, extension)` returns `Vec<HighlightSpan>` with byte ranges and colors. Supports Rust and JSON. Maps AST node types to catppuccin mocha colors. |

## Patterns

- `EditorView` owns a `FileTree` and a `Vec<OpenFile>`. When the user clicks a file in the tree, a callback fires `EditorView::open_file()`, which either switches to an existing tab or creates a new `Entity<FileViewer>`.
- `FileViewer` is fully hand-rolled (cursor, selection, scroll, key handling) because GPUI provides no built-in multi-line text editor. It uses `uniform_list` for virtual scrolling and absolute-positioned overlays for cursor/selection/highlights.
- Sub-tabs within `EditorView` are a simplified tab bar (no drag support) to avoid conflicting with the outer pane system's `TabDragPayload`.
