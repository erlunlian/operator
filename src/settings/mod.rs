pub mod settings_panel;

use gpui::*;

/// Global application settings.
pub struct AppSettings {
    pub vim_mode: bool,
}

impl Global for AppSettings {}

impl AppSettings {
    pub fn init(cx: &mut App) {
        cx.set_global(AppSettings { vim_mode: false });
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn vim_mode(cx: &App) -> bool {
        cx.global::<Self>().vim_mode
    }
}
