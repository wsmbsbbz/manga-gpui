//! Small reusable widgets built on design tokens.

use std::{rc::Rc, sync::Arc};

use gpui_kit::{prelude::FluentBuilder as _, *};
use manga_core::{ComicSummary, ImageRequest};

use crate::{
    images::{ImagePipeline, ImageState, image_state},
    theme::{colors, layout, radius, size, space, text},
};

pub type OnComic = Rc<dyn Fn(&ComicSummary, &mut Window, &mut App)>;

/// Shared button geometry. GPUI allows one `hover`/`active` style per
/// element, so variants add their own on top of this.
fn button_base(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .gap(space::s2())
        .h(size::control_h())
        .px(space::s3())
        .rounded(radius::sm())
        .text_size(text::body())
        .border_1()
        .cursor_pointer()
        .child(label.into())
}

/// Neutral button.
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let c = colors();
    button_base(id, label)
        .text_color(c.text)
        .bg(c.surface_raised)
        .border_color(c.border)
        .hover(|s| s.bg(c.surface_hover).border_color(c.border_strong))
        .active(|s| s.opacity(0.85))
}

/// Accent call-to-action.
pub fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let c = colors();
    button_base(id, label)
        .bg(c.accent)
        .border_color(c.accent)
        .text_color(c.on_accent)
        .font_weight(FontWeight::SEMIBOLD)
        .hover(|s| s.opacity(0.92))
        .active(|s| s.opacity(0.8))
}

/// Segmented-control item.
pub fn segment(id: impl Into<ElementId>, label: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    let c = colors();
    div()
        .id(id.into())
        .h(size::segment_h())
        .px(space::s3())
        .flex()
        .items_center()
        .rounded(radius::sm())
        .text_size(text::small())
        .cursor_pointer()
        .when(selected, |d| d.bg(c.surface_hover).text_color(c.text).font_weight(FontWeight::MEDIUM))
        .when(!selected, |d| d.text_color(c.text_muted).hover(|s| s.text_color(c.text)))
        .child(label.into())
}

pub fn spinner_label(label: impl Into<SharedString>) -> Div {
    let c = colors();
    div()
        .flex()
        .items_center()
        .gap(space::s2())
        .text_size(text::body())
        .text_color(c.text_muted)
        .child(div().size(px(6.)).rounded(radius::full()).bg(c.accent))
        .child(label.into())
}

pub fn empty_state(title: impl Into<SharedString>, hint: impl Into<SharedString>) -> Div {
    let c = colors();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(space::s2())
        .child(div().text_size(text::title()).text_color(c.text).child(title.into()))
        .child(div().text_size(text::body()).text_color(c.text_faint).max_w(px(420.)).text_center().child(hint.into()))
}

/// A decoded image filling a box, with a skeleton while loading.
pub fn cover_image(pipeline: &Entity<ImagePipeline>, req: Option<&ImageRequest>, cx: &mut App) -> AnyElement {
    let c = colors();
    let base = div().size_full().overflow_hidden().rounded(radius::md()).bg(c.skeleton);
    let Some(req) = req else { return base.into_any_element() };
    match image_state(pipeline, req, 0, cx) {
        ImageState::Ready(d) => base
            .child(img(d.tiles[0].clone()).size_full().rounded(radius::md()).object_fit(ObjectFit::Cover))
            .into_any_element(),
        ImageState::Loading => base.into_any_element(),
        ImageState::Failed(_) => base
            .flex()
            .items_center()
            .justify_center()
            .text_size(text::caption())
            .text_color(c.text_faint)
            .child("无封面")
            .into_any_element(),
    }
}

fn comic_card(
    ix: usize,
    comic: &ComicSummary,
    card_w: f32,
    pipeline: &Entity<ImagePipeline>,
    on_open: &OnComic,
    cx: &mut App,
) -> AnyElement {
    let c = colors();
    let on_open = on_open.clone();
    let comic_cl = comic.clone();
    div()
        .id(("card", ix))
        .w(px(card_w))
        .flex()
        .flex_col()
        .gap(space::s2())
        .cursor_pointer()
        .group("card")
        .on_click(move |_, window, cx| on_open(&comic_cl, window, cx))
        .child(
            div()
                .w(px(card_w))
                .h(px(card_w * layout::cover_ratio()))
                .rounded(radius::md())
                .border_1()
                .border_color(c.border)
                .shadow_md()
                .group_hover("card", |s| s.border_color(c.accent))
                .child(cover_image(pipeline, comic.cover.as_ref(), cx)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(text::body())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(c.text)
                        .line_clamp(2)
                        .group_hover("card", |s| s.text_color(c.accent))
                        .child(comic.title.clone()),
                )
                .when_some(comic.subtitle.clone(), |d, s| {
                    d.child(div().text_size(text::small()).text_color(c.text_faint).truncate().child(s))
                }),
        )
        .into_any_element()
}

/// Virtualized grid: only visible rows are built, covers decode on demand.
pub fn comic_grid(
    id: &'static str,
    comics: Arc<Vec<ComicSummary>>,
    available_w: f32,
    scroll: UniformListScrollHandle,
    pipeline: Entity<ImagePipeline>,
    on_open: OnComic,
) -> impl IntoElement {
    let gap = layout::card_gap();
    let pad = layout::content_pad();
    let inner = (available_w - pad * 2.).max(layout::card_min_w());
    let cols = (((inner + gap) / (layout::card_min_w() + gap)).floor() as usize).max(1);
    let card_w = (inner - gap * (cols as f32 - 1.)) / cols as f32;
    let rows = comics.len().div_ceil(cols);

    uniform_list(id, rows, move |range, _window, cx| {
        range
            .map(|row| {
                let start = row * cols;
                let end = (start + cols).min(comics.len());
                div()
                    .px(px(pad))
                    .pb(px(gap + 6.))
                    .flex()
                    .gap(px(gap))
                    .children((start..end).map(|i| comic_card(i, &comics[i], card_w, &pipeline, &on_open, cx)))
            })
            .collect()
    })
    .track_scroll(&scroll)
    .size_full()
}

/// Page header with a title and an optional trailing slot.
pub fn page_header(title: impl Into<SharedString>, subtitle: Option<SharedString>) -> Div {
    let c = colors();
    div()
        .flex()
        .items_end()
        .justify_between()
        .gap(space::s4())
        .px(px(layout::content_pad()))
        .pt(size::titlebar_inset())
        .pb(space::s5())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space::s1())
                .child(
                    div()
                        .text_size(text::display())
                        .font_weight(FontWeight::BOLD)
                        .text_color(c.text)
                        .child(title.into()),
                )
                .when_some(subtitle, |d, s| {
                    d.child(div().text_size(text::small()).text_color(c.text_faint).truncate().child(s))
                }),
        )
}
