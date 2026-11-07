pub mod detail;
pub mod history;
pub mod library;
pub mod reader;
pub mod search;
pub mod shell;

use std::sync::Arc;

use manga_core::{ComicDetail, ComicSummary};

/// Navigation requests emitted by views and handled by the shell.
#[derive(Clone)]
pub enum Nav {
    OpenComic {
        source: String,
        comic: ComicSummary,
    },
    Read {
        source: String,
        detail: Arc<ComicDetail>,
        chapter_ix: usize,
        page: usize,
    },
    /// Open the reader at the saved position; detail is loaded first.
    Resume {
        source: String,
        comic: ComicSummary,
        chapter_id: String,
        page: usize,
    },
    Back,
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
