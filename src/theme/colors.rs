use gpui::{rgb, Rgba};
use std::sync::OnceLock;
use std::sync::RwLock;

/// A complete color theme.
#[derive(Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub bg: u32,
    pub surface: u32,
    pub surface_hover: u32,
    pub border: u32,
    pub text: u32,
    pub text_muted: u32,
    pub accent: u32,
    pub tab_active_bg: u32,
    pub tab_inactive_bg: u32,
    pub diff_added: u32,
    pub diff_removed: u32,
    // Syntax highlighting
    pub syn_comment: u32,
    pub syn_keyword: u32,
    pub syn_string: u32,
    pub syn_number: u32,
    pub syn_function: u32,
    pub syn_type: u32,
    pub syn_variable: u32,
    pub syn_variable_builtin: u32,
    pub syn_operator: u32,
    pub syn_punctuation: u32,
    pub syn_property: u32,
    pub syn_attribute: u32,
}

// ── Theme Presets ──

pub const CATPPUCCIN_MOCHA: Theme = Theme {
    name: "Catppuccin Mocha",
    bg: 0x1e1e2e,
    surface: 0x181825,
    surface_hover: 0x313244,
    border: 0x45475a,
    text: 0xcdd6f4,
    text_muted: 0xa6adc8,
    accent: 0x89b4fa,
    tab_active_bg: 0x1e1e2e,
    tab_inactive_bg: 0x11111b,
    diff_added: 0xa6e3a1,
    diff_removed: 0xf38ba8,
    syn_comment: 0x6c7086,
    syn_keyword: 0xcba6f7,
    syn_string: 0xa6e3a1,
    syn_number: 0xfab387,
    syn_function: 0x89b4fa,
    syn_type: 0xf9e2af,
    syn_variable: 0xcdd6f4,
    syn_variable_builtin: 0xf38ba8,
    syn_operator: 0x89dceb,
    syn_punctuation: 0x9399b2,
    syn_property: 0x89b4fa,
    syn_attribute: 0xf5c2e7,
};

pub const ONE_DARK: Theme = Theme {
    name: "One Dark",
    bg: 0x282c34,
    surface: 0x21252b,
    surface_hover: 0x2c313a,
    border: 0x3e4451,
    text: 0xabb2bf,
    text_muted: 0x7f848e,
    accent: 0x61afef,
    tab_active_bg: 0x282c34,
    tab_inactive_bg: 0x21252b,
    diff_added: 0x98c379,
    diff_removed: 0xe06c75,
    syn_comment: 0x5c6370,
    syn_keyword: 0xc678dd,
    syn_string: 0x98c379,
    syn_number: 0xd19a66,
    syn_function: 0x61afef,
    syn_type: 0xe5c07b,
    syn_variable: 0xabb2bf,
    syn_variable_builtin: 0xe06c75,
    syn_operator: 0x56b6c2,
    syn_punctuation: 0x7f848e,
    syn_property: 0x61afef,
    syn_attribute: 0xc678dd,
};

pub const TOKYO_NIGHT: Theme = Theme {
    name: "Tokyo Night",
    bg: 0x1a1b26,
    surface: 0x16161e,
    surface_hover: 0x292e42,
    border: 0x3b4261,
    text: 0xc0caf5,
    text_muted: 0x565f89,
    accent: 0x7aa2f7,
    tab_active_bg: 0x1a1b26,
    tab_inactive_bg: 0x16161e,
    diff_added: 0x9ece6a,
    diff_removed: 0xf7768e,
    syn_comment: 0x565f89,
    syn_keyword: 0xbb9af7,
    syn_string: 0x9ece6a,
    syn_number: 0xff9e64,
    syn_function: 0x7aa2f7,
    syn_type: 0xe0af68,
    syn_variable: 0xc0caf5,
    syn_variable_builtin: 0xf7768e,
    syn_operator: 0x89ddff,
    syn_punctuation: 0x565f89,
    syn_property: 0x73daca,
    syn_attribute: 0xbb9af7,
};

pub const DRACULA: Theme = Theme {
    name: "Dracula",
    bg: 0x282a36,
    surface: 0x21222c,
    surface_hover: 0x44475a,
    border: 0x44475a,
    text: 0xf8f8f2,
    text_muted: 0x6272a4,
    accent: 0xbd93f9,
    tab_active_bg: 0x282a36,
    tab_inactive_bg: 0x21222c,
    diff_added: 0x50fa7b,
    diff_removed: 0xff5555,
    syn_comment: 0x6272a4,
    syn_keyword: 0xff79c6,
    syn_string: 0xf1fa8c,
    syn_number: 0xbd93f9,
    syn_function: 0x50fa7b,
    syn_type: 0x8be9fd,
    syn_variable: 0xf8f8f2,
    syn_variable_builtin: 0xff5555,
    syn_operator: 0xff79c6,
    syn_punctuation: 0x6272a4,
    syn_property: 0x66d9ef,
    syn_attribute: 0x50fa7b,
};

pub const GRUVBOX_DARK: Theme = Theme {
    name: "Gruvbox Dark",
    bg: 0x282828,
    surface: 0x1d2021,
    surface_hover: 0x3c3836,
    border: 0x504945,
    text: 0xebdbb2,
    text_muted: 0xa89984,
    accent: 0x83a598,
    tab_active_bg: 0x282828,
    tab_inactive_bg: 0x1d2021,
    diff_added: 0xb8bb26,
    diff_removed: 0xfb4934,
    syn_comment: 0x928374,
    syn_keyword: 0xfb4934,
    syn_string: 0xb8bb26,
    syn_number: 0xd3869b,
    syn_function: 0xfabd2f,
    syn_type: 0x83a598,
    syn_variable: 0xebdbb2,
    syn_variable_builtin: 0xfe8019,
    syn_operator: 0x8ec07c,
    syn_punctuation: 0xa89984,
    syn_property: 0x83a598,
    syn_attribute: 0xd3869b,
};

pub const NORD: Theme = Theme {
    name: "Nord",
    bg: 0x2e3440,
    surface: 0x272c36,
    surface_hover: 0x3b4252,
    border: 0x434c5e,
    text: 0xeceff4,
    text_muted: 0x81a1c1,
    accent: 0x88c0d0,
    tab_active_bg: 0x2e3440,
    tab_inactive_bg: 0x272c36,
    diff_added: 0xa3be8c,
    diff_removed: 0xbf616a,
    syn_comment: 0x616e88,
    syn_keyword: 0x81a1c1,
    syn_string: 0xa3be8c,
    syn_number: 0xb48ead,
    syn_function: 0x88c0d0,
    syn_type: 0x8fbcbb,
    syn_variable: 0xeceff4,
    syn_variable_builtin: 0xd08770,
    syn_operator: 0x81a1c1,
    syn_punctuation: 0x81a1c1,
    syn_property: 0x88c0d0,
    syn_attribute: 0xb48ead,
};

pub const ROSE_PINE: Theme = Theme {
    name: "Rosé Pine",
    bg: 0x191724,
    surface: 0x1f1d2e,
    surface_hover: 0x26233a,
    border: 0x403d52,
    text: 0xe0def4,
    text_muted: 0x908caa,
    accent: 0xc4a7e7,
    tab_active_bg: 0x191724,
    tab_inactive_bg: 0x1f1d2e,
    diff_added: 0x9ccfd8,
    diff_removed: 0xeb6f92,
    syn_comment: 0x6e6a86,
    syn_keyword: 0x31748f,
    syn_string: 0xf6c177,
    syn_number: 0xebbcba,
    syn_function: 0xc4a7e7,
    syn_type: 0x9ccfd8,
    syn_variable: 0xe0def4,
    syn_variable_builtin: 0xeb6f92,
    syn_operator: 0x908caa,
    syn_punctuation: 0x6e6a86,
    syn_property: 0xc4a7e7,
    syn_attribute: 0x9ccfd8,
};

pub const GITHUB_DARK: Theme = Theme {
    name: "GitHub Dark",
    bg: 0x0d1117,
    surface: 0x161b22,
    surface_hover: 0x1c2128,
    border: 0x30363d,
    text: 0xe6edf3,
    text_muted: 0x8b949e,
    accent: 0x58a6ff,
    tab_active_bg: 0x0d1117,
    tab_inactive_bg: 0x161b22,
    diff_added: 0x3fb950,
    diff_removed: 0xf85149,
    syn_comment: 0x8b949e,
    syn_keyword: 0xff7b72,
    syn_string: 0xa5d6ff,
    syn_number: 0x79c0ff,
    syn_function: 0xd2a8ff,
    syn_type: 0xffa657,
    syn_variable: 0xe6edf3,
    syn_variable_builtin: 0xffa657,
    syn_operator: 0xff7b72,
    syn_punctuation: 0x8b949e,
    syn_property: 0x79c0ff,
    syn_attribute: 0xd2a8ff,
};

pub const GITHUB_LIGHT: Theme = Theme {
    name: "GitHub Light",
    bg: 0xffffff,
    surface: 0xf6f8fa,
    surface_hover: 0xeaeef2,
    border: 0xd0d7de,
    text: 0x1f2328,
    text_muted: 0x656d76,
    accent: 0x0969da,
    tab_active_bg: 0xffffff,
    tab_inactive_bg: 0xf6f8fa,
    diff_added: 0x1a7f37,
    diff_removed: 0xcf222e,
    syn_comment: 0x6e7781,
    syn_keyword: 0xcf222e,
    syn_string: 0x0a3069,
    syn_number: 0x0550ae,
    syn_function: 0x8250df,
    syn_type: 0x953800,
    syn_variable: 0x1f2328,
    syn_variable_builtin: 0x953800,
    syn_operator: 0xcf222e,
    syn_punctuation: 0x6e7781,
    syn_property: 0x0550ae,
    syn_attribute: 0x8250df,
};

pub const CATPPUCCIN_LATTE: Theme = Theme {
    name: "Catppuccin Latte",
    bg: 0xeff1f5,
    surface: 0xe6e9ef,
    surface_hover: 0xccd0da,
    border: 0xbcc0cc,
    text: 0x4c4f69,
    text_muted: 0x6c6f85,
    accent: 0x1e66f5,
    tab_active_bg: 0xeff1f5,
    tab_inactive_bg: 0xe6e9ef,
    diff_added: 0x40a02b,
    diff_removed: 0xd20f39,
    syn_comment: 0x9ca0b0,
    syn_keyword: 0x8839ef,
    syn_string: 0x40a02b,
    syn_number: 0xfe640b,
    syn_function: 0x1e66f5,
    syn_type: 0xdf8e1d,
    syn_variable: 0x4c4f69,
    syn_variable_builtin: 0xd20f39,
    syn_operator: 0x04a5e5,
    syn_punctuation: 0x7c7f93,
    syn_property: 0x1e66f5,
    syn_attribute: 0xea76cb,
};

pub const ALL_THEMES: &[Theme] = &[
    CATPPUCCIN_MOCHA,
    ONE_DARK,
    TOKYO_NIGHT,
    DRACULA,
    GRUVBOX_DARK,
    NORD,
    ROSE_PINE,
    GITHUB_DARK,
    GITHUB_LIGHT,
    CATPPUCCIN_LATTE,
];

pub const DEFAULT_THEME: Theme = GITHUB_DARK;

// ── Active theme ──

static ACTIVE_THEME: OnceLock<RwLock<Theme>> = OnceLock::new();

fn active() -> &'static RwLock<Theme> {
    ACTIVE_THEME.get_or_init(|| RwLock::new(DEFAULT_THEME))
}

pub fn set_theme(theme: Theme) {
    *active().write().unwrap() = theme;
}

pub fn set_theme_by_name(name: &str) {
    for t in ALL_THEMES {
        if t.name == name {
            set_theme(*t);
            return;
        }
    }
}

// ── Color accessor functions (used throughout the app) ──

pub fn bg() -> Rgba { rgb(active().read().unwrap().bg) }
pub fn surface() -> Rgba { rgb(active().read().unwrap().surface) }
pub fn surface_hover() -> Rgba { rgb(active().read().unwrap().surface_hover) }
pub fn border() -> Rgba { rgb(active().read().unwrap().border) }
pub fn text() -> Rgba { rgb(active().read().unwrap().text) }
pub fn text_muted() -> Rgba { rgb(active().read().unwrap().text_muted) }
pub fn accent() -> Rgba { rgb(active().read().unwrap().accent) }
pub fn tab_active_bg() -> Rgba { rgb(active().read().unwrap().tab_active_bg) }
pub fn tab_inactive_bg() -> Rgba { rgb(active().read().unwrap().tab_inactive_bg) }
pub fn diff_added() -> Rgba { rgb(active().read().unwrap().diff_added) }
pub fn diff_removed() -> Rgba { rgb(active().read().unwrap().diff_removed) }

// Syntax colors
pub fn syn_comment() -> Rgba { rgb(active().read().unwrap().syn_comment) }
pub fn syn_keyword() -> Rgba { rgb(active().read().unwrap().syn_keyword) }
pub fn syn_string() -> Rgba { rgb(active().read().unwrap().syn_string) }
pub fn syn_number() -> Rgba { rgb(active().read().unwrap().syn_number) }
pub fn syn_function() -> Rgba { rgb(active().read().unwrap().syn_function) }
pub fn syn_type() -> Rgba { rgb(active().read().unwrap().syn_type) }
pub fn syn_variable() -> Rgba { rgb(active().read().unwrap().syn_variable) }
pub fn syn_variable_builtin() -> Rgba { rgb(active().read().unwrap().syn_variable_builtin) }
pub fn syn_operator() -> Rgba { rgb(active().read().unwrap().syn_operator) }
pub fn syn_punctuation() -> Rgba { rgb(active().read().unwrap().syn_punctuation) }
pub fn syn_property() -> Rgba { rgb(active().read().unwrap().syn_property) }
pub fn syn_attribute() -> Rgba { rgb(active().read().unwrap().syn_attribute) }
