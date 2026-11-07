//! Local library source.
//!
//! Layout under the library root (all names sorted naturally):
//!
//! ```text
//! root/
//!   一部漫画/            ← comic
//!     cover.jpg          ← optional cover
//!     第1话/ 001.jpg …   ← chapter folder
//!     第2话.cbz          ← chapter archive (.cbz / .zip)
//!   单行本/ 001.jpg …    ← images directly inside → one chapter
//!   单本.cbz             ← archive at root → comic with one chapter
//! ```
//!
//! IDs are paths relative to the root, so they stay stable across runs.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use futures::{FutureExt, future::BoxFuture};
use manga_core::{
    Capabilities, Chapter, ChapterGroup, ComicDetail, ComicSource, ComicSummary, ImageRequest, Paged, Result,
    SourceError, SourceMeta, natural_cmp,
};
use manga_net::Runtime;
use parking_lot::RwLock;

pub const KEY: &str = "local";
const PAGE_SIZE: usize = 60;
const IMAGE_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];
const ARCHIVE_EXT: &[&str] = &["cbz", "zip"];
/// Chapter id used when a comic has no chapter subdivisions.
const SINGLE: &str = ".";
/// Guards against hostile archives with absurd central directories.
const MAX_ARCHIVE_ENTRIES: usize = 20_000;

/// Library listing cached per root. A scan touches every comic folder and
/// opens archives for covers, so pagination and search reuse one scan.
type ScanCache = Arc<RwLock<Option<(PathBuf, Option<std::time::SystemTime>, Arc<Vec<ComicSummary>>)>>>;

pub struct LocalSource {
    meta: SourceMeta,
    root: Arc<RwLock<Option<PathBuf>>>,
    scan: ScanCache,
    runtime: Runtime,
}

impl LocalSource {
    pub fn new(root: Option<PathBuf>, runtime: Runtime) -> Self {
        Self {
            meta: SourceMeta {
                key: KEY,
                name: "本地书库",
                capabilities: Capabilities { search: true, cache_images: false, ..Default::default() },
            },
            root: Arc::new(RwLock::new(root)),
            scan: Arc::default(),
            runtime,
        }
    }

    pub fn set_root(&self, root: Option<PathBuf>) {
        *self.root.write() = root;
        self.invalidate();
    }

    /// Forget the cached listing; the next explore/search rescans.
    pub fn invalidate(&self) {
        *self.scan.write() = None;
    }

    /// The full listing for `root`, scanning only on a cache miss.
    fn cached_scan(&self) -> BoxFuture<'static, Result<Arc<Vec<ComicSummary>>>> {
        let cache = self.scan.clone();
        self.blocking(move |root| {
            // Root mtime changes when comics are added/removed/renamed at
            // the top level; edits deeper inside need the 刷新 button.
            let mtime = fs::metadata(&root).and_then(|m| m.modified()).ok();
            if let Some((r, t, list)) = cache.read().as_ref()
                && *r == root
                && *t == mtime
            {
                return Ok(list.clone());
            }
            let list = Arc::new(scan_library(&root)?);
            *cache.write() = Some((root, mtime, list.clone()));
            Ok(list)
        })
    }

    pub fn root(&self) -> Option<PathBuf> {
        self.root.read().clone()
    }

    /// Run blocking filesystem work on the IO runtime.
    fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(PathBuf) -> Result<T> + Send + 'static,
    ) -> BoxFuture<'static, Result<T>> {
        let root = self.root();
        let pending = self.runtime.spawn_blocking(move || {
            let root = root.ok_or_else(|| SourceError::Other("尚未选择书库文件夹".into()))?;
            f(root)
        });
        async move { pending.await.unwrap_or_else(|| Err(SourceError::Other("任务已取消".into()))) }.boxed()
    }
}

impl ComicSource for LocalSource {
    fn meta(&self) -> &SourceMeta {
        &self.meta
    }

    fn explore(&self, page: u32) -> BoxFuture<'static, Result<Paged<ComicSummary>>> {
        let all = self.cached_scan();
        async move { Ok(paginate(all.await?.iter().cloned(), page)) }.boxed()
    }

    fn search(&self, query: &str, page: u32) -> BoxFuture<'static, Result<Paged<ComicSummary>>> {
        let q = query.trim().to_lowercase();
        let all = self.cached_scan();
        async move {
            let all = all.await?;
            Ok(paginate(all.iter().filter(|c| c.title.to_lowercase().contains(&q)).cloned(), page))
        }
        .boxed()
    }

    fn detail(&self, comic_id: &str) -> BoxFuture<'static, Result<ComicDetail>> {
        let id = comic_id.to_string();
        self.blocking(move |root| comic_detail(&root, &id))
    }

    fn pages(&self, comic_id: &str, chapter_id: &str) -> BoxFuture<'static, Result<Vec<ImageRequest>>> {
        let (cid, ch) = (comic_id.to_string(), chapter_id.to_string());
        self.blocking(move |root| {
            let comic = safe_join(&root, &cid)?;
            let target = if ch == SINGLE { comic } else { safe_join(&comic, &ch)? };
            chapter_images(&target)
        })
    }
}

fn paginate(items: impl Iterator<Item = ComicSummary>, page: u32) -> Paged<ComicSummary> {
    let start = (page.max(1) as usize - 1) * PAGE_SIZE;
    let mut it = items.skip(start);
    let page_items: Vec<_> = it.by_ref().take(PAGE_SIZE).collect();
    Paged { items: page_items, has_more: it.next().is_some() }
}

/// Rejects ids that would escape the library root.
fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let rel_path = Path::new(rel);
    use std::path::Component;
    let escapes = rel_path.is_absolute() || rel_path.components().any(|c| matches!(c, Component::ParentDir));
    // "", "." etc. would resolve to `base` itself (the library root).
    let names_nothing = !rel_path.components().any(|c| matches!(c, Component::Normal(_)));
    if escapes || names_nothing {
        return Err(SourceError::NotFound(rel.into()));
    }
    let p = base.join(rel_path);
    if p.exists() { Ok(p) } else { Err(SourceError::NotFound(rel.into())) }
}

fn ext_of(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

fn is_image(p: &Path) -> bool {
    IMAGE_EXT.contains(&ext_of(p).as_str())
}

fn is_archive(p: &Path) -> bool {
    ARCHIVE_EXT.contains(&ext_of(p).as_str())
}

fn is_hidden(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'))
}

fn name_of(p: &Path) -> String {
    p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string()
}

fn stem_of(p: &Path) -> String {
    p.file_stem().and_then(|n| n.to_str()).unwrap_or_default().to_string()
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> =
        fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| !is_hidden(p)).collect();
    v.sort_by(|a, b| natural_cmp(&name_of(a), &name_of(b)));
    v
}

fn scan_library(root: &Path) -> Result<Vec<ComicSummary>> {
    if !root.is_dir() {
        return Err(SourceError::NotFound(root.display().to_string()));
    }
    let mut out = Vec::new();
    for p in sorted_entries(root) {
        let id = name_of(&p);
        if p.is_dir() {
            let chapters = list_chapters(&p);
            if chapters.is_empty() {
                continue;
            }
            out.push(ComicSummary {
                id,
                title: name_of(&p),
                subtitle: Some(format!("{} 章", chapters.len())),
                cover: find_cover(&p, &chapters),
                tags: vec![],
            });
        } else if is_archive(&p) {
            out.push(ComicSummary {
                id,
                title: stem_of(&p),
                subtitle: Some("单本".into()),
                cover: first_archive_image(&p),
                tags: vec![],
            });
        }
    }
    Ok(out)
}

/// Chapters of a comic folder: subfolders with images and archives. If the
/// folder holds images directly, it is a single chapter.
fn list_chapters(comic: &Path) -> Vec<Chapter> {
    let entries = sorted_entries(comic);
    let mut chapters: Vec<Chapter> = entries
        .iter()
        .filter(|p| (p.is_dir() && sorted_entries(p).iter().any(|f| is_image(f))) || (p.is_file() && is_archive(p)))
        .map(|p| Chapter { id: name_of(p), title: if p.is_dir() { name_of(p) } else { stem_of(p) } })
        .collect();
    let direct_images = entries.iter().filter(|p| p.is_file() && is_image(p) && !is_cover_name(p)).count();
    if chapters.is_empty() && direct_images > 0 {
        chapters.push(Chapter { id: SINGLE.into(), title: "全一话".into() });
    }
    chapters
}

fn is_cover_name(p: &Path) -> bool {
    stem_of(p).eq_ignore_ascii_case("cover")
}

fn find_cover(comic: &Path, chapters: &[Chapter]) -> Option<ImageRequest> {
    let entries = sorted_entries(comic);
    if let Some(c) = entries.iter().find(|p| p.is_file() && is_image(p) && is_cover_name(p)) {
        return Some(ImageRequest::File(c.clone()));
    }
    let first = chapters.first()?;
    if first.id == SINGLE {
        return entries.into_iter().find(|p| p.is_file() && is_image(p)).map(ImageRequest::File);
    }
    let ch = comic.join(&first.id);
    if ch.is_dir() {
        sorted_entries(&ch).into_iter().find(|p| is_image(p)).map(ImageRequest::File)
    } else {
        first_archive_image(&ch)
    }
}

fn archive_image_names(archive: &Path) -> Result<Vec<String>> {
    let file = fs::File::open(archive)?;
    let zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| SourceError::Io(e.to_string()))?;
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        return Err(SourceError::Io(format!("压缩包条目过多（{}）", zip.len())));
    }
    let mut names: Vec<String> = zip
        .file_names()
        .filter(|n| !n.ends_with('/') && !n.contains("__MACOSX") && is_image(Path::new(n)))
        .map(str::to_string)
        .collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    Ok(names)
}

fn first_archive_image(archive: &Path) -> Option<ImageRequest> {
    let first = archive_image_names(archive).ok()?.into_iter().next()?;
    Some(ImageRequest::ArchiveEntry { archive: archive.to_path_buf(), entry: first })
}

fn chapter_images(target: &Path) -> Result<Vec<ImageRequest>> {
    let pages: Vec<ImageRequest> = if target.is_dir() {
        sorted_entries(target)
            .into_iter()
            .filter(|p| p.is_file() && is_image(p) && !is_cover_name(p))
            .map(ImageRequest::File)
            .collect()
    } else if is_archive(target) {
        archive_image_names(target)?
            .into_iter()
            .map(|entry| ImageRequest::ArchiveEntry { archive: target.to_path_buf(), entry })
            .collect()
    } else {
        vec![]
    };
    if pages.is_empty() { Err(SourceError::NotFound("本章没有图片".into())) } else { Ok(pages) }
}

fn comic_detail(root: &Path, id: &str) -> Result<ComicDetail> {
    let p = safe_join(root, id)?;
    let (title, chapters, cover) = if p.is_dir() {
        let chapters = list_chapters(&p);
        let cover = find_cover(&p, &chapters);
        (name_of(&p), chapters, cover)
    } else {
        // Root-level archive: the archive itself is the only chapter.
        (stem_of(&p), vec![Chapter { id: SINGLE.into(), title: "全一话".into() }], first_archive_image(&p))
    };
    let mut meta = std::collections::BTreeMap::new();
    meta.insert("位置".to_string(), vec![p.display().to_string()]);
    meta.insert("章节".to_string(), vec![format!("{} 章", chapters.len())]);
    Ok(ComicDetail {
        summary: ComicSummary { id: id.into(), title, subtitle: None, cover, tags: vec![] },
        description: None,
        meta,
        groups: vec![ChapterGroup { name: "章节".into(), chapters }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    fn make_zip(p: &Path, names: &[&str]) {
        let f = fs::File::create(p).unwrap();
        let mut z = zip::ZipWriter::new(f);
        for n in names {
            z.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(b"x").unwrap();
        }
        z.finish().unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        touch(&r.join("A漫画/第10话/1.jpg"));
        touch(&r.join("A漫画/第2话/2.png"));
        touch(&r.join("A漫画/第2话/10.png"));
        touch(&r.join("A漫画/cover.jpg"));
        make_zip(&r.join("A漫画/第11话.cbz"), &["p10.jpg", "p9.jpg", "__MACOSX/x.jpg"]);
        touch(&r.join("B单行本/01.jpg"));
        touch(&r.join("空文件夹/readme.txt"));
        make_zip(&r.join("C单本.cbz"), &["a.png"]);
        d
    }

    #[test]
    fn scans_library() {
        let d = fixture();
        let all = scan_library(d.path()).unwrap();
        let titles: Vec<_> = all.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["A漫画", "B单行本", "C单本"]);
        assert_eq!(all[0].cover, Some(ImageRequest::File(d.path().join("A漫画/cover.jpg"))));
    }

    #[test]
    fn chapters_sorted_naturally() {
        let d = fixture();
        let det = comic_detail(d.path(), "A漫画").unwrap();
        let ids: Vec<_> = det.chapters().map(|c| c.title.as_str()).collect();
        assert_eq!(ids, ["第2话", "第10话", "第11话"]);
        let imgs = chapter_images(&d.path().join("A漫画/第2话")).unwrap();
        assert_eq!(imgs[0], ImageRequest::File(d.path().join("A漫画/第2话/2.png")));
        let z = chapter_images(&d.path().join("A漫画/第11话.cbz")).unwrap();
        assert_eq!(z.len(), 2);
        assert!(matches!(&z[0], ImageRequest::ArchiveEntry { entry, .. } if entry == "p9.jpg"));
    }

    #[test]
    fn single_chapter_and_root_archive() {
        let d = fixture();
        let b = comic_detail(d.path(), "B单行本").unwrap();
        assert_eq!(b.chapters().next().unwrap().id, SINGLE);
        let c = comic_detail(d.path(), "C单本.cbz").unwrap();
        assert_eq!(c.chapters().count(), 1);
    }

    #[test]
    fn rejects_path_escape() {
        let d = fixture();
        assert!(safe_join(d.path(), "../etc").is_err());
        assert!(safe_join(d.path(), "/etc").is_err());
        assert!(safe_join(d.path(), "").is_err());
        assert!(safe_join(d.path(), ".").is_err());
        assert!(safe_join(d.path(), "A漫画").is_ok());
    }

    #[test]
    fn source_end_to_end() {
        let d = fixture();
        let src = LocalSource::new(Some(d.path().to_path_buf()), Runtime::new().unwrap());
        let r = futures::executor::block_on(src.search("单", 1)).unwrap();
        assert_eq!(r.items.len(), 2);
        let p = futures::executor::block_on(src.pages("C单本.cbz", SINGLE)).unwrap();
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn scan_is_cached_until_invalidated() {
        let d = fixture();
        let src = LocalSource::new(Some(d.path().to_path_buf()), Runtime::new().unwrap());
        assert_eq!(futures::executor::block_on(src.explore(1)).unwrap().items.len(), 3);
        // A change deeper than the root is not noticed until invalidated…
        touch(&d.path().join("B单行本/02.jpg"));
        fs::remove_dir_all(d.path().join("A漫画/第2话")).unwrap();
        let cached = futures::executor::block_on(src.explore(1)).unwrap();
        assert_eq!(cached.items[0].subtitle.as_deref(), Some("3 章"));
        src.invalidate();
        let fresh = futures::executor::block_on(src.explore(1)).unwrap();
        assert_eq!(fresh.items[0].subtitle.as_deref(), Some("2 章"));
        // …while a new top-level comic bumps the root mtime and rescans.
        std::thread::sleep(std::time::Duration::from_millis(20));
        touch(&d.path().join("D新漫画/1.jpg"));
        assert_eq!(futures::executor::block_on(src.explore(1)).unwrap().items.len(), 4);
    }

    #[test]
    fn paginate_has_more_exact() {
        let mk = |n: usize| {
            (0..n).map(|i| ComicSummary {
                id: i.to_string(),
                title: i.to_string(),
                subtitle: None,
                cover: None,
                tags: vec![],
            })
        };
        assert!(!paginate(mk(PAGE_SIZE), 1).has_more);
        assert!(paginate(mk(PAGE_SIZE + 1), 1).has_more);
        assert_eq!(paginate(mk(PAGE_SIZE + 1), 2).items.len(), 1);
    }
}
