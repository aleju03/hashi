use serde_json::Value;

use super::gpu::{Frame, Gpu, TexId};
use super::text::Text;

// stable runs a note's animation at 60 fps whatever the skin says
const FRAME_MS: f64 = 1000.0 / 60.0;
// mania-hub's replay player's judgement pop and combo bump
const POP_MS: f64 = 280.0;
const POP_HOLD_MS: f64 = 80.0;
const POP_FADE_MS: f64 = 80.0;
const COMBO_POP_MS: f64 = 300.0;
const COMBO_POP_SCALE: f64 = 1.4;
// a skin without judgement art gets mania-hub's words and colours
const JUDGEMENT_TEXT: [(&str, [f64; 3]); 7] = [
    ("", [1.0, 1.0, 1.0]),
    ("MAX", [0.702, 0.961, 1.0]),
    ("300", [1.0, 0.8, 0.133]),
    ("200", [0.533, 0.855, 0.125]),
    ("100", [0.353, 0.561, 1.0]),
    ("50", [0.8, 0.533, 0.0]),
    ("MISS", [1.0, 0.267, 0.267]),
];

pub enum Op {
    Fill { r: [f64; 4], c: [f64; 4], a: f64 },
    Poly { p: Vec<[f64; 2]>, c: [f64; 4] },
    RRect { r: [f64; 4], rad: f64, c: [f64; 4] },
    Img { s: usize, src: [f64; 4], dst: [f64; 4], flip: Option<f64>, clip: Option<[f64; 4]>, add: bool },
}

#[derive(Clone, Copy)]
pub struct Art {
    s: Option<usize>,
    h: f64,
    top: Option<f64>,
}

struct Body {
    s: usize,
    sw: f64,
    sh: f64,
    // the image's rows before a texture's limit cut it to `sh`
    full: f64,
    style: f64,
    row: f64,
}

struct Column {
    x: f64,
    w: f64,
    tap: Vec<Art>,
    head: Vec<Art>,
    tail: Vec<Art>,
    body: Option<Body>,
    tap_color: [f64; 4],
    head_color: [f64; 4],
    light: Vec<Op>,
    key_up: Vec<Op>,
    key_down: Vec<Op>,
    lighting: Vec<Op>,
}

#[derive(Clone, Copy)]
struct Glyph {
    s: usize,
    w: f64,
    h: f64,
}

struct ComboFont {
    digits: Vec<Option<Glyph>>,
    cell: f64,
    overlap: f64,
    y: f64,
}

// the accuracy in the skin's score font, laid out as stable does: right-aligned at `right`, each digit
// a `cell` wide, the % `slot` wide whatever its art
struct ScoreFont {
    digits: Vec<Option<Glyph>>,
    dot: Option<Glyph>,
    percent: Option<Glyph>,
    cell: f64,
    slot: f64,
    overlap: f64,
    right: f64,
    top: f64,
}

struct Hud {
    x: f64,
    judgement_y: f64,
    judgements: Vec<Option<Glyph>>,
    combo: Option<ComboFont>,
    combo_text: Option<f64>,
    score: Option<ScoreFont>,
}

pub struct FrozenTap {
    pub c: usize,
    pub y: f64,
    pub t: f64,
}

pub struct FrozenHold {
    pub c: usize,
    pub head: f64,
    pub tail: f64,
    pub t: f64,
}

pub struct Stage {
    pub width: f64,
    pub height: f64,
    pub upscroll: bool,
    lazer_bodies: bool,
    keys_under_notes: bool,
    pub hit_y: f64,
    sizes: Vec<(f64, f64)>,
    base: Vec<Op>,
    hint: Vec<Op>,
    line: Vec<Op>,
    over: Vec<Op>,
    top: Vec<Op>,
    columns: Vec<Column>,
    hud: Option<Hud>,
    body_color: [f64; 4],
    pub frozen: (Vec<FrozenTap>, Vec<FrozenHold>, usize),
    tex: Vec<TexId>,
    // the helper's id for each sprite, the same while the sprite is unchanged
    ids: Vec<Option<u64>>,
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn quad(v: &Value) -> [f64; 4] {
    let a = v.as_array();
    let g = |i: usize| a.and_then(|a| a.get(i)).map_or(0.0, f);
    [g(0), g(1), g(2), g(3)]
}

fn ops(v: &Value) -> Vec<Op> {
    let Some(list) = v.as_array() else { return vec![] };
    list.iter()
        .filter_map(|o| {
            Some(match o.get("t")?.as_str()? {
                "fill" => Op::Fill { r: quad(&o["r"]), c: quad(&o["c"]), a: o.get("a").map_or(1.0, f) },
                "poly" => Op::Poly { p: o["p"].as_array()?.iter().map(|p| [f(&p[0]), f(&p[1])]).collect(), c: quad(&o["c"]) },
                "rrect" => Op::RRect { r: quad(&o["r"]), rad: f(&o["rad"]), c: quad(&o["c"]) },
                "img" => Op::Img {
                    s: o["s"].as_u64()? as usize,
                    src: quad(&o["src"]),
                    dst: quad(&o["dst"]),
                    flip: o.get("flip").and_then(Value::as_f64),
                    clip: o.get("clip").filter(|c| c.is_array()).map(quad),
                    add: o.get("add").and_then(Value::as_bool).unwrap_or(false),
                },
                // a lane's own background is cleared before it's filled, over the black stage the same thing
                _ => return None,
            })
        })
        .collect()
}

fn arts(v: &Value) -> Vec<Art> {
    v.as_array()
        .map(|a| a.iter().map(|x| Art { s: x.get("s").and_then(Value::as_u64).map(|s| s as usize), h: f(&x["h"]), top: x.get("top").and_then(Value::as_f64) }).collect())
        .unwrap_or_default()
}

fn glyph(v: &Value) -> Option<Glyph> {
    Some(Glyph { s: v.get("s")?.as_u64()? as usize, w: f(&v["w"]), h: f(&v["h"]) })
}

impl Stage {
    // `old`'s textures that the new scene still uses are kept, so an edit uploads only what it changed
    pub fn new(json: &Value, sprites: &[mania_bridge::player::Sprite], gpu: &mut Gpu, old: Option<Stage>) -> Stage {
        let ids: Vec<Option<u64>> = (0..sprites.len()).map(|i| json["sprites"][i][2].as_u64()).collect();
        let mut had: std::collections::HashMap<u64, Vec<TexId>> = std::collections::HashMap::new();
        let mut gone = vec![];
        if let Some(old) = old {
            for (id, t) in old.ids.iter().zip(old.tex) {
                match id {
                    Some(id) => had.entry(*id).or_default().push(t),
                    None => gone.push(t),
                }
            }
        }
        let tex = sprites
            .iter()
            .zip(&ids)
            .map(|(s, id)| id.and_then(|id| had.get_mut(&id)?.pop()).unwrap_or_else(|| gpu.texture(s.width, s.height, &s.rgba)))
            .collect();
        for t in gone.into_iter().chain(had.into_values().flatten()) {
            gpu.drop_texture(t);
        }
        let columns = json["columns"]
            .as_array()
            .map(|cols| {
                cols.iter()
                    .map(|c| Column {
                        x: f(&c["x"]),
                        w: f(&c["w"]),
                        tap: arts(&c["tap"]),
                        head: arts(&c["head"]),
                        tail: arts(&c["tail"]),
                        body: c.get("body").filter(|b| b.is_object()).map(|b| Body { s: b["s"].as_u64().unwrap_or(0) as usize, sw: f(&b["sw"]), sh: f(&b["sh"]), full: b["full"].as_f64().unwrap_or_else(|| f(&b["sh"])), style: f(&b["style"]), row: f(&b["row"]) }),
                        tap_color: quad(&c["tapColor"]),
                        head_color: quad(&c["headColor"]),
                        light: ops(&c["light"]),
                        key_up: ops(&c["keyUp"]),
                        key_down: ops(&c["keyDown"]),
                        lighting: ops(&c["lighting"]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let h = &json["hud"];
        let hud = h.is_object().then(|| Hud {
            x: f(&h["x"]),
            judgement_y: f(&h["judgementY"]),
            judgements: h["judgements"].as_array().map(|a| a.iter().map(glyph).collect()).unwrap_or_default(),
            combo: h.get("combo").filter(|c| c.is_object()).map(|c| ComboFont {
                digits: c["digits"].as_array().map(|a| a.iter().map(glyph).collect()).unwrap_or_default(),
                cell: f(&c["cell"]),
                overlap: f(&c["overlap"]),
                y: f(&c["y"]),
            }),
            combo_text: h.get("comboText").and_then(Value::as_f64),
            score: h.get("score").filter(|c| c.is_object()).map(|c| ScoreFont {
                digits: c["digits"].as_array().map(|a| a.iter().map(glyph).collect()).unwrap_or_default(),
                dot: glyph(&c["dot"]),
                percent: glyph(&c["percent"]),
                cell: f(&c["cell"]),
                slot: f(&c["slot"]),
                overlap: f(&c["overlap"]),
                right: f(&c["right"]),
                top: f(&c["top"]),
            }),
        });
        let fr = &json["frozen"];
        let frozen = (
            fr["taps"].as_array().map(|a| a.iter().map(|t| FrozenTap { c: t["c"].as_u64().unwrap_or(0) as usize, y: f(&t["y"]), t: f(&t["t"]) }).collect()).unwrap_or_default(),
            fr["holds"]
                .as_array()
                .map(|a| a.iter().map(|t| FrozenHold { c: t["c"].as_u64().unwrap_or(0) as usize, head: f(&t["head"]), tail: f(&t["tail"]), t: f(&t["t"]) }).collect())
                .unwrap_or_default(),
            fr["pressed"].as_u64().unwrap_or(0) as usize,
        );
        Stage {
            width: f(&json["width"]),
            height: json.get("height").map_or(720.0, f),
            upscroll: json["upscroll"].as_bool().unwrap_or(false),
            lazer_bodies: json["lazerBodies"].as_bool().unwrap_or(false),
            keys_under_notes: json["keysUnderNotes"].as_bool().unwrap_or(false),
            hit_y: f(&json["hitY"]),
            sizes: json["sprites"].as_array().map(|a| a.iter().map(|s| (f(&s[0]), f(&s[1]))).collect()).unwrap_or_default(),
            base: ops(&json["base"]),
            hint: ops(&json["hint"]),
            line: ops(&json["line"]),
            over: ops(&json["over"]),
            top: ops(&json["top"]),
            columns,
            hud,
            body_color: quad(&json["bodyColor"]),
            frozen,
            tex,
            ids,
        }
    }

    pub fn keys(&self) -> usize {
        self.columns.len()
    }

    fn map_y(&self, y: f64) -> f64 {
        if self.upscroll {
            self.height - y
        } else {
            y
        }
    }
}

pub struct Tap {
    pub c: usize,
    pub y: f64,
    pub t: f64,
}

pub struct Hold {
    pub c: usize,
    pub head: f64,
    pub tail: f64,
    pub t: f64,
}

pub struct Pop {
    pub j: u8,
    pub t: f64,
}

pub struct Overlay {
    pub now: f64,
    pub pop: Option<Pop>,
    pub combo: u32,
    pub combo_at: f64,
    pub acc: Option<f64>,
}

// scene units to screen pixels: k pixels a unit, from (ox, oy)
pub struct Pen<'a> {
    pub frame: &'a mut Frame,
    pub stage: &'a Stage,
    pub k: f64,
    pub ox: f64,
    pub oy: f64,
}

fn premul(c: [f64; 4], alpha: f64) -> [f32; 4] {
    let a = c[3] / 255.0 * alpha;
    [(c[0] / 255.0 * a) as f32, (c[1] / 255.0 * a) as f32, (c[2] / 255.0 * a) as f32, a as f32]
}

impl Pen<'_> {
    fn px(&self, r: [f64; 4]) -> [f32; 4] {
        let (k, ox, oy) = (self.k, self.ox, self.oy);
        [(ox + r[0] * k) as f32, (oy + r[1] * k) as f32, (ox + r[2] * k) as f32, (oy + r[3] * k) as f32]
    }

    fn ops(&mut self, ops: &[Op]) {
        for o in ops {
            self.op(o);
        }
    }

    fn op(&mut self, o: &Op) {
        match o {
            Op::Fill { r, c, a } => {
                let rect = self.px([r[0], r[1], r[0] + r[2], r[1] + r[3]]);
                self.frame.fill(rect, premul(*c, *a));
            }
            Op::Poly { p, c } => {
                let color = premul(*c, 1.0);
                let pt = |q: [f64; 2]| [(self.ox + q[0] * self.k) as f32, (self.oy + q[1] * self.k) as f32];
                for i in 1..p.len().saturating_sub(1) {
                    self.frame.triangle([pt(p[0]), pt(p[i]), pt(p[i + 1])], color);
                }
            }
            Op::RRect { r, rad, c } => {
                let radius = rad.min(r[2] / 2.0).min(r[3] / 2.0);
                let rect = self.px([r[0], r[1], r[0] + r[2], r[1] + r[3]]);
                self.frame.round_rect(rect, (radius * self.k) as f32, premul(*c, 1.0));
            }
            Op::Img { s, src, dst, flip, clip, add } => self.image(*s, *src, *dst, *flip, *clip, *add, 1.0),
        }
    }

    // a canvas drawImage: flipped about `flip` (translate, scale -1) and cut to `clip` first
    #[allow(clippy::too_many_arguments)]
    fn image(&mut self, s: usize, src: [f64; 4], dst: [f64; 4], flip: Option<f64>, clip: Option<[f64; 4]>, add: bool, alpha: f64) {
        let Some(&tex) = self.stage.tex.get(s) else { return };
        let (tw, th) = self.stage.sizes.get(s).copied().unwrap_or((1.0, 1.0));
        let (mut u0, mut u1) = (src[0] / tw, (src[0] + src[2]) / tw);
        let (mut v0, mut v1) = (src[1] / th, (src[1] + src[3]) / th);
        let (mut x0, mut x1) = (dst[0], dst[0] + dst[2]);
        let (mut y0, mut y1) = (dst[1], dst[1] + dst[3]);
        if let Some(axis) = flip {
            (y0, y1) = (axis - y1, axis - y0);
            (v0, v1) = (v1, v0);
        }
        if let Some(c) = clip {
            let (cx0, cy0, cx1, cy1) = (c[0], c[1], c[0] + c[2], c[1] + c[3]);
            let cut = |a0: f64, a1: f64, b0: f64, b1: f64, t0: f64, t1: f64| -> Option<(f64, f64, f64, f64)> {
                let (n0, n1) = (a0.max(b0), a1.min(b1));
                if n1 <= n0 || a1 <= a0 {
                    return None;
                }
                let at = |x: f64| t0 + (x - a0) / (a1 - a0) * (t1 - t0);
                Some((n0, n1, at(n0), at(n1)))
            };
            let Some(xs) = cut(x0, x1, cx0, cx1, u0, u1) else { return };
            let Some(ys) = cut(y0, y1, cy0, cy1, v0, v1) else { return };
            (x0, x1, u0, u1) = xs;
            (y0, y1, v0, v1) = ys;
        }
        let rect = self.px([x0, y0, x1, y1]);
        let a = alpha as f32;
        self.frame.quad(tex, add, rect, [u0 as f32, v0 as f32, u1 as f32, v1 as f32], [a, a, a, a]);
    }

    fn sprite(&mut self, s: usize, dst: [f64; 4], flip: Option<f64>, clip: Option<[f64; 4]>) {
        let (w, h) = self.stage.sizes.get(s).copied().unwrap_or((1.0, 1.0));
        self.image(s, [0.0, 0.0, w, h], dst, flip, clip, false, 1.0);
    }

    fn tap(&mut self, column: usize, y: f64, time: f64) {
        let s = self.stage;
        let Some(c) = s.columns.get(column) else { return };
        let anchor = s.map_y(y);
        if let Some(f) = art(&c.tap, time) {
            let top = if s.upscroll { anchor } else { anchor - f.h };
            self.sprite(f.s.unwrap(), [c.x, top, c.w, f.h], s.upscroll.then_some(top * 2.0 + f.h), None);
            return;
        }
        let h = (c.w * 0.3).max(10.0);
        self.op(&Op::RRect { r: [c.x + 1.0, if s.upscroll { anchor } else { anchor - h }, c.w - 2.0, h], rad: 4.0, c: c.tap_color });
    }

    fn hold(&mut self, column: usize, head_y: f64, tail_y: f64, time: f64) {
        let s = self.stage;
        let Some(c) = s.columns.get(column) else { return };
        let up = s.upscroll;
        let (x, w) = (c.x, c.w);
        let head = art(&c.head, time).or_else(|| art(&c.tap, time));
        let tail = art(&c.tail, time);
        let head_end = s.map_y(head_y);
        let tail_end = s.map_y(tail_y);
        let head_h = head.map_or((w * 0.3).max(10.0), |f| f.h);
        // a blank tail has no box, so the body runs right to the end line
        let art_top = tail.and_then(|t| t.top);
        let tail_h = match (tail, art_top) {
            (Some(t), Some(_)) => t.h,
            _ => 0.0,
        };
        let head_top = if up { head_end } else { head_end - head_h };
        let tail_top = if up { tail_end } else { tail_end - tail_h };
        let head_side = if up { head_end + head_h / 2.0 } else { head_end - head_h / 2.0 };
        let tail_side = if up { tail_top + tail_h } else { tail_top };
        let mut body_top = if up { head_side } else { tail_side };
        let mut body_bottom = if up { tail_side } else { head_side };
        if s.lazer_bodies {
            let half = tail.map_or(head_h, |t| t.h) / 2.0;
            if up {
                body_bottom = tail_end + half;
            } else {
                body_top = tail_end - half;
            }
        }
        let (mut visible_top, mut visible_bottom) = (body_top, body_bottom);
        if let (true, Some(at)) = (tail_h > 0.0 && !s.lazer_bodies, art_top) {
            let centre = tail_top + tail_h / 2.0;
            let edge = tail_top + if up { at } else { 1.0 - at } * tail_h;
            if up {
                visible_bottom = visible_bottom.min(centre.max(edge + 1.0));
            } else {
                visible_top = visible_top.max(centre.min(edge - 1.0));
            }
        }
        if visible_bottom > visible_top {
            match &c.body {
                Some(b) => {
                    let from_bottom = if b.style == 2.0 && !s.lazer_bodies { !up } else { up };
                    let flip = from_bottom.then_some(body_top + body_bottom);
                    let clip = Some([x, visible_top, w, visible_bottom - visible_top]);
                    if b.style == 0.0 {
                        self.image(b.s, [0.0, 0.0, b.sw, b.sh], [x, body_top, w, body_bottom - body_top], flip, clip, false, 1.0);
                    } else {
                        for t in tiles(body_top, body_bottom, b.sh, b.full, b.row) {
                            self.image(b.s, [0.0, t.3, b.sw, t.2], [x, t.0, w, t.1], flip, clip, false, 1.0);
                        }
                    }
                }
                None => {
                    let r = [x + w * 0.16, visible_top, w * 0.68, visible_bottom - visible_top];
                    self.op(&Op::Fill { r, c: s.body_color, a: 0.9 });
                }
            }
        }
        if let (Some(t), true) = (tail, tail_h > 0.0) {
            // lazer masks the hold at the head's centre
            let clip = s.lazer_bodies.then(|| if up { [x, head_side, w, s.height - head_side] } else { [x, 0.0, w, head_side] });
            self.sprite(t.s.unwrap(), [x, tail_top, w, tail_h], (!up).then_some(tail_top * 2.0 + tail_h), clip);
        }
        match head {
            Some(h) => self.sprite(h.s.unwrap(), [x, head_top, w, head_h], up.then_some(head_top * 2.0 + head_h), None),
            None => self.op(&Op::RRect { r: [x + 1.0, head_top, w - 2.0, head_h], rad: 4.0, c: c.head_color }),
        }
    }

    // skin.js's skinDraw. lit: the columns showing hit lighting
    pub fn stage(&mut self, holds: &[Hold], taps: &[Tap], pressed: &[bool], lit: &[bool], hud: bool) {
        let s = self.stage;
        let on = |v: &[bool], i: usize| v.get(i).copied().unwrap_or(false);
        self.ops(&s.base);
        for (i, c) in s.columns.iter().enumerate() {
            if on(pressed, i) {
                self.ops(&c.light);
            }
        }
        self.ops(&s.hint);
        let keys = |pen: &mut Self| {
            for (i, c) in s.columns.iter().enumerate() {
                pen.ops(if on(pressed, i) { &c.key_down } else { &c.key_up });
            }
        };
        if s.keys_under_notes {
            keys(self);
        }
        for h in holds {
            self.hold(h.c, h.head, h.tail, h.t);
        }
        for t in taps {
            self.tap(t.c, t.y, t.t);
        }
        self.ops(&s.line);
        if !s.keys_under_notes {
            keys(self);
        }
        self.ops(&s.over);
        for (i, c) in s.columns.iter().enumerate() {
            if on(lit, i) {
                self.ops(&c.lighting);
            }
        }
        if !hud {
            self.ops(&s.top);
        }
    }

    // at (x, y) in scene units, (ax, ay) the anchor in it from 0 (left, top) to 1 (right, bottom)
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, gpu: &mut Gpu, text: &mut Text, s: &str, size: f64, (x, y): (f64, f64), (ax, ay): (f64, f64), color: [f64; 3], alpha: f64, scale_y: f64, shadow: bool) {
        let Some(d) = text.get(gpu, s, (size * self.k) as f32) else { return };
        let (w, h) = (d.width as f64, d.height as f64 * scale_y);
        let (x0, y0) = ((self.ox + x * self.k - w * ax) as f32, (self.oy + y * self.k - h * ay) as f32);
        let (w, h) = (w as f32, h as f32);
        let a = alpha as f32;
        if shadow {
            let o = (2.0 * self.k).max(1.0) as f32;
            self.frame.quad(d.tex, false, [x0 + o, y0 + o, x0 + w + o, y0 + h + o], [0.0, 0.0, 1.0, 1.0], [0.0, 0.0, 0.0, a * 0.45]);
        }
        let c = [(color[0] * alpha) as f32, (color[1] * alpha) as f32, (color[2] * alpha) as f32, a];
        self.frame.quad(d.tex, false, [x0, y0, x0 + w, y0 + h], [0.0, 0.0, 1.0, 1.0], c);
    }

    // the last judgement popping at ScorePosition, the combo at ComboPosition, accuracy in the corner
    pub fn overlay(&mut self, gpu: &mut Gpu, text: &mut Text, o: &Overlay) {
        let s = self.stage;
        if let Some(hud) = &s.hud {
            if let Some(pop) = &o.pop {
                let since = (o.now - pop.t).max(0.0);
                if since < POP_MS + POP_HOLD_MS + POP_FADE_MS {
                    let alpha = 1.0 - ((since - POP_MS - POP_HOLD_MS).max(0.0) / POP_FADE_MS);
                    let scale = 0.8 + 0.2 * elastic((since / POP_MS).min(1.0));
                    match hud.judgements.get(pop.j as usize).copied().flatten() {
                        Some(g) => {
                            let (w, h) = (g.w * scale, g.h * scale);
                            let (sw, sh) = s.sizes.get(g.s).copied().unwrap_or((1.0, 1.0));
                            self.image(g.s, [0.0, 0.0, sw, sh], [hud.x - w / 2.0, hud.judgement_y - h / 2.0, w, h], None, None, false, alpha);
                        }
                        None => {
                            let (word, color) = JUDGEMENT_TEXT[(pop.j as usize).min(6)];
                            self.text(gpu, text, word, (s.height * 0.035).max(16.0) * scale, (hud.x, hud.judgement_y), (0.5, 0.5), color, alpha, 1.0, false);
                        }
                    }
                }
            }
            if o.combo > 0 {
                let bump = (o.now - o.combo_at).max(0.0);
                let sy = if bump < COMBO_POP_MS { COMBO_POP_SCALE - (COMBO_POP_SCALE - 1.0) * (1.0 - (1.0 - bump / COMBO_POP_MS).powi(2)) } else { 1.0 };
                let digits: Vec<usize> = o.combo.to_string().bytes().map(|b| (b - b'0') as usize).collect();
                match &hud.combo {
                    Some(font) => {
                        let glyphs: Option<Vec<Glyph>> = digits.iter().map(|&d| font.digits.get(d).copied().flatten()).collect();
                        if let Some(glyphs) = glyphs {
                            let n = glyphs.len() as f64;
                            let mut x = hud.x - (font.cell * n - font.overlap * (n - 1.0)) / 2.0;
                            for g in glyphs {
                                let (sw, sh) = s.sizes.get(g.s).copied().unwrap_or((1.0, 1.0));
                                self.image(g.s, [0.0, 0.0, sw, sh], [x + (font.cell - g.w) / 2.0, font.y - g.h * sy / 2.0, g.w, g.h * sy], None, None, false, 1.0);
                                x += font.cell - font.overlap;
                            }
                        }
                    }
                    None => {
                        if let Some(y) = hud.combo_text {
                            self.text(gpu, text, &o.combo.to_string(), s.height * 0.05, (hud.x, y), (0.5, 0.5), [1.0; 3], 1.0, sy, false);
                        }
                    }
                }
            }
        }
        if let Some(acc) = o.acc {
            // rounded down like osu! does, 99.996% with a Good in it isn't 100%
            let shown = ((acc * 100.0 + 1e-6).floor() / 100.0).clamp(0.0, 100.0);
            let words = format!("{shown:.2}%");
            match s.hud.as_ref().and_then(|h| h.score.as_ref()) {
                Some(font) => self.accuracy(font, &words),
                None => self.text(gpu, text, &words, 30.0, (s.width - 20.0, 16.0), (1.0, 0.0), [1.0; 3], 1.0, 1.0, true),
            }
        }
    }

    // the % shows only as much of its art as its slot holds
    fn accuracy(&mut self, font: &ScoreFont, words: &str) {
        let s = self.stage;
        let glyph = |c: char| match c {
            '0'..='9' => font.digits.get(c as usize - '0' as usize).copied().flatten(),
            '.' => font.dot,
            _ => font.percent,
        };
        let advance = |c: char| match c {
            '0'..='9' => font.cell,
            '%' => font.slot,
            _ => glyph(c).map_or(font.cell, |g| g.w),
        };
        let n = words.chars().count() as f64;
        let total: f64 = words.chars().map(advance).sum::<f64>() - font.overlap * (n - 1.0);
        let mut x = font.right - total;
        for c in words.chars() {
            if let Some(g) = glyph(c) {
                let (sw, sh) = s.sizes.get(g.s).copied().unwrap_or((1.0, 1.0));
                let (dx, frac) = match c {
                    '0'..='9' => ((font.cell - g.w) / 2.0, 1.0),
                    '%' => (0.0, (font.slot / g.w.max(1e-6)).min(1.0)),
                    _ => (0.0, 1.0),
                };
                self.image(g.s, [0.0, 0.0, sw * frac, sh], [x + dx, font.top, g.w * frac, g.h], None, None, false, 1.0);
            }
            x += advance(c) - font.overlap;
        }
    }

    pub fn fps(&mut self, gpu: &mut Gpu, text: &mut Text, fps: u32) {
        self.text(gpu, text, &format!("{fps} fps"), 12.0, (6.0, 4.0), (0.0, 0.0), [1.0; 3], 0.6, 1.0, false);
    }

    pub fn note(&mut self, gpu: &mut Gpu, text: &mut Text, s: &str) {
        let y = self.stage.height - 14.0;
        self.text(gpu, text, s, 15.0, (16.0, y), (0.0, 1.0), [1.0; 3], 0.85, 1.0, true);
    }

    pub fn seek_bar(&mut self, gpu: &mut Gpu, text: &mut Text, bar: &SeekBar, accent: [f64; 3]) {
        let a = bar.alpha;
        if a <= 0.0 {
            return;
        }
        let r = bar.frame;
        let px = |x: f64, y: f64| ((self.ox + x * self.k) as f32, (self.oy + y * self.k) as f32);
        let (x0, y0) = px(r[0], r[1]);
        let (x1, y1) = px(r[0] + r[2], r[1] + r[3]);
        let k = self.k as f32;
        let fa = a as f32;
        self.frame.round_rect([x0, y0, x1, y1], 12.0 * k, [0.0, 0.0, 0.0, 0.6 * fa]);
        let (t0, ty) = px(bar.track.0, bar.track.2);
        let (t1, _) = px(bar.track.1, bar.track.2);
        let at = t0 + (t1 - t0) * bar.done.clamp(0.0, 1.0) as f32;
        let half = 2.0 * k;
        self.frame.round_rect([t0, ty - half, t1, ty + half], half, [0.22 * fa, 0.22 * fa, 0.22 * fa, 0.22 * fa]);
        let lit = [(accent[0] * a) as f32, (accent[1] * a) as f32, (accent[2] * a) as f32, fa];
        if at > t0 {
            self.frame.round_rect([t0, ty - half, at, ty + half], half, lit);
        }
        let ring = 9.0 * k;
        let glow = [(accent[0] * a * 0.4) as f32, (accent[1] * a * 0.4) as f32, (accent[2] * a * 0.4) as f32, fa * 0.4];
        self.frame.round_rect([at - ring, ty - ring, at + ring, ty + ring], ring, glow);
        let knob = if bar.held { 7.5 } else { 6.0 } * k;
        self.frame.round_rect([at - knob, ty - knob, at + knob, ty + knob], knob, [fa, fa, fa, fa]);
        self.text(gpu, text, &bar.time, 13.0, (r[0] + r[2] - 16.0, bar.track.2), (1.0, 0.5), [1.0; 3], a, 1.0, false);
    }
}

pub struct SeekBar {
    // x, y, w, h in scene units
    pub frame: [f64; 4],
    // its track: left, right, middle
    pub track: (f64, f64, f64),
    pub done: f64,
    pub time: String,
    pub alpha: f64,
    pub held: bool,
}

impl SeekBar {
    pub fn at(width: f64, height: f64) -> SeekBar {
        let w = (width - 48.0).min(720.0);
        let (h, x) = (36.0, (width - w) / 2.0);
        let y = height - 24.0 - h;
        SeekBar { frame: [x, y, w, h], track: (x + 16.0, x + w - 120.0, y + h / 2.0), done: 0.0, time: String::new(), alpha: 0.0, held: false }
    }

    pub fn holds(&self, (x, y): (f64, f64)) -> bool {
        let r = self.frame;
        x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
    }

    pub fn along(&self, x: f64) -> f64 {
        ((x - self.track.0) / (self.track.1 - self.track.0).max(1.0)).clamp(0.0, 1.0)
    }
}

// stable starts a note's animation when it spawns
fn art(frames: &[Art], time: f64) -> Option<Art> {
    if frames.len() < 2 {
        return frames.first().copied().filter(|f| f.s.is_some());
    }
    let n = frames.len() as i64;
    let i = ((-time / FRAME_MS).floor() as i64).rem_euclid(n) as usize;
    Some(frames[i]).filter(|f| f.s.is_some())
}

// whole pixels: (top, height, texture rows, first texture row), two half-covered rows over each
// other show as a seam at every join. A body cut to a texture's limit, `rows` of its `full`, has its
// last row drawn on over the rest, as the game repeats the whole image
fn tiles(top: f64, bottom: f64, rows: f64, full: f64, scale: f64) -> Vec<(f64, f64, f64, f64)> {
    let mut out = vec![];
    if !(scale > 0.0 && rows > 0.0 && bottom > top) {
        return out;
    }
    let rest = (full - rows).max(0.0);
    let mut y = top;
    while y < bottom - 0.01 && out.len() < 4096 {
        let n = rows.min((bottom - y) / scale);
        let t = y.round();
        out.push((t, ((y + n * scale).round() - t).max(1.0), n, 0.0));
        y += n * scale;
        if rest > 0.0 && y < bottom - 0.01 {
            let m = rest.min((bottom - y) / scale);
            let t = y.round();
            out.push((t, ((y + m * scale).round() - t).max(1.0), 1.0, rows - 1.0));
            y += m * scale;
        }
    }
    out
}

fn elastic(t: f64) -> f64 {
    if t <= 0.0 || t >= 1.0 {
        return t.clamp(0.0, 1.0);
    }
    2f64.powf(-10.0 * t) * ((t * 10.0 - 0.75) * (2.0 * std::f64::consts::PI / 3.0)).sin() + 1.0
}
