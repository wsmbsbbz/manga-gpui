//! Manual live check: `cargo run -p manga-source-manhuagui --example live`.
use manga_core::ComicSource;

fn main() {
    let src = manga_source_manhuagui::ManHuaGuiSource::new(manga_net::Runtime::new().unwrap());
    let e = futures::executor::block_on(src.explore(1)).unwrap();
    println!("explore: {} items, more={} first={:?}", e.items.len(), e.has_more, e.items.first().map(|c| &c.title));
    let s = futures::executor::block_on(src.search("海贼", 1)).unwrap();
    println!(
        "search: {} items, more={} first={:?}",
        s.items.len(),
        s.has_more,
        s.items.first().map(|c| (&c.id, &c.title))
    );
    let d = futures::executor::block_on(src.detail("1128")).unwrap();
    println!("detail: {} meta={:?}", d.summary.title, d.meta.keys().collect::<Vec<_>>());
    for g in &d.groups {
        let f = g.chapters.first().unwrap();
        let l = g.chapters.last().unwrap();
        println!("  group {} n={} first={} last={}", g.name, g.chapters.len(), f.title, l.title);
    }
    let ch = d.groups[0].chapters.last().unwrap();
    let p = futures::executor::block_on(src.pages("1128", &ch.id)).unwrap();
    println!("pages of {}: {} first={:?}", ch.title, p.len(), p.first());
}
