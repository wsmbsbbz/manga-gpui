//! ManHuaGui (漫画柜) website source.
//!
//! An unofficial, read-only client that reads the public website's HTML, for
//! local personal use. It implements the same [`ComicSource`] trait as the
//! other sources.
//!
//! - Browse: `/list/update_pN.html` (latest updates).
//! - Search: `/s/<keyword>_pN.html`.
//! - Detail: `/comic/<id>/`; chapters are in the page, or LZString-compressed
//!   in `#__VIEWSTATE` for adult-gated comics.
//! - Pages: `/comic/<id>/<chapter>.html` carries a packed script whose data is
//!   decoded natively (see [`unpack`]); no JavaScript is executed.
//!
//! The site bans IPs that crawl quickly, so page requests are serialized with
//! a minimum gap ([`MIN_GAP`]). Images come from a CDN that requires the site
//! as Referer; the image pipeline sends that header. Login, favorites and
//! comments are intentionally not implemented.

mod lzstring;
mod parse;
mod unpack;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use futures::{FutureExt, future::BoxFuture};
use manga_core::{
    Capabilities, ComicDetail, ComicSource, ComicSummary, ImageRequest, Paged, Result, SourceError, SourceMeta,
};
use manga_net::Runtime;

pub const KEY: &str = "manhuagui";
pub const BASE: &str = "https://www.manhuagui.com";
/// Minimum spacing between HTML page requests.
pub const MIN_GAP: Duration = Duration::from_millis(800);
/// Keep remote browsing bounded; search is the real discovery tool.
const BROWSE_PAGE_CAP: u32 = 5;
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36";

pub struct ManHuaGuiSource {
    meta: SourceMeta,
    runtime: Runtime,
    http: Http,
}

impl ManHuaGuiSource {
    pub fn new(runtime: Runtime) -> Self {
        Self::with_base(runtime, BASE)
    }

    fn with_base(runtime: Runtime, base: &str) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            meta: SourceMeta {
                key: KEY,
                name: "漫画柜",
                capabilities: Capabilities {
                    search: true,
                    cache_images: true,
                    incremental_browse: true,
                    ..Default::default()
                },
            },
            runtime,
            http: Http { client, base: base.trim_end_matches('/').into(), last: Arc::default(), gap: MIN_GAP },
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
struct Http {
    client: reqwest::Client,
    base: Arc<str>,
    /// When the previous request was sent. Held across the wait so requests
    /// go out one at a time.
    last: Arc<tokio::sync::Mutex<Option<Instant>>>,
    gap: Duration,
}

impl Http {
    async fn html(&self, path: &str) -> Result<String> {
        let url = format!("{}{}", self.base, path);
        {
            let mut last = self.last.lock().await;
            if let Some(prev) = *last {
                let wait = self.gap.saturating_sub(prev.elapsed());
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
            }
            *last = Some(Instant::now());
        }
        let resp = self
            .client
            .get(&url)
            .header("Referer", parse::REFERER)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "zh-CN,zh;q=0.9")
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;
        let code = resp.status().as_u16();
        let body = resp.text().await.map_err(|e| SourceError::Network(e.to_string()))?;
        match code {
            200 => Ok(body),
            404 => Err(SourceError::NotFound(url)),
            403 | 429 => {
                Err(SourceError::Other(format!("站点拒绝访问（HTTP {code}），可能是访问过快被临时限制，请稍后再试")))
            }
            _ => Err(SourceError::Network(format!("HTTP {code}"))),
        }
    }
}

impl ComicSource for ManHuaGuiSource {
    fn meta(&self) -> &SourceMeta {
        &self.meta
    }

    fn explore(&self, page: u32) -> BoxFuture<'static, Result<Paged<ComicSummary>>> {
        let http = self.http.clone();
        let page = page.clamp(1, BROWSE_PAGE_CAP);
        self.run(async move {
            let html = http.html(&format!("/list/update_p{page}.html")).await?;
            let mut paged = parse::parse_list(&html);
            if page >= BROWSE_PAGE_CAP {
                paged.has_more = false;
            }
            Ok(paged)
        })
    }

    fn search(&self, query: &str, page: u32) -> BoxFuture<'static, Result<Paged<ComicSummary>>> {
        let http = self.http.clone();
        let page = page.max(1);
        let q = urlencode(query.trim());
        self.run(async move {
            if q.is_empty() {
                return Ok(Paged { items: vec![], has_more: false });
            }
            let html = http.html(&format!("/s/{q}_p{page}.html")).await?;
            Ok(parse::parse_search(&html, page))
        })
    }

    fn detail(&self, comic_id: &str) -> BoxFuture<'static, Result<ComicDetail>> {
        let http = self.http.clone();
        let id = comic_id.to_string();
        self.run(async move {
            check_id(&id)?;
            let html = http.html(&format!("/comic/{id}/")).await?;
            let detail = parse::parse_detail(&html, &id)?;
            if detail.groups.is_empty() {
                return Err(SourceError::NotFound("没有章节".into()));
            }
            Ok(detail)
        })
    }

    fn pages(&self, comic_id: &str, chapter_id: &str) -> BoxFuture<'static, Result<Vec<ImageRequest>>> {
        let http = self.http.clone();
        let (id, ep) = (comic_id.to_string(), chapter_id.to_string());
        self.run(async move {
            check_id(&id)?;
            check_id(&ep)?;
            let html = http.html(&format!("/comic/{id}/{ep}.html")).await?;
            let data = unpack::img_data_from_html(&html).map_err(SourceError::Parse)?;
            Ok(parse::page_requests(&data))
        })
    }
}

/// Ids are numeric; reject anything else before it reaches a URL path.
fn check_id(id: &str) -> Result<()> {
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(SourceError::NotFound(format!("无效的 id：{id}")))
    }
}

/// Percent-encoding for a path segment (keeps the dep surface small).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serves canned responses; returns the request lines it saw.
    fn mock(responses: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<Vec<(String, Instant)>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut seen = vec![];
            for (status, body) in responses {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut req = Vec::new();
                let mut buf = [0; 1024];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buf).unwrap();
                    assert!(n > 0);
                    req.extend_from_slice(&buf[..n]);
                }
                let req = String::from_utf8(req).unwrap();
                seen.push((req.lines().next().unwrap().to_string(), Instant::now()));
                assert!(req.to_ascii_lowercase().contains("referer: https://www.manhuagui.com/"));
                write!(
                    socket,
                    "HTTP/1.1 {status} X\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            seen
        });
        (base, server)
    }

    fn source(base: &str, gap: Duration) -> ManHuaGuiSource {
        let mut s = ManHuaGuiSource::with_base(Runtime::new().unwrap(), base);
        s.http.client = reqwest::Client::builder().no_proxy().build().unwrap();
        s.http.gap = gap;
        s
    }

    #[test]
    fn reads_chapter_pages_end_to_end() {
        let script = include_str!("../testdata/chapter_script.js");
        let (base, server) = mock(vec![(200, format!("<html><script>{script}</script></html>"))]);
        let pages = futures::executor::block_on(source(&base, Duration::ZERO).pages("44373", "782036")).unwrap();
        assert_eq!(pages.len(), 12);
        let ImageRequest::Http { url, headers } = &pages[0] else { panic!() };
        assert!(url.starts_with("https://i.hamreus.com/ps1/"), "{url}");
        assert!(url.contains("?e=") && url.contains("&m="), "{url}");
        assert!(headers.iter().any(|(k, _)| k == "Referer"));
        let seen = server.join().unwrap();
        assert!(seen[0].0.starts_with("GET /comic/44373/782036.html "));
    }

    #[test]
    fn requests_are_spaced_out() {
        let ok = || (200, include_str!("../testdata/list.html").to_string());
        let (base, server) = mock(vec![ok(), ok()]);
        let src = source(&base, Duration::from_millis(300));
        let (a, b) = futures::executor::block_on(async { futures::join!(src.explore(1), src.explore(2)) });
        assert!(a.is_ok() && b.is_ok());
        let seen = server.join().unwrap();
        assert!(seen[1].1.duration_since(seen[0].1) >= Duration::from_millis(250), "{seen:?}");
    }

    #[test]
    fn maps_http_errors() {
        let (base, server) = mock(vec![(404, String::new()), (403, String::new())]);
        let src = source(&base, Duration::ZERO);
        assert!(matches!(futures::executor::block_on(src.detail("1")), Err(SourceError::NotFound(_))));
        assert!(
            matches!(futures::executor::block_on(src.detail("2")), Err(SourceError::Other(m)) if m.contains("403"))
        );
        server.join().unwrap();
    }

    #[test]
    fn rejects_non_numeric_ids_without_requesting() {
        let src = source("http://127.0.0.1:9", Duration::ZERO);
        assert!(matches!(futures::executor::block_on(src.detail("../x")), Err(SourceError::NotFound(_))));
        assert!(matches!(futures::executor::block_on(src.pages("1", "2?x")), Err(SourceError::NotFound(_))));
    }

    #[test]
    fn urlencode_handles_cjk_and_specials() {
        assert_eq!(urlencode("海贼"), "%E6%B5%B7%E8%B4%BC");
        assert_eq!(urlencode("a_b/c"), "a%5Fb%2Fc");
    }
}
