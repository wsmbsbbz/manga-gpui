use std::sync::{Arc, mpsc};

use gpui_kit::{App, AppContext, Entity, Global};
use manga_core::{ComicSource, SourceRegistry};
use manga_net::{ImageFetcher, Runtime};
use manga_source_copy::CopySource;
use manga_source_local::LocalSource;
use manga_store::{AppConfig, DiskCache, History, HistoryEntry, app_dirs};

use crate::images::ImagePipeline;

/// Process-wide services, installed once as a GPUI global.
pub struct AppState {
    pub registry: SourceRegistry,
    pub local: Arc<LocalSource>,
    pub history: Arc<History>,
    pub history_writer: HistoryWriter,
    /// Effective config: the saved file plus runtime-only env overrides.
    pub config: AppConfig,
    /// Small decoded covers for grids.
    pub covers: Entity<ImagePipeline>,
    /// Full-size reader pages.
    pub pages: Entity<ImagePipeline>,
}

impl Global for AppState {}

impl AppState {
    pub fn init(cx: &mut App) -> anyhow::Result<()> {
        let (data_dir, cache_dir) = app_dirs();
        let mut config = AppConfig::load();
        // Dev/demo overrides. Runtime-only: `update_config` never saves them.
        if let Some(root) = std::env::var_os("MANGA_LIBRARY") {
            config.library_root = Some(root.into());
        }
        if let Ok(mode) = std::env::var("MANGA_READER_MODE") {
            config.reader_mode = mode;
        }
        Self::init_with(cx, &data_dir, &cache_dir, config)
    }

    pub fn init_with(
        cx: &mut App,
        data_dir: &std::path::Path,
        cache_dir: &std::path::Path,
        config: AppConfig,
    ) -> anyhow::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let runtime = Runtime::new()?;
        let disk = Arc::new(DiskCache::open(cache_dir.join("images"), config.disk_cache_mb * 1024 * 1024)?);
        let fetcher = ImageFetcher::new(runtime.clone(), Some(disk), 6);
        let history = Arc::new(History::open(&data_dir.join("library.sqlite3"))?);
        let history_writer = HistoryWriter::spawn(history.clone());

        let local = Arc::new(LocalSource::new(config.library_root.clone(), runtime.clone()));
        let mut registry = SourceRegistry::default();
        registry.register(local.clone() as Arc<dyn ComicSource>);

        // CopyManga website source (unofficial, local/research use). The host
        // can be overridden by env for when the default mirror changes.
        let copy_host = std::env::var("MANGA_COPY_API").ok().or_else(|| config.copy_api_host.clone());
        let copy_token = std::env::var("MANGA_COPY_TOKEN").ok();
        let copy = Arc::new(CopySource::new(runtime.clone(), copy_host, copy_token));
        registry.register(copy as Arc<dyn ComicSource>);

        let mb = 1024 * 1024;
        let covers = cx.new(|_| ImagePipeline::new(fetcher.clone(), 420, 96 * mb));
        let pages = cx.new(|_| ImagePipeline::new(fetcher, 1800, config.memory_budget_mb as usize * mb));

        cx.set_global(AppState { registry, local, history, history_writer, config, covers, pages });
        Ok(())
    }

    pub fn get(cx: &App) -> &AppState {
        cx.global::<AppState>()
    }

    pub fn source(cx: &App, key: &str) -> Option<Arc<dyn ComicSource>> {
        Self::get(cx).registry.get(key)
    }

    /// Apply a user-initiated change to both the effective config and the
    /// on-disk file. Env overrides live only in the effective config, so
    /// they never leak into the saved file.
    pub fn update_config(cx: &mut App, f: impl Fn(&mut AppConfig)) {
        let mut saved = AppConfig::load();
        f(&mut saved);
        saved.save();
        f(&mut cx.global_mut::<AppState>().config);
    }
}

enum HistoryMsg {
    Record(Box<HistoryEntry>),
    Flush(futures::channel::oneshot::Sender<()>),
}

/// All history writes go through one thread, so they land in the order
/// they were issued (an older page can never overwrite a newer one).
#[derive(Clone)]
pub struct HistoryWriter(mpsc::Sender<HistoryMsg>);

impl HistoryWriter {
    fn spawn(history: Arc<History>) -> Self {
        let (tx, rx) = mpsc::channel::<HistoryMsg>();
        std::thread::Builder::new()
            .name("manga-history".into())
            .spawn(move || {
                // `recv` drains every queued message before reporting the
                // channel closed, so nothing enqueued is dropped.
                while let Ok(msg) = rx.recv() {
                    match msg {
                        HistoryMsg::Record(e) => {
                            if let Err(err) = history.record(&e) {
                                log::warn!("save progress: {err}");
                            }
                        }
                        HistoryMsg::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .expect("spawn history writer");
        Self(tx)
    }

    pub fn record(&self, entry: HistoryEntry) {
        let _ = self.0.send(HistoryMsg::Record(Box::new(entry)));
    }

    /// Resolves once every write issued before this call is in sqlite.
    pub fn flush(&self) -> impl std::future::Future<Output = ()> + use<> {
        let (tx, rx) = futures::channel::oneshot::channel();
        let _ = self.0.send(HistoryMsg::Flush(tx));
        async move {
            let _ = rx.await;
        }
    }
}
