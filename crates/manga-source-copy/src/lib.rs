//! CopyManga (拷贝漫画) website source.
//!
//! An unofficial, read-only client for CopyManga's app JSON API, for local
//! personal use while studying GPUI. It implements the same [`ComicSource`]
//! trait as the local source, so the UI treats both the same way.
//!
//! See [`sign`] for how requests are authenticated and why. Request signing
//! uses a constant already published in the community source config; there is
//! no account secret or user data involved. Images are plain CDN URLs.
//!
//! Scope: explore / search / detail / pages (the reading flow). Login and
//! favorites are intentionally not implemented.

mod parse;
mod sign;

use std::{sync::Arc, time::Duration};

use futures::{FutureExt, future::BoxFuture};
use manga_core::{
    Capabilities, ChapterGroup, ComicDetail, ComicSource, ComicSummary, ImageRequest, Paged, Result, SourceError,
    SourceMeta,
};
use manga_net::Runtime;
use serde_json::Value;
use sign::Device;

pub const KEY: &str = "copy_manga";

/// Default API host. Mirrors change; override with `MANGA_COPY_API` or config.
pub const DEFAULT_API: &str = "api.copy2000.online";
const PAGE_SIZE: u64 = 30;
const CHAPTER_PAGE: u64 = 100;
/// Keep remote browsing bounded; search is the real discovery tool.
const BROWSE_PAGE_CAP: u32 = 5;

pub struct CopySource {
    meta: SourceMeta,
    runtime: Runtime,
    client: reqwest::Client,
    /// e.g. `https://api.copy2000.online`
    base: Arc<str>,
    device: Device,
    token: Option<String>,
    region: String,
    quality: String,
}

impl CopySource {
    pub fn new(runtime: Runtime, api_host: Option<String>, token: Option<String>) -> Self {
        let host = api_host.filter(|h| !h.trim().is_empty()).unwrap_or_else(|| DEFAULT_API.to_string());
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            meta: SourceMeta {
                key: KEY,
                name: "拷贝漫画",
                capabilities: Capabilities {
                    search: true,
                    cache_images: true,
                    incremental_browse: true,
                    ..Default::default()
                },
            },
            runtime,
            client,
            base: format!("https://{}", host.trim_end_matches('/')).into(),
            device: Device::generate(),
            token,
            region: "0".to_string(),
            quality: "1500".to_string(),
        }
    }

    /// Everything an HTTP call needs, cheaply cloneable into an async block.
    fn ctx(&self) -> Ctx {
        Ctx {
            client: self.client.clone(),
            base: self.base.clone(),
            device: self.device.clone(),
            token: self.token.clone(),
            region: self.region.clone(),
        }
    }

    fn run<T: Send + 'static>(
        &self,
        fut: impl Future<Output = Result<T>> + Send + 'static,
    ) -> BoxFuture<'static, Result<T>> {
        let pending = self.runtime.spawn(fut);
        async move { pending.await.unwrap_or_else(|| Err(SourceError::Other("请求已取消".into()))) }.boxed()
    }
}

#[derive(Clone)]
struct Ctx {
    client: reqwest::Client,
    base: Arc<str>,
    device: Device,
    token: Option<String>,
    region: String,
}

impl Ctx {
    /// GET `path` (relative to the API base), returning `results`.
    async fn get(&self, path: String) -> Result<Value> {
        let url = format!("{}{}", self.base, path);
        let headers = sign::headers(&self.device, self.token.as_deref(), &self.region);
        let mut rb = self.client.get(&url);
        for (k, v) in &headers {
            rb = rb.header(k, v);
        }
        let resp = rb.send().await.map_err(|e| SourceError::Network(e.to_string()))?;
        let code = resp.status().as_u16();
        let body = resp.bytes().await.map_err(|e| SourceError::Network(e.to_string()))?;
        match code {
            200 => {
                let v: Value = serde_json::from_slice(&body).map_err(|e| SourceError::Parse(e.to_string()))?;
                results_of(v)
            }
            // "Expected available in N seconds".
            210 => Err(SourceError::RateLimited(extract_wait(&String::from_utf8_lossy(&body)).unwrap_or(40))),
            401 => Err(SourceError::LoginRequired),
            404 => Err(SourceError::NotFound(url)),
            other => Err(SourceError::Network(format!("HTTP {other}"))),
        }
    }
}

fn results_of(v: Value) -> Result<Value> {
    if let Some(r) = v.get("results").filter(|r| !r.is_null()) {
        return Ok(r.clone());
    }
    let msg = v.get("message").and_then(Value::as_str).unwrap_or("接口无结果").to_string();
    Err(SourceError::Other(msg))
}

fn extract_wait(body: &str) -> Option<u64> {
    let i = body.find("in ")? + 3;
    let rest = &body[i..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

async fn load_chapters(ctx: &Ctx, comic_id: &str, groups: &[(String, String)]) -> Result<Vec<ChapterGroup>> {
    let mut out = Vec::new();
    for (path_word, name) in groups {
        let mut chapters = Vec::new();
        let mut offset = 0u64;
        loop {
            let path = format!(
                "/api/v3/comic/{comic_id}/group/{path_word}/chapters?limit={CHAPTER_PAGE}&offset={offset}&platform=3"
            );
            let results = ctx.get(path).await?;
            let (mut page, total) = parse::parse_chapter_page(&results);
            chapters.append(&mut page);
            offset += CHAPTER_PAGE;
            if offset >= total || chapters.is_empty() {
                break;
            }
        }
        if !chapters.is_empty() {
            out.push(ChapterGroup { name: name.clone(), chapters });
        }
    }
    if out.is_empty() {
        return Err(SourceError::NotFound("没有章节".into()));
    }
    Ok(out)
}

impl ComicSource for CopySource {
    fn meta(&self) -> &SourceMeta {
        &self.meta
    }

    fn explore(&self, page: u32) -> BoxFuture<'static, Result<Paged<ComicSummary>>> {
        let ctx = self.ctx();
        let page = page.clamp(1, BROWSE_PAGE_CAP);
        self.run(async move {
            let offset = (page as u64 - 1) * PAGE_SIZE;
            let path =
                format!("/api/v3/comics?limit={PAGE_SIZE}&offset={offset}&ordering=-datetime_updated&platform=3");
            let results = ctx.get(path).await?;
            let mut paged = parse::parse_list(&results, PAGE_SIZE, page);
            if page >= BROWSE_PAGE_CAP {
                paged.has_more = false;
            }
            Ok(paged)
        })
    }

    fn search(&self, query: &str, page: u32) -> BoxFuture<'static, Result<Paged<ComicSummary>>> {
        let ctx = self.ctx();
        let page = page.max(1);
        let q = urlencode(query.trim());
        self.run(async move {
            let offset = (page as u64 - 1) * PAGE_SIZE;
            let path = format!("/api/v3/search/comic?limit={PAGE_SIZE}&offset={offset}&q={q}&q_type=&platform=3");
            let results = ctx.get(path).await?;
            Ok(parse::parse_list(&results, PAGE_SIZE, page))
        })
    }

    fn detail(&self, comic_id: &str) -> BoxFuture<'static, Result<ComicDetail>> {
        let ctx = self.ctx();
        let id = comic_id.to_string();
        self.run(async move {
            let path = format!("/api/v3/comic2/{id}?in_mainland=true&platform=3");
            let results = ctx.get(path).await?;
            let mut detail = parse::parse_detail(&results, &id)?;
            let groups = parse::parse_groups(&results);
            detail.groups = load_chapters(&ctx, &id, &groups).await?;
            Ok(detail)
        })
    }

    fn pages(&self, comic_id: &str, chapter_id: &str) -> BoxFuture<'static, Result<Vec<ImageRequest>>> {
        let ctx = self.ctx();
        let (id, ep) = (comic_id.to_string(), chapter_id.to_string());
        let quality = self.quality.clone();
        self.run(async move {
            let path = format!("/api/v3/comic/{id}/chapter2/{ep}?in_mainland=true&platform=3");
            let results = ctx.get(path).await?;
            parse::parse_pages(&results, &quality)
        })
    }
}

/// Minimal percent-encoding for query values (keeps the dep surface small).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_wait_parses_message() {
        assert_eq!(extract_wait("Expected available in 37 seconds"), Some(37));
        assert_eq!(extract_wait("no number here"), None);
    }

    #[test]
    fn results_of_reports_message_on_missing() {
        let err = results_of(serde_json::json!({"code": 400, "message": "参数错误"})).unwrap_err();
        assert!(matches!(err, SourceError::Other(m) if m == "参数错误"));
    }

    #[test]
    fn urlencode_handles_cjk_and_spaces() {
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("海贼"), "%E6%B5%B7%E8%B4%BC");
    }
}
