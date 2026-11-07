//! Pure JSON → domain mapping. No network, no `self`: every function here is
//! unit-tested against sample response bodies.

use manga_core::{Chapter, ComicDetail, ComicSummary, ImageRequest, Paged, SourceError};
use serde_json::Value;

type Result<T> = std::result::Result<T, SourceError>;

/// `results.list` / `results.total` listings (search, category, favorites).
pub fn parse_list(results: &Value, page_size: u64, page: u32) -> Paged<ComicSummary> {
    let items: Vec<ComicSummary> =
        results.get("list").and_then(Value::as_array).map(|a| a.iter().map(parse_comic).collect()).unwrap_or_default();
    let total = results.get("total").and_then(Value::as_u64).unwrap_or(0);
    let seen = (page as u64) * page_size;
    Paged { items, has_more: seen < total }
}

/// A comic object, possibly wrapped as `{ "comic": { … } }`.
pub fn parse_comic(v: &Value) -> ComicSummary {
    let c = v.get("comic").filter(|c| !c.is_null()).unwrap_or(v);
    let author = c
        .get("author")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|a| str_at(a, "name"))
        .filter(|s| !s.is_empty());
    let tags = named_list(c.get("theme"));
    ComicSummary {
        id: str_at(c, "path_word").unwrap_or_default(),
        title: str_at(c, "name").unwrap_or_default(),
        subtitle: author,
        cover: str_at(c, "cover").filter(|s| !s.is_empty()).map(http_image),
        tags,
    }
}

/// `comic2/{id}` detail body (without chapters, which load separately).
pub fn parse_detail(results: &Value, id: &str) -> Result<ComicDetail> {
    let comic = results.get("comic").ok_or_else(|| SourceError::Parse("缺少 comic 字段".into()))?;
    let summary = parse_comic(comic);
    let summary = ComicSummary { id: id.to_string(), ..summary };

    let mut meta = std::collections::BTreeMap::new();
    let authors = comic
        .get("author")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| str_at(x, "name")).collect::<Vec<_>>())
        .unwrap_or_default();
    if !authors.is_empty() {
        meta.insert("作者".to_string(), authors);
    }
    let themes = named_list(comic.get("theme"));
    if !themes.is_empty() {
        meta.insert("标签".to_string(), themes);
    }
    if let Some(status) = comic.get("status").and_then(|s| str_at(s, "display")) {
        meta.insert("状态".to_string(), vec![status]);
    }
    if let Some(updated) = str_at(comic, "datetime_updated").filter(|s| !s.is_empty()) {
        meta.insert("更新".to_string(), vec![updated]);
    }

    let description = str_at(comic, "brief").filter(|s| !s.is_empty());
    Ok(ComicDetail { summary, description, meta, groups: vec![] })
}

/// Ordered `{ path_word, name }` groups from a detail body.
pub fn parse_groups(results: &Value) -> Vec<(String, String)> {
    let mut groups: Vec<(String, String)> = results
        .get("groups")
        .and_then(Value::as_object)
        .map(|m| {
            m.values().filter_map(|g| Some((str_at(g, "path_word")?, str_at(g, "name").unwrap_or_default()))).collect()
        })
        .unwrap_or_default();
    // "default" first, then the rest; keeps the main series before extras.
    groups.sort_by_key(|(pw, _)| if pw == "default" { 0 } else { 1 });
    groups
}

/// One page of a chapter listing: `(uuid, name)` and the total count.
pub fn parse_chapter_page(results: &Value) -> (Vec<Chapter>, u64) {
    let list = results
        .get("list")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| Some(Chapter { id: str_at(e, "uuid")?, title: str_at(e, "name").unwrap_or_default() }))
                .collect()
        })
        .unwrap_or_default();
    let total = results.get("total").and_then(Value::as_u64).unwrap_or(0);
    (list, total)
}

/// `chapter2/{ep}` image list, reordered by `words` and quality-rewritten.
pub fn parse_pages(results: &Value, quality: &str) -> Result<Vec<ImageRequest>> {
    let chapter = results.get("chapter").ok_or_else(|| SourceError::Parse("缺少 chapter 字段".into()))?;
    let urls: Vec<String> = chapter
        .get("contents")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|c| str_at(c, "url")).collect())
        .unwrap_or_default();
    let words: Vec<usize> = chapter
        .get("words")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|w| w.as_u64().map(|n| n as usize)).collect())
        .unwrap_or_default();
    if urls.is_empty() {
        return Err(SourceError::NotFound("本章没有图片".into()));
    }

    // `words[i]` is the display position of `contents[i]`. Fall back to source
    // order when the arrays disagree.
    let mut ordered = vec![String::new(); urls.len()];
    if words.len() == urls.len() && words.iter().all(|&w| w < urls.len()) {
        for (i, url) in urls.iter().enumerate() {
            ordered[words[i]] = url.clone();
        }
    } else {
        ordered = urls;
    }

    Ok(ordered.into_iter().map(|u| http_image(rewrite_quality(&u, quality))).collect())
}

/// Replace the CDN quality suffix, e.g. `….c800x.jpg` → `….c1500x.webp`.
/// Matches the JS regex `([./])c\d+x\.[a-zA-Z]+$`; other URLs pass through.
fn rewrite_quality(url: &str, quality: &str) -> String {
    let Some(dot) = url.rfind('.') else { return url.to_string() };
    let before = &url[..dot]; // strips ".<ext>"
    let Some(sep) = before.rfind(['/', '.']) else { return url.to_string() };
    let seg = &before[sep + 1..]; // e.g. "c800x"
    let is_quality_seg = seg
        .strip_prefix('c')
        .and_then(|s| s.strip_suffix('x'))
        .is_some_and(|mid| !mid.is_empty() && mid.bytes().all(|b| b.is_ascii_digit()));
    if is_quality_seg {
        format!("{}{}c{quality}x.webp", &before[..sep], &before[sep..sep + 1])
    } else {
        url.to_string()
    }
}

fn http_image(url: String) -> ImageRequest {
    // CopyManga images are plain CDN URLs; no auth headers needed.
    ImageRequest::Http { url, headers: vec![] }
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn named_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(|t| str_at(t, "name")).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comic_unwraps_and_maps() {
        let v = json!({"comic": {
            "path_word": "douluo", "name": "斗罗大陆", "cover": "https://cdn/x.jpg",
            "author": [{"name": "唐家三少", "path_word": "a"}],
            "theme": [{"name": "热血"}, {"name": "奇幻"}]
        }});
        let c = parse_comic(&v);
        assert_eq!(c.id, "douluo");
        assert_eq!(c.title, "斗罗大陆");
        assert_eq!(c.subtitle.as_deref(), Some("唐家三少"));
        assert_eq!(c.tags, ["热血", "奇幻"]);
        assert!(matches!(c.cover, Some(ImageRequest::Http { .. })));
    }

    #[test]
    fn list_has_more_tracks_total() {
        let results = json!({"list": [{"path_word":"a","name":"A"}], "total": 100});
        let p = parse_list(&results, 30, 1);
        assert_eq!(p.items.len(), 1);
        assert!(p.has_more); // 30 < 100
        let last = parse_list(&results, 30, 4);
        assert!(!last.has_more); // 120 >= 100
    }

    #[test]
    fn detail_builds_meta_and_groups() {
        let results = json!({
            "comic": {
                "path_word": "x", "name": "N", "cover": "c", "brief": "简介",
                "author": [{"name": "作者1"}],
                "theme": [{"name": "标签1"}],
                "status": {"display": "连载中"},
                "datetime_updated": "2026-01-01"
            },
            "groups": {
                "default": {"path_word": "default", "name": "单话"},
                "bonus": {"path_word": "bonus", "name": "番外"}
            }
        });
        let d = parse_detail(&results, "x").unwrap();
        assert_eq!(d.summary.id, "x");
        assert_eq!(d.description.as_deref(), Some("简介"));
        assert_eq!(d.meta.get("作者").unwrap(), &vec!["作者1".to_string()]);
        assert_eq!(d.meta.get("状态").unwrap(), &vec!["连载中".to_string()]);
        let g = parse_groups(&results);
        assert_eq!(g[0].0, "default"); // default sorts first
    }

    #[test]
    fn chapters_map_uuid_and_name() {
        let results = json!({"list": [{"uuid":"u1","name":"第1话"},{"uuid":"u2","name":"第2话"}], "total": 2});
        let (chs, total) = parse_chapter_page(&results);
        assert_eq!(total, 2);
        assert_eq!(chs[1].id, "u2");
        assert_eq!(chs[1].title, "第2话");
    }

    #[test]
    fn pages_reorder_by_words_and_rewrite_quality() {
        let results = json!({"chapter": {
            "contents": [{"url":"https://cdn/a.c800x.jpg"}, {"url":"https://cdn/b.c800x.jpg"}],
            "words": [1, 0]
        }});
        let pages = parse_pages(&results, "1500").unwrap();
        // words say contents[0]→pos1, contents[1]→pos0, so b then a.
        let urls: Vec<String> = pages
            .iter()
            .map(|p| match p {
                ImageRequest::Http { url, .. } => url.clone(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(urls, ["https://cdn/b.c1500x.webp", "https://cdn/a.c1500x.webp"]);
    }

    #[test]
    fn pages_fall_back_when_words_mismatch() {
        let results = json!({"chapter": {
            "contents": [{"url":"https://cdn/a.webp"}, {"url":"https://cdn/b.webp"}],
            "words": [5]
        }});
        let pages = parse_pages(&results, "1500").unwrap();
        assert_eq!(pages.len(), 2);
    }

    #[test]
    fn rewrite_quality_leaves_odd_urls_alone() {
        assert_eq!(rewrite_quality("https://cdn/plain.webp", "1500"), "https://cdn/plain.webp");
        assert_eq!(rewrite_quality("https://cdn/x.c6x.jpg", "1500"), "https://cdn/x.c1500x.webp");
    }

    #[test]
    fn empty_pages_is_not_found() {
        let results = json!({"chapter": {"contents": [], "words": []}});
        assert!(matches!(parse_pages(&results, "1500"), Err(SourceError::NotFound(_))));
    }
}
