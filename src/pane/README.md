# pane/

The split pane system. Manages a recursive tree of tab groups that can be split horizontally or vertically via drag-and-drop.

## Files

| File | Purpose |
|---|---|
| `mod.rs` | Module exports. Re-exports `PaneGroup`. |
| `pane.rs` | Legacy `Pane` / `PaneContent` enum. Minimal — most logic has moved to `pane_group.rs`. |
| `pane_group.rs` | The main file (~1000 lines). Defines the core layout data structures and rendering. |

## Key types in `pane_group.rs`

- **`PaneGroup`** — Entity that owns the root `SplitNode` and tracks drop targets / focused group.
- **`SplitNode`** — Recursive enum:
  - `Leaf(TabGroup)` — a single tab group with a tab bar
  - `Split { axis, children, ratios }` — a split container with resizable children
- **`TabGroup`** — holds `tabs: Vec<Entity<Tab>>` and `active_tab_ix`. Each tab group renders its own tab bar with drag handles.
- **`SplitAxis`** — `Horizontal` (side by side) or `Vertical` (stacked).

## Patterns

- **Drag-to-split**: Tabs can be dragged between groups. Dropping on the edge of a group creates a new split. Uses `TabDragPayload` with `on_drag` / `on_drop` and directional drop zones (left/right/top/bottom/center).
- **Resize handles**: Between split children, rendered as draggable dividers. Ratios are stored as `Vec<f32>` and updated on mouse drag.
- **Recursive rendering**: `render_tree()` walks the `SplitNode` tree. Leaves render a tab bar + active tab content. Splits render children in a flex row/column with resize handles between them.
- **Tab bar**: Rendered inline in each leaf. Shows tab titles with close buttons, drag handles, and a "+" button. The active tab's content fills the remaining space below.
