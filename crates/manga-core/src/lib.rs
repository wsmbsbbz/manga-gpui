//! Domain model and the `ComicSource` abstraction.
//!
//! This crate has no UI or runtime dependency. Every source (local folders
//! today, websites later) implements [`ComicSource`]; the app only talks to
//! that trait through a [`SourceRegistry`].

use std::{collections::BTreeMap, fmt, path::PathBuf, sync::Arc};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

mod natord;
pub use natord::natural_cmp;

pub type Result<T, E = SourceError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error, Clone)]
pub enum SourceError {
    #[error("网络错误：{0}")]
    Network(String),
    #[error("请求过于频繁，请 {0} 秒后重试")]
    RateLimited(u64),
    #[error("需要登录")]
    LoginRequired,
    #[error("未找到：{0}")]
    NotFound(String),
    #[error("解析失败：{0}")]
    Parse(String),
    #[error("读取失败：{0}")]
    Io(String),
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for SourceError {
    fn from(e: std::io::Error) -> Self {
        SourceError::Io(e.to_string())
    }
}

/// Identifies a source, e.g. `local`, `copy_manga`.
pub type SourceKey = &'static str;

/// What a source can do. The UI hides features a source does not declare.
#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    pub search: bool,
    pub login: bool,
    pub favorites: bool,
    pub categories: bool,
    /// Whether fetched image bytes should go to the disk cache. Local files
    /// are already on disk and set this to `false`.
    pub cache_images: bool,
    /// Browse one page at a time (remote sources) rather than loading the
    /// whole listing up front (local folders).
    pub incremental_browse: bool,
}

#[derive(Debug, Clone)]
pub struct SourceMeta {
    pub key: SourceKey,
    pub name: &'static str,
    pub capabilities: Capabilities,
}

/// A comic as shown in lists and grids.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComicSummary {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub cover: Option<ImageRequest>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Chapter {
    pub id: String,
    pub title: String,
}

/// Chapters are grouped (e.g. "话", "卷", "番外"). Order is preserved.
#[derive(Debug, Clone)]
pub struct ChapterGroup {
    pub name: String,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, Clone)]
pub struct ComicDetail {
    pub summary: ComicSummary,
    pub description: Option<String>,
    /// Ordered metadata rows, e.g. "作者" → ["…"].
    pub meta: BTreeMap<String, Vec<String>>,
    pub groups: Vec<ChapterGroup>,
}

impl ComicDetail {
    /// All chapters flattened in reading order.
    pub fn chapters(&self) -> impl Iterator<Item = &Chapter> {
        self.groups.iter().flat_map(|g| g.chapters.iter())
    }

    pub fn chapter_index(&self, id: &str) -> Option<usize> {
        self.chapters().position(|c| c.id == id)
    }

    pub fn chapter_at(&self, ix: usize) -> Option<&Chapter> {
        self.chapters().nth(ix)
    }
}

/// One page of a paginated listing.
#[derive(Debug, Clone)]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub has_more: bool,
}

/// Everything needed to fetch one image's bytes. Sources that need custom
/// headers (referer, auth) attach them here, so the image pipeline never has
/// to know source specifics.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ImageRequest {
    File(PathBuf),
    ArchiveEntry {
        archive: PathBuf,
        entry: String,
    },
    Http {
        url: String,
        /// Never persisted (history, caches): headers may carry tokens.
        /// Sources re-attach them when they produce fresh requests.
        #[serde(skip_serializing, default)]
        headers: Vec<(String, String)>,
    },
}

impl ImageRequest {
    /// Stable identity used for caches and in-flight de-duplication. Headers
    /// are deliberately excluded: they carry auth, not identity.
    pub fn cache_key(&self) -> String {
        match self {
            ImageRequest::File(p) => format!("file:{}", p.display()),
            ImageRequest::ArchiveEntry { archive, entry } => {
                format!("zip:{}!{}", archive.display(), entry)
            }
            ImageRequest::Http { url, .. } => format!("http:{url}"),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, ImageRequest::Http { .. })
    }
}

impl fmt::Debug for ImageRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print headers: they may contain tokens.
        f.write_str(&self.cache_key())
    }
}

/// A comic provider. All methods return `'static` futures so the app can run
/// them on the network runtime and drop them to cancel.
pub trait ComicSource: Send + Sync + 'static {
    fn meta(&self) -> &SourceMeta;

    /// Home / browse listing. `page` starts at 1.
    fn explore(&self, page: u32) -> BoxFuture<'static, Result<Paged<ComicSummary>>>;

    fn search(&self, query: &str, page: u32) -> BoxFuture<'static, Result<Paged<ComicSummary>>>;

    fn detail(&self, comic_id: &str) -> BoxFuture<'static, Result<ComicDetail>>;

    /// Ordered page images of one chapter.
    fn pages(&self, comic_id: &str, chapter_id: &str) -> BoxFuture<'static, Result<Vec<ImageRequest>>>;
}

/// Reads raw image bytes for an [`ImageRequest`]. Implemented by the network
/// layer; sources never fetch images themselves.
pub trait ByteCache: Send + Sync + 'static {
    fn get(&self, key: &str) -> Option<bytes::Bytes>;
    fn put(&self, key: &str, data: &[u8]);
}

#[derive(Default, Clone)]
pub struct SourceRegistry {
    sources: Vec<Arc<dyn ComicSource>>,
}

impl SourceRegistry {
    pub fn register(&mut self, source: Arc<dyn ComicSource>) {
        let key = source.meta().key;
        self.sources.retain(|s| s.meta().key != key);
        self.sources.push(source);
    }

    pub fn get(&self, key: &str) -> Option<Arc<dyn ComicSource>> {
        self.sources.iter().find(|s| s.meta().key == key).cloned()
    }

    pub fn all(&self) -> &[Arc<dyn ComicSource>] {
        &self.sources
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_ignores_headers() {
        let a = ImageRequest::Http { url: "https://x/1.webp".into(), headers: vec![("a".into(), "1".into())] };
        let b = ImageRequest::Http { url: "https://x/1.webp".into(), headers: vec![] };
        assert_eq!(a.cache_key(), b.cache_key());
        assert!(!format!("{a:?}").contains("a"));
    }

    #[test]
    fn chapter_index_flattens_groups() {
        let ch = |id: &str| Chapter { id: id.into(), title: id.into() };
        let d = ComicDetail {
            summary: ComicSummary { id: "c".into(), title: "c".into(), subtitle: None, cover: None, tags: vec![] },
            description: None,
            meta: Default::default(),
            groups: vec![
                ChapterGroup { name: "话".into(), chapters: vec![ch("1"), ch("2")] },
                ChapterGroup { name: "番外".into(), chapters: vec![ch("x")] },
            ],
        };
        assert_eq!(d.chapter_index("x"), Some(2));
        assert_eq!(d.chapter_at(1).unwrap().id, "2");
    }
}
