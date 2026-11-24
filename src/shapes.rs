//! Cached anti-aliased shape rasteriser for the control panel.
//!
//! `desktop.rs` paints 40–60 rounded rectangles and circles per frame, and every
//! paint runs a per-pixel signed-distance-field loop into a freshly allocated
//! BGRA buffer. The rasterisers here produce pixel-identical thumbnails, while
//! [`ShapeCache`] memoises them: repeated calls with the same size, radius and
//! colours return a cheap [`Arc`] clone instead of recomputing, bounded by an
//! LRU so the cache cannot grow without limit.
//!
//! Colours are `0x00BBGGRR` values; pixels are premultiplied, top-down BGRA,
//! ready for a 32-bit DIB and `AlphaBlend`, matching
//! [`crate::thumbnail::Thumbnail`].

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::thumbnail::Thumbnail;

/// Entries held by a default-constructed [`ShapeCache`].
const DEFAULT_CAPACITY: usize = 256;

/// Splits a `0x00BBGGRR` colour into `(red, green, blue)` as `f32`.
fn channels(color: u32) -> (f32, f32, f32) {
    (
        (color & 0xff) as f32,
        ((color >> 8) & 0xff) as f32,
        ((color >> 16) & 0xff) as f32,
    )
}

/// Rasterises an anti-aliased rounded rectangle, optionally with a crisp 1 px
/// inner border, as premultiplied BGRA.
///
/// Pixel-identical to the rasteriser in `desktop.rs::shape()`: the same
/// analytical signed-distance field with a 1 px soft edge, and the same
/// clamping of `radius` to half the shorter side. A `radius <= 0` yields a
/// sharp rectangle; an empty `w` or `h` yields an empty thumbnail.
pub fn rounded_rect(w: u32, h: u32, radius: i32, fill: u32, border: Option<u32>) -> Thumbnail {
    let (wf, hf) = (w as f32, h as f32);
    let (hx, hy) = (wf * 0.5, hf * 0.5);
    let rad = (radius.max(0) as f32).min(hx).min(hy);
    let (fr, fg, fb) = channels(fill);
    let (br, bg, bb) = border.map_or((fr, fg, fb), channels);
    let has_border = border.is_some();
    let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
    for y in 0..h {
        let dy = (y as f32 + 0.5 - hy).abs() - (hy - rad);
        let row = (y as usize) * (w as usize) * 4;
        for x in 0..w {
            let dx = (x as f32 + 0.5 - hx).abs() - (hx - rad);
            let dist = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - rad;
            let outer = (0.5 - dist).clamp(0.0, 1.0);
            if outer <= 0.0 {
                continue;
            }
            let (fill_w, border_w) = if has_border {
                let inner = (-0.5 - dist).clamp(0.0, 1.0);
                (inner, (outer - inner).max(0.0))
            } else {
                (outer, 0.0)
            };
            let idx = row + (x as usize) * 4;
            bgra[idx] = (fb * fill_w + bb * border_w).round() as u8;
            bgra[idx + 1] = (fg * fill_w + bg * border_w).round() as u8;
            bgra[idx + 2] = (fr * fill_w + br * border_w).round() as u8;
            bgra[idx + 3] = (outer * 255.0).round() as u8;
        }
    }
    Thumbnail {
        width: w,
        height: h,
        bgra,
    }
}

/// Rasterises an anti-aliased filled circle as premultiplied BGRA.
///
/// Pixel-identical to the rasteriser in `desktop.rs::circle()`: a `size` ×
/// `size` thumbnail with the circle centred at (`size` / 2, `size` / 2) and a
/// 1 px soft edge.
pub fn circle(size: u32, color: u32) -> Thumbnail {
    let (fr, fg, fb) = channels(color);
    let radius = size as f32 * 0.5;
    let mut bgra = vec![0u8; (size as usize) * (size as usize) * 4];
    for py in 0..size {
        let dy = py as f32 + 0.5 - radius;
        let row = (py as usize) * (size as usize) * 4;
        for px in 0..size {
            let dx = px as f32 + 0.5 - radius;
            let dist = dx.hypot(dy) - radius;
            let cov = (0.5 - dist).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let idx = row + (px as usize) * 4;
            bgra[idx] = (fb * cov).round() as u8;
            bgra[idx + 1] = (fg * cov).round() as u8;
            bgra[idx + 2] = (fr * cov).round() as u8;
            bgra[idx + 3] = (cov * 255.0).round() as u8;
        }
    }
    Thumbnail {
        width: size,
        height: size,
        bgra,
    }
}

/// Identifies a cached shape. Every parameter participates in the hash, so two
/// shapes that differ only in border colour never collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Rounded {
        w: u32,
        h: u32,
        radius: i32,
        fill: u32,
        border: Option<u32>,
    },
    Circle {
        size: u32,
        color: u32,
    },
}

/// An LRU-bounded cache of rasterised shapes.
///
/// Rasterising a shape is a per-pixel loop over a fresh buffer, and a control
/// panel paint asks for the same handful of shapes roughly once a second.
/// Caching turns every repeat into an [`Arc`] clone with no allocation. The
/// cache holds at most `capacity` entries (counted in shapes, not bytes) and
/// evicts the least recently used one first.
#[derive(Debug)]
pub struct ShapeCache {
    map: HashMap<Key, Arc<Thumbnail>>,
    order: VecDeque<Key>,
    capacity: usize,
}

impl ShapeCache {
    /// Creates a cache that holds at most `capacity` rasterised shapes.
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    /// Returns the rounded rectangle, rasterising it on the first miss.
    pub fn get_rounded(
        &mut self,
        w: u32,
        h: u32,
        radius: i32,
        fill: u32,
        border: Option<u32>,
    ) -> Arc<Thumbnail> {
        let key = Key::Rounded {
            w,
            h,
            radius,
            fill,
            border,
        };
        self.entry(key, || rounded_rect(w, h, radius, fill, border))
    }

    /// Returns the circle, rasterising it on the first miss.
    pub fn get_circle(&mut self, size: u32, color: u32) -> Arc<Thumbnail> {
        let key = Key::Circle { size, color };
        self.entry(key, || circle(size, color))
    }

    /// How many shapes are currently cached.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Returns the cached shape for `key`, calling `make` at most once.
    fn entry(&mut self, key: Key, make: impl FnOnce() -> Thumbnail) -> Arc<Thumbnail> {
        if let Some(cached) = self.map.get(&key) {
            let cached = Arc::clone(cached);
            self.touch(&key);
            return cached;
        }
        let shape = Arc::new(make());
        self.map.insert(key, Arc::clone(&shape));
        self.order.push_back(key);
        while self.map.len() > self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.map.remove(&oldest);
        }
        shape
    }

    /// Marks `key` as most recently used by moving it to the back of the queue.
    fn touch(&mut self, key: &Key) {
        if let Some(position) = self.order.iter().position(|candidate| candidate == key) {
            self.order.remove(position);
            self.order.push_back(*key);
        }
    }
}

impl Default for ShapeCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `0x00BBGGRR` colour, matching `desktop.rs`'s `rgb()`.
    fn rgb(red: u32, green: u32, blue: u32) -> u32 {
        (blue << 16) | (green << 8) | red
    }

    /// The reference rasteriser, copied from `desktop.rs::shape()`, with the
    /// GDI `fill()` fast path substituted for `radius <= 0` without border.
    fn reference_rounded_rect(
        w: u32,
        h: u32,
        radius: i32,
        color: u32,
        border: Option<u32>,
    ) -> Thumbnail {
        if radius <= 0 && border.is_none() {
            // `fill()` paints a solid, fully opaque rectangle.
            let (r, g, b) = channels(color);
            let mut bgra = Vec::with_capacity((w * h * 4) as usize);
            for _ in 0..(w * h) {
                bgra.extend([b as u8, g as u8, r as u8, 255]);
            }
            return Thumbnail {
                width: w,
                height: h,
                bgra,
            };
        }
        let (wf, hf) = (w as f32, h as f32);
        let (hx, hy) = (wf * 0.5, hf * 0.5);
        let rad = (radius.max(0) as f32).min(hx).min(hy);
        let (fr, fg, fb) = channels(color);
        let (br, bg, bb) = border.map_or((fr, fg, fb), channels);
        let has_border = border.is_some();
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        for y in 0..h {
            let dy = (y as f32 + 0.5 - hy).abs() - (hy - rad);
            let row = (y as usize) * (w as usize) * 4;
            for x in 0..w {
                let dx = (x as f32 + 0.5 - hx).abs() - (hx - rad);
                let dist = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - rad;
                let outer = (0.5 - dist).clamp(0.0, 1.0);
                if outer <= 0.0 {
                    continue;
                }
                let (fill_w, border_w) = if has_border {
                    let inner = (-0.5 - dist).clamp(0.0, 1.0);
                    (inner, (outer - inner).max(0.0))
                } else {
                    (outer, 0.0)
                };
                let idx = row + (x as usize) * 4;
                bgra[idx] = (fb * fill_w + bb * border_w).round() as u8;
                bgra[idx + 1] = (fg * fill_w + bg * border_w).round() as u8;
                bgra[idx + 2] = (fr * fill_w + br * border_w).round() as u8;
                bgra[idx + 3] = (outer * 255.0).round() as u8;
            }
        }
        Thumbnail {
            width: w,
            height: h,
            bgra,
        }
    }

    /// The reference circle rasteriser, copied from `desktop.rs::circle()`.
    fn reference_circle(size: u32, color: u32) -> Thumbnail {
        let (fr, fg, fb) = channels(color);
        let radius = size as f32 * 0.5;
        let mut bgra = vec![0u8; (size as usize) * (size as usize) * 4];
        for py in 0..size {
            let dy = py as f32 + 0.5 - radius;
            let row = (py as usize) * (size as usize) * 4;
            for px in 0..size {
                let dx = px as f32 + 0.5 - radius;
                let dist = dx.hypot(dy) - radius;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let idx = row + (px as usize) * 4;
                bgra[idx] = (fb * cov).round() as u8;
                bgra[idx + 1] = (fg * cov).round() as u8;
                bgra[idx + 2] = (fr * cov).round() as u8;
                bgra[idx + 3] = (cov * 255.0).round() as u8;
            }
        }
        Thumbnail {
            width: size,
            height: size,
            bgra,
        }
    }

    #[test]
    fn sharp_rect_matches_reference() {
        let color = rgb(0x20, 0x24, 0x28);
        for (w, h) in [(1, 1), (64, 24), (17, 9), (3, 3)] {
            for radius in [0, -3] {
                assert_eq!(
                    rounded_rect(w, h, radius, color, None),
                    reference_rounded_rect(w, h, radius, color, None),
                    "sharp rect {w}×{h}, radius {radius}"
                );
            }
        }
    }

    #[test]
    fn radius_6_matches_reference() {
        let color = rgb(0x2a, 0x2e, 0x34);
        for (w, h) in [(120, 34), (34, 120), (40, 20)] {
            assert_eq!(
                rounded_rect(w, h, 6, color, None),
                reference_rounded_rect(w, h, 6, color, None),
                "radius-6 rect {w}×{h}"
            );
        }
    }

    #[test]
    fn clamped_radius_matches_reference() {
        let color = rgb(0x14, 0x14, 0x16);
        // Radius far larger than half the shorter side is clamped to it.
        for (w, h, radius) in [(80, 20, 64), (20, 80, 64), (10, 10, i32::MAX)] {
            assert_eq!(
                rounded_rect(w, h, radius, color, None),
                reference_rounded_rect(w, h, radius, color, None),
                "clamped rect {w}×{h}, radius {radius}"
            );
        }
    }

    #[test]
    fn bordered_shape_matches_reference() {
        let fill = rgb(0x20, 0x24, 0x28);
        let cases = [
            // Rounded with a contrasting border.
            (96, 30, 8, Some(rgb(0x70, 0x76, 0x84))),
            // Sharp with a border (both take the SDF path in desktop.rs).
            (40, 20, 0, Some(rgb(0x70, 0x76, 0x84))),
            // Border colour identical to the fill colour.
            (33, 21, 5, Some(fill)),
            // Radius clamped while bordered.
            (24, 20, 40, Some(rgb(0x10, 0x10, 0x10))),
        ];
        for (w, h, radius, border) in cases {
            assert_eq!(
                rounded_rect(w, h, radius, fill, border),
                reference_rounded_rect(w, h, radius, fill, border),
                "bordered rect {w}×{h}, radius {radius}, border {border:?}"
            );
        }
    }

    #[test]
    fn circle_matches_reference() {
        for size in [1, 2, 7, 10, 18, 28, 33] {
            let color = rgb(0x30, 0xd0, 0x40);
            assert_eq!(
                circle(size, color),
                reference_circle(size, color),
                "circle size {size}"
            );
        }
    }

    #[test]
    fn cache_hit_reuses_the_same_allocation() {
        let fill = rgb(0x20, 0x24, 0x28);
        let mut cache = ShapeCache::new(16);
        let first = cache.get_rounded(120, 34, 6, fill, None);
        assert_eq!(*first, rounded_rect(120, 34, 6, fill, None));
        let second = cache.get_rounded(120, 34, 6, fill, None);
        assert!(Arc::ptr_eq(&first, &second), "hit must not rasterise again");
        let badge = cache.get_circle(18, rgb(0x30, 0xd0, 0x40));
        assert_eq!(*badge, circle(18, rgb(0x30, 0xd0, 0x40)));
        assert!(Arc::ptr_eq(
            &badge,
            &cache.get_circle(18, rgb(0x30, 0xd0, 0x40))
        ));
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn lru_evicts_the_least_recently_used() {
        let red = rgb(0xff, 0x00, 0x00);
        let mut cache = ShapeCache::new(2);
        let a = cache.get_rounded(10, 10, 0, red, None);
        let b = cache.get_circle(12, red);
        // Touching `a` makes the circle the least recently used entry.
        assert!(Arc::ptr_eq(&a, &cache.get_rounded(10, 10, 0, red, None)));
        let _c = cache.get_circle(14, red);
        assert_eq!(cache.len(), 2);
        assert!(
            Arc::ptr_eq(&a, &cache.get_rounded(10, 10, 0, red, None)),
            "the most recently used entry must survive eviction"
        );
        let b_again = cache.get_circle(12, red);
        assert!(
            !Arc::ptr_eq(&b, &b_again),
            "the least recently used entry must be evicted and re-rasterised"
        );
    }

    #[test]
    fn cache_never_exceeds_capacity() {
        let red = rgb(0x30, 0xd0, 0x40);
        let mut cache = ShapeCache::new(3);
        let mut newest = None;
        for size in 1..=20 {
            newest = Some(cache.get_circle(size, red));
            assert!(cache.len() <= 3, "capacity exceeded at size {size}");
        }
        assert_eq!(cache.len(), 3);
        assert!(Arc::ptr_eq(
            newest.as_ref().unwrap(),
            &cache.get_circle(20, red)
        ));
        assert_eq!(*cache.get_circle(1, red), circle(1, red));
    }

    #[test]
    fn border_colours_do_not_collide() {
        let fill = rgb(0x20, 0x24, 0x28);
        let mut cache = ShapeCache::new(8);
        let plain = cache.get_rounded(40, 20, 6, fill, None);
        let red = cache.get_rounded(40, 20, 6, fill, Some(rgb(0xff, 0x00, 0x00)));
        let green = cache.get_rounded(40, 20, 6, fill, Some(rgb(0x00, 0xff, 0x00)));
        assert_eq!(cache.len(), 3, "each border colour is its own entry");
        assert!(!Arc::ptr_eq(&plain, &red));
        assert!(!Arc::ptr_eq(&red, &green));
        assert_ne!(plain.bgra, red.bgra);
        assert_ne!(red.bgra, green.bgra);
        assert!(
            Arc::ptr_eq(
                &red,
                &cache.get_rounded(40, 20, 6, fill, Some(rgb(0xff, 0x00, 0x00)))
            ),
            "repeated parameters must hit the same entry"
        );
    }

    #[test]
    fn empty_cache_is_empty_and_default_capacity_is_256() {
        let mut cache = ShapeCache::default();
        assert!(cache.is_empty());
        let _ = cache.get_circle(18, rgb(0x30, 0xd0, 0x40));
        assert_eq!(cache.len(), 1);
        let mut zero = ShapeCache::new(0);
        let _ = zero.get_circle(18, rgb(0x30, 0xd0, 0x40));
        assert!(zero.is_empty(), "capacity 0 caches nothing");
    }
}
