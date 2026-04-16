#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

mod actions;
mod app;
mod command_center;
mod debug;
mod editor;
mod git;
mod pane;
mod recent_projects;
mod right_panel;
mod session;
mod settings;
mod tab;
mod terminal;
mod text_input;
mod theme;
mod ui;
mod updater;
mod util;
mod workspace;

use gpui::*;

use crate::actions::*;
use crate::app::OperatorApp;

#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use cocoa::appkit::{NSApp, NSApplication, NSImage};
    use cocoa::base::nil;
    use cocoa::foundation::NSData;
    use objc::runtime::Object;

    unsafe {
        let icon_data: &[u8] = include_bytes!("../resources/operator_icon.png");
        let ns_data: *mut Object =
            NSData::dataWithBytes_length_(nil, icon_data.as_ptr() as *const _, icon_data.len() as u64);
        let ns_image: *mut Object = NSImage::initWithData_(NSImage::alloc(nil), ns_data);
        let app = NSApp();
        app.setApplicationIconImage_(ns_image);
    }
}

/// Disable the native titlebar window drag so that interactive elements (tabs)
/// in the titlebar area can receive mouse events for drag-reorder/split.
#[cfg(target_os = "macos")]
fn disable_titlebar_drag(window: &mut gpui::Window) {
    use raw_window_handle::HasWindowHandle;
    if let Ok(handle) = window.window_handle() {
        let raw = handle.as_raw();
        if let raw_window_handle::RawWindowHandle::AppKit(appkit) = raw {
            unsafe {
                let ns_view: cocoa::base::id = appkit.ns_view.as_ptr() as _;
                let ns_window: cocoa::base::id = objc::msg_send![ns_view, window];
                let _: () = objc::msg_send![ns_window, setMovableByWindowBackground: false];
                let _: () = objc::msg_send![ns_window, setMovable: false];
            }
        }
    }
}

fn main() {
    env_logger::init();

    Application::new().run(|cx: &mut App| {
        #[cfg(target_os = "macos")]
        set_dock_icon();

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
            KeyBinding::new("cmd-e", ToggleFilesPanel, None),
            KeyBinding::new("cmd-shift-r", TogglePrPanel, None),
            KeyBinding::new("ctrl-tab", NextTab, None),
            KeyBinding::new("ctrl-shift-tab", PrevTab, None),
            KeyBinding::new("cmd-s", SaveFile, Some("FileEditor")),
            KeyBinding::new("cmd-f", FindInFile, Some("FileEditor")),
            KeyBinding::new("cmd-f", FindInFile, Some("GitDiffPanel")),
            KeyBinding::new("cmd-f", FindInFile, Some("PrDiffPanel")),
            KeyBinding::new("cmd-p", FindFile, None),
            KeyBinding::new("cmd-shift-f", SearchWorkspace, None),
            KeyBinding::new("cmd-,", ToggleSettings, None),
            KeyBinding::new("cmd-k", ToggleCommandCenter, None),
            KeyBinding::new("cmd-l", EditPrLink, None),
            KeyBinding::new("cmd-shift-l", NewPrReview, None),
            KeyBinding::new("cmd-w", crate::settings::settings_panel::CloseSettingsWindow, Some("SettingsPanel")),
            KeyBinding::new("escape", crate::settings::settings_panel::CloseSettingsWindow, Some("SettingsPanel")),
            KeyBinding::new("cmd-o", OpenDirectory, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("ctrl-shift-d", ToggleDebugPanel, None),
            KeyBinding::new("cmd-1", ActivateWorkspace1, None),
            KeyBinding::new("cmd-2", ActivateWorkspace2, None),
            KeyBinding::new("cmd-3", ActivateWorkspace3, None),
            KeyBinding::new("cmd-4", ActivateWorkspace4, None),
            KeyBinding::new("cmd-5", ActivateWorkspace5, None),
            KeyBinding::new("cmd-6", ActivateWorkspace6, None),
            KeyBinding::new("cmd-7", ActivateWorkspace7, None),
            KeyBinding::new("cmd-8", ActivateWorkspace8, None),
            KeyBinding::new("cmd-9", ActivateWorkspace9, None),
        ]);

        cx.set_menus(vec![
            Menu {
                name: "Operator".into(),
                items: vec![
                    MenuItem::action(
                        &format!("About Operator v{}", env!("CARGO_PKG_VERSION")),
                        About,
                    ),
                    MenuItem::separator(),
                    MenuItem::action("Check for Updates...", CheckForUpdates),
                    MenuItem::action("Settings...", ToggleSettings),
                    MenuItem::separator(),
                    MenuItem::os_submenu("Services", SystemMenuType::Services),
                    MenuItem::separator(),
                    MenuItem::action("Hide Operator", Hide),
                    MenuItem::action("Hide Others", HideOthers),
                    MenuItem::action("Show All", ShowAll),
                    MenuItem::separator(),
                    MenuItem::action("Quit Operator", Quit),
                ],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Window", NewWorkspace),
                    MenuItem::action("New Tab", NewTab),
                    MenuItem::separator(),
                    MenuItem::action("Open Directory...", OpenDirectory),
                    MenuItem::separator(),
                    MenuItem::action("Save", SaveFile),
                    MenuItem::separator(),
                    MenuItem::action("Close Tab", CloseTab),
                ],
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::os_action("Undo", Undo, OsAction::Undo),
                    MenuItem::os_action("Redo", Redo, OsAction::Redo),
                    MenuItem::separator(),
                    MenuItem::os_action("Cut", Cut, OsAction::Cut),
                    MenuItem::os_action("Copy", Copy, OsAction::Copy),
                    MenuItem::os_action("Paste", Paste, OsAction::Paste),
                    MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
                    MenuItem::separator(),
                    MenuItem::action("Find in File", FindInFile),
                    MenuItem::action("Find File...", FindFile),
                    MenuItem::action("Search Workspace...", SearchWorkspace),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Toggle Sidebar", ToggleSidebar),
                    MenuItem::action("Files Panel", ToggleFilesPanel),
                    MenuItem::action("Git Diff Panel", ToggleDiffPanel),
                    MenuItem::action("Pull Request Panel", TogglePrPanel),
                    MenuItem::separator(),
                    MenuItem::action("Split Pane Right", SplitPane),
                    MenuItem::action("Split Pane Down", SplitPaneVertical),
                    MenuItem::separator(),
                    MenuItem::action("Command Center", ToggleCommandCenter),
                    MenuItem::action("Debug Panel", ToggleDebugPanel),
                ],
            },
            Menu {
                name: "Window".into(),
                items: vec![
                    MenuItem::action("Minimize", Minimize),
                    MenuItem::action("Zoom", Zoom),
                    MenuItem::separator(),
                    MenuItem::action("Next Tab", NextTab),
                    MenuItem::action("Previous Tab", PrevTab),
                ],
            },
        ]);

        // Wire up macOS app-level menu actions
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());

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
                titlebar: Some(TitlebarOptions {
                    title: Some("Operator".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(9.0), px(9.0))),
                }),
                ..Default::default()
            },
            |window, cx| {
                #[cfg(target_os = "macos")]
                disable_titlebar_drag(window);

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
