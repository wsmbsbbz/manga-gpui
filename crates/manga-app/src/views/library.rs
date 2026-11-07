use std::{rc::Rc, sync::Arc};

use gpui_kit::{prelude::FluentBuilder as _, *};
use manga_core::ComicSummary;

use super::Nav;
use crate::{
    state::AppState,
    theme::{colors, layout, space, text},
    ui,
};

enum Load {
    /// Local source with no folder chosen yet.
    NeedsFolder,
    Loading,
    Ready,
    Failed(SharedString),
}

/// Browse listing of the selected source.
pub struct LibraryView {
    source_key: String,
    comics: Arc<Vec<ComicSummary>>,
    load: Load,
    scroll: UniformListScrollHandle,
    _task: Option<Task<()>>,
}

impl EventEmitter<Nav> for LibraryView {}

impl LibraryView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            source_key: manga_source_local::KEY.to_string(),
            comics: Arc::default(),
            load: Load::Loading,
            scroll: UniformListScrollHandle::new(),
            _task: None,
        };
        this.reload(cx);
        this
    }

    pub fn set_source(&mut self, key: String, cx: &mut Context<Self>) {
        if key == self.source_key {
            return;
        }
        self.source_key = key;
        self.comics = Arc::default();
        self.scroll = UniformListScrollHandle::new();
        self.reload(cx);
    }

    fn is_local(&self) -> bool {
        self.source_key == manga_source_local::KEY
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(source) = AppState::source(cx, &self.source_key) else {
            self.load = Load::Failed("来源不可用".into());
            cx.notify();
            return;
        };
        // The local source needs a folder before it can list anything.
        if self.is_local() && AppState::get(cx).local.root().is_none() {
            self.load = Load::NeedsFolder;
            cx.notify();
            return;
        }
        self.load = Load::Loading;
        self._task = Some(cx.spawn(async move |this, cx| {
            // Loops until `has_more` is false. Remote sources bound this
            // themselves (explore caps the page count); local is cheap.
            let mut all = Vec::new();
            let mut page = 1;
            let result = loop {
                match source.explore(page).await {
                    Ok(p) => {
                        all.extend(p.items);
                        if !p.has_more {
                            break Ok(());
                        }
                        page += 1;
                    }
                    Err(e) => break Err(e),
                }
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.comics = Arc::new(all);
                        this.load = Load::Ready;
                    }
                    Err(e) => this.load = Load::Failed(e.to_string().into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择为书库".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(root) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                AppState::get(cx).local.set_root(Some(root.clone()));
                AppState::update_config(cx, |c| c.library_root = Some(root.clone()));
                this.reload(cx);
            });
        })
        .detach();
    }

    fn title(&self, cx: &App) -> SharedString {
        AppState::source(cx, &self.source_key).map(|s| s.meta().name.into()).unwrap_or_else(|| "书架".into())
    }
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let subtitle: Option<SharedString> = if self.is_local() {
            AppState::get(cx).local.root().map(|r| r.display().to_string().into())
        } else if matches!(self.load, Load::Ready) {
            Some(format!("{} 部", self.comics.len()).into())
        } else {
            None
        };
        let header = ui::page_header(self.title(cx), subtitle).child(
            div()
                .flex()
                .gap(space::s2())
                .child(ui::button("refresh", "刷新").on_click(cx.listener(|this, _, _, cx| {
                    if this.is_local() {
                        AppState::get(cx).local.invalidate();
                    }
                    this.reload(cx)
                })))
                .when(self.is_local(), |d| {
                    d.child(
                        ui::button("pick", "选择文件夹").on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
                    )
                }),
        );

        let available_w = f32::from(window.viewport_size().width) - f32::from(layout::sidebar_w());
        let body: AnyElement = match &self.load {
            Load::NeedsFolder => ui::empty_state(
                "还没有书库",
                "选择一个存放漫画的文件夹。每个子文件夹是一部漫画，其中的子文件夹或 .cbz 是章节。",
            )
            .child(
                div().pt(space::s4()).child(
                    ui::primary_button("pick-empty", "选择文件夹")
                        .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
                ),
            )
            .into_any_element(),
            Load::Loading if self.comics.is_empty() => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(ui::spinner_label("正在加载…"))
                .into_any_element(),
            Load::Failed(e) => ui::empty_state("加载失败", e.clone()).into_any_element(),
            _ if self.comics.is_empty() => ui::empty_state("这里是空的", "没有找到任何漫画。").into_any_element(),
            _ => {
                let weak = cx.weak_entity();
                let key = self.source_key.clone();
                let on_open: ui::OnComic = Rc::new(move |comic, _, cx| {
                    let (key, comic) = (key.clone(), comic.clone());
                    let _ = weak.update(cx, |_, cx| cx.emit(Nav::OpenComic { source: key, comic }));
                });
                let pipeline = AppState::get(cx).covers.clone();
                ui::comic_grid("library-grid", self.comics.clone(), available_w, self.scroll.clone(), pipeline, on_open)
                    .into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().text_size(text::body()).text_color(c.text).child(body))
    }
}
