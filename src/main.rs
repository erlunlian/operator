#![allow(dead_code)]

mod actions;
mod app;
mod command_center;
mod editor;
mod git;
mod pane;
mod recent_projects;
mod session;
mod settings;
mod tab;
mod terminal;
mod text_input;
mod theme;
mod util;
mod workspace;

use gpui::*;

use crate::actions::*;
use crate::app::OperatorApp;

fn main() {
    env_logger::init();

    Application::new().run(|cx: &mut App| {
        // Load bundled icon font (Nerd Font Symbols)
        let icon_font = std::borrow::Cow::Borrowed(
            include_bytes!("../resources/icons.ttf").as_slice(),
        );
        if let Err(e) = cx.text_system().add_fonts(vec![icon_font]) {
            log::error!("Failed to load icon font: {e}");
        }

        crate::settings::AppSettings::init(cx);

        cx.bind_keys([
            KeyBinding::new("cmd-n", NewWorkspace, None),
            KeyBinding::new("cmd-t", NewTab, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            KeyBinding::new("cmd-d", SplitPane, None),
            KeyBinding::new("cmd-shift-d", SplitPaneVertical, None),
            KeyBinding::new("cmd-b", ToggleSidebar, None),
            KeyBinding::new("cmd-shift-g", ToggleDiffPanel, None),
            KeyBinding::new("ctrl-tab", NextTab, None),
            KeyBinding::new("ctrl-shift-tab", PrevTab, None),
            KeyBinding::new("cmd-e", NewEditorTab, None),
            KeyBinding::new("cmd-s", SaveFile, Some("FileEditor")),
            KeyBinding::new("cmd-f", FindInFile, Some("FileEditor")),
            KeyBinding::new("cmd-shift-f", SearchWorkspace, None),
            KeyBinding::new("cmd-,", ToggleSettings, None),
            KeyBinding::new("cmd-k", ToggleCommandCenter, None),
            KeyBinding::new("cmd-w", crate::settings::settings_panel::CloseSettingsWindow, Some("SettingsPanel")),
        ]);

        let saved = crate::session::load_session();

        let window_bounds = saved
            .as_ref()
            .and_then(|s| {
                match (
                    s.settings.window_x,
                    s.settings.window_y,
                    s.settings.window_width,
                    s.settings.window_height,
                ) {
                    (Some(x), Some(y), Some(w), Some(h)) if w > 100.0 && h > 100.0 => {
                        Some(WindowBounds::Windowed(Bounds {
                            origin: point(px(x), px(y)),
                            size: size(px(w), px(h)),
                        }))
                    }
                    _ => None,
                }
            })
            .unwrap_or_else(|| {
                WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1200.0), px(800.0)),
                    cx,
                ))
            });

        cx.open_window(
            WindowOptions {
                window_bounds: Some(window_bounds),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|cx| {
                    if let Some(state) = saved {
                        state.restore(cx)
                    } else {
                        OperatorApp::new(cx)
                    }
                })
            },
        )
        .unwrap();
    });
}
