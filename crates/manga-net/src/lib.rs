//! Bridges GPUI (its own executor) with tokio-based IO.
//!
//! GPUI's executor is not a tokio runtime, and `reqwest` needs one. We own a
//! small multi-threaded tokio runtime and hand back [`Pending`] futures that
//! are executor-agnostic: awaiting one from `cx.spawn` works, and dropping it
//! aborts the tokio task, so cancelling a GPUI `Task` cancels the IO too.

use std::{
    collections::HashMap,
    future::Future,
    io::Read,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use futures::{
    FutureExt,
    channel::oneshot,
    future::{BoxFuture, Shared},
};
use manga_core::{ByteCache, ImageRequest, Result, SourceError};
use parking_lot::Mutex;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct Runtime {
    handle: tokio::runtime::Handle,
    // Keeps the runtime alive for as long as any clone exists.
    _rt: Arc<tokio::runtime::Runtime>,
}

impl Runtime {
    pub fn new() -> std::io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(3)
            .thread_name("manga-io")
            .enable_all()
            .build()?;
        Ok(Self { handle: rt.handle().clone(), _rt: Arc::new(rt) })
    }

    pub fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    /// Run `fut` on the IO runtime. The returned future resolves to `None`
    /// only if the task panicked; dropping it aborts the task.
    pub fn spawn<T: Send + 'static>(&self, fut: impl Future<Output = T> + Send + 'static) -> Pending<T> {
        let (tx, rx) = oneshot::channel();
        let join = self.handle.spawn(async move {
            let _ = tx.send(fut.await);
        });
        Pending { rx, abort: join.abort_handle(), detached: false }
    }

    /// Run blocking work (filesystem scans, archive reads) on tokio's
    /// blocking pool. Dropping the result only detaches; blocking work cannot
    /// be interrupted mid-way.
    pub fn spawn_blocking<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Pending<T> {
        let (tx, rx) = oneshot::channel();
        let join = self.handle.spawn_blocking(move || {
            let _ = tx.send(f());
        });
        Pending { rx, abort: join.abort_handle(), detached: false }
    }
}

pub struct Pending<T> {
    rx: oneshot::Receiver<T>,
    abort: tokio::task::AbortHandle,
    detached: bool,
}

impl<T> Future for Pending<T> {
    type Output = Option<T>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.rx.poll_unpin(cx).map(|r| r.ok())
    }
}

impl<T> Pending<T> {
    /// Let the task run to completion without anyone awaiting it
    /// (fire-and-forget writes). Without this, dropping aborts the task.
    pub fn detach(mut self) {
        self.detached = true;
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if !self.detached {
            self.abort.abort();
        }
    }
}

type SharedFetch = Shared<BoxFuture<'static, std::result::Result<Bytes, SourceError>>>;
type InflightMap = Arc<Mutex<HashMap<String, Inflight>>>;

struct Inflight {
    shared: SharedFetch,
    /// Live `Pending`s waiting on this download.
    waiters: usize,
    abort: tokio::task::AbortHandle,
    /// Distinguishes a re-created entry from the one a guard was issued for.
    generation: u64,
}

/// Held by each waiter; the last one out aborts the download. Removal and
/// abort happen under the map lock, so a new `fetch` can never join a
/// download that is being aborted.
struct WaiterGuard {
    map: InflightMap,
    key: String,
    generation: u64,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        let abort = {
            let mut map = self.map.lock();
            match map.get_mut(&self.key).filter(|e| e.generation == self.generation) {
                Some(e) => {
                    e.waiters -= 1;
                    if e.waiters == 0 { map.remove(&self.key).map(|e| e.abort) } else { None }
                }
                None => None,
            }
        };
        // Abort outside the lock: the task's own `DoneGuard` takes it too.
        if let Some(a) = abort {
            a.abort();
        }
    }
}

/// Held by the download task; removes its entry when it finishes or panics.
struct DoneGuard {
    map: InflightMap,
    key: String,
    generation: u64,
}

impl Drop for DoneGuard {
    fn drop(&mut self) {
        let mut map = self.map.lock();
        if map.get(&self.key).is_some_and(|e| e.generation == self.generation) {
            map.remove(&self.key);
        }
    }
}

/// Fetches image bytes for any [`ImageRequest`].
///
/// - identical in-flight requests share one download
/// - remote requests are bounded per process (`max_remote`)
/// - remote bytes go through the optional disk [`ByteCache`]
#[derive(Clone)]
pub struct ImageFetcher {
    runtime: Runtime,
    client: reqwest::Client,
    cache: Option<Arc<dyn ByteCache>>,
    remote_permits: Arc<Semaphore>,
    inflight: InflightMap,
    next_gen: Arc<std::sync::atomic::AtomicU64>,
}

impl ImageFetcher {
    pub fn new(runtime: Runtime, cache: Option<Arc<dyn ByteCache>>, max_remote: usize) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .build()
            .expect("reqwest client");
        Self {
            runtime,
            client,
            cache,
            remote_permits: Arc::new(Semaphore::new(max_remote.max(1))),
            inflight: Default::default(),
            next_gen: Default::default(),
        }
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.client
    }

    /// Fetch bytes. Identical concurrent requests share one download; the
    /// download is aborted once every caller has dropped its `Pending`.
    pub fn fetch(&self, req: ImageRequest) -> Pending<Result<Bytes>> {
        let key = req.cache_key();
        let (shared, guard) = {
            let mut inflight = self.inflight.lock();
            let entry = match inflight.get_mut(&key) {
                Some(e) => e,
                None => {
                    let generation = self.next_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let this = self.clone();
                    let done = DoneGuard { map: self.inflight.clone(), key: key.clone(), generation };
                    let task = self.runtime.handle.spawn(async move {
                        // Removes the entry on completion *and* on panic.
                        let _done = done;
                        this.fetch_uncached(req).await
                    });
                    let abort = task.abort_handle();
                    let fut: BoxFuture<'static, _> = async move {
                        task.await.unwrap_or_else(|e| Err(SourceError::Other(format!("fetch task: {e}"))))
                    }
                    .boxed();
                    inflight.entry(key.clone()).or_insert(Inflight {
                        shared: fut.shared(),
                        waiters: 0,
                        abort,
                        generation,
                    })
                }
            };
            entry.waiters += 1;
            let guard = WaiterGuard { map: self.inflight.clone(), key, generation: entry.generation };
            (entry.shared.clone(), guard)
        };
        // The guard lives inside the waiter task: it drops whether the waiter
        // completes or is aborted (its `Pending` dropped).
        self.runtime.spawn(async move {
            let _guard = guard;
            shared.await
        })
    }

    async fn fetch_uncached(&self, req: ImageRequest) -> Result<Bytes> {
        match req {
            ImageRequest::File(path) => Ok(tokio::fs::read(&path).await?.into()),
            ImageRequest::ArchiveEntry { archive, entry } => {
                tokio::task::spawn_blocking(move || read_zip_entry(&archive, &entry))
                    .await
                    .map_err(|e| SourceError::Other(e.to_string()))?
            }
            ImageRequest::Http { ref url, ref headers } => {
                let key = req.cache_key();
                if let Some(cache) = &self.cache {
                    let cache = cache.clone();
                    let k = key.clone();
                    let hit = tokio::task::spawn_blocking(move || cache.get(&k)).await.ok().flatten();
                    if let Some(b) = hit {
                        return Ok(b);
                    }
                }
                let _permit = self.remote_permits.acquire().await.map_err(|e| SourceError::Other(e.to_string()))?;
                let bytes = self.get_with_retry(url, headers).await?;
                if let Some(cache) = &self.cache {
                    let cache = cache.clone();
                    let b = bytes.clone();
                    tokio::task::spawn_blocking(move || cache.put(&key, &b));
                }
                Ok(bytes)
            }
        }
    }

    async fn get_with_retry(&self, url: &str, headers: &[(String, String)]) -> Result<Bytes> {
        const ATTEMPTS: usize = 3;
        let mut delay = Duration::from_millis(400);
        let mut last = SourceError::Network("unreachable".into());
        for attempt in 1..=ATTEMPTS {
            let mut rb = self.client.get(url);
            for (k, v) in headers {
                rb = rb.header(k, v);
            }
            match rb.send().await {
                Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                    Ok(b) => return Ok(b),
                    // Connection reset mid-body: retryable.
                    Err(e) => last = SourceError::Network(e.to_string()),
                },
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    match code {
                        404 | 410 => return Err(SourceError::NotFound(url.into())),
                        401 | 403 => return Err(SourceError::LoginRequired),
                        // Other client errors will not change on retry.
                        400..=499 if code != 408 && code != 429 => {
                            return Err(SourceError::Network(format!("HTTP {code}")));
                        }
                        _ => last = SourceError::Network(format!("HTTP {code}")),
                    }
                }
                Err(e) => last = SourceError::Network(e.to_string()),
            }
            if attempt < ATTEMPTS {
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
        }
        Err(last)
    }
}

pub fn read_zip_entry(archive: &std::path::Path, entry: &str) -> Result<Bytes> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| SourceError::Io(e.to_string()))?;
    let f = zip.by_name(entry).map_err(|e| SourceError::NotFound(format!("{entry}: {e}")))?;
    // The declared size is attacker-controlled; never trust it for
    // allocation, and stop reading once past the cap (deflate bombs).
    if f.size() > MAX_ENTRY_BYTES {
        return Err(SourceError::Io(format!("{entry}: 图片过大（{} 字节）", f.size())));
    }
    let mut buf = Vec::with_capacity((f.size() as usize).min(8 << 20));
    f.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_ENTRY_BYTES {
        return Err(SourceError::Io(format!("{entry}: 解压后超过上限")));
    }
    Ok(buf.into())
}

/// Largest single image we accept from an archive.
pub const MAX_ENTRY_BYTES: u64 = 64 << 20;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_resolves_and_dedups() {
        let rt = Runtime::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        std::fs::write(&p, b"hello").unwrap();
        let f = ImageFetcher::new(rt.clone(), None, 2);
        let a = f.fetch(ImageRequest::File(p.clone()));
        let b = f.fetch(ImageRequest::File(p));
        let (a, b) = futures::executor::block_on(async { futures::join!(a, b) });
        assert_eq!(&a.unwrap().unwrap()[..], b"hello");
        assert_eq!(&b.unwrap().unwrap()[..], b"hello");
    }

    #[test]
    fn last_waiter_drop_aborts_download() {
        use std::io::Read as _;
        // Local server that accepts and never answers.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (conn_tx, conn_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((s, _)) = listener.accept() {
                let _ = conn_tx.send(s);
            }
        });
        let f = ImageFetcher::new(Runtime::new().unwrap(), None, 2);
        let req = ImageRequest::Http { url: format!("http://{addr}/x.png"), headers: vec![] };
        let a = f.fetch(req.clone());
        let b = f.fetch(req);
        let mut sock = conn_rx.recv_timeout(Duration::from_secs(5)).expect("request reached server");
        assert_eq!(f.inflight.lock().len(), 1, "two waiters share one download");
        drop(a);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(f.inflight.lock().len(), 1, "still one waiter left");
        drop(b);
        std::thread::sleep(Duration::from_millis(200));
        assert!(f.inflight.lock().is_empty());
        // The aborted client closes the connection: the server reads EOF.
        sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut buf = [0u8; 4096];
        let mut closed = false;
        for _ in 0..20 {
            match sock.read(&mut buf) {
                Ok(0) => {
                    closed = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(closed, "download was aborted");
    }

    #[test]
    fn detached_pending_completes() {
        let rt = Runtime::new().unwrap();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f2 = flag.clone();
        rt.spawn_blocking(move || {
            std::thread::sleep(Duration::from_millis(50));
            f2.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .detach();
        std::thread::sleep(Duration::from_millis(300));
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn dropping_pending_aborts() {
        let rt = Runtime::new().unwrap();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f2 = flag.clone();
        let p = rt.spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            f2.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        drop(p);
        std::thread::sleep(Duration::from_millis(400));
        assert!(!flag.load(std::sync::atomic::Ordering::SeqCst));
    }
}
