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
