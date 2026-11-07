//! Persistence: disk image cache, reading history, app config.

mod config;
mod disk_cache;
mod history;

pub use config::{AppConfig, app_dirs};
pub use disk_cache::DiskCache;
pub use history::{History, HistoryEntry};
