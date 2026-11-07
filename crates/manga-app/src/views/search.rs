use std::{rc::Rc, sync::Arc};

use gpui_kit::{
    component::input::{Input, InputEvent, InputState},
    *,
};
use manga_core::ComicSummary;

use super::Nav;
use crate::{
    state::AppState,
    theme::{colors, layout, radius, size, space},
    ui,
};

enum Phase {
    Idle,
    Searching,
    Done,
    Failed(SharedString),
}

pub struct SearchView {
    input: Entity<InputState>,
    results: Arc<Vec<ComicSummary>>,
    phase: Phase,
    last_query: String,
    scroll: UniformListScrollHandle,
    _task: Option<Task<()>>,
    _sub: Subscription,
}

impl EventEmitter<Nav> for SearchView {}

impl SearchView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("搜索书名，回车确认"));
        let sub = cx.subscribe(&input, |this, input, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let q = input.read(cx).value().to_string();
                this.run(q, cx);
            }
        });
        Self {
            input,
            results: Arc::default(),
            phase: Phase::Idle,
            last_query: String::new(),
            scroll: UniformListScrollHandle::new(),
            _task: None,
            _sub: sub,
        }
    }

    fn run(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.last_query = query.clone();
        self.phase = Phase::Searching;
        let source = AppState::get(cx).local.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let r = manga_core::ComicSource::search(&*source, &query, 1).await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(p) => {
                        this.results = Arc::new(p.items);
                        this.phase = Phase::Done;
                    }
                    Err(e) => this.phase = Phase::Failed(e.to_string().into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for SearchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let subtitle = match self.phase {
            Phase::Done => Some(format!("“{}” · {} 个结果", self.last_query, self.results.len()).into()),
            _ => None,
        };
        let available_w = f32::from(window.viewport_size().width) - f32::from(layout::sidebar_w());
        let body: AnyElement = match &self.phase {
            Phase::Idle => ui::empty_state("搜索书库", "输入书名的一部分即可，不区分大小写。").into_any_element(),
            Phase::Searching => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(ui::spinner_label("搜索中…"))
                .into_any_element(),
            Phase::Failed(e) => ui::empty_state("搜索失败", e.clone()).into_any_element(),
            Phase::Done if self.results.is_empty() => {
                ui::empty_state("没有找到", "换个关键词试试。").into_any_element()
            }
            Phase::Done => {
                let weak = cx.weak_entity();
                let on_open: ui::OnComic = Rc::new(move |comic, _, cx| {
                    let _ = weak.update(cx, |_, cx| {
                        cx.emit(Nav::OpenComic { source: manga_source_local::KEY.into(), comic: comic.clone() })
                    });
                });
                ui::comic_grid(
                    "search-grid",
                    self.results.clone(),
                    available_w,
                    self.scroll.clone(),
                    AppState::get(cx).covers.clone(),
                    on_open,
                )
                .into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(ui::page_header("搜索", subtitle))
            .child(
                div().px(px(layout::content_pad())).pb(space::s5()).child(
                    div()
                        .max_w(size::search_w())
                        .rounded(radius::md())
                        .bg(c.surface)
                        .child(Input::new(&self.input).cleanable(true)),
                ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
