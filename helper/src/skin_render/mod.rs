mod canvas;
mod edit;
mod ini;
mod library;
mod load;
mod pattern;
mod scene;
mod write;

use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tiny_skia::BlendMode;

pub(crate) use library::{images, thumb};
pub(crate) use write::{apply, edited, fix_tiffs, pack, tiffs, undo};

use canvas::{tinted, Draw, Texture};
use ini::{hex, Rgba};
use load::{Asset, Profile, Settings};
use pattern::{Layout, HIT_LINE_MIN_FRACTION};
use scene::{Body, Column, Frame, Op, Scene};

const WIDTH: f64 = 1280.0;
const HEIGHT: f64 = 720.0;
// skin art is authored in a 768-tall space and the stage in a 480-tall one
const UNIT: f64 = 480.0 / 768.0;
// lazer stretches a body that isn't NoteBodyStyle 0 over 32800 of its 768ths
const LAZER_BODY_SPAN: f64 = 32800.0 * UNIT;
const BLACK: Rgba = [0, 0, 0, 255];
const WHITE: Rgba = [255, 255, 255, 255];
const FAINT: Rgba = [255, 255, 255, 64];
const PINK: Rgba = [255, 102, 171, 255];

// mania-hub's skin preview (src/lib/skin-preview-render.ts) ported as is, down to Chrome's canvas
// quirks, since that one is checked against the game 1:1. A change there belongs here too.
pub(crate) fn render(dir: &Path, keys: usize) -> Result<Vec<u8>, String> {
    let keys = keys.clamp(1, 10);
    let (settings, profile) = load::import(dir, keys)?;
    Builder::new(&settings, &profile, keys).scene().png()
}

// the skin as loaded, which every edit starts from
struct Base {
    settings: Settings,
    profile: Profile,
    skin: load::Skin,
    sources: edit::Sources,
}

type Slot<T> = Mutex<Option<(String, Arc<T>)>>;

// The page asks for a scene and then each of its sprites, and again on every edit. One skin load
// is kept for those, and the scene of the latest edit.
static BASE: Slot<Base> = Mutex::new(None);
static SCENE: Slot<Scene> = Mutex::new(None);

fn slot<T>(slot: &Slot<T>, key: String, make: impl FnOnce() -> Result<T, String>) -> Result<Arc<T>, String> {
    let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((k, v)) = held.as_ref() {
        if *k == key {
            return Ok(v.clone());
        }
    }
    *held = None;
    let v = Arc::new(make()?);
    *held = Some((key, v.clone()));
    Ok(v)
}

fn cached(dir: &Path, keys: usize, at: &str, edits: &str) -> Result<Arc<Scene>, String> {
    let keys = keys.clamp(1, 10);
    let base_key = format!("{}\n{keys}\n{at}", dir.display());
    slot(&SCENE, format!("{base_key}\n{edits}"), || {
        let base = slot(&BASE, base_key, || {
            let skin = load::Skin::open(dir);
            let (settings, profile) = load::import_from(&skin, keys)?;
            Ok(Base { settings, profile, skin, sources: Default::default() })
        })?;
        let (profile, _, _) = edit::apply(&base.profile, &edit::Edits::parse(edits), keys, &base.skin, &base.sources, false);
        let mut builder = Builder::new(&base.settings, &profile, keys);
        // recoloured notes would tint the backdrop, which should stay the skin's while it's edited
        builder.accent = skin_accent(&base.profile);
        let mut scene = builder.scene();
        scene.cut_now = cut_now(&base.profile, keys);
        scene.own = own_names(&base.skin, &base.profile, keys);
        Ok(scene)
    })
}

// everything the page needs to draw the preview itself, sprites by index
pub(crate) fn scene(dir: &Path, keys: usize, at: &str, edits: &str) -> Result<Value, String> {
    Ok(cached(dir, keys, at, edits)?.json())
}

pub(crate) fn sprite(dir: &Path, keys: usize, at: &str, edits: &str, id: usize) -> Result<Option<Vec<u8>>, String> {
    Ok(cached(dir, keys, at, edits)?.sprite_png(id))
}

// the still with edits drawn in, to check the page against
pub(crate) fn render_edited(dir: &Path, keys: usize, at: &str, edits: &str) -> Result<Vec<u8>, String> {
    cached(dir, keys, at, edits)?.png()
}

fn own_names(skin: &load::Skin, p: &Profile, keys: usize) -> Vec<Value> {
    let name = |a: &Option<Asset>| a.as_ref().and_then(|a| library::name_of(skin, a));
    (0..keys)
        .map(|c| {
            let col = p.columns.get(c).cloned().unwrap_or_default();
            serde_json::json!({
                "note": name(&col.tap),
                "head": name(&col.head),
                "body": name(&col.body),
                "tail": name(&col.tail),
                "key": name(&col.key),
                "keyDown": name(&col.key_down),
            })
        })
        .collect()
}

// each column's transparent lead-in at the tail end of the body, in skin units
fn cut_now(p: &Profile, keys: usize) -> Vec<Option<f64>> {
    (0..keys)
        .map(|c| {
            let asset = p.columns.get(c)?.body.as_ref()?;
            let tex = asset.image.as_ref()?;
            let full = tex.height().max(asset.source_height.unwrap_or(0.0));
            let rows = tex.alpha_top()? * tex.height();
            Some(rows / edit::rows_per_unit(asset.scale, full, p.lazer_bodies))
        })
        .collect()
}

fn image(asset: Option<&Asset>) -> Option<&Arc<Texture>> {
    asset?.image.as_ref()
}

struct Builder<'a> {
    s: &'a Settings,
    p: &'a Profile,
    keys: usize,
    l: Layout,
    up: bool,
    sprites: Vec<Arc<Texture>>,
    accent: Rgba,
}

impl<'a> Builder<'a> {
    fn new(s: &'a Settings, p: &'a Profile, keys: usize) -> Self {
        let l = pattern::layout(&p.column_widths, p.column_width, p.column_spacing, p.column_start, keys);
        Builder { s, p, keys, l, up: s.upscroll, sprites: vec![], accent: skin_accent(p) }
    }

    fn sprite(&mut self, tex: &Arc<Texture>) -> usize {
        if let Some(i) = self.sprites.iter().position(|t| Arc::ptr_eq(t, tex)) {
            return i;
        }
        self.sprites.push(tex.clone());
        self.sprites.len() - 1
    }

    fn image(&mut self, tex: &Arc<Texture>, dst: [f64; 4], how: Draw) -> Op {
        let sprite = self.sprite(tex);
        Op::Image { sprite, src: [0.0, 0.0, tex.width(), tex.height()], dst, how }
    }

    fn image_at(&mut self, tex: &Arc<Texture>, x: f64, y: f64, w: f64, h: f64, flip: bool) -> Op {
        let how = if flip { Draw::flipped(y * 2.0 + h) } else { Draw::default() };
        self.image(tex, [x, y, w, h], how)
    }

    fn map_y(&self, y: f64) -> f64 {
        if self.up {
            HEIGHT - y
        } else {
            y
        }
    }

    // sprite height follows the art's aspect at WidthForNoteHeightScale, not the lane width
    fn note_height(&self, tex: &Texture) -> f64 {
        let width = (self.p.note_height_scale * self.l.scale).max(1.0);
        (tex.height() * (width / tex.width())).max(1.0)
    }

    fn stage_height(&self, asset: &Asset, tex: &Texture) -> f64 {
        (asset.native_height(tex) * UNIT * self.l.scale).max(1.0)
    }

    fn frames(&mut self, asset: Option<&Asset>, tail: bool) -> Vec<Frame> {
        let Some(asset) = asset else { return vec![] };
        std::iter::once(asset)
            .chain(&asset.frames)
            .map(|a| match &a.image {
                Some(tex) => Frame {
                    sprite: Some(self.sprite(tex)),
                    h: self.note_height(tex),
                    top: if tail { tex.alpha_top() } else { None },
                },
                None => Frame { sprite: None, h: 0.0, top: None },
            })
            .collect()
    }

    fn scene(mut self) -> Scene {
        let (p, s, keys) = (self.p, self.s, self.keys);
        let assets: Vec<load::Column> = (0..keys).map(|c| p.columns.get(c).cloned().unwrap_or_default()).collect();
        let accent = self.accent;

        let hit = s_position(p.hit_position, s.hit_position);
        let hit_gap = hit * UNIT * self.l.scale;
        let judgment_y = (HEIGHT * HIT_LINE_MIN_FRACTION).max((HEIGHT * 0.95).min(HEIGHT - hit_gap));
        let tallest = (0..keys)
            .map(|col| match image(assets[col].tap.as_ref()) {
                Some(tex) => self.note_height(tex),
                None => (self.l.lane_widths[col] * 0.3).max(10.0),
            })
            .fold(f64::NEG_INFINITY, f64::max);
        let pattern = pattern::pattern(keys, judgment_y, tallest.min(HEIGHT * 0.24));
        let line_y = self.map_y(judgment_y);

        let mut base = self.backdrop(accent);
        let l = &self.l;
        base.push(Op::Fill { rect: [l.stage_x, 0.0, l.stage_width, HEIGHT], color: BLACK, alpha: 1.0 });
        for col in 0..keys {
            if let Some(Some(bg)) = p.backgrounds.get(col) {
                let rect = [l.lane_xs[col], 0.0, l.lane_widths[col], HEIGHT];
                base.push(Op::Clear { rect });
                base.push(Op::Fill { rect, color: *bg, alpha: 1.0 });
            }
        }
        base.extend(self.column_lines());

        let stage = &p.stage;
        let mut columns = vec![];
        let mut tints: Vec<(Rgba, Arc<Texture>)> = vec![];
        for (col, a) in assets.iter().enumerate() {
            let (x, w) = (self.l.lane_xs[col], self.l.lane_widths[col]);
            let mut light = vec![];
            if let Some((asset, tex)) = stage.light.as_ref().and_then(|a| Some((a, a.image.as_ref()?))) {
                // the column light's bottom sits at LightPosition, not the hit position; measured
                // from the judgement line so the declared gap survives the card clamping the line
                let height = self.stage_height(asset, tex);
                let light_units = 480.0 - p.light_position.unwrap_or(413.0);
                let shift = (hit * UNIT - light_units) * self.l.scale;
                let tint = stage.light_colors.get(col).copied().flatten();
                let art = match tint {
                    Some(c) => match tints.iter().find(|(t, _)| *t == c) {
                        Some((_, art)) => art.clone(),
                        None => {
                            let art = Arc::new(tinted(tex, c));
                            tints.push((c, art.clone()));
                            art
                        }
                    },
                    None => tex.clone(),
                };
                let how = Draw { mips: tint.is_none(), ..Default::default() };
                light.push(if self.up {
                    let y = line_y - shift;
                    self.image(&art, [x, y, w, height], Draw { flip: Some(y * 2.0 + height), ..how })
                } else {
                    self.image(&art, [x, line_y + shift - height, w, height], how)
                });
            }
            let key_up = self.key(a.key.as_ref(), x, w, hit_gap, line_y, FAINT);
            let key_down = self.key(a.key_down.as_ref().or(a.key.as_ref()), x, w, hit_gap, line_y, accent);
            let mut lighting = vec![];
            if let Some((asset, tex)) = stage.lighting.as_ref().and_then(|a| Some((a, a.image.as_ref()?))) {
                // additive like in game, LightingNWidth overriding the art's own width
                let declared = stage.lighting_widths.get(col).copied().filter(|w| *w > 0.0);
                let lw = (declared.unwrap_or(asset.native_width(tex) * UNIT) * self.l.scale).max(1.0);
                let lh = (lw * (tex.height() / tex.width())).max(1.0);
                let how = Draw { blend: BlendMode::Plus, ..Default::default() };
                lighting.push(self.image(tex, [x + w / 2.0 - lw / 2.0, line_y - lh / 2.0, lw, lh], how));
            }
            let body = a.body.as_ref().and_then(|asset| {
                let tex = asset.image.as_ref()?;
                // stable gives each texel row a fixed 480/768 of a unit whatever the art's width;
                // lazer stretches one copy over its span
                let full = tex.height().max(asset.source_height.unwrap_or(0.0));
                let row = if p.lazer_bodies { LAZER_BODY_SPAN * self.l.scale / full } else { UNIT * self.l.scale / asset.scale };
                let style = p.body_styles.get(col).copied().unwrap_or(1.0);
                Some(Body { sprite: self.sprite(tex), sw: tex.width(), sh: tex.height(), style, row })
            });
            columns.push(Column {
                x,
                w,
                tap: self.frames(a.tap.as_ref(), false),
                head: self.frames(a.head.as_ref(), false),
                tail: self.frames(a.tail.as_ref().or(a.head.as_ref()).or(a.tap.as_ref()), true),
                body,
                tap_color: p.tap_colors.get(col).copied().flatten().unwrap_or(p.tap_color),
                head_color: p.head_colors.get(col).copied().flatten().unwrap_or(p.head_color),
                light,
                key_up,
                key_down,
                lighting,
            });
        }

        let mut hint = vec![];
        if let Some((asset, tex)) = stage.hint.as_ref().and_then(|a| Some((a, a.image.as_ref()?))) {
            let height = self.stage_height(asset, tex);
            let (x, w) = (self.l.stage_x, self.l.stage_width);
            hint.push(if self.up {
                self.image_at(tex, x, line_y, w, height, true)
            } else {
                self.image_at(tex, x, line_y - height / 2.0, w, height, false)
            });
        }

        // black on black is how circle skins hide the default-on line, so no invented fallback
        let mut line = vec![];
        if p.judgement_line {
            let color = p.judgement_line_color.unwrap_or(FAINT);
            line.push(Op::Fill { rect: [self.l.stage_x, line_y - 1.0, self.l.stage_width, 2.0], color, alpha: 1.0 });
        }

        let mut over = vec![];
        if let Some((asset, tex)) = stage.bottom.as_ref().and_then(|a| Some((a, a.image.as_ref()?))) {
            // never stretched to the stage: native size in the 480 space on both axes, so a canvas
            // taller than 480 hangs off the top and is cut
            let w = (asset.native_width(tex) * self.l.scale).max(1.0);
            let h = (asset.native_height(tex) * self.l.scale).max(1.0);
            let x = self.l.stage_x + (self.l.stage_width - w) / 2.0;
            over.push(if self.up { self.image_at(tex, x, 0.0, w, h, true) } else { self.image_at(tex, x, HEIGHT - h, w, h, false) });
        }
        for (asset, left) in [(stage.left.as_ref(), true), (stage.right.as_ref(), false)] {
            let Some((asset, tex)) = asset.and_then(|a| Some((a, a.image.as_ref()?))) else { continue };
            let w = (asset.native_width(tex) * UNIT * self.l.scale).max(1.0);
            let x = if left { self.l.stage_x - w } else { self.l.stage_x + self.l.stage_width };
            over.push(self.image(tex, [x, 0.0, w, HEIGHT], Draw::default()));
        }

        let top = self.judgement_and_combo();
        let encoded = self.sprites.iter().map(|_| Default::default()).collect();
        Scene {
            sprites: self.sprites,
            encoded,
            upscroll: self.up,
            lazer_bodies: p.lazer_bodies,
            keys_under_notes: p.keys_under_notes,
            hit_y: judgment_y,
            hit_units: 480.0 - hit * UNIT,
            cut_now: vec![],
            own: vec![],
            body_color: s.ln_body_color,
            base,
            hint,
            line,
            over,
            top,
            columns,
            pattern,
        }
    }

    // key art is authored against the stage's bottom edge, not the card's, so it lines up with the
    // notes even where the card clamps the hit line
    fn key(&mut self, asset: Option<&Asset>, x: f64, w: f64, hit_gap: f64, line_y: f64, fallback: Rgba) -> Vec<Op> {
        if let Some((asset, tex)) = asset.and_then(|a| Some((a, a.image.as_ref()?))) {
            let h = self.stage_height(asset, tex);
            return vec![if self.up {
                self.image_at(tex, x, line_y - hit_gap, w, h, true)
            } else {
                self.image_at(tex, x, line_y + hit_gap - h, w, h, false)
            }];
        }
        let h = (HEIGHT * 0.012).max(6.0);
        let top = if self.up { line_y - h - 2.0 } else { line_y + 2.0 };
        vec![Op::RoundRect { rect: [x + 2.0, top, w - 4.0, h], radius: 2.0, color: fallback }]
    }

    fn backdrop(&self, accent: Rgba) -> Vec<Op> {
        const FLOOR: f64 = 0.085;
        let base = hex("#16121d");
        let shade = |amount: f64| -> Rgba {
            let ch = |i: usize| ini::round(base[i] as f64 + (accent[i] as f64 - base[i] as f64) * amount) as u8;
            [ch(0), ch(1), ch(2), 255]
        };
        let mut ops = vec![Op::Fill { rect: [0.0, 0.0, WIDTH, HEIGHT], color: shade(FLOOR), alpha: 1.0 }];
        for t in pattern::backdrop() {
            let h = t.side * 0.866;
            let points = vec![[t.x, t.y - h / 2.0], [t.x - t.side / 2.0, t.y + h / 2.0], [t.x + t.side / 2.0, t.y + h / 2.0]];
            ops.push(Op::Poly { points, color: shade(FLOOR + t.tone * 0.16) });
        }
        ops
    }

    // keys+1 boundaries, outer edges included; a skin that never sets ColumnLineWidth gets 2-unit
    // lines everywhere and explicit zeros hide them
    fn column_lines(&self) -> Vec<Op> {
        let (p, l, keys) = (self.p, &self.l, self.keys);
        let color = p.line_color.unwrap_or(WHITE);
        let mut ops = vec![];
        for boundary in 0..=keys {
            let units = if p.line_widths.is_empty() { 2.0 } else { p.line_widths.get(boundary).copied().unwrap_or(0.0) };
            if units <= 0.0 {
                continue;
            }
            let w = (units * l.scale).max(1.0);
            let x = if boundary == 0 {
                l.stage_x
            } else if boundary == keys {
                l.stage_x + l.stage_width - w
            } else {
                (l.lane_xs[boundary - 1] + l.lane_widths[boundary - 1] + l.lane_xs[boundary]) / 2.0 - w / 2.0
            };
            ops.push(Op::Fill { rect: [x, 0.0, w, HEIGHT], color, alpha: 1.0 });
        }
        ops
    }

    fn judgement_and_combo(&mut self) -> Vec<Op> {
        let (p, s) = (self.p, self.s);
        let cx = self.l.stage_x + self.l.stage_width / 2.0;
        let (scale, up) = (self.l.scale, self.up);
        let position = |own: Option<f64>, fallback: f64| {
            let from_bottom = s_position(own, fallback) * UNIT * scale;
            if up {
                from_bottom
            } else {
                HEIGHT - from_bottom
            }
        };
        let mut ops = vec![];
        if let Some((asset, tex)) = p.judgement.as_ref().and_then(|a| Some((a, a.image.as_ref()?))) {
            let (nw, nh) = (tex.width() / asset.scale, tex.height() / asset.scale);
            let w = (self.l.stage_width * 0.9).min(nw * UNIT * scale);
            let h = nh * (w / nw);
            let cy = (HEIGHT * 0.05).max((HEIGHT * 0.8).min(position(p.score_position, s.score_position)));
            ops.push(self.image(tex, [cx - w / 2.0, cy - h / 2.0, w, h], Draw::default()));
        }

        // ComboPosition pushed off the stage is how a skin turns the counter off
        let Some(combo) = p.combo.as_ref().filter(|_| !p.combo_hidden) else { return ops };
        let unit = UNIT * scale * p.combo_scale;
        let size = |a: &Asset, t: &Texture| ((a.native_width(t) * unit).max(1.0), (a.native_height(t) * unit).max(1.0));
        let digit = |d: usize| combo.digits[d].as_ref().and_then(|a| Some((a, a.image.as_ref()?)));
        let glyphs: Option<Vec<(&Asset, &Arc<Texture>)>> = [7, 2, 7].iter().map(|&d| digit(d)).collect();
        let Some(glyphs) = glyphs else { return ops };
        let sizes: Vec<(f64, f64)> = glyphs.iter().map(|(a, t)| size(a, t)).collect();
        // tabular: every digit takes the widest digit's cell, ComboOverlap pulling them together
        let cell = (0..10)
            .filter_map(digit)
            .map(|(a, t)| size(a, t).0)
            .chain(sizes.iter().map(|s| s.0))
            .fold(f64::NEG_INFINITY, f64::max);
        let overlap = combo.overlap * unit;
        let total = cell * 3.0 - overlap * 2.0;
        let tallest = sizes.iter().map(|s| s.1).fold(f64::NEG_INFINITY, f64::max);
        let cy = (HEIGHT * 0.05 + tallest / 2.0).max((HEIGHT * 0.8 + tallest / 2.0).min(position(p.combo_position, s.combo_position)));
        let mut x = cx - total / 2.0;
        for ((_, tex), (w, h)) in glyphs.iter().zip(&sizes) {
            ops.push(self.image(tex, [x + (cell - w) / 2.0, cy - h / 2.0, *w, *h], Draw::default()));
            x += cell - overlap;
        }
        ops
    }
}

// the keymode's own [Mania] value when it has one, else the skin-wide one
fn s_position(own: Option<f64>, fallback: f64) -> f64 {
    own.unwrap_or(fallback).clamp(0.0, 768.0)
}

// Average of the opaque pixels of the first tap or hold head shrunk to 12x12; None when that is
// effectively black
// the note art carries a skin's identity far more reliably than its skin.ini colours
fn skin_accent(p: &Profile) -> Rgba {
    let declared = p.tap_colors.iter().flatten().next().copied().unwrap_or(p.tap_color);
    accent(p).or(Some(declared).filter(|c| c[..3].iter().any(|&v| v >= 40))).unwrap_or(PINK)
}

fn accent(p: &Profile) -> Option<Rgba> {
    for column in &p.columns {
        for tex in [image(column.tap.as_ref()), image(column.head.as_ref())].into_iter().flatten() {
            let mut sample = canvas::Canvas::new(12, 12);
            sample.image(tex, 0.0, 0.0, 12.0, 12.0, Draw::default());
            let (mut r, mut g, mut b, mut weight) = (0.0, 0.0, 0.0, 0.0);
            for px in sample.pixmap.pixels() {
                let c = px.demultiply();
                let a = c.alpha() as f64 / 255.0;
                if a < 0.4 {
                    continue;
                }
                r += c.red() as f64 * a;
                g += c.green() as f64 * a;
                b += c.blue() as f64 * a;
                weight += a;
            }
            if weight < 4.0 {
                continue;
            }
            let (r, g, b) = (r / weight, g / weight, b / weight);
            if r.max(g).max(b) < 40.0 {
                continue;
            }
            let ch = |v: f64| ini::round(v).clamp(0.0, 255.0) as u8;
            return Some([ch(r), ch(g), ch(b), 255]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    // renders every skin in SKIN_RENDER_DIR to SKIN_RENDER_OUT as <skin>_<keys>k.png
    #[test]
    #[ignore]
    fn render_skins() {
        let dir = std::env::var("SKIN_RENDER_DIR").expect("SKIN_RENDER_DIR");
        let out = std::env::var("SKIN_RENDER_OUT").expect("SKIN_RENDER_OUT");
        let keys: Vec<usize> = std::env::var("SKIN_RENDER_KEYS").unwrap_or("4,7".into()).split(',').filter_map(|k| k.parse().ok()).collect();
        for entry in std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.path().is_dir()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            for &k in &keys {
                match super::render(&entry.path(), k) {
                    Ok(png) => std::fs::write(format!("{out}/{name}_{k}k.png"), png).unwrap(),
                    Err(e) => println!("{name} {k}K: {e}"),
                }
            }
        }
    }
}
