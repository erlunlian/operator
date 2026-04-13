# workspace/

Workspace model and sidebar. A workspace represents an open project directory with its own pane layout.

## Files

| File | Purpose |
|---|---|
| `mod.rs` | Module exports. Re-exports `Workspace` and `WorkspaceSidebar`. |
| `workspace.rs` | `Workspace` entity — owns a project directory, a `PaneGroup` layout, a name, and git branch info. Methods: `new()` (with directory), `new_empty()` (welcome screen), `set_directory()`, `add_tab()`, `add_editor_tab()`, `close_tab()`, `split_active_pane()`. Watches the git branch in a background task. |
| `sidebar.rs` | `WorkspaceSidebar` — stateless rendering helper. Renders a list of workspace cards showing name, directory, git branch, and Claude status indicator. Includes a "New Workspace" button at the bottom. Uses `Rc<dyn Fn(...)>` callbacks for workspace selection and creation. |

## Patterns

- `OperatorApp` owns `Vec<Entity<Workspace>>`. The sidebar renders cards for each workspace; clicking one sets `active_workspace_ix`.
- Each workspace has its own independent `PaneGroup` layout, so split/tab state is per-workspace.
- `WorkspaceSidebar` is purely functional — it takes data (`Vec<WorkspaceCardData>`) and callbacks, renders them, and returns a div. No entity needed.
- Git branch is polled via `git rev-parse --abbrev-ref HEAD` on a background executor, updated every few seconds.
