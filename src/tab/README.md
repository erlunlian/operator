# tab/

Tab model and tab bar rendering.

## Files

| File | Purpose |
|---|---|
| `mod.rs` | Module exports. |
| `tab.rs` | `Tab` entity — the tab data model. Each tab has a `title: SharedString` and a `TabContent` enum variant: `Terminal(Entity<TerminalView>)` or `Editor(Entity<EditorView>)`. Constructors: `Tab::new()` (terminal), `Tab::new_editor()`. The `render_content()` method returns the active content as an `AnyElement`. |
| `tab_bar.rs` | `TabBar` — stateless rendering helper for the tab bar strip. Renders tab titles with close buttons and a "+" new tab button. Used by `pane_group.rs` when rendering leaf nodes. |

## Patterns

- `Tab` is an entity (`Entity<Tab>`) so it can be passed around, cloned, and read from different contexts.
- `TabContent` is the enum that determines what a tab displays. Adding a new tab type means adding a variant here and wiring up its constructor and `render_content()` arm.
- Tab bar rendering is separate from the tab model — `pane_group.rs` handles the actual tab bar UI inline (with drag support), while `tab_bar.rs` provides a simpler reusable version.
