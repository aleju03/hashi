use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tiny_skia::{BlendMode, Color, FillRule, FilterQuality, Mask, Paint, PathBuilder, Pattern, Pixmap, Rect, SpreadMode, Transform};

use super::ini::Rgba;

// tells textures apart for the page, which keeps the sprites it already has
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct Texture {
    pub pixmap: Pixmap,
    pub id: u64,
    // what Chrome's decode cache hands the canvas for a downscaled draw, from level 1
    levels: Mutex<Vec<Arc<Pixmap>>>,
    alpha_top: OnceLock<Option<f64>>,
}

impl Texture {
    pub fn new(pixmap: Pixmap) -> Self {
        Texture { pixmap, id: NEXT_ID.fetch_add(1, Ordering::Relaxed), levels: Mutex::default(), alpha_top: OnceLock::new() }
    }

    pub fn width(&self) -> f64 {
        self.pixmap.width() as f64
    }

    pub fn height(&self) -> f64 {
        self.pixmap.height() as f64
    }

    // the first row with a visible pixel as a fraction of the height, None for blank art
    pub fn alpha_top(&self) -> Option<f64> {
        *self.alpha_top.get_or_init(|| {
            let w = self.pixmap.width() as usize * 4;
            let top = self.pixmap.data().chunks_exact(w).position(|row| row.chunks_exact(4).any(|p| p[3] >= 25))?;
            Some(top as f64 / self.pixmap.height() as f64)
        })
    }

    // the rows under the last one with something seen in it as a fraction of the height, None for
    // blank art. Black doesn't count: receptor art often sits on a black panel that's lost on the stage
    pub fn alpha_bottom(&self) -> Option<f64> {
        let w = self.pixmap.width() as usize * 4;
        let seen = |p: &[u8]| p[3] >= 25 && p[0].max(p[1]).max(p[2]) >= 48;
        let last = self.pixmap.data().chunks_exact(w).rposition(|row| row.chunks_exact(4).any(seen))?;
        Some((self.pixmap.height() as usize - last - 1) as f64 / self.pixmap.height() as f64)
    }

    // Chrome's decode cache builds each level from the one before it, halving the size rounded up
    // with a truncating bilinear filter
    fn level(&self, level: usize) -> Option<Arc<Pixmap>> {
        if level == 0 {
            return None;
        }
        let mut levels = self.levels.lock().unwrap_or_else(|e| e.into_inner());
        while levels.len() < level {
            let next = levels.len() + 1;
            let (w, h) = (level_size(self.pixmap.width(), next), level_size(self.pixmap.height(), next));
            let scaled = resample(levels.last().map_or(&self.pixmap, |l| l), w, h);
            levels.push(Arc::new(scaled));
        }
        Some(levels[level - 1].clone())
    }
}

fn level_size(n: u32, level: usize) -> u32 {
    n.div_ceil(1 << level).max(1)
}

fn resample(src: &Pixmap, w: u32, h: u32) -> Pixmap {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    if (sw, sh) == (w as usize, h as usize) {
        return src.clone();
    }
    let taps = |i: u32, n: u32, sn: usize| {
        let f = ((i as f64 + 0.5) * sn as f64 / n as f64 - 0.5).clamp(0.0, (sn - 1) as f64);
        let i0 = f.floor() as usize;
        (i0, (i0 + 1).min(sn - 1), f - i0 as f64)
    };
    let data = src.data();
    let mut out = Pixmap::new(w, h).expect("non-empty mip level");
    let dst = out.data_mut();
    for y in 0..h {
        let (y0, y1, ty) = taps(y, h, sh);
        for x in 0..w {
            let (x0, x1, tx) = taps(x, w, sw);
            let px = |xx: usize, yy: usize, c: usize| data[(yy * sw + xx) * 4 + c] as f64;
            for c in 0..4 {
                let top = px(x0, y0, c) * (1.0 - tx) + px(x1, y0, c) * tx;
                let bottom = px(x0, y1, c) * (1.0 - tx) + px(x1, y1, c) * tx;
                dst[((y * w + x) * 4) as usize + c] = (top * (1.0 - ty) + bottom * ty) as u8;
            }
        }
    }
    out
}

// the deepest level whose size still covers the drawn size
fn mip_level(src_w: f64, src_h: f64, dst_w: f64, dst_h: f64) -> usize {
    let (tw, th) = ((dst_w + 0.5).floor() as u32, (dst_h + 0.5).floor() as u32);
    let (w, h) = (src_w.ceil() as u32, src_h.ceil() as u32);
    if tw == 0 || th == 0 {
        return 0;
    }
    let mut level = 0;
    while (w > 1 || h > 1) && level < 30 && level_size(w, level + 1) >= tw && level_size(h, level + 1) >= th {
        level += 1;
    }
    level
}

#[derive(Clone, Copy)]
pub(super) struct Draw {
    pub flip: Option<f64>,
    pub clip: Option<[f64; 4]>,
    pub blend: BlendMode,
    pub mips: bool,
}

impl Default for Draw {
    fn default() -> Self {
        Draw { flip: None, clip: None, blend: BlendMode::SourceOver, mips: true }
    }
}

impl Draw {
    // mirrors a draw across the horizontal line through `axis / 2`
    pub fn flipped(axis: f64) -> Self {
        Draw { flip: Some(axis), ..Default::default() }
    }
}

pub(super) struct Canvas {
    pub pixmap: Pixmap,
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Option<Rect> {
    Rect::from_xywh(x as f32, y as f32, w as f32, h as f32)
}

fn paint_color(c: Rgba) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        Canvas { pixmap: Pixmap::new(w, h).expect("canvas size") }
    }

    fn mask(&self, clip: Option<[f64; 4]>) -> Option<Mask> {
        let [x, y, w, h] = clip?;
        let mut mask = Mask::new(self.pixmap.width(), self.pixmap.height())?;
        if let Some(r) = rect(x, y, w, h) {
            mask.fill_path(&PathBuilder::from_rect(r), FillRule::Winding, true, Transform::identity());
        }
        Some(mask)
    }

    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64, color: Rgba, alpha: f64) {
        let Some(r) = rect(x, y, w, h) else { return };
        let mut c = paint_color(color);
        c.apply_opacity(alpha as f32);
        let mut paint = Paint::default();
        paint.set_color(c);
        self.pixmap.fill_rect(r, &paint, Transform::identity(), None);
    }

    pub fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let Some(r) = rect(x, y, w, h) else { return };
        let paint = Paint { blend_mode: BlendMode::Clear, ..Default::default() };
        self.pixmap.fill_rect(r, &paint, Transform::identity(), None);
    }

    pub fn fill_path(&mut self, path: &tiny_skia::Path, color: Rgba) {
        let mut paint = Paint::default();
        paint.set_color(paint_color(color));
        self.pixmap.fill_path(path, &paint, FillRule::Winding, Transform::identity(), None);
    }

    pub fn round_rect(&mut self, x: f64, y: f64, w: f64, h: f64, radius: f64, color: Rgba) {
        let r = radius.min(w / 2.0).min(h / 2.0).max(0.0) as f32;
        let (x, y, w, h) = (x as f32, y as f32, w as f32, h as f32);
        // the cubic that best fits a quarter circle
        let k = r * 0.552_284_8;
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y);
        pb.line_to(x + w - r, y);
        pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
        pb.line_to(x + w, y + h - r);
        pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
        pb.line_to(x + r, y + h);
        pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
        pb.line_to(x, y + r);
        pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
        pb.close();
        if let Some(path) = pb.finish() {
            self.fill_path(&path, color);
        }
    }

    pub fn image(&mut self, tex: &Texture, x: f64, y: f64, w: f64, h: f64, how: Draw) {
        self.image_part(tex, [0.0, 0.0, tex.width(), tex.height()], [x, y, w, h], how);
    }

    pub fn image_part(&mut self, tex: &Texture, src: [f64; 4], dst: [f64; 4], how: Draw) {
        let [sx, sy, sw, sh] = src;
        let [dx, mut dy, dw, dh] = dst;
        if sw <= 0.0 || sh <= 0.0 || dw <= 0.0 || dh <= 0.0 {
            return;
        }
        let (kx, mut ky) = (dw / sw, dh / sh);
        let level = if how.mips { mip_level(sw, sh, dw, dh) } else { 0 };
        let mip = tex.level(level);
        let pixmap = mip.as_deref().unwrap_or(&tex.pixmap);
        let (lx, ly) = (tex.width() / pixmap.width() as f64, tex.height() / pixmap.height() as f64);
        let mut origin_y = dy - sy * ky;
        if let Some(axis) = how.flip {
            dy = axis - dy - dh;
            origin_y = axis - origin_y;
            ky = -ky;
        }
        let (mut ax, ay) = ((kx * lx) as f32, (ky * ly) as f32);
        let (tx, ty) = ((dx - sx * kx) as f32, origin_y as f32);
        // tiny-skia samples any translate-only pattern nearest, even off the pixel grid, where Skia
        // stays bilinear
        if ax == 1.0 && ay == 1.0 && (tx.fract() != 0.0 || ty.fract() != 0.0) {
            ax = 1.0 + f32::EPSILON;
        }
        let shader_ts = Transform::from_row(ax, 0.0, 0.0, ay, tx, ty);
        let shader = Pattern::new(pixmap.as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, 1.0, shader_ts);
        // Blink antialiases an image only when it's under a pixel thin; otherwise Skia fills the
        // pixels whose centres the rect covers
        let anti_alias = dw < 1.0 || dh < 1.0;
        let r = if anti_alias {
            rect(dx, dy, dw, dh)
        } else {
            // clamped first: tiny-skia rounds a negative edge toward zero
            let (cw, ch) = (self.pixmap.width() as f64, self.pixmap.height() as f64);
            let snap = |v: f64, max: f64| (v + 0.5).floor().clamp(0.0, max);
            let (l, t) = (snap(dx, cw), snap(dy, ch));
            let (r, b) = (snap(dx + dw, cw), snap(dy + dh, ch));
            if r <= l || b <= t {
                return;
            }
            rect(l, t, r - l, b - t)
        };
        let Some(r) = r else { return };
        let paint = Paint { shader, blend_mode: how.blend, anti_alias, force_hq_pipeline: false };
        let mask = self.mask(how.clip);
        self.pixmap.fill_rect(r, &paint, Transform::identity(), mask.as_ref());
    }
}

// what Canvas2D's multiply-then-destination-in leaves when a white light texture is tinted
pub(super) fn tinted(tex: &Texture, color: Rgba) -> Texture {
    let mut pixmap = tex.pixmap.clone();
    for p in pixmap.data_mut().chunks_exact_mut(4) {
        let da = p[3] as f64 / 255.0;
        for c in 0..3 {
            let s = color[c] as f64 / 255.0;
            let d = p[c] as f64 / 255.0;
            p[c] = ((s * (1.0 - da) + s * d) * da * 255.0 + 0.5) as u8;
        }
    }
    Texture::new(pixmap)
}
