use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// `~/Library/Application Support/manga-gpui` on macOS (data) and the
/// platform cache dir for images.
pub fn app_dirs() -> (PathBuf, PathBuf) {
    let data = dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("manga-gpui");
    let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("manga-gpui");
    (data, cache)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Root folder scanned by the local source.
    pub library_root: Option<PathBuf>,
    /// Reader mode persisted across sessions: "scroll" | "ltr" | "rtl".
    pub reader_mode: String,
    pub preload_ahead: usize,
    pub disk_cache_mb: u64,
    pub memory_budget_mb: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            library_root: None,
            reader_mode: "scroll".into(),
            preload_ahead: 5,
            disk_cache_mb: 1024,
            memory_budget_mb: 384,
        }
    }
}

impl AppConfig {
    fn path() -> PathBuf {
        app_dirs().0.join("config.json")
    }

    pub fn load() -> Self {
        let p = Self::path();
        let Ok(bytes) = std::fs::read(&p) else { return Self::default() };
        match serde_json::from_slice(&bytes) {
            Ok(c) => c,
            Err(e) => {
                // Keep the user's broken file instead of overwriting it on
                // the next save.
                log::warn!("config.json unreadable ({e}); backed up to config.json.bak");
                let _ = std::fs::rename(&p, p.with_extension("json.bak"));
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = p.with_extension("json.tmp");
        if let Ok(json) = serde_json::to_vec_pretty(self)
            && std::fs::write(&tmp, json).is_ok()
        {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}
