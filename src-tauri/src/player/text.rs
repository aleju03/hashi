use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

use super::gpu::{Gpu, TexId};

const FONT: &[u8] = include_bytes!("../../fonts/TrackerRound-SemiBold.ttf");
// strings drawn into textures, the least recently used dropped past this many
const KEPT: usize = 48;

pub struct Text {
    font: FontRef<'static>,
    // and the frame each was last drawn in
    kept: Vec<(String, u32, TexId, f32, f32, u64)>,
    frame: u64,
}

pub struct Drawn {
    pub tex: TexId,
    pub width: f32,
    pub height: f32,
}

impl Text {
    pub fn new() -> Text {
        Text { font: FontRef::try_from_slice(FONT).expect("font"), kept: vec![], frame: 0 }
    }

    pub fn next_frame(&mut self) {
        self.frame += 1;
    }

    // white, `px` tall, as a texture the size of its ink plus a little room
    pub fn get(&mut self, gpu: &mut Gpu, text: &str, px: f32) -> Option<Drawn> {
        let size = px.round().max(4.0) as u32;
        if let Some(i) = self.kept.iter().position(|(t, s, ..)| t == text && *s == size) {
            let mut k = self.kept.remove(i);
            k.5 = self.frame;
            let drawn = Drawn { tex: k.2, width: k.3, height: k.4 };
            self.kept.push(k);
            return Some(drawn);
        }
        let font = self.font.as_scaled(PxScale::from(size as f32));
        let pad = 2.0;
        let mut x = pad;
        let mut glyphs = vec![];
        let mut last = None;
        for c in text.chars() {
            let id = font.glyph_id(c);
            if let Some(prev) = last {
                x += font.kern(prev, id);
            }
            glyphs.push(id.with_scale_and_position(size as f32, ab_glyph::point(x, pad + font.ascent())));
            x += font.h_advance(id);
            last = Some(id);
        }
        let (w, h) = ((x + pad).ceil().max(1.0) as u32, (font.ascent() - font.descent() + pad * 2.0).ceil().max(1.0) as u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for g in glyphs {
            let Some(outline) = self.font.outline_glyph(g) else { continue };
            let b = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                    return;
                }
                let i = ((py as u32 * w + px as u32) * 4) as usize;
                let a = (cov.clamp(0.0, 1.0) * 255.0).round() as u8;
                let a = a.max(rgba[i + 3]);
                rgba[i..i + 4].copy_from_slice(&[a, a, a, a]);
            });
        }
        let tex = gpu.texture(w, h, &rgba);
        // a texture drawn this frame is still in its Frame and a freed slot gets reused, so only ones
        // from earlier frames go; past KEPT the cache grows for this frame
        while self.kept.len() >= KEPT && self.kept[0].5 != self.frame {
            let (_, _, old, ..) = self.kept.remove(0);
            gpu.drop_texture(old);
        }
        self.kept.push((text.to_string(), size, tex, w as f32, h as f32, self.frame));
        Some(Drawn { tex, width: w as f32, height: h as f32 })
    }
}
