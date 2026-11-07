use std::{rc::Rc, sync::Arc};

use gpui_kit::{prelude::FluentBuilder as _, *};
use manga_core::{ComicSource, ComicSummary};

use super::Nav;
use crate::{
    state::AppState,
    theme::{colors, layout, space, text},
    ui,
};

enum Load {
    Idle,
    Loading,
    Ready,
    Failed(SharedString),
}

/// Browse listing of the active source (the local library for now).
pub struct LibraryView {
    comics: Arc<Vec<ComicSummary>>,
    load: Load,
    scroll: UniformListScrollHandle,
    _task: Option<Task<()>>,
}

impl EventEmitter<Nav> for LibraryView {}

impl LibraryView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this =
            Self { comics: Arc::default(), load: Load::Idle, scroll: UniformListScrollHandle::new(), _task: None };
        this.reload(cx);
        this
    }

    fn source(cx: &App) -> Arc<dyn ComicSource> {
        AppState::get(cx).local.clone()
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if AppState::get(cx).local.root().is_none() {
            self.load = Load::Idle;
            cx.notify();
            return;
        }
        self.load = Load::Loading;
        let source = Self::source(cx);
        self._task = Some(cx.spawn(async move |this, cx| {
            // Local listings are cheap; load every page up front.
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
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let root = AppState::get(cx).local.root();
        let subtitle = match (&self.load, &root) {
            (Load::Ready, Some(r)) => Some(format!("{} 部 · {}", self.comics.len(), r.display()).into()),
            (_, Some(r)) => Some(r.display().to_string().into()),
            _ => None,
        };
        let header = ui::page_header("书架", subtitle).child(
            div()
                .flex()
                .gap(space::s2())
                .when(root.is_some(), |d| {
                    d.child(ui::button("refresh", "刷新").on_click(cx.listener(|this, _, _, cx| {
                        AppState::get(cx).local.invalidate();
                        this.reload(cx)
                    })))
                })
                .child(ui::button("pick", "选择文件夹").on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx)))),
        );

        let available_w = f32::from(window.viewport_size().width) - f32::from(layout::sidebar_w());
        let body: AnyElement = match &self.load {
            Load::Idle => ui::empty_state(
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
                .child(ui::spinner_label("正在扫描书库…"))
                .into_any_element(),
            Load::Failed(e) => ui::empty_state("加载失败", e.clone()).into_any_element(),
            _ if self.comics.is_empty() => {
                ui::empty_state("书库是空的", "文件夹里没有找到漫画图片或 .cbz 文件。").into_any_element()
            }
            _ => {
                let weak = cx.weak_entity();
                let on_open: ui::OnComic = Rc::new(move |comic, _, cx| {
                    let _ = weak.update(cx, |_, cx| {
                        cx.emit(Nav::OpenComic { source: manga_source_local::KEY.into(), comic: comic.clone() })
                    });
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
