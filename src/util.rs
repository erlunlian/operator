use std::path::Path;

/// Replace home directory prefix with `~` for display.
pub fn short_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        let path_str = path.to_string_lossy();
        let home_str = home.to_string_lossy();
        if path_str.starts_with(home_str.as_ref()) {
            return format!("~{}", &path_str[home_str.len()..]);
        }
    }
    path.to_string_lossy().to_string()
}

/// The font family name for the bundled Nerd Font icons.
pub const ICON_FONT: &str = "MesloLGS NF";

/// Return a color for the file icon based on extension.
pub fn file_icon_color(name: &str) -> gpui::Rgba {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => gpui::rgb(0xdea584),          // orange
        "js" | "mjs" | "cjs" | "jsx" => gpui::rgb(0xf0db4f), // yellow
        "ts" | "mts" | "tsx" => gpui::rgb(0x3178c6),          // blue
        "html" | "htm" => gpui::rgb(0xe44d26),  // orange-red
        "css" | "scss" | "sass" => gpui::rgb(0x563d7c), // purple
        "json" => gpui::rgb(0xa6e3a1),          // green
        "yaml" | "yml" => gpui::rgb(0xf9e2af),  // yellow
        "toml" => gpui::rgb(0x9a9a9a),          // gray
        "md" | "mdx" => gpui::rgb(0x89b4fa),    // light blue
        "py" | "pyi" => gpui::rgb(0x3572a5),    // python blue
        "go" => gpui::rgb(0x00add8),             // go cyan
        "sh" | "bash" | "zsh" | "fish" => gpui::rgb(0x89e051), // green
        "c" | "h" => gpui::rgb(0x555555),        // dark gray
        "cpp" | "cc" | "cxx" | "hpp" => gpui::rgb(0xf34b7d), // pink
        "java" => gpui::rgb(0xb07219),           // brown
        "swift" => gpui::rgb(0xf05138),          // swift orange
        "rb" => gpui::rgb(0xcc342d),             // ruby red
        "lua" => gpui::rgb(0x000080),            // navy
        "txt" | "pdf" => gpui::rgb(0x888888),    // gray
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" => gpui::rgb(0xa855f7), // purple
        "lock" => gpui::rgb(0x666666),           // dark gray
        "sql" => gpui::rgb(0xe38c00),            // orange
        "xml" | "plist" => gpui::rgb(0xe37933),  // orange
        _ => gpui::rgb(0x6c7086),                // muted
    }
}

/// Nerd Font icon for directories.
pub fn dir_icon() -> &'static str {
    "\u{f07b}" //  (nf-fa-folder)
}

/// Nerd Font icon for open directories.
pub fn dir_icon_open() -> &'static str {
    "\u{f07c}" //  (nf-fa-folder_open)
}

/// Return a Nerd Font icon for a file based on its name/extension.
pub fn icon_for_file(name: &str) -> &'static str {
    // Check special filenames first
    let lower = name.to_lowercase();
    match lower.as_str() {
        "cargo.toml" | "cargo.lock" => return "\u{e7a8}",   //  (rust)
        "makefile" | "justfile" => return "\u{e779}",        //  (terminal)
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" => return "\u{f308}", //  (docker)
        ".gitignore" | ".gitmodules" | ".gitattributes" => return "\u{e702}", //  (git)
        "license" | "license.md" | "license.txt" => return "\u{f0219}", // 󰈙
        "readme.md" | "readme.txt" | "readme" => return "\u{f48a}", //  (book)
        "package.json" | "package-lock.json" => return "\u{e718}", //  (nodejs)
        ".env" | ".env.local" | ".env.production" => return "\u{f462}", //
        _ => {}
    }

    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        // Rust
        "rs" => "\u{e7a8}",            //
        // JavaScript / TypeScript
        "js" | "mjs" | "cjs" => "\u{e74e}",  //
        "jsx" => "\u{e7ba}",                   //
        "ts" | "mts" => "\u{e628}",           //
        "tsx" => "\u{e7ba}",                   //
        // Web
        "html" | "htm" => "\u{e736}",  //
        "css" => "\u{e749}",            //
        "scss" | "sass" => "\u{e603}",  //
        // Data / Config
        "json" => "\u{e60b}",          //
        "yaml" | "yml" => "\u{e6a8}",  //
        "toml" => "\u{e6b2}",          //
        "xml" | "plist" => "\u{e619}", //
        // Markdown / Docs
        "md" | "mdx" => "\u{e73e}",    //
        "txt" => "\u{f0219}",          // 󰈙
        "pdf" => "\u{e67d}",           //
        // Shell
        "sh" | "bash" | "zsh" | "fish" => "\u{e795}", //
        // Python
        "py" | "pyi" => "\u{e73c}",    //
        // Go
        "go" => "\u{e626}",            //
        // C / C++
        "c" | "h" => "\u{e61e}",       //
        "cpp" | "cc" | "cxx" | "hpp" => "\u{e61d}", //
        // Java / Kotlin
        "java" => "\u{e738}",          //
        "kt" | "kts" => "\u{e634}",    //
        // Swift
        "swift" => "\u{e755}",         //
        // Ruby
        "rb" => "\u{e791}",            //
        // PHP
        "php" => "\u{e73d}",           //
        // Lua
        "lua" => "\u{e620}",           //
        // Images
        "png" | "jpg" | "jpeg" | "gif" | "webp" => "\u{f1c5}", //
        "svg" => "\u{f0721}",          // 󰜡
        "ico" => "\u{f1c5}",           //
        // Lock
        "lock" => "\u{f023}",          //
        // Git
        "diff" | "patch" => "\u{e702}", //
        // SQL
        "sql" => "\u{f1c0}",           //
        // Default
        _ => "\u{f016}",               //  (file)
    }
}
