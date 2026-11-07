//! Design tokens: colors, spacing, radii, type scale, control sizes, motion.
//! Shared and repeated values live here. One-off geometry local to a single
//! view (e.g. reader overlay math) may stay inline.

use gpui_kit::{Hsla, Pixels, px};

/// Neutral-dark palette tuned for reading: low-glare background, warm accent.
pub struct Palette {
    pub bg: Hsla,
    pub surface: Hsla,
    pub surface_raised: Hsla,
    pub surface_hover: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub reader_bg: Hsla,
    pub overlay: Hsla,
    pub skeleton: Hsla,
}

pub const fn c(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla { h: h / 360.0, s, l, a }
}

pub static COLORS: Palette = Palette {
    bg: c(230.0, 0.10, 0.075, 1.0),
    surface: c(230.0, 0.09, 0.105, 1.0),
    surface_raised: c(230.0, 0.08, 0.14, 1.0),
    surface_hover: c(230.0, 0.08, 0.17, 1.0),
    border: c(230.0, 0.08, 0.19, 1.0),
    border_strong: c(230.0, 0.08, 0.26, 1.0),
    text: c(230.0, 0.15, 0.93, 1.0),
    text_muted: c(230.0, 0.08, 0.64, 1.0),
    text_faint: c(230.0, 0.06, 0.45, 1.0),
    accent: c(18.0, 0.92, 0.60, 1.0),
    accent_soft: c(18.0, 0.92, 0.60, 0.14),
    on_accent: c(18.0, 0.40, 0.08, 1.0),
    danger: c(355.0, 0.75, 0.62, 1.0),
    reader_bg: c(0.0, 0.0, 0.03, 1.0),
    overlay: c(230.0, 0.12, 0.06, 0.86),
    skeleton: c(230.0, 0.08, 0.15, 1.0),
};

pub fn colors() -> &'static Palette {
    &COLORS
}

/// 4pt spacing scale.
pub mod space {
    use super::*;
    pub fn s1() -> Pixels {
        px(4.)
    }
    pub fn s2() -> Pixels {
        px(8.)
    }
    pub fn s3() -> Pixels {
        px(12.)
    }
    pub fn s4() -> Pixels {
        px(16.)
    }
    pub fn s5() -> Pixels {
        px(20.)
    }
    pub fn s6() -> Pixels {
        px(24.)
    }
    pub fn s8() -> Pixels {
        px(32.)
    }
}

pub mod radius {
    use super::*;
    pub fn sm() -> Pixels {
        px(6.)
    }
    pub fn md() -> Pixels {
        px(10.)
    }
    pub fn lg() -> Pixels {
        px(14.)
    }
    pub fn full() -> Pixels {
        px(999.)
    }
}

/// Type scale (px).
pub mod text {
    use super::*;
    pub fn caption() -> Pixels {
        px(11.)
    }
    pub fn small() -> Pixels {
        px(12.)
    }
    pub fn body() -> Pixels {
        px(13.)
    }
    pub fn title() -> Pixels {
        px(15.)
    }
    pub fn heading() -> Pixels {
        px(20.)
    }
    pub fn display() -> Pixels {
        px(26.)
    }
}

pub mod layout {
    use super::*;
    pub fn sidebar_w() -> Pixels {
        px(208.)
    }
    pub fn card_min_w() -> f32 {
        148.
    }
    pub fn card_gap() -> f32 {
        18.
    }
    /// Cover aspect (h / w).
    pub fn cover_ratio() -> f32 {
        1.38
    }
    pub fn content_pad() -> f32 {
        28.
    }
}

/// Control and component dimensions.
pub mod size {
    use super::*;
    /// Standard button height.
    pub fn control_h() -> Pixels {
        px(30.)
    }
    /// Segmented-control item height.
    pub fn segment_h() -> Pixels {
        px(26.)
    }
    /// Sidebar navigation row height.
    pub fn nav_item_h() -> Pixels {
        px(34.)
    }
    /// Chapter chip.
    pub fn chip_h() -> Pixels {
        px(34.)
    }
    pub fn chip_min_w() -> Pixels {
        px(96.)
    }
    pub fn chip_max_w() -> Pixels {
        px(220.)
    }
    /// Glyph column in nav rows.
    pub fn icon() -> Pixels {
        px(16.)
    }
    /// Brand dot in the sidebar.
    pub fn dot() -> Pixels {
        px(10.)
    }
    /// Space reserved under the transparent macOS title bar.
    pub fn titlebar_inset() -> Pixels {
        px(44.)
    }
    pub fn thumb_w() -> Pixels {
        px(52.)
    }
    pub fn thumb_h() -> Pixels {
        px(72.)
    }
    pub fn detail_cover_w() -> f32 {
        200.
    }
    pub fn search_w() -> Pixels {
        px(560.)
    }
    pub fn cta_h() -> Pixels {
        px(36.)
    }
}

/// Motion durations (ms).
pub mod motion {
    /// Reader toolbar auto-hide delay.
    pub const TOOLBAR_HIDE: u64 = 2200;
}
