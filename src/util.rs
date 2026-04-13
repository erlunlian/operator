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

/// Return a unicode icon for a directory.
pub fn dir_icon() -> &'static str {
    "\u{1F4C1}" // 📁
}

/// Return a unicode icon for an expanded directory.
pub fn dir_icon_open() -> &'static str {
    "\u{1F4C2}" // 📂
}

/// Return a unicode icon for a file based on its name/extension.
pub fn file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        // Rust
        "rs" => "\u{1F980}",           // 🦀
        // JavaScript / TypeScript
        "js" | "mjs" | "cjs" | "jsx" => "\u{1F7E8}", // 🟨
        "ts" | "mts" | "tsx" => "\u{1F7E6}",          // 🟦
        // Web
        "html" | "htm" => "\u{1F310}", // 🌐
        "css" | "scss" | "sass" => "\u{1F3A8}",  // 🎨
        // Data / Config
        "json" => "\u{1F4CB}",         // 📋
        "yaml" | "yml" => "\u{2699}\u{FE0F}",  // ⚙️
        "toml" => "\u{2699}\u{FE0F}",  // ⚙️
        "xml" => "\u{1F4C4}",          // 📄
        // Markdown / Docs
        "md" | "mdx" => "\u{1F4DD}",   // 📝
        "txt" => "\u{1F4C4}",          // 📄
        "pdf" => "\u{1F4D5}",          // 📕
        // Shell
        "sh" | "bash" | "zsh" | "fish" => "\u{1F41A}", // 🐚
        // Python
        "py" | "pyi" => "\u{1F40D}",   // 🐍
        // Go
        "go" => "\u{1F439}",           // 🐹
        // C / C++
        "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" => "\u{1F1E8}", // 🇨
        // Java
        "java" => "\u{2615}",          // ☕
        // Swift
        "swift" => "\u{1F426}",        // 🐦
        // Ruby
        "rb" => "\u{1F48E}",           // 💎
        // Lua
        "lua" => "\u{1F319}",          // 🌙
        // Images
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" => "\u{1F5BC}\u{FE0F}", // 🖼️
        // Lock files
        "lock" => "\u{1F512}",         // 🔒
        // Git
        "diff" | "patch" => "\u{1F4CA}", // 📊
        // Docker
        "dockerfile" => "\u{1F40B}",   // 🐋
        // SQL
        "sql" => "\u{1F5C3}\u{FE0F}",  // 🗃️
        // Default
        _ => "\u{1F4C4}",              // 📄
    }
}

/// Icon for special filenames (checked before extension).
pub fn file_icon_special(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    match lower.as_str() {
        "cargo.toml" | "cargo.lock" => Some("\u{1F980}"),  // 🦀
        "makefile" | "justfile" => Some("\u{1F6E0}\u{FE0F}"),  // 🛠️
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" => Some("\u{1F40B}"), // 🐋
        ".gitignore" | ".gitmodules" | ".gitattributes" => Some("\u{1F500}"), // 🔀
        "license" | "license.md" | "license.txt" => Some("\u{1F4DC}"), // 📜
        "readme.md" | "readme.txt" | "readme" => Some("\u{1F4D6}"), // 📖
        "package.json" | "package-lock.json" => Some("\u{1F4E6}"), // 📦
        ".env" | ".env.local" | ".env.production" => Some("\u{1F510}"), // 🔐
        _ => None,
    }
}

/// Get the best icon for a filename (special name first, then by extension).
pub fn icon_for_file(name: &str) -> &'static str {
    if let Some(icon) = file_icon_special(name) {
        icon
    } else {
        file_icon(name)
    }
}
