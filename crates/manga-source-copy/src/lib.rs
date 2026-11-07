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
const BOOTSTRAP_URL: &str = "https://api.copy-manga.com/api/v3/system/network2?platform=3";
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
    initialized: Arc<futures::lock::Mutex<bool>>,
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
            initialized: Arc::default(),
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
            initialized: self.initialized.clone(),
            bootstrap_url: BOOTSTRAP_URL.into(),
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
    initialized: Arc<futures::lock::Mutex<bool>>,
    bootstrap_url: Arc<str>,
}

impl Ctx {
    /// GET `path` (relative to the API base), returning `results`.
    async fn get(&self, path: String) -> Result<Value> {
        {
            let mut initialized = self.initialized.lock().await;
            if !*initialized {
                // Venera initializes the app API before browsing with this identity.
                self.request(&self.bootstrap_url).await?;
                *initialized = true;
            }
        }
        let url = format!("{}{}", self.base, path);
        self.request(&url).await
    }

    async fn request(&self, url: &str) -> Result<Value> {
        let headers = sign::headers(&self.device, self.token.as_deref(), &self.region);
        let mut rb = self.client.get(url);
        for (k, v) in &headers {
            rb = rb.header(k, v);
        }
        let resp = rb.send().await.map_err(|e| SourceError::Network(e.to_string()))?;
        let code = resp.status().as_u16();
        let retry_after =
            resp.headers().get(reqwest::header::RETRY_AFTER).and_then(|h| h.to_str().ok()).map(str::to_owned);
        let body = resp.bytes().await.map_err(|e| SourceError::Network(e.to_string()))?;
        decode_response(code, retry_after.as_deref(), &body, url)
    }
}

fn decode_response(code: u16, retry_after: Option<&str>, body: &[u8], url: &str) -> Result<Value> {
    if code == 200 {
        let v = serde_json::from_slice(body).map_err(|e| SourceError::Parse(e.to_string()))?;
        return results_of(v, retry_after, url);
    }
    let v = serde_json::from_slice(body).unwrap_or(Value::Null);
    Err(response_error(code as u64, &v, retry_after, url))
}

fn results_of(v: Value, retry_after: Option<&str>, url: &str) -> Result<Value> {
    // Error envelopes can contain `results.detail`; it is not successful data.
    if let Some(code) = v.get("code").and_then(Value::as_u64).filter(|&code| code != 200) {
        return Err(response_error(code, &v, retry_after, url));
    }
    if let Some(r) = v.get("results").filter(|r| !r.is_null()) {
        return Ok(r.clone());
    }
    let msg = v.get("message").and_then(Value::as_str).unwrap_or("接口无结果").to_string();
    Err(SourceError::Other(msg))
}

fn response_error(code: u64, v: &Value, retry_after: Option<&str>, url: &str) -> SourceError {
    let msg = v
        .pointer("/results/detail")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| v.get("message").and_then(Value::as_str).filter(|s| !s.trim().is_empty()));
    match code {
        210 | 429 => {
            // 210 also means client restrictions; never invent a cooldown.
            let wait = if code == 429 {
                retry_after.and_then(|s| {
                    s.parse::<u64>().ok().or_else(|| {
                        let date = chrono::DateTime::parse_from_rfc2822(s).ok()?;
                        Some((date.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64)
                    })
                })
            } else {
                None
            };
            if let Some(wait) = wait.or_else(|| msg.and_then(extract_wait)) {
                SourceError::RateLimited(wait)
            } else {
                SourceError::Other(format!(
                    "{}：{}",
                    if code == 429 { "请求过于频繁" } else { "站点拒绝请求（210）" },
                    msg.unwrap_or("服务端未提供具体原因，请稍后手动重试")
                ))
            }
        }
        401 => SourceError::LoginRequired,
        404 => SourceError::NotFound(if url.is_empty() { msg.unwrap_or("接口资源").into() } else { url.into() }),
        _ => match msg {
            Some(msg) => SourceError::Other(msg.into()),
            None => SourceError::Network(format!("HTTP {code}")),
        },
    }
}

fn extract_wait(body: &str) -> Option<u64> {
    let rest = body.split_once("Expected available in ")?.1;
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[end..].starts_with(" seconds").then(|| rest[..end].parse().ok()).flatten()
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

    fn mock_api(responses: Vec<(u16, &'static str)>) -> (Ctx, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            for (status, body) in responses {
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            if std::time::Instant::now() >= deadline {
                                return requests;
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("mock accept: {e}"),
                    }
                };
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buf).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                }
                requests.push(String::from_utf8(request).unwrap());
                write!(socket, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        let ctx = Ctx {
            client: reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().unwrap(),
            bootstrap_url: format!("{base}/bootstrap").into(),
            base: base.into(),
            device: Device::generate(),
            token: None,
            region: "0".into(),
            initialized: Arc::default(),
        };
        (ctx, server)
    }

    #[test]
    fn initializes_once_before_concurrent_api_requests_with_same_identity() {
        const OK: &str = r#"{"code":200,"results":{}}"#;
        let (ctx, server) = mock_api(vec![(200, OK); 3]);
        let other = ctx.clone();
        let rt = Runtime::new().unwrap();
        let (a, b) = futures::executor::block_on(
            rt.spawn(async move { futures::join!(ctx.get("/detail".into()), other.get("/chapters".into())) }),
        )
        .unwrap();
        assert!(a.is_ok() && b.is_ok());
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with("GET /bootstrap "));
        for path in ["/detail", "/chapters"] {
            assert!(requests[1..].iter().any(|r| r.starts_with(&format!("GET {path} "))));
        }
        for key in ["device", "deviceinfo", "pseudoid", "authorization", "region"] {
            let header = |request: &String| {
                request
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case(key).then(|| value.trim().to_owned())
                    })
                    .unwrap()
            };
            assert!(requests.iter().all(|r| header(r) == header(&requests[0])), "changed {key}");
        }
    }

    #[test]
    fn failed_initialization_blocks_api_and_can_be_retried() {
        const OK: &str = r#"{"code":200,"results":{}}"#;
        let (ctx, server) = mock_api(vec![(503, "unavailable"), (200, OK), (200, OK)]);
        let rt = Runtime::new().unwrap();
        futures::executor::block_on(rt.spawn(async move {
            assert!(ctx.get("/detail".into()).await.is_err());
            assert!(ctx.get("/detail".into()).await.is_ok());
        }))
        .unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with("GET /bootstrap "));
        assert!(requests[1].starts_with("GET /bootstrap "));
        assert!(requests[2].starts_with("GET /detail "));
    }

    #[test]
    fn extract_wait_parses_message() {
        assert_eq!(extract_wait("Expected available in 37 seconds"), Some(37));
        assert_eq!(extract_wait("no number here"), None);
        assert_eq!(extract_wait("Expected available in 37 minutes"), None);
        assert_eq!(extract_wait("Something in 37 seconds"), None);
    }

    #[test]
    fn results_of_reports_message_on_missing() {
        let err = results_of(serde_json::json!({"code": 400, "message": "参数错误"}), None, "").unwrap_err();
        assert!(matches!(err, SourceError::Other(m) if m == "参数错误"));
    }

    #[test]
    fn results_of_rejects_business_error_with_results() {
        let err = results_of(
            serde_json::json!({
                "code": 210,
                "message": "請到官網更新最新APP，安裝正版之後等待1小時",
                "results": {"detail": "請到官網更新最新APP，安裝正版之後等待1小時"}
            }),
            None,
            "",
        )
        .unwrap_err();
        assert!(matches!(err, SourceError::Other(m) if m.contains("等待1小時")));
    }

    #[test]
    fn http_210_preserves_client_restriction_instead_of_default_wait() {
        let body = serde_json::json!({
            "code": 210,
            "message": "請到官網更新最新APP(https://www.copy4000.com/download), 您或您身邊的人曾經下載過破解版本的拷貝漫畫，請下載安裝正版之後等待1小時，限制會自動解除。"
        }).to_string();
        let err = decode_response(210, Some("40"), body.as_bytes(), "https://example.test").unwrap_err();
        assert!(matches!(err, SourceError::Other(m) if m.contains("等待1小時") && !m.contains("40 秒")));
    }

    #[test]
    fn real_rate_limits_use_only_reported_wait_times() {
        let body = br#"{"message":"Expected available in 37 seconds"}"#;
        for code in [210, 429] {
            assert!(matches!(decode_response(code, None, body, ""), Err(SourceError::RateLimited(37))));
        }
        assert!(matches!(decode_response(429, Some("61"), body, ""), Err(SourceError::RateLimited(61))));
        assert!(matches!(
            decode_response(429, Some("Wed, 21 Oct 2015 07:28:00 GMT"), b"", ""),
            Err(SourceError::RateLimited(0))
        ));
        for code in [210, 429] {
            assert!(matches!(decode_response(code, Some("invalid"), b"", ""), Err(SourceError::Other(_))));
        }
    }

    #[test]
    fn business_errors_preserve_http_response_context() {
        assert!(matches!(
            decode_response(200, Some("61"), br#"{"code":429,"message":"Too many requests"}"#, "comic-url"),
            Err(SourceError::RateLimited(61))
        ));
        assert!(matches!(
            decode_response(200, None, br#"{"code":404,"message":"Not found"}"#, "comic-url"),
            Err(SourceError::NotFound(url)) if url == "comic-url"
        ));
    }

    #[test]
    fn decode_response_handles_success_and_business_errors() {
        assert_eq!(
            decode_response(200, None, br#"{"code":200,"results":{"list":[]}}"#, "").unwrap(),
            serde_json::json!({"list":[]})
        );
        assert!(matches!(
            decode_response(200, None, br#"{"code":401,"results":{"detail":"login"}}"#, ""),
            Err(SourceError::LoginRequired)
        ));
        assert!(matches!(decode_response(401, None, b"", ""), Err(SourceError::LoginRequired)));
        assert!(
            matches!(decode_response(404, None, b"", "comic-url"), Err(SourceError::NotFound(m)) if m == "comic-url")
        );
        assert!(
            matches!(decode_response(503, None, b"unavailable", ""), Err(SourceError::Network(m)) if m == "HTTP 503")
        );
        assert!(matches!(decode_response(200, None, b"invalid-json", ""), Err(SourceError::Parse(_))));
    }

    #[test]
    fn urlencode_handles_cjk_and_spaces() {
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("海贼"), "%E6%B5%B7%E8%B4%BC");
    }
}
