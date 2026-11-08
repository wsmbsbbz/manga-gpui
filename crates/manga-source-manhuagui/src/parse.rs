//! Pure HTML → domain mapping. No network: every function is unit-tested
//! against trimmed copies of real pages.

use std::collections::BTreeMap;

use manga_core::{Chapter, ChapterGroup, ComicDetail, ComicSummary, ImageRequest, Paged, SourceError};
use scraper::{ElementRef, Html, Selector};

use crate::{lzstring, unpack::ImgData};

type Result<T> = std::result::Result<T, SourceError>;

pub const REFERER: &str = "https://www.manhuagui.com/";
pub const IMAGE_HOST: &str = "https://i.hamreus.com";

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("static selector")
}

fn text(e: ElementRef) -> String {
    e.text().collect::<String>().trim().to_string()
}

/// Text of an element's own text nodes, ignoring children like `<i>12p</i>`.
fn own_text(e: ElementRef) -> String {
    e.children().filter_map(|n| n.value().as_text()).map(|t| &**t).collect::<String>().trim().to_string()
}

/// `/comic/1128/` → `1128`.
fn comic_id_from_href(href: &str) -> Option<String> {
    let id = href.split('/').filter(|s| !s.is_empty()).nth(1)?;
    href.contains("/comic/").then(|| id.to_string()).filter(|s| !s.is_empty())
}

fn absolute(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else if url.starts_with('/') {
        format!("https://www.manhuagui.com{url}")
    } else {
        url.to_string()
    }
}

pub fn cover_image(url: &str) -> ImageRequest {
    // The cover CDN does not check Referer, but sending it is harmless and
    // mirrors what the website does.
    ImageRequest::Http { url: absolute(url), headers: vec![("Referer".into(), REFERER.into())] }
}

fn img_src(e: ElementRef) -> Option<String> {
    // Lazy-loaded grid covers keep the real URL in `data-src`.
    let v = e.value();
    v.attr("data-src").or_else(|| v.attr("src")).filter(|s| !s.trim().is_empty()).map(str::to_string)
}

/// `<strong>` texts of `.result-count`.
fn result_counts(doc: &Html) -> Vec<u64> {
    doc.select(&sel(".result-count strong")).filter_map(|s| text(s).replace(',', "").parse().ok()).collect()
}

/// Category / browse listing (`/list/..._pN.html`): `#contList > li`.
pub fn parse_list(html: &str) -> Paged<ComicSummary> {
    let doc = Html::parse_document(html);
    let items = doc.select(&sel("#contList > li")).filter_map(parse_grid_item).collect();
    // "第 <strong>1</strong> / <strong>1482</strong> 页"
    let counts = result_counts(&doc);
    let has_more = matches!(counts.as_slice(), [cur, max, ..] if cur < max);
    Paged { items, has_more }
}

fn parse_grid_item(li: ElementRef) -> Option<ComicSummary> {
    let a = li.select(&sel(".ell > a")).next()?;
    let id = comic_id_from_href(a.value().attr("href")?)?;
    let title = a.value().attr("title").map(str::to_string).unwrap_or_else(|| text(a));
    let cover = li.select(&sel("img")).next().and_then(img_src).map(|u| cover_image(&u));
    let subtitle = li.select(&sel(".tt")).next().map(text).filter(|s| !s.is_empty());
    let mut tags = vec![];
    if let Some(up) = li.select(&sel(".updateon")).next() {
        let date = own_text(up).replace("更新于：", "");
        if !date.is_empty() {
            tags.push(date);
        }
    }
    Some(ComicSummary { id, title, subtitle, cover, tags })
}

/// Search results (`/s/<kw>_pN.html`): `.book-result li.cf`, 10 per page.
pub fn parse_search(html: &str, page: u32) -> Paged<ComicSummary> {
    const PER_PAGE: u64 = 10;
    let doc = Html::parse_document(html);
    let items: Vec<_> = doc.select(&sel(".book-result li.cf")).filter_map(parse_search_item).collect();
    // `搜索“<strong>kw</strong>”共查找到 <strong>17</strong> 部漫画`: the total
    // is the last number (the keyword itself may or may not be numeric).
    let total = doc.select(&sel(".result-count strong")).last().and_then(|s| text(s).parse::<u64>().ok());
    let has_more = total.is_some_and(|t| (page as u64) * PER_PAGE < t) && !items.is_empty();
    Paged { items, has_more }
}

fn parse_search_item(li: ElementRef) -> Option<ComicSummary> {
    let a = li.select(&sel(".book-detail dt > a")).next()?;
    let id = comic_id_from_href(a.value().attr("href")?)?;
    let title = text(a);
    let cover = li.select(&sel(".book-cover img")).next().and_then(img_src).map(|u| cover_image(&u));
    let mut subtitle = None;
    let mut tags = vec![];
    for span in li.select(&sel(".book-detail dd.tags > span")) {
        let label = span.select(&sel("strong")).next().map(text).unwrap_or_default();
        let links: Vec<String> = span.select(&sel("a")).map(text).filter(|s| !s.is_empty()).collect();
        match label.trim_end_matches('：') {
            "作者" => subtitle = Some(links.join(" / ")).filter(|s| !s.is_empty()),
            "类型" => tags.extend(links),
            "状态" => tags.extend(span.select(&sel(".red")).next().map(text)),
            _ => {}
        }
    }
    Some(ComicSummary { id, title, subtitle, cover, tags })
}

/// Comic page (`/comic/<id>/`). Chapters are in the page, or for
/// adult-gated comics LZString-compressed in `#__VIEWSTATE`.
pub fn parse_detail(html: &str, id: &str) -> Result<ComicDetail> {
    let doc = Html::parse_document(html);
    let book = doc.select(&sel(".book-cont")).next().ok_or_else(|| SourceError::Parse("缺少 .book-cont".into()))?;
    let title = book.select(&sel(".book-title h1")).next().map(text).unwrap_or_default();
    if title.is_empty() {
        return Err(SourceError::Parse("缺少标题".into()));
    }
    let alt = book.select(&sel(".book-title h2")).next().map(text).filter(|s| !s.is_empty());
    let cover = book.select(&sel(".hcover img")).next().and_then(img_src).map(|u| cover_image(&u));

    let description = book
        .select(&sel("#intro-all"))
        .next()
        .map(|e| {
            let ps: Vec<String> = e.select(&sel("p")).map(text).filter(|s| !s.is_empty()).collect();
            if ps.is_empty() { text(e) } else { ps.join("\n") }
        })
        .or_else(|| book.select(&sel("#intro-cut")).next().map(text))
        .filter(|s| !s.is_empty());

    let mut meta = BTreeMap::new();
    let mut tags = vec![];
    for span in book.select(&sel(".detail-list li > span")) {
        let Some(label) = span.select(&sel("strong")).next().map(text) else { continue };
        let key = label.trim_end_matches('：').trim_start_matches("漫画").trim_start_matches("出品").to_string();
        if span
            .parent()
            .and_then(ElementRef::wrap)
            .is_some_and(|li| li.value().has_class("status", scraper::CaseSensitivity::CaseSensitive))
        {
            let reds: Vec<String> = span.select(&sel(".red")).map(text).collect();
            if let Some(s) = reds.first() {
                meta.insert("状态".to_string(), vec![s.clone()]);
            }
            if let Some(d) = reds.get(1) {
                meta.insert("更新".to_string(), vec![d.clone()]);
            }
            continue;
        }
        let values: Vec<String> = span.select(&sel("a")).map(text).filter(|s| !s.is_empty()).collect();
        if values.is_empty() || key == "字母索引" {
            continue;
        }
        if key == "剧情" {
            tags.extend(values.iter().cloned());
        }
        meta.insert(key, values);
    }
    let subtitle = meta.get("作者").map(|a| a.join(" / ")).or(alt.clone());
    if let Some(alt) = alt {
        meta.entry("别名".to_string()).or_insert_with(Vec::new).insert(0, alt);
    }

    let groups = parse_chapter_groups(&doc);
    Ok(ComicDetail {
        summary: ComicSummary { id: id.to_string(), title, subtitle, cover, tags },
        description,
        meta,
        groups,
    })
}

fn parse_chapter_groups(doc: &Html) -> Vec<ChapterGroup> {
    let groups = chapter_groups_in(doc);
    if !groups.is_empty() {
        return groups;
    }
    // Adult-gated comics: the chapter block is shipped compressed.
    let state = doc.select(&sel("#__VIEWSTATE")).next().and_then(|e| e.value().attr("value").map(str::to_string));
    let Some(html) = state.and_then(|v| lzstring::decompress_from_base64(&v)) else { return vec![] };
    let frag = Html::parse_fragment(&format!("<div class=\"chapter\">{html}</div>"));
    chapter_groups_in(&frag)
}

/// Pairs each `.chapter h4` title with the `.chapter-list` that follows it.
fn chapter_groups_in(doc: &Html) -> Vec<ChapterGroup> {
    let names: Vec<String> = doc.select(&sel(".chapter h4")).map(text).collect();
    let mut out = vec![];
    for (i, list) in doc.select(&sel(".chapter-list")).enumerate() {
        // Blocks (`<ul>`, ~90 chapters each) run oldest → newest, but each
        // block lists newest first; reverse within each block.
        let blocks: Vec<ElementRef> = list.select(&sel("ul")).collect();
        let blocks = if blocks.is_empty() { vec![list] } else { blocks };
        let chapters: Vec<Chapter> = blocks
            .into_iter()
            .flat_map(|ul| {
                let mut block: Vec<Chapter> = ul.select(&sel("li > a")).filter_map(parse_chapter_link).collect();
                block.reverse();
                block
            })
            .collect();
        if chapters.is_empty() {
            continue;
        }
        let name = names.get(i).cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| "连载".to_string());
        out.push(ChapterGroup { name, chapters });
    }
    out
}

fn parse_chapter_link(a: ElementRef) -> Option<Chapter> {
    let href = a.value().attr("href")?;
    let id = href.rsplit('/').next()?.trim_end_matches(".html").to_string();
    if id.is_empty() {
        return None;
    }
    let title = a
        .value()
        .attr("title")
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .or_else(|| a.select(&sel("span")).next().map(own_text))
        .unwrap_or_else(|| id.clone());
    Some(Chapter { id, title })
}

/// Page image requests from decoded `imgData`.
pub fn page_requests(data: &ImgData) -> Vec<ImageRequest> {
    let path = encode_path(&data.path);
    data.files
        .iter()
        .map(|f| ImageRequest::Http {
            url: format!("{IMAGE_HOST}{path}{}?e={}&m={}", encode_path(f), data.sl.e, data.sl.m),
            headers: vec![("Referer".into(), REFERER.into())],
        })
        .collect()
}

/// Percent-encodes everything outside the URL path-safe ASCII set. Paths
/// contain CJK chapter names; encoding here keeps cache keys stable.
fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = include_str!("../testdata/list.html");
    const SEARCH: &str = include_str!("../testdata/search.html");
    const DETAIL: &str = include_str!("../testdata/detail.html");

    #[test]
    fn parses_list_page() {
        let p = parse_list(LIST);
        assert_eq!(p.items.len(), 3);
        assert!(p.has_more);
        let first = &p.items[0];
        assert_eq!(first.id, "55256");
        assert_eq!(first.title, "大家的亲吻俱乐部");
        assert_eq!(first.subtitle.as_deref(), Some("更新至第37话"));
        assert_eq!(first.tags, vec!["2026-11-08"]);
        assert!(
            matches!(&first.cover, Some(ImageRequest::Http { url, .. }) if url == "https://cf.mhgui.com/cpic/b/55256.jpg")
        );
        // Lazy-loaded item uses data-src.
        assert!(matches!(&p.items[2].cover, Some(ImageRequest::Http { url, .. }) if url.ends_with("44890_47.jpg")));
    }

    #[test]
    fn last_list_page_has_no_more() {
        let html =
            LIST.replace("<strong>1</strong> / <strong>1482</strong>", "<strong>9</strong> / <strong>9</strong>");
        assert!(!parse_list(&html).has_more);
    }

    #[test]
    fn parses_search_page() {
        let p = parse_search(SEARCH, 1);
        assert_eq!(p.items.len(), 2);
        assert!(p.has_more, "17 results, 10 per page");
        assert!(!parse_search(SEARCH, 2).has_more);
        let op = &p.items[0];
        assert_eq!(op.id, "1128");
        assert_eq!(op.title, "ONE PIECE航海王");
        assert_eq!(op.subtitle.as_deref(), Some("尾田荣一郎"));
        assert_eq!(op.tags, vec!["连载中", "热血", "冒险", "格斗"]);
    }

    #[test]
    fn search_without_results() {
        let p = parse_search("<html><body><div class=\"book-result\"></div></body></html>", 1);
        assert!(p.items.is_empty() && !p.has_more);
    }

    #[test]
    fn parses_detail_page() {
        let d = parse_detail(DETAIL, "44373").unwrap();
        assert_eq!(d.summary.title, "航海王学园");
        assert_eq!(d.summary.subtitle.as_deref(), Some("小路壮平"));
        assert_eq!(d.summary.tags, vec!["冒险"]);
        assert!(d.description.as_deref().unwrap().starts_with("克比转学"));
        assert_eq!(d.meta["状态"], vec!["连载中"]);
        assert_eq!(d.meta["更新"], vec!["2024-10-19"]);
        assert_eq!(d.meta["地区"], vec!["日本漫画"]);
        assert_eq!(d.meta["年代"], vec!["2022年"]);
        assert_eq!(d.meta["别名"], vec!["ONE PIECE学园", "海贼学园", "海贼王学院"]);
        assert!(!d.meta.contains_key("字母索引"));

        let names: Vec<_> = d.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["单话", "单行本"]);
        let ids: Vec<_> = d.groups[0].chapters.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["731706", "731707", "769873", "782036"], "oldest first across <ul> blocks");
        assert_eq!(d.groups[0].chapters[3].title, "第50话");
        assert_eq!(d.groups[1].chapters[0].title, "第01卷");
    }

    #[test]
    fn empty_viewstate_yields_no_chapters() {
        // An empty/undecodable view state must not fabricate chapters.
        let html = "<div class=\"book-cont\"><div class=\"book-title\"><h1>x</h1></div></div><input id=\"__VIEWSTATE\" value=\"\">";
        let d = parse_detail(html, "1").unwrap();
        assert!(d.groups.is_empty());
    }

    #[test]
    fn detail_without_book_is_parse_error() {
        assert!(matches!(parse_detail("<html></html>", "1"), Err(SourceError::Parse(_))));
    }

    #[test]
    fn page_urls_are_encoded_and_carry_referer() {
        let data = ImgData {
            files: vec!["a b.jpg.webp".into()],
            path: "/ps1/x/第50话/".into(),
            sl: crate::unpack::Sl { e: "1".into(), m: "abc".into() },
        };
        let reqs = page_requests(&data);
        let ImageRequest::Http { url, headers } = &reqs[0] else { panic!() };
        assert_eq!(url, "https://i.hamreus.com/ps1/x/%E7%AC%AC50%E8%AF%9D/a%20b.jpg.webp?e=1&m=abc");
        assert_eq!(headers, &vec![("Referer".to_string(), REFERER.to_string())]);
    }

    #[test]
    fn comic_ids_from_hrefs() {
        assert_eq!(comic_id_from_href("/comic/1128/").as_deref(), Some("1128"));
        assert_eq!(comic_id_from_href("/list/japan/"), None);
    }
}
