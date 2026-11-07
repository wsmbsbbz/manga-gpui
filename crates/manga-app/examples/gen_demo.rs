//! Generates an original, abstract demo library for manual testing.
//!
//! `cargo run -p manga-app --example gen_demo -- <out_dir>`
//!
//! Covers: chapter folders, a .cbz chapter, a root-level .cbz, a cover file,
//! and tall webtoon strips (> 4096 px, exercising tiling).

use std::{fs, io::Write, path::Path};

use image::{Rgb, RgbImage};

fn hue(h: f32, s: f32, l: f32) -> Rgb<u8> {
    let c = (1. - (2. * l - 1.).abs()) * s;
    let hp = (h % 360.) / 60.;
    let x = c * (1. - (hp % 2. - 1.).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    let m = l - c / 2.;
    Rgb([((r + m) * 255.) as u8, ((g + m) * 255.) as u8, ((b + m) * 255.) as u8])
}

fn rect(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, c: Rgb<u8>) {
    for yy in y..(y + h).min(img.height()) {
        for xx in x..(x + w).min(img.width()) {
            img.put_pixel(xx, yy, c);
        }
    }
}

fn frame(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, t: u32, c: Rgb<u8>) {
    rect(img, x, y, w, t, c);
    rect(img, x, y + h - t, w, t, c);
    rect(img, x, y, t, h, c);
    rect(img, x + w - t, y, t, h, c);
}

fn circle(img: &mut RgbImage, cx: i64, cy: i64, r: i64, c: Rgb<u8>) {
    for y in (cy - r).max(0)..(cy + r).min(img.height() as i64) {
        for x in (cx - r).max(0)..(cx + r).min(img.width() as i64) {
            if (x - cx).pow(2) + (y - cy).pow(2) <= r * r {
                img.put_pixel(x as u32, y as u32, c);
            }
        }
    }
}

/// Page index shown as a row of blocks in the corner (no fonts needed).
fn page_marker(img: &mut RgbImage, n: u32) {
    let ink = Rgb([30, 30, 36]);
    for i in 0..n {
        rect(img, 40 + (i % 10) * 26, img.height() - 70 - (i / 10) * 26, 18, 18, ink);
    }
}

/// A manga-style page: paper background, panel grid, abstract shapes.
fn manga_page(seed: u32, w: u32, h: u32) -> RgbImage {
    let mut img = RgbImage::from_pixel(w, h, Rgb([246, 244, 238]));
    let base = (seed * 47) as f32;
    let m = 36;
    let rows = 2 + seed % 2;
    let row_h = (h - m * 2 - (rows - 1) * 18) / rows;
    for r in 0..rows {
        let y = m + r * (row_h + 18);
        let split = if (seed + r) % 3 == 0 { 1 } else { 2 };
        let col_w = (w - m * 2 - (split - 1) * 18) / split;
        for cix in 0..split {
            let x = m + cix * (col_w + 18);
            let fill = hue(base + (r * 40 + cix * 90) as f32, 0.35, 0.82);
            rect(&mut img, x, y, col_w, row_h, fill);
            circle(
                &mut img,
                (x + col_w / 2) as i64,
                (y + row_h / 2) as i64,
                (row_h.min(col_w) / 4) as i64,
                hue(base + 180., 0.5, 0.55),
            );
            frame(&mut img, x, y, col_w, row_h, 5, Rgb([20, 20, 24]));
        }
    }
    page_marker(&mut img, seed % 20 + 1);
    img
}

/// A tall webtoon strip: vertical gradient with floating shapes.
fn strip(seed: u32, w: u32, h: u32) -> RgbImage {
    let mut img = RgbImage::new(w, h);
    for y in 0..h {
        let c = hue((seed * 60) as f32 + y as f32 / h as f32 * 120., 0.45, 0.25 + 0.5 * (y as f32 / h as f32));
        for x in 0..w {
            img.put_pixel(x, y, c);
        }
    }
    let mut y = 300;
    let mut k = 0;
    while y + 400 < h {
        let x = if k % 2 == 0 { 80 } else { w - 480 };
        rect(&mut img, x, y, 400, 300, Rgb([250, 250, 250]));
        frame(&mut img, x, y, 400, 300, 6, Rgb([20, 20, 24]));
        circle(&mut img, (x + 200) as i64, (y + 150) as i64, 90, hue((seed * 60 + k * 30) as f32, 0.6, 0.5));
        y += 900;
        k += 1;
    }
    page_marker(&mut img, seed % 20 + 1);
    img
}

fn save(img: &RgbImage, p: &Path) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    img.save(p).unwrap();
}

fn jpeg_bytes(img: &RgbImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

fn cbz(p: &Path, pages: &[RgbImage]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut z = zip::ZipWriter::new(fs::File::create(p).unwrap());
    for (i, img) in pages.iter().enumerate() {
        z.start_file(format!("{:03}.jpg", i + 1), zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(&jpeg_bytes(img)).unwrap();
    }
    z.finish().unwrap();
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: gen_demo <out_dir>");
    let root = Path::new(&out);

    // Paged manga: folder chapters + one .cbz chapter + cover.
    let book = root.join("星屑观测站");
    save(&manga_page(99, 1000, 1440), &book.join("cover.jpg"));
    for ch in 1..=3u32 {
        for p in 1..=12u32 {
            save(&manga_page(ch * 13 + p, 1000, 1440), &book.join(format!("第{ch}话/{p:03}.jpg")));
        }
    }
    cbz(&book.join("第4话.cbz"), &(1..=10).map(|p| manga_page(60 + p, 1000, 1440)).collect::<Vec<_>>());

    // Webtoon: tall strips over the 4096 px tile height.
    let toon = root.join("漫游长卷");
    for ch in 1..=2u32 {
        for p in 1..=6u32 {
            let h = 3000 + (p % 3) * 1800; // 3000 / 4800 / 6600
            save(&strip(ch * 7 + p, 800, h), &toon.join(format!("第{ch}章/{p:02}.png")));
        }
    }

    // Images directly in the comic folder → single chapter.
    for p in 1..=8u32 {
        save(&manga_page(200 + p, 900, 1300), &root.join("短篇集").join(format!("{p:02}.jpg")));
    }

    // Root-level archive.
    cbz(&root.join("单行本样张.cbz"), &(1..=6).map(|p| manga_page(300 + p, 1000, 1440)).collect::<Vec<_>>());

    println!("demo library written to {}", root.display());
}
