mod images;
mod state;
mod theme;
mod ui;
mod views;

use gpui_kit::*;

use crate::{state::AppState, views::shell::Shell};

actions!(manga, [Quit]);

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);

        if let Err(e) = AppState::init(cx) {
            log::error!("startup failed: {e:#}");
            cx.quit();
            return;
        }

        views::reader::bind_keys(cx);
        // Single-window app: quit once the last window closes.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![Menu {
            name: "Manga".into(), items: vec![MenuItem::action("退出", Quit)], disabled: false
        }]);

        let bounds = Bounds::centered(None, size(px(1280.), px(860.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(760.), px(520.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Manga".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(14.))),
            }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Shell::new(window, cx))).expect("open window");
        cx.activate(true);
    });
}
