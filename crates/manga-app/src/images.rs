//! Image pipeline: fetch bytes → decode + downscale + tile on the background
//! executor → byte-budgeted LRU of `RenderImage`s.
//!
//! Why not `img(url)`: GPUI's URI loader cannot attach per-request headers,
//! keeps every image forever and decodes at full resolution. Here we:
//! - decode at most `max_width` device pixels wide (screen-sized, not source-sized)
//! - split very tall strips into tiles so no texture exceeds GPU limits
//! - evict least-recently-used images over a byte budget and release their
//!   atlas textures with `window.drop_image`
//! - schedule loads by priority with bounded concurrency, and cancel loads
//!   the reader no longer needs

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use gpui_kit::{App, Context, RenderImage, SharedString, Task, Window};
use image::{DynamicImage, Frame, imageops::FilterType};
use manga_core::ImageRequest;
use manga_net::ImageFetcher;

/// Largest tile height in pixels. Well under Metal/D3D/Vulkan 2D texture
/// limits (≥ 8192), and small enough that partially visible strips only
/// keep a few tiles hot.
const TILE_H: u32 = 4096;
const MAX_ACTIVE: usize = 4;

pub struct Decoded {
    pub tiles: Vec<Arc<RenderImage>>,
    /// Size in device pixels after downscaling.
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
}

impl Decoded {
    /// Height / width.
    pub fn aspect(&self) -> f32 {
        self.height as f32 / self.width.max(1) as f32
    }

    /// Height of tile `i` as a fraction of the full image height.
    pub fn tile_fraction(&self, i: usize) -> f32 {
        let start = i as u32 * TILE_H;
        let h = (self.height - start.min(self.height)).min(TILE_H);
        h as f32 / self.height.max(1) as f32
    }
}

pub enum ImageState {
    Loading,
    Ready(Arc<Decoded>),
    Failed(SharedString),
}

enum Slot {
    Queued { priority: i32 },
    Loading(#[allow(dead_code)] Task<()>),
    Ready { image: Arc<Decoded>, last_used: u64 },
    Failed(SharedString),
}

pub struct ImagePipeline {
    fetcher: ImageFetcher,
    max_width: u32,
    budget_bytes: usize,
    used_bytes: usize,
    tick: u64,
    slots: HashMap<String, (ImageRequest, Slot)>,
    active: usize,
    pump_scheduled: bool,
    /// Evicted images whose atlas textures must be released on the next frame.
    to_drop: Vec<Arc<RenderImage>>,
}

impl ImagePipeline {
    pub fn new(fetcher: ImageFetcher, max_width: u32, budget_bytes: usize) -> Self {
        Self {
            fetcher,
            max_width,
            budget_bytes,
            used_bytes: 0,
            tick: 0,
            slots: HashMap::new(),
            active: 0,
            pump_scheduled: false,
            to_drop: Vec::new(),
        }
    }

    /// Current state of `req`, queueing it if unknown. Lower `priority`
    /// loads first. Touching a ready image marks it recently used.
    pub fn get(&mut self, req: &ImageRequest, priority: i32, cx: &mut Context<Self>) -> ImageState {
        self.tick += 1;
        let key = req.cache_key();
        match self.slots.get_mut(&key) {
            Some((_, Slot::Ready { image, last_used })) => {
                *last_used = self.tick;
                return ImageState::Ready(image.clone());
            }
            Some((_, Slot::Failed(e))) => return ImageState::Failed(e.clone()),
            Some((_, Slot::Queued { priority: p })) => {
                // Callers compute priority relative to *now* (e.g. distance
                // from the current page), so the latest value wins.
                *p = priority;
                return ImageState::Loading;
            }
            Some((_, Slot::Loading(_))) => return ImageState::Loading,
            None => {}
        }
        self.slots.insert(key, (req.clone(), Slot::Queued { priority }));
        self.schedule_pump(cx);
        ImageState::Loading
    }

    /// Pump once after the current effect cycle, so a render pass can
    /// enqueue many requests (and their priorities) before any starts.
    fn schedule_pump(&mut self, cx: &mut Context<Self>) {
        if self.pump_scheduled {
            return;
        }
        self.pump_scheduled = true;
        let this = cx.entity();
        cx.defer(move |cx| {
            this.update(cx, |p, cx| {
                p.pump_scheduled = false;
                p.pump(cx);
            })
        });
    }

    /// Peek without queueing or touching.
    pub fn peek(&self, req: &ImageRequest) -> Option<Arc<Decoded>> {
        match self.slots.get(&req.cache_key()) {
            Some((_, Slot::Ready { image, .. })) => Some(image.clone()),
            _ => None,
        }
    }

    /// Queue `req` without needing its state (prefetch).
    pub fn prefetch(&mut self, req: &ImageRequest, priority: i32, cx: &mut Context<Self>) {
        let _ = self.get(req, priority, cx);
    }

    /// Cancel queued/in-flight loads not in `keep`. Ready images stay cached.
    /// Freed concurrency slots are refilled from the queue.
    pub fn cancel_except(&mut self, keep: &HashSet<String>, cx: &mut Context<Self>) {
        let mut cancelled_active = 0;
        self.slots.retain(|k, (_, slot)| match slot {
            Slot::Queued { .. } => keep.contains(k),
            Slot::Loading(_) => {
                let keep_it = keep.contains(k);
                if !keep_it {
                    cancelled_active += 1;
                }
                keep_it
            }
            _ => true,
        });
        self.active -= cancelled_active;
        if cancelled_active > 0 {
            self.schedule_pump(cx);
        }
    }

    /// Forget a failed image so it is retried on the next `get`.
    pub fn retry(&mut self, req: &ImageRequest) {
        let key = req.cache_key();
        if matches!(self.slots.get(&key), Some((_, Slot::Failed(_)))) {
            self.slots.remove(&key);
        }
    }

    /// Release textures of evicted images. Call from a render pass.
    pub fn release_textures(&mut self, window: &mut Window) {
        for img in self.to_drop.drain(..) {
            let _ = window.drop_image(img);
        }
    }

    pub fn pump(&mut self, cx: &mut Context<Self>) {
        while self.active < MAX_ACTIVE {
            let next = self
                .slots
                .iter()
                .filter_map(|(k, (_, s))| match s {
                    Slot::Queued { priority } => Some((*priority, k.clone())),
                    _ => None,
                })
                .min();
            let Some((_, key)) = next else { break };
            let req = self.slots[&key].0.clone();
            let fetch = self.fetcher.fetch(req);
            let max_w = self.max_width;
            let k = key.clone();
            let task = cx.spawn(async move |this, cx| {
                let result = match fetch.await {
                    Some(Ok(bytes)) => cx.background_executor().spawn(async move { decode(&bytes, max_w) }).await,
                    Some(Err(e)) => Err(e.to_string()),
                    None => Err("已取消".to_string()),
                };
                let _ = this.update(cx, |this, cx| this.finish(k, result, cx));
            });
            self.active += 1;
            if let Some((_, slot)) = self.slots.get_mut(&key) {
                *slot = Slot::Loading(task);
            }
        }
    }

    fn finish(&mut self, key: String, result: Result<Decoded, String>, cx: &mut Context<Self>) {
        // A cancelled slot was already removed (and `active` decremented).
        let Some((_, slot)) = self.slots.get_mut(&key) else { return };
        if !matches!(slot, Slot::Loading(_)) {
            return;
        }
        self.active -= 1;
        self.tick += 1;
        match result {
            Ok(d) => {
                self.used_bytes += d.bytes;
                *slot = Slot::Ready { image: Arc::new(d), last_used: self.tick };
                self.evict();
            }
            Err(e) => {
                log::warn!("image {key}: {e}");
                *slot = Slot::Failed(e.into());
            }
        }
        self.pump(cx);
        cx.notify();
    }

    fn evict(&mut self) {
        if self.used_bytes <= self.budget_bytes {
            return;
        }
        let mut ready: Vec<(u64, String)> = self
            .slots
            .iter()
            .filter_map(|(k, (_, s))| match s {
                Slot::Ready { last_used, .. } => Some((*last_used, k.clone())),
                _ => None,
            })
            .collect();
        ready.sort();
        // Never evict the most recent few: they are on screen right now.
        let protect = 6.min(ready.len());
        for (_, key) in ready.into_iter().rev().skip(protect).rev() {
            if self.used_bytes <= self.budget_bytes * 9 / 10 {
                break;
            }
            if let Some((_, Slot::Ready { image, .. })) = self.slots.remove(&key) {
                self.used_bytes -= image.bytes;
                self.to_drop.extend(image.tiles.iter().cloned());
            }
        }
    }
}

fn decode(bytes: &[u8], max_width: u32) -> Result<Decoded, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("解码失败：{e}"))?;
    let img = downscale(img, max_width);
    let mut rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    // GPUI textures are BGRA.
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let tiles = if height <= TILE_H {
        vec![Arc::new(RenderImage::new(vec![Frame::new(rgba)]))]
    } else {
        (0..height.div_ceil(TILE_H))
            .map(|i| {
                let y = i * TILE_H;
                let h = TILE_H.min(height - y);
                let tile = image::imageops::crop_imm(&rgba, 0, y, width, h).to_image();
                Arc::new(RenderImage::new(vec![Frame::new(tile)]))
            })
            .collect()
    };
    Ok(Decoded { tiles, width, height, bytes: width as usize * height as usize * 4 })
}

fn downscale(img: DynamicImage, max_width: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w <= max_width {
        return img;
    }
    let nh = ((h as u64 * max_width as u64) / w as u64).max(1) as u32;
    // Triangle is ~3x faster than Lanczos3 and visually equivalent when
    // shrinking line art by < 2x.
    img.resize_exact(max_width, nh, FilterType::Triangle)
}

/// Convenience for views holding an `Entity<ImagePipeline>`.
pub fn image_state(
    pipeline: &gpui_kit::Entity<ImagePipeline>,
    req: &ImageRequest,
    priority: i32,
    cx: &mut App,
) -> ImageState {
    pipeline.update(cx, |p, cx| p.get(req, priority, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn downscales_and_swaps_channels() {
        let d = decode(&png(2000, 1000), 1000).unwrap();
        assert_eq!((d.width, d.height), (1000, 500));
        assert_eq!(d.tiles.len(), 1);
        // red → BGRA: byte 2 is red
        let b = d.tiles[0].as_bytes(0).unwrap();
        assert_eq!(&b[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn tiles_tall_strips() {
        let d = decode(&png(100, 9000), 1000).unwrap();
        assert_eq!(d.tiles.len(), 3);
        let total: f32 = (0..3).map(|i| d.tile_fraction(i)).sum();
        assert!((total - 1.0).abs() < 1e-4);
    }
}
