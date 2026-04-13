# theme/

Color palette for the UI. Uses catppuccin mocha as the base theme.

## Files

| File | Purpose |
|---|---|
| `mod.rs` | Module export for `colors`. |
| `colors.rs` | `colors` module — provides named color functions: `bg()`, `surface()`, `surface_hover()`, `border()`, `text()`, `text_muted()`, `accent()`. Each returns an `Hsla` color value. All UI components import from here for consistent theming. |

## Patterns

- Colors are functions, not constants, so they can be called in any rendering context.
- To change the theme, modify the color values in `colors.rs`. All UI components reference these functions, so changes propagate everywhere.
- Syntax highlighting colors are separate — defined in `editor/syntax.rs` using catppuccin mocha palette values directly.
