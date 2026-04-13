pub mod settings_panel;

use gpui::*;

use crate::theme::colors;

/// Global application settings.
pub struct AppSettings {
    pub vim_mode: bool,
    pub theme: String,
}

impl Global for AppSettings {}

impl AppSettings {
    pub fn init(cx: &mut App) {
        cx.set_global(AppSettings {
            vim_mode: false,
            theme: colors::CATPPUCCIN_MOCHA.name.to_string(),
        });
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn vim_mode(cx: &App) -> bool {
        cx.global::<Self>().vim_mode
    }

    pub fn set_theme(name: &str, cx: &mut App) {
        colors::set_theme_by_name(name);
        cx.update_global::<AppSettings, _>(|settings, _cx| {
            settings.theme = name.to_string();
        });
    }
}
