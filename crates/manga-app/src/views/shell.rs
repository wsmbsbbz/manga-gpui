use gpui_kit::{prelude::FluentBuilder as _, *};

use super::{
    Nav, detail::DetailView, history::HistoryView, library::LibraryView, reader::ReaderView, search::SearchView,
};
use crate::{
    state::AppState,
    theme::{colors, layout, radius, size, space, text},
};

/// Dev/demo hook: `MANGA_DEBUG_ROUTE=<comic_id>[/<chapter_id>[/<page>]]`
/// opens the detail page, or the reader at that chapter/page, on startup.
fn debug_route() -> Option<Nav> {
    let route = std::env::var("MANGA_DEBUG_ROUTE").ok().filter(|r| !r.trim().is_empty())?;
    let mut parts = route.splitn(3, '/');
    let id = parts.next()?.to_string();
    let comic = manga_core::ComicSummary { id: id.clone(), title: id, subtitle: None, cover: None, tags: vec![] };
    let source = std::env::var("MANGA_SOURCE").unwrap_or_else(|_| manga_source_local::KEY.to_string());
    Some(match parts.next() {
        None => Nav::OpenComic { source, comic },
        Some(chapter_id) => Nav::Resume {
            source,
            comic,
            chapter_id: chapter_id.to_string(),
            page: parts.next().and_then(|p| p.parse().ok()).unwrap_or(0),
        },
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Library,
    Search,
    History,
}

enum OverlayView {
    Detail(Entity<DetailView>),
    Reader(Entity<ReaderView>),
}

/// A pushed view and its navigation subscription; popping drops both.
struct Overlay {
    view: OverlayView,
    _sub: Subscription,
}

/// Window root: sidebar + active tab, with detail and reader stacked on top.
pub struct Shell {
    tab: Tab,
    /// Key of the source the Library and Search tabs browse.
    source: String,
    stack: Vec<Overlay>,
    library: Entity<LibraryView>,
    search: Entity<SearchView>,
    history: Entity<HistoryView>,
    _tab_subs: Vec<Subscription>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = cx.new(LibraryView::new);
        let search = cx.new(|cx| SearchView::new(window, cx));
        let history = cx.new(HistoryView::new);
        let subs = vec![
            cx.subscribe_in(&library, window, |this, _, nav: &Nav, w, cx| this.navigate(nav.clone(), w, cx)),
            cx.subscribe_in(&search, window, |this, _, nav: &Nav, w, cx| this.navigate(nav.clone(), w, cx)),
            cx.subscribe_in(&history, window, |this, _, nav: &Nav, w, cx| this.navigate(nav.clone(), w, cx)),
        ];
        let mut tab = Tab::Library;
        match std::env::var("MANGA_DEBUG_ROUTE").as_deref() {
            Ok("@history") => tab = Tab::History,
            Ok("@search") => tab = Tab::Search,
            _ => {
                if let Some(nav) = debug_route() {
                    cx.defer_in(window, move |this, window, cx| this.navigate(nav, window, cx));
                }
            }
        }
        // Dev hook: start on a given source (e.g. MANGA_SOURCE=copy_manga).
        let source = std::env::var("MANGA_SOURCE")
            .ok()
            .filter(|k| AppState::get(cx).registry.get(k).is_some())
            .unwrap_or_else(|| manga_source_local::KEY.to_string());
        if source != manga_source_local::KEY {
            library.update(cx, |v, cx| v.set_source(source.clone(), cx));
            search.update(cx, |v, cx| v.set_source(source.clone(), cx));
        }
        Self { tab, source, stack: vec![], library, search, history, _tab_subs: subs }
    }

    fn select_source(&mut self, key: &'static str, cx: &mut Context<Self>) {
        if self.source == key {
            return;
        }
        self.source = key.to_string();
        self.stack.clear();
        self.library.update(cx, |v, cx| v.set_source(key.into(), cx));
        self.search.update(cx, |v, cx| v.set_source(key.into(), cx));
        self.tab = Tab::Library;
        cx.notify();
    }

    fn navigate(&mut self, nav: Nav, window: &mut Window, cx: &mut Context<Self>) {
        match nav {
            Nav::Back => {
                let popped = self.stack.pop();
                if let Some(Overlay { view: OverlayView::Reader(_), .. }) = popped {
                    // Leaving the reader: progress changed, refresh lists.
                    self.history.update(cx, |h, cx| h.reload(cx));
                    if let Some(OverlayView::Detail(d)) = self.stack.last().map(|o| &o.view) {
                        d.update(cx, |d, cx| d.refresh_progress(cx));
                    }
                }
            }
            Nav::OpenComic { source, comic } => {
                let view = cx.new(|cx| DetailView::new(source, comic, cx));
                self.push(OverlayView::Detail(view.clone()), &view, window, cx);
            }
            Nav::Read { source, detail, chapter_ix, page } => {
                let view = cx.new(|cx| ReaderView::new(source, detail, chapter_ix, page, window, cx));
                self.push(OverlayView::Reader(view.clone()), &view, window, cx);
            }
            Nav::Resume { source, comic, chapter_id, page } => {
                let view = cx.new(|cx| {
                    let mut d = DetailView::new(source, comic, cx);
                    d.resume_when_loaded(chapter_id, page);
                    d
                });
                self.push(OverlayView::Detail(view.clone()), &view, window, cx);
            }
        }
        cx.notify();
    }

    fn push<V: EventEmitter<Nav>>(
        &mut self,
        overlay: OverlayView,
        entity: &Entity<V>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sub = cx.subscribe_in(entity, window, |this, _, nav: &Nav, w, cx| this.navigate(nav.clone(), w, cx));
        self.stack.push(Overlay { view: overlay, _sub: sub });
    }

    fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.stack.clear();
        if tab == Tab::History {
            self.history.update(cx, |h, cx| h.reload(cx));
        }
        cx.notify();
    }

    fn nav_item(
        &self,
        id: &'static str,
        glyph: &'static str,
        label: &'static str,
        tab: Tab,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = colors();
        let selected = self.tab == tab && self.stack.is_empty();
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(space::s3())
            .h(size::nav_item_h())
            .px(space::s3())
            .rounded(radius::sm())
            .text_size(text::body())
            .cursor_pointer()
            .when(selected, |d| d.bg(c.accent_soft).text_color(c.accent).font_weight(FontWeight::MEDIUM))
            .when(!selected, |d| d.text_color(c.text_muted).hover(|s| s.bg(c.surface_hover).text_color(c.text)))
            .child(div().w(size::icon()).text_center().child(glyph))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| this.select_tab(tab, cx)))
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        div()
            .w(layout::sidebar_w())
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(c.surface)
            .border_r_1()
            .border_color(c.border)
            .child(div().h(size::titlebar_inset()).window_control_area(WindowControlArea::Drag))
            .child(
                div()
                    .px(space::s4())
                    .pb(space::s5())
                    .flex()
                    .items_center()
                    .gap(space::s2())
                    .child(div().size(size::dot()).rounded(radius::full()).bg(c.accent))
                    .child(
                        div()
                            .text_size(text::title())
                            .font_weight(FontWeight::BOLD)
                            .text_color(c.text)
                            .child("Manga GPUI"),
                    ),
            )
            .child(
                div()
                    .px(space::s2())
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(self.nav_item("nav-lib", "▤", "书架", Tab::Library, cx))
                    .child(self.nav_item("nav-search", "⌕", "搜索", Tab::Search, cx))
                    .child(self.nav_item("nav-hist", "◷", "历史", Tab::History, cx)),
            )
            .child(div().flex_1())
            .child(self.source_selector(cx))
    }

    fn source_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let sources: Vec<(&'static str, &'static str)> =
            AppState::get(cx).registry.all().iter().map(|s| (s.meta().key, s.meta().name)).collect();
        div()
            .px(space::s2())
            .pb(space::s3())
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div().px(space::s3()).pb(space::s1()).text_size(text::caption()).text_color(c.text_faint).child("来源"),
            )
            .children(sources.into_iter().map(|(key, name)| {
                let selected = self.source == key && self.stack.is_empty();
                div()
                    .id(SharedString::from(format!("src-{key}")))
                    .flex()
                    .items_center()
                    .h(size::segment_h())
                    .px(space::s3())
                    .rounded(radius::sm())
                    .text_size(text::small())
                    .cursor_pointer()
                    .when(selected, |d| d.bg(c.surface_hover).text_color(c.text).font_weight(FontWeight::MEDIUM))
                    .when(!selected, |d| d.text_color(c.text_muted).hover(|s| s.text_color(c.text)))
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| this.select_source(key, cx)))
            }))
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let top = self.stack.last().map(|o| &o.view);
        if let Some(OverlayView::Reader(r)) = top {
            return div().size_full().bg(c.reader_bg).child(r.clone()).into_any_element();
        }
        // Outside the reader, still release textures of evicted pages.
        AppState::get(cx).pages.clone().update(cx, |p, _| p.release_textures(window));
        AppState::get(cx).covers.clone().update(cx, |p, _| p.release_textures(window));
        let content: AnyElement = match top {
            Some(OverlayView::Detail(d)) => d.clone().into_any_element(),
            _ => match self.tab {
                Tab::Library => self.library.clone().into_any_element(),
                Tab::Search => self.search.clone().into_any_element(),
                Tab::History => self.history.clone().into_any_element(),
            },
        };
        div()
            .size_full()
            .flex()
            .bg(c.bg)
            .text_color(c.text)
            .font_family(".SystemUIFont")
            .child(self.sidebar(cx))
            .child(div().flex_1().h_full().min_w_0().child(content))
            .into_any_element()
    }
}
