use std::rc::Rc;

use gpui_kit::*;
use manga_store::HistoryEntry;

use super::{Nav, now_secs};
use crate::{
    state::AppState,
    theme::{colors, layout, radius, size, space, text},
    ui,
};

pub struct HistoryView {
    entries: Rc<Vec<HistoryEntry>>,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<Nav> for HistoryView {}

impl HistoryView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self { entries: Rc::default(), scroll: UniformListScrollHandle::new() };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.entries = Rc::new(AppState::get(cx).history.recent(500));
        cx.notify();
    }
}

fn relative_time(t: i64) -> String {
    let d = (now_secs() - t).max(0);
    match d {
        0..60 => "刚刚".into(),
        60..3600 => format!("{} 分钟前", d / 60),
        3600..86400 => format!("{} 小时前", d / 3600),
        _ => format!("{} 天前", d / 86400),
    }
}

impl Render for HistoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let body: AnyElement = if self.entries.is_empty() {
            ui::empty_state("还没有阅读记录", "打开任意一部漫画开始阅读，进度会自动保存。").into_any_element()
        } else {
            let entries = self.entries.clone();
            let weak = cx.weak_entity();
            let covers = AppState::get(cx).covers.clone();
            uniform_list("history-list", entries.len(), move |range, _, cx| {
                range
                    .map(|i| {
                        let e = &entries[i];
                        let nav = Nav::Resume {
                            source: e.source.clone(),
                            comic: e.comic.clone(),
                            chapter_id: e.chapter_id.clone(),
                            page: e.page,
                        };
                        let weak = weak.clone();
                        div().px(px(layout::content_pad())).pb(space::s2()).child(
                            div()
                                .id(("hist", i))
                                .flex()
                                .items_center()
                                .gap(space::s4())
                                .p(space::s2())
                                .rounded(radius::md())
                                .cursor_pointer()
                                .hover(|s| s.bg(c.surface_hover))
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |_, cx| cx.emit(nav.clone()));
                                })
                                .child(div().w(size::thumb_w()).h(size::thumb_h()).flex_none().child(ui::cover_image(
                                    &covers,
                                    e.comic.cover.as_ref(),
                                    cx,
                                )))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(space::s1())
                                        .child(
                                            div()
                                                .text_size(text::title())
                                                .text_color(c.text)
                                                .truncate()
                                                .child(e.comic.title.clone()),
                                        )
                                        .child(div().text_size(text::small()).text_color(c.text_muted).child(format!(
                                            "读到 {} · 第 {} 页",
                                            e.chapter_title,
                                            e.page + 1
                                        ))),
                                )
                                .child(
                                    div()
                                        .text_size(text::small())
                                        .text_color(c.text_faint)
                                        .child(relative_time(e.updated_at)),
                                ),
                        )
                    })
                    .collect()
            })
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(ui::page_header("历史", Some(format!("{} 条记录", self.entries.len()).into())))
            .child(div().flex_1().min_h_0().child(body))
    }
}
