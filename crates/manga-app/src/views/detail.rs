use std::sync::Arc;

use gpui_kit::{prelude::FluentBuilder as _, *};
use manga_core::{ComicDetail, ComicSummary};
use manga_store::HistoryEntry;

use super::Nav;
use crate::{
    state::AppState,
    theme::{colors, layout, radius, size, space, text},
    ui,
};

pub struct DetailView {
    source: String,
    comic: ComicSummary,
    detail: Option<Arc<ComicDetail>>,
    error: Option<SharedString>,
    progress: Option<HistoryEntry>,
    pending_resume: Option<(String, usize)>,
    _task: Option<Task<()>>,
}

impl EventEmitter<Nav> for DetailView {}

impl DetailView {
    pub fn new(source: String, comic: ComicSummary, cx: &mut Context<Self>) -> Self {
        let progress = AppState::get(cx).history.get(&source, &comic.id);
        let mut this = Self { source, comic, detail: None, error: None, progress, pending_resume: None, _task: None };
        this.load(cx);
        this
    }

    /// Jump straight into the reader once chapters are known.
    pub fn resume_when_loaded(&mut self, chapter_id: String, page: usize) {
        self.pending_resume = Some((chapter_id, page));
    }

    pub fn refresh_progress(&mut self, cx: &mut Context<Self>) {
        self.progress = AppState::get(cx).history.get(&self.source, &self.comic.id);
        cx.notify();
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let Some(source) = AppState::source(cx, &self.source) else {
            self.error = Some("来源不可用".into());
            return;
        };
        let fut = source.detail(&self.comic.id);
        self._task = Some(cx.spawn(async move |this, cx| {
            let r = fut.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(d) => {
                        let d = Arc::new(d);
                        this.detail = Some(d.clone());
                        if let Some((ch, page)) = this.pending_resume.take() {
                            let ix = d.chapter_index(&ch).unwrap_or(0);
                            this.read(ix, page, cx);
                        }
                    }
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            });
        }));
    }

    fn read(&mut self, chapter_ix: usize, page: usize, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.clone() else { return };
        cx.emit(Nav::Read { source: self.source.clone(), detail, chapter_ix, page });
    }

    fn continue_target(&self) -> (usize, usize) {
        let Some(d) = &self.detail else { return (0, 0) };
        match &self.progress {
            Some(p) => d.chapter_index(&p.chapter_id).map(|i| (i, p.page)).unwrap_or((0, 0)),
            None => (0, 0),
        }
    }
}

impl Render for DetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let covers = AppState::get(cx).covers.clone();
        let cover_req = self.detail.as_ref().and_then(|d| d.summary.cover.clone()).or(self.comic.cover.clone());

        let back = div()
            .flex()
            .px(px(layout::content_pad()))
            .pt(size::titlebar_inset())
            .child(ui::button("back", "‹  返回").on_click(cx.listener(|_, _, _, cx| cx.emit(Nav::Back))));

        let (cont_ix, cont_page) = self.continue_target();
        let read_label: SharedString = match (&self.progress, &self.detail) {
            (Some(p), Some(_)) => format!("继续阅读 · {} 第 {} 页", p.chapter_title, p.page + 1).into(),
            _ => "开始阅读".into(),
        };

        // Prefer the loaded title (the passed-in summary may be a stub, e.g.
        // when resuming from history or a debug route).
        let title = self.detail.as_ref().map(|d| d.summary.title.clone()).unwrap_or_else(|| self.comic.title.clone());
        let mut info = div().flex_1().min_w_0().flex().flex_col().gap(space::s3()).child(
            div()
                .text_size(text::display())
                .font_weight(FontWeight::BOLD)
                .text_color(c.text)
                .line_clamp(2)
                .child(title),
        );
        if let Some(d) = &self.detail {
            for (k, v) in &d.meta {
                info = info.child(
                    div()
                        .flex()
                        .gap(space::s3())
                        .text_size(text::body())
                        .child(div().w(px(40.)).flex_none().text_color(c.text_faint).child(k.clone()))
                        .child(div().min_w_0().text_color(c.text_muted).truncate().child(v.join(" / "))),
                );
            }
            if let Some(desc) = &d.description {
                info = info
                    .child(div().text_size(text::body()).text_color(c.text_muted).line_clamp(4).child(desc.clone()));
            }
            info = info.child(
                div().pt(space::s2()).flex().gap(space::s2()).child(
                    ui::primary_button("read", read_label)
                        .h(size::cta_h())
                        .px(space::s5())
                        .on_click(cx.listener(move |this, _, _, cx| this.read(cont_ix, cont_page, cx))),
                ),
            );
        } else if let Some(e) = &self.error {
            info = info.child(div().text_color(c.danger).text_size(text::body()).child(e.clone()));
            let mut actions = div().flex().gap(space::s2()).child(ui::button("retry-detail", "重试").on_click(
                cx.listener(|this, _, _, cx| {
                    this.load(cx);
                    cx.notify();
                }),
            ));
            if self.source == manga_source_copy::KEY {
                let url = format!("https://www.copy4000.com/comic/{}", self.comic.id);
                actions = actions
                    .child(ui::button("open-official", "在官网打开").on_click(move |_, _, cx| cx.open_url(&url)));
            }
            info = info.child(actions);
        } else {
            info = info.child(ui::spinner_label("加载中…"));
        }

        let hero = div()
            .px(px(layout::content_pad()))
            .pt(space::s6())
            .pb(space::s8())
            .flex()
            .gap(space::s8())
            .child(
                div()
                    .w(px(size::detail_cover_w()))
                    .h(px(size::detail_cover_w() * layout::cover_ratio()))
                    .flex_none()
                    .rounded(radius::lg())
                    .shadow_lg()
                    .child(ui::cover_image(&covers, cover_req.as_ref(), cx)),
            )
            .child(info);

        let mut chapters = div().px(px(layout::content_pad())).pb(space::s8()).flex().flex_col().gap(space::s6());
        if let Some(d) = &self.detail {
            let current = self.progress.as_ref().map(|p| p.chapter_id.clone());
            let mut flat_ix = 0usize;
            for g in &d.groups {
                let mut wrap = div().flex().flex_wrap().gap(space::s2());
                for ch in &g.chapters {
                    let ix = flat_ix;
                    flat_ix += 1;
                    let is_current = current.as_deref() == Some(ch.id.as_str());
                    wrap = wrap.child(
                        div()
                            .id(("ch", ix))
                            .min_w(size::chip_min_w())
                            .max_w(size::chip_max_w())
                            .h(size::chip_h())
                            .px(space::s3())
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(radius::sm())
                            .border_1()
                            .text_size(text::body())
                            .cursor_pointer()
                            .when(is_current, |d| d.border_color(c.accent).bg(c.accent_soft).text_color(c.accent))
                            .when(!is_current, |d| {
                                d.border_color(c.border)
                                    .bg(c.surface)
                                    .text_color(c.text_muted)
                                    .hover(|s| s.bg(c.surface_hover).text_color(c.text).border_color(c.border_strong))
                            })
                            .child(div().truncate().child(ch.title.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| this.read(ix, 0, cx))),
                    );
                }
                chapters = chapters.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space::s3())
                        .child(
                            div()
                                .text_size(text::title())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c.text)
                                .child(format!("{} · {}", g.name, g.chapters.len())),
                        )
                        .child(wrap),
                );
            }
        }

        div().id("detail-scroll").size_full().overflow_y_scroll().child(back).child(hero).child(chapters)
    }
}
