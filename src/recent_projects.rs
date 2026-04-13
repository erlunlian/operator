use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const MAX_RECENT: usize = 50;

#[derive(Serialize, Deserialize, Default)]
pub struct RecentProjects {
    pub paths: Vec<PathBuf>,
}

impl RecentProjects {
    fn file_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("operator");
        config_dir.join("recent_projects.json")
    }

    pub fn load() -> Self {
        let path = Self::file_path();
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return Self::default(),
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::file_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Add a project directory to the front of the list (deduplicating).
    pub fn add(&mut self, dir: PathBuf) {
        self.paths.retain(|p| p != &dir);
        self.paths.insert(0, dir);
        self.paths.truncate(MAX_RECENT);
        self.save();
    }
}
