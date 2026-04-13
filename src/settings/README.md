# settings/

Application settings — global state and the settings panel UI.

## Files

| File | Purpose |
|---|---|
| `mod.rs` | `AppSettings` — a GPUI `Global` struct holding app-wide settings (currently just `vim_mode: bool`). Provides `init(cx)` to register the global, and `get(cx)` / `vim_mode(cx)` accessors. Settings are persisted via the session system in `session.rs`. |
| `settings_panel.rs` | `SettingsPanel` entity — renders a settings window with toggle switches. Opens as a separate GPUI window via Cmd+,. Cmd+W closes the window. Currently has one toggle: Vim Mode. |

## Patterns

- Settings are stored as a GPUI global (`impl Global for AppSettings`), accessible from anywhere via `cx.global::<AppSettings>()`.
- To add a new setting: add a field to `AppSettings`, add a toggle in `SettingsPanel::render()`, and add the field to `SettingsState` in `session.rs` for persistence.
- The settings panel opens as its own window (not an overlay in the main window), so it has its own keybindings and focus context.
