//! Reader: vertical continuous scroll (webtoon) and single-page paged mode
//! (left-to-right / right-to-left), with zoom, pan, keyboard and click-zone
//! navigation, progress memory and priority prefetch.

use std::{
    collections::HashSet,
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{prelude::FluentBuilder as _, *};
use manga_core::{ComicDetail, ComicSource, ImageRequest};
use manga_store::HistoryEntry;

use super::{Nav, now_secs};
use crate::{
    images::{Decoded, ImagePipeline, ImageState, image_state},
    state::AppState,
    state::HistoryWriter,
    theme::{colors, motion, radius, space, text},
    ui,
};

actions!(
    reader,
    [
        NextPage,
        PrevPage,
        ArrowLeft,
        ArrowRight,
        ScrollDown,
        ScrollUp,
        NextChapter,
        PrevChapter,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        CycleMode,
        ToggleToolbar,
        CloseReader
    ]
);

const CTX: &str = "Reader";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", NextPage, Some(CTX)),
        KeyBinding::new("pagedown", NextPage, Some(CTX)),
        KeyBinding::new("shift-space", PrevPage, Some(CTX)),
        KeyBinding::new("pageup", PrevPage, Some(CTX)),
        KeyBinding::new("left", ArrowLeft, Some(CTX)),
        KeyBinding::new("right", ArrowRight, Some(CTX)),
        KeyBinding::new("down", ScrollDown, Some(CTX)),
        KeyBinding::new("up", ScrollUp, Some(CTX)),
        KeyBinding::new("]", NextChapter, Some(CTX)),
        KeyBinding::new("[", PrevChapter, Some(CTX)),
        KeyBinding::new("=", ZoomIn, Some(CTX)),
        KeyBinding::new("cmd-=", ZoomIn, Some(CTX)),
        KeyBinding::new("-", ZoomOut, Some(CTX)),
        KeyBinding::new("cmd--", ZoomOut, Some(CTX)),
        KeyBinding::new("0", ZoomReset, Some(CTX)),
        KeyBinding::new("cmd-0", ZoomReset, Some(CTX)),
        KeyBinding::new("m", CycleMode, Some(CTX)),
        KeyBinding::new("enter", ToggleToolbar, Some(CTX)),
        KeyBinding::new("escape", CloseReader, Some(CTX)),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Scroll,
    Ltr,
    Rtl,
}

impl Mode {
    fn from_config(s: &str) -> Self {
        match s {
            "ltr" => Mode::Ltr,
            "rtl" => Mode::Rtl,
            _ => Mode::Scroll,
        }
    }
    fn as_config(self) -> &'static str {
        match self {
            Mode::Scroll => "scroll",
            Mode::Ltr => "ltr",
            Mode::Rtl => "rtl",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Mode::Scroll => "条漫",
            Mode::Ltr => "左→右",
            Mode::Rtl => "右→左",
        }
    }
    fn next(self) -> Self {
        match self {
            Mode::Scroll => Mode::Ltr,
            Mode::Ltr => Mode::Rtl,
            Mode::Rtl => Mode::Scroll,
        }
    }
}

/// Placeholder aspect (h / w) for pages whose size is not known yet.
const PLACEHOLDER_ASPECT: f32 = 1.42;
const MAX_SCROLL_W: f32 = 880.;

struct Drag {
    start: Point<Pixels>,
    pan0: Point<Pixels>,
    moved: bool,
}

pub struct ReaderView {
    source_key: String,
    source: Arc<dyn ComicSource>,
    detail: Arc<ComicDetail>,
    chapter_ix: usize,
    pages: Rc<Vec<ImageRequest>>,
    loading: bool,
    error: Option<SharedString>,
    current: usize,
    visible: Range<usize>,
    mode: Mode,
    zoom: f32,
    pan: Point<Pixels>,
    list: ListState,
    ready_seen: HashSet<usize>,
    pending_remeasure: Vec<usize>,
    toolbar: bool,
    hide_task: Option<Task<()>>,
    focus: FocusHandle,
    drag: Option<Drag>,
    wheel_acc: f32,
    wheel_armed: bool,
    last_wheel: Instant,
    history: HistoryWriter,
    next_chapter: Option<(usize, Rc<Vec<ImageRequest>>)>,
    next_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    load_task: Option<Task<()>>,
    pipeline: Entity<ImagePipeline>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Nav> for ReaderView {}

impl ReaderView {
    pub fn new(
        source_key: String,
        detail: Arc<ComicDetail>,
        chapter_ix: usize,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = AppState::get(cx);
        let source = state.registry.get(&source_key).expect("source registered");
        let pipeline = state.pages.clone();
        let mode = Mode::from_config(&state.config.reader_mode);

        let list = ListState::new(0, ListAlignment::Top, px(1600.));
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |ev, _, cx| {
            let range = ev.visible_range.clone();
            let _ = weak.update(cx, |this, cx| this.on_visible(range, cx));
        });

        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        let subs = vec![cx.observe(&pipeline, |this, _, cx| this.on_images_changed(cx))];
        let mut this = Self {
            source_key,
            source,
            detail,
            chapter_ix,
            pages: Rc::default(),
            loading: true,
            error: None,
            current: 0,
            visible: 0..0,
            mode,
            zoom: 1.0,
            pan: Point::default(),
            list,
            ready_seen: HashSet::new(),
            pending_remeasure: Vec::new(),
            toolbar: true,
            hide_task: None,
            focus,
            drag: None,
            wheel_acc: 0.,
            wheel_armed: true,
            last_wheel: Instant::now(),
            history: AppState::get(cx).history_writer.clone(),
            next_chapter: None,
            next_task: None,
            save_task: None,
            load_task: None,
            pipeline,
            _subs: subs,
        };
        this.open_chapter(chapter_ix, page, cx);
        this.poke_toolbar(cx);
        this
    }

    // ---- chapter / page state -------------------------------------------------

    fn chapter_count(&self) -> usize {
        self.detail.chapters().count()
    }

    fn chapter_title(&self) -> SharedString {
        self.detail.chapter_at(self.chapter_ix).map(|c| c.title.clone()).unwrap_or_default().into()
    }

    /// `page == usize::MAX` opens at the last page (used when paging backwards).
    fn open_chapter(&mut self, ix: usize, page: usize, cx: &mut Context<Self>) {
        let Some(ch) = self.detail.chapter_at(ix).cloned() else { return };
        self.save_now(cx);
        self.chapter_ix = ix;
        self.zoom = 1.0;
        self.pan = Point::default();
        self.error = None;

        if let Some((nix, pages)) = self.next_chapter.take()
            && nix == ix
        {
            self.set_pages(pages, page, cx);
            return;
        }
        self.loading = true;
        self.pages = Rc::default();
        self.list.reset(0);
        let fut = self.source.pages(&self.detail.summary.id, &ch.id);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let r = fut.await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(pages) => this.set_pages(Rc::new(pages), page, cx),
                Err(e) => {
                    this.loading = false;
                    this.error = Some(e.to_string().into());
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn set_pages(&mut self, pages: Rc<Vec<ImageRequest>>, page: usize, cx: &mut Context<Self>) {
        let page = page.min(pages.len().saturating_sub(1));
        self.loading = false;
        self.pages = pages;
        self.ready_seen.clear();
        self.pending_remeasure.clear();
        self.next_task = None;
        // +1: end-of-chapter footer item in scroll mode.
        self.list.reset(self.pages.len() + 1);
        self.list.scroll_to(ListOffset { item_ix: page, offset_in_item: px(0.) });
        self.current = page;
        self.visible = page..page + 1;
        self.after_page_change(cx);
    }

    fn go_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.pages.is_empty() {
            return;
        }
        let page = page.min(self.pages.len() - 1);
        if self.mode == Mode::Scroll {
            self.list.scroll_to(ListOffset { item_ix: page, offset_in_item: px(0.) });
        }
        self.pan = Point::default();
        self.current = page;
        self.after_page_change(cx);
    }

    /// While a chapter loads `pages` is empty; without this guard a held key
    /// would skip one chapter per key-repeat.
    fn busy(&self) -> bool {
        self.loading || self.pages.is_empty()
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        if self.current + 1 < self.pages.len() {
            self.go_page(self.current + 1, cx);
        } else {
            self.next_chapter_action(cx);
        }
    }

    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        if self.current > 0 {
            self.go_page(self.current - 1, cx);
        } else if self.chapter_ix > 0 {
            self.open_chapter(self.chapter_ix - 1, usize::MAX, cx);
        }
    }

    fn next_chapter_action(&mut self, cx: &mut Context<Self>) {
        if self.chapter_ix + 1 < self.chapter_count() {
            self.open_chapter(self.chapter_ix + 1, 0, cx);
        }
    }

    fn prev_chapter_action(&mut self, cx: &mut Context<Self>) {
        if self.chapter_ix > 0 {
            self.open_chapter(self.chapter_ix - 1, 0, cx);
        }
    }

    fn on_visible(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        if self.mode != Mode::Scroll || self.pages.is_empty() {
            return;
        }
        // Called from inside the list's scroll handler while GPUI holds the
        // `ListState` borrow: never touch `self.list` here.
        let n = self.pages.len();
        // Item `n` is the end-of-chapter footer; once it shows, the reader
        // is on the last page even if that page's top is above the viewport.
        let footer_visible = range.end > n;
        let range = range.start.min(n - 1)..range.end.min(n);
        self.visible = range.clone();
        let cur = if footer_visible { n - 1 } else { range.start };
        if cur != self.current {
            self.current = cur;
            self.after_page_change(cx);
        }
    }

    fn after_page_change(&mut self, cx: &mut Context<Self>) {
        self.prefetch(cx);
        self.maybe_prefetch_next_chapter(cx);
        self.schedule_save(cx);
        cx.notify();
    }

    // ---- prefetch ---------------------------------------------------------------

    fn prefetch(&mut self, cx: &mut Context<Self>) {
        let n = self.pages.len();
        if n == 0 {
            return;
        }
        let ahead = AppState::get(cx).config.preload_ahead.max(1);
        let cur = self.current;
        // `visible` is only maintained by the scroll list; paged mode shows
        // exactly the current page.
        let visible = if self.mode == Mode::Scroll { self.visible.clone() } else { cur..cur + 1 };
        let lo = cur.saturating_sub(1).min(visible.start);
        let hi = (cur + ahead).min(n - 1).max(visible.end.saturating_sub(1).min(n - 1));
        let pages = self.pages.clone();
        let next = self.next_chapter.as_ref().map(|(_, p)| p.clone());
        self.pipeline.update(cx, |p, cx| {
            let mut keep = HashSet::new();
            for i in lo..=hi {
                // Pages behind the reader rank after the next page ahead.
                p.prefetch(&pages[i], page_priority(i, cur), cx);
                keep.insert(pages[i].cache_key());
            }
            if let Some(next) = next {
                for r in next.iter().take(2) {
                    p.prefetch(r, 100, cx);
                    keep.insert(r.cache_key());
                }
            }
            p.cancel_except(&keep, cx);
        });
    }

    fn maybe_prefetch_next_chapter(&mut self, cx: &mut Context<Self>) {
        let n = self.pages.len();
        let next_ix = self.chapter_ix + 1;
        if n == 0 || self.current + 3 < n || next_ix >= self.chapter_count() {
            return;
        }
        if self.next_chapter.is_some() || self.next_task.is_some() {
            return;
        }
        let Some(ch) = self.detail.chapter_at(next_ix).cloned() else { return };
        let fut = self.source.pages(&self.detail.summary.id, &ch.id);
        self.next_task = Some(cx.spawn(async move |this, cx| {
            if let Ok(pages) = fut.await {
                let _ = this.update(cx, |this, cx| {
                    if this.chapter_ix + 1 == next_ix {
                        this.next_chapter = Some((next_ix, Rc::new(pages)));
                        this.prefetch(cx);
                    }
                });
            }
        }));
    }

    fn on_images_changed(&mut self, cx: &mut Context<Self>) {
        // Variable-height list: a page that just decoded has a real height now.
        let n = self.pages.len();
        if n == 0 {
            return;
        }
        let lo = self.visible.start.min(self.current).saturating_sub(2);
        let hi = (self.visible.end.max(self.current + 1) + 8).min(n);
        let p = self.pipeline.read(cx);
        let newly: Vec<usize> =
            (lo..hi).filter(|i| !self.ready_seen.contains(i) && p.peek(&self.pages[*i]).is_some()).collect();
        // Applied at the start of `render`, where the list is guaranteed not
        // to be mid-layout (this observer can fire while it is).
        for i in newly {
            self.ready_seen.insert(i);
            self.pending_remeasure.push(i);
        }
        cx.notify();
    }

    fn apply_pending_remeasure(&mut self) {
        for i in self.pending_remeasure.drain(..) {
            self.list.remeasure_items(i..i + 1);
        }
    }

    // ---- progress -----------------------------------------------------------------

    fn entry(&self) -> Option<HistoryEntry> {
        let ch = self.detail.chapter_at(self.chapter_ix)?;
        if self.pages.is_empty() {
            return None;
        }
        Some(HistoryEntry {
            source: self.source_key.clone(),
            comic: self.detail.summary.clone(),
            chapter_id: ch.id.clone(),
            chapter_title: ch.title.clone(),
            page: self.current,
            updated_at: now_secs(),
        })
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(600)).await;
            let _ = this.update(cx, |this, cx| this.save_now(cx));
        }));
    }

    /// Enqueue the current position on the ordered history writer.
    fn save_now(&mut self, _cx: &mut Context<Self>) {
        self.save_task = None;
        if let Some(e) = self.entry() {
            self.history.record(e);
        }
    }

    /// Save, wait until it is in sqlite, then leave — so the detail and
    /// history views read the final position.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.save_now(cx);
        let flushed = self.history.flush();
        cx.spawn(async move |this, cx| {
            flushed.await;
            let _ = this.update(cx, |_, cx| cx.emit(Nav::Back));
        })
        .detach();
    }

    // ---- view state -------------------------------------------------------------

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if mode == self.mode {
            return;
        }
        let was_scroll = self.mode == Mode::Scroll;
        self.mode = mode;
        self.zoom = 1.0;
        self.pan = Point::default();
        if mode == Mode::Scroll && !was_scroll {
            self.list.remeasure();
            self.list.scroll_to(ListOffset { item_ix: self.current, offset_in_item: px(0.) });
        }
        AppState::update_config(cx, |c| c.reader_mode = mode.as_config().into());
        self.prefetch(cx);
        cx.notify();
    }

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let zoom = (zoom * 20.).round() / 20.;
        let zoom = zoom.clamp(0.5, 3.0);
        if (zoom - self.zoom).abs() < f32::EPSILON {
            return;
        }
        self.zoom = zoom;
        if zoom <= 1.0 {
            self.pan = Point::default();
        }
        if self.mode == Mode::Scroll {
            let top = self.list.logical_scroll_top();
            self.list.remeasure();
            self.list.scroll_to(ListOffset { item_ix: top.item_ix, offset_in_item: px(0.) });
        }
        cx.notify();
    }

    /// Show the toolbars and hide them again after a short idle period.
    fn poke_toolbar(&mut self, cx: &mut Context<Self>) {
        if !self.toolbar {
            self.toolbar = true;
            cx.notify();
        }
        self.hide_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(motion::TOOLBAR_HIDE)).await;
            let _ = this.update(cx, |this, cx| {
                this.toolbar = false;
                cx.notify();
            });
        }));
    }

    fn toggle_toolbar(&mut self, cx: &mut Context<Self>) {
        if self.toolbar {
            self.toolbar = false;
            self.hide_task = None;
            cx.notify();
        } else {
            self.poke_toolbar(cx);
        }
    }

    fn arrow(&mut self, rightward: bool, cx: &mut Context<Self>) {
        match (self.mode, rightward) {
            (Mode::Rtl, true) | (Mode::Ltr, false) | (Mode::Scroll, false) => self.prev_page(cx),
            _ => self.next_page(cx),
        }
    }

    fn scroll_mode_step(&mut self, down: bool, viewport_h: Pixels, cx: &mut Context<Self>) {
        let d = viewport_h * 0.85;
        self.list.scroll_by(if down { d } else { -d });
        cx.notify();
    }

    // ---- rendering ----------------------------------------------------------------

    fn scroll_content_w(&self, vw: f32) -> f32 {
        (vw.min(MAX_SCROLL_W) * self.zoom).min(vw).max(240.)
    }

    fn render_scroll(&self, vw: f32, cx: &mut Context<Self>) -> AnyElement {
        let pages = self.pages.clone();
        let pipeline = self.pipeline.clone();
        let content_w = self.scroll_content_w(vw);
        let current = self.current;
        let weak = cx.weak_entity();
        let has_next = self.chapter_ix + 1 < self.chapter_count();
        let next_title = self.detail.chapter_at(self.chapter_ix + 1).map(|c| c.title.clone());
        list(self.list.clone(), move |ix, _, cx| {
            if ix >= pages.len() {
                return chapter_footer(has_next, next_title.clone(), weak.clone()).into_any_element();
            }
            let state = image_state(&pipeline, &pages[ix], page_priority(ix, current), cx);
            div()
                .w_full()
                .flex()
                .justify_center()
                .child(page_box(ix, state, content_w, &pipeline, &pages[ix]))
                .into_any_element()
        })
        .size_full()
        .into_any_element()
    }

    fn render_paged(&self, vw: f32, vh: f32, cx: &mut Context<Self>) -> AnyElement {
        let c = colors();
        let Some(req) = self.pages.get(self.current).cloned() else { return div().into_any_element() };
        let state = image_state(&self.pipeline, &req, page_priority(self.current, self.current), cx);
        let mut canvas = div().id("page-canvas").size_full().relative().overflow_hidden();
        match state {
            ImageState::Ready(d) => {
                let (w, h) = fit_contain(&d, vw, vh, self.zoom);
                let x = (vw - w) / 2. + f32::from(self.pan.x);
                let y = (vh - h) / 2. + f32::from(self.pan.y);
                canvas =
                    canvas.child(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).flex().flex_col().children(
                        d.tiles.iter().enumerate().map(|(i, t)| img(t.clone()).w(px(w)).h(px(h * d.tile_fraction(i)))),
                    ));
            }
            ImageState::Loading => {
                canvas = canvas
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::spinner_label(format!("第 {} 页", self.current + 1)));
            }
            ImageState::Failed(e) => {
                let pipeline = self.pipeline.clone();
                canvas = canvas.flex().items_center().justify_center().child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(space::s3())
                        .child(div().text_color(c.danger).text_size(text::body()).child(e))
                        .child(ui::button("retry", "重试").on_click(move |_, _, cx| {
                            pipeline.update(cx, |p, cx| {
                                p.retry(&req);
                                cx.notify();
                            });
                        })),
                );
            }
        }
        canvas
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, _| {
                    this.drag = Some(Drag { start: ev.position, pan0: this.pan, moved: false });
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                this.poke_toolbar(cx);
                let zoom = this.zoom;
                if let Some(drag) = this.drag.as_mut().filter(|_| ev.dragging()) {
                    let delta = ev.position - drag.start;
                    if f32::from(delta.x).abs() + f32::from(delta.y).abs() > 4. {
                        drag.moved = true;
                    }
                    if zoom > 1.0 {
                        this.pan = drag.pan0 + delta;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseUpEvent, _, cx| {
                    let Some(drag) = this.drag.take() else { return };
                    if drag.moved {
                        return;
                    }
                    let x = f32::from(ev.position.x);
                    if x < vw * 0.3 {
                        this.arrow(false, cx);
                    } else if x > vw * 0.7 {
                        this.arrow(true, cx);
                    } else {
                        this.toggle_toolbar(cx);
                    }
                }),
            )
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                let delta = match ev.delta {
                    ScrollDelta::Pixels(p) => p,
                    ScrollDelta::Lines(l) => point(px(l.x * 40.), px(l.y * 40.)),
                };
                if this.zoom > 1.0 {
                    this.pan += delta;
                    cx.notify();
                    return;
                }
                // One flip per gesture: after a flip, trackpad inertia keeps
                // sending events; re-arm only after the wheel goes quiet.
                let now = Instant::now();
                let idle = now.duration_since(this.last_wheel) > Duration::from_millis(WHEEL_IDLE_MS);
                this.last_wheel = now;
                if idle {
                    this.wheel_armed = true;
                    this.wheel_acc = 0.;
                }
                if !this.wheel_armed {
                    return;
                }
                this.wheel_acc += f32::from(delta.y);
                if this.wheel_acc.abs() > 80. {
                    let down = this.wheel_acc < 0.;
                    this.wheel_acc = 0.;
                    this.wheel_armed = false;
                    if down { this.next_page(cx) } else { this.prev_page(cx) }
                }
            }))
            .into_any_element()
    }

    fn render_top_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let modes = [Mode::Scroll, Mode::Ltr, Mode::Rtl];
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(52.))
            .pl(px(84.)) // traffic lights
            .pr(space::s4())
            .flex()
            .items_center()
            .gap(space::s3())
            .bg(c.overlay)
            .border_b_1()
            .border_color(c.border)
            .occlude()
            .window_control_area(WindowControlArea::Drag)
            .child(ui::button("close", "‹ 返回").on_click(cx.listener(|this, _, _, cx| this.close(cx))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(space::s2())
                    .child(
                        div()
                            .text_size(text::title())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c.text)
                            .truncate()
                            .child(self.detail.summary.title.clone()),
                    )
                    .child(
                        div().text_size(text::small()).text_color(c.text_muted).flex_none().child(self.chapter_title()),
                    ),
            )
            .child(div().flex().gap(px(2.)).p(px(3.)).rounded(radius::md()).bg(c.surface).children(
                modes.into_iter().map(|m| {
                    ui::segment(SharedString::from(format!("mode-{}", m.as_config())), m.label(), self.mode == m)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_mode(m, cx)))
                }),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .p(px(3.))
                    .rounded(radius::md())
                    .bg(c.surface)
                    .child(
                        ui::segment("zoom-out", "－", false)
                            .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom - 0.1, cx))),
                    )
                    .child(
                        ui::segment("zoom-reset", format!("{:.0}%", self.zoom * 100.), false)
                            .on_click(cx.listener(|this, _, _, cx| this.set_zoom(1.0, cx))),
                    )
                    .child(
                        ui::segment("zoom-in", "＋", false)
                            .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom + 0.1, cx))),
                    ),
            )
    }

    fn render_bottom_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let n = self.pages.len();
        let cur = self.current;
        let rtl = self.mode == Mode::Rtl;
        let segments: Vec<AnyElement> = (0..n)
            .map(|i| {
                let filled = i <= cur;
                div()
                    .id(("seg", i))
                    .flex_1()
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .group("seg")
                    .child(
                        div()
                            .w_full()
                            .h(px(if i == cur { 6. } else { 4. }))
                            .bg(if filled { c.accent } else { c.border_strong })
                            .hover(|s| s.h(px(8.))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.go_page(i, cx)))
                    .into_any_element()
            })
            .collect();
        let mut track = div().flex_1().flex().gap(px(1.)).rounded(radius::full()).overflow_hidden();
        track = if rtl { track.flex_row_reverse() } else { track };
        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .h(px(52.))
            .px(space::s4())
            .flex()
            .items_center()
            .gap(space::s4())
            .bg(c.overlay)
            .border_t_1()
            .border_color(c.border)
            .occlude()
            .child(
                ui::button("prev-ch", "上一章")
                    .when(self.chapter_ix == 0, |d| d.opacity(0.4))
                    .on_click(cx.listener(|this, _, _, cx| this.prev_chapter_action(cx))),
            )
            .child(track.children(segments))
            .child(div().w(px(72.)).text_center().text_size(text::small()).text_color(c.text_muted).child(if n == 0 {
                "–".to_string()
            } else {
                format!("{} / {}", cur + 1, n)
            }))
            .child(
                ui::button("next-ch", "下一章")
                    .when(self.chapter_ix + 1 >= self.chapter_count(), |d| d.opacity(0.4))
                    .on_click(cx.listener(|this, _, _, cx| this.next_chapter_action(cx))),
            )
    }
}

/// Load priority of page `i` when the reader is on `cur` (lower first):
/// the current page, then pages ahead, each slightly before the page the
/// same distance behind. Used by both rendering and prefetch.
fn page_priority(i: usize, cur: usize) -> i32 {
    if i >= cur { (i - cur) as i32 * 2 } else { (cur - i) as i32 * 2 + 1 }
}

const WHEEL_IDLE_MS: u64 = 250;

impl Drop for ReaderView {
    /// Window closed / app quit without going through `close()`.
    fn drop(&mut self) {
        if let Some(e) = self.entry() {
            self.history.record(e);
        }
    }
}

fn fit_contain(d: &Decoded, vw: f32, vh: f32, zoom: f32) -> (f32, f32) {
    let aspect = d.aspect();
    let w = vw.min(vh / aspect);
    (w * zoom, w * aspect * zoom)
}

fn page_box(ix: usize, state: ImageState, w: f32, pipeline: &Entity<ImagePipeline>, req: &ImageRequest) -> AnyElement {
    let c = colors();
    match state {
        ImageState::Ready(d) => {
            let h = w * d.aspect();
            div()
                .w(px(w))
                .flex()
                .flex_col()
                .children(
                    d.tiles.iter().enumerate().map(|(i, t)| img(t.clone()).w(px(w)).h(px(h * d.tile_fraction(i)))),
                )
                .into_any_element()
        }
        ImageState::Loading => div()
            .w(px(w))
            .h(px(w * PLACEHOLDER_ASPECT))
            .flex()
            .items_center()
            .justify_center()
            .text_size(text::heading())
            .text_color(c.text_faint)
            .child(format!("{}", ix + 1))
            .into_any_element(),
        ImageState::Failed(e) => {
            let pipeline = pipeline.clone();
            let req = req.clone();
            div()
                .id(("failed", ix))
                .w(px(w))
                .h(px(w * 0.6))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(space::s2())
                .cursor_pointer()
                .child(div().text_color(c.danger).text_size(text::body()).child(format!("第 {} 页加载失败", ix + 1)))
                .child(div().text_color(c.text_faint).text_size(text::small()).child(format!("{e} · 点击重试")))
                .on_click(move |_, _, cx| {
                    pipeline.update(cx, |p, cx| {
                        p.retry(&req);
                        cx.notify();
                    })
                })
                .into_any_element()
        }
    }
}

fn chapter_footer(has_next: bool, next_title: Option<String>, weak: WeakEntity<ReaderView>) -> impl IntoElement {
    let c = colors();
    let mut d = div().w_full().py(px(64.)).flex().flex_col().items_center().gap(space::s3());
    if has_next {
        d = d.child(div().text_size(text::small()).text_color(c.text_faint).child("本章完")).child(
            ui::primary_button("footer-next", format!("下一章 · {}", next_title.unwrap_or_default()))
                .h(px(36.))
                .on_click(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| this.next_chapter_action(cx));
                }),
        );
    } else {
        d = d.child(div().text_size(text::body()).text_color(c.text_faint).child("已经是最后一章了"));
    }
    d
}

impl Focusable for ReaderView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ReaderView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        self.pipeline.update(cx, |p, _| p.release_textures(window));
        self.apply_pending_remeasure();
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));

        let body: AnyElement = if let Some(e) = &self.error {
            ui::empty_state("本章加载失败", e.clone()).into_any_element()
        } else if self.loading {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(ui::spinner_label("正在加载章节…"))
                .into_any_element()
        } else if self.mode == Mode::Scroll {
            div()
                .id("scroll-body")
                .size_full()
                .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _, cx| this.poke_toolbar(cx)))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_toolbar(cx)))
                .child(self.render_scroll(vw, cx))
                .into_any_element()
        } else {
            self.render_paged(vw, vh, cx)
        };

        div()
            .id("reader")
            .key_context(CTX)
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(c.reader_bg)
            .text_color(c.text)
            .font_family(".SystemUIFont")
            .on_action(cx.listener(|this, _: &NextPage, w, cx| {
                if this.mode == Mode::Scroll {
                    this.scroll_mode_step(true, w.viewport_size().height, cx)
                } else {
                    this.next_page(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &PrevPage, w, cx| {
                if this.mode == Mode::Scroll {
                    this.scroll_mode_step(false, w.viewport_size().height, cx)
                } else {
                    this.prev_page(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ArrowLeft, _, cx| this.arrow(false, cx)))
            .on_action(cx.listener(|this, _: &ArrowRight, _, cx| this.arrow(true, cx)))
            .on_action(cx.listener(|this, _: &ScrollDown, _, cx| {
                if this.mode == Mode::Scroll {
                    this.list.scroll_by(px(120.));
                    cx.notify();
                } else if this.zoom > 1.0 {
                    this.pan.y -= px(120.);
                    cx.notify();
                } else {
                    this.next_page(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ScrollUp, _, cx| {
                if this.mode == Mode::Scroll {
                    this.list.scroll_by(px(-120.));
                    cx.notify();
                } else if this.zoom > 1.0 {
                    this.pan.y += px(120.);
                    cx.notify();
                } else {
                    this.prev_page(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextChapter, _, cx| this.next_chapter_action(cx)))
            .on_action(cx.listener(|this, _: &PrevChapter, _, cx| this.prev_chapter_action(cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.set_zoom(this.zoom + 0.1, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.set_zoom(this.zoom - 0.1, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.set_zoom(1.0, cx)))
            .on_action(cx.listener(|this, _: &CycleMode, _, cx| this.set_mode(this.mode.next(), cx)))
            .on_action(cx.listener(|this, _: &ToggleToolbar, _, cx| this.toggle_toolbar(cx)))
            .on_action(cx.listener(|this, _: &CloseReader, _, cx| this.close(cx)))
            .child(body)
            .when(self.toolbar, |d| d.child(self.render_top_bar(cx)).child(self.render_bottom_bar(cx)))
    }
}
