use std::sync::{Arc, OnceLock};

use serde_json::{json, Value};
use tiny_skia::{BlendMode, PathBuilder};

use super::canvas::{Canvas, Draw, Texture};
use super::ini::Rgba;
use super::pattern::{body_tiles, hold_boxes, Pattern};
use super::{HEIGHT, WIDTH};

pub(super) enum Op {
    Fill { rect: [f64; 4], color: Rgba, alpha: f64 },
    Clear { rect: [f64; 4] },
    Poly { points: Vec<[f64; 2]>, color: Rgba },
    RoundRect { rect: [f64; 4], radius: f64, color: Rgba },
    Image { sprite: usize, src: [f64; 4], dst: [f64; 4], how: Draw },
}

impl Op {
    fn json(&self) -> Value {
        match self {
            Op::Fill { rect, color, alpha } => json!({ "t": "fill", "r": rect, "c": color, "a": alpha }),
            Op::Clear { rect } => json!({ "t": "clear", "r": rect }),
            Op::Poly { points, color } => json!({ "t": "poly", "p": points, "c": color }),
            Op::RoundRect { rect, radius, color } => json!({ "t": "rrect", "r": rect, "rad": radius, "c": color }),
            Op::Image { sprite, src, dst, how } => json!({
                "t": "img",
                "s": sprite,
                "src": src,
                "dst": dst,
                "flip": how.flip,
                "clip": how.clip,
                "add": how.blend == BlendMode::Plus,
            }),
        }
    }
}

fn ops_json(ops: &[Op]) -> Value {
    Value::Array(ops.iter().map(Op::json).collect())
}

// one animation frame of note art at the size it's drawn; top is where a tail's visible art starts
#[derive(Clone)]
pub(super) struct Frame {
    pub sprite: Option<usize>,
    pub h: f64,
    pub top: Option<f64>,
}

impl Frame {
    fn json(&self) -> Value {
        json!({ "s": self.sprite, "h": self.h, "top": self.top })
    }
}

// stable starts each note's animation when it spawns, at a fixed 60 fps whatever the skin says
fn pick(frames: &[Frame], time_ms: f64) -> Option<&Frame> {
    if frames.len() < 2 {
        return frames.first();
    }
    let index = ((-time_ms) / (1000.0 / 60.0)).floor() as i64;
    frames.get(index.rem_euclid(frames.len() as i64) as usize)
}

fn drawn(frames: &[Frame], time_ms: f64) -> Option<&Frame> {
    pick(frames, time_ms).filter(|f| f.sprite.is_some())
}

pub(super) struct Body {
    pub sprite: usize,
    pub sw: f64,
    pub sh: f64,
    pub style: f64,
    // screen pixels per texel row of a cascaded body
    pub row: f64,
}

pub(super) struct Column {
    pub x: f64,
    pub w: f64,
    pub tap: Vec<Frame>,
    pub head: Vec<Frame>,
    pub tail: Vec<Frame>,
    pub body: Option<Body>,
    pub tap_color: Rgba,
    pub head_color: Rgba,
    pub light: Vec<Op>,
    pub key_up: Vec<Op>,
    pub key_down: Vec<Op>,
    pub lighting: Vec<Op>,
}

pub(super) struct Scene {
    pub sprites: Vec<Arc<Texture>>,
    pub encoded: Vec<OnceLock<Option<Vec<u8>>>>,
    pub upscroll: bool,
    pub lazer_bodies: bool,
    pub keys_under_notes: bool,
    pub hit_y: f64,
    // the judgement line in skin units from the top, what a note travels in its scroll time
    pub hit_units: f64,
    pub cut_now: Vec<Option<f64>>,
    // each column's images as skin.ini names them, before any edit
    pub own: Vec<Value>,
    pub body_color: Rgba,
    pub base: Vec<Op>,
    pub hint: Vec<Op>,
    pub line: Vec<Op>,
    pub over: Vec<Op>,
    pub top: Vec<Op>,
    pub columns: Vec<Column>,
    pub pattern: Pattern,
}

impl Scene {
    fn map_y(&self, y: f64) -> f64 {
        if self.upscroll {
            HEIGHT - y
        } else {
            y
        }
    }

    fn image(&self, sprite: usize, dst: [f64; 4], how: Draw) -> Op {
        let tex = &self.sprites[sprite];
        Op::Image { sprite, src: [0.0, 0.0, tex.width(), tex.height()], dst, how }
    }

    // notes grow away from the judgement line and flip on upscroll
    pub fn tap(&self, column: usize, y: f64, time_ms: f64, out: &mut Vec<Op>) {
        let c = &self.columns[column];
        let anchor = self.map_y(y);
        if let Some(f) = drawn(&c.tap, time_ms) {
            let top = if self.upscroll { anchor } else { anchor - f.h };
            let how = if self.upscroll { Draw::flipped(top * 2.0 + f.h) } else { Draw::default() };
            out.push(self.image(f.sprite.unwrap(), [c.x, top, c.w, f.h], how));
            return;
        }
        let h = (c.w * 0.3).max(10.0);
        let top = if self.upscroll { anchor } else { anchor - h };
        out.push(Op::RoundRect { rect: [c.x + 1.0, top, c.w - 2.0, h], radius: 4.0, color: c.tap_color });
    }

    pub fn hold(&self, column: usize, head_y: f64, tail_y: f64, time_ms: f64, out: &mut Vec<Op>) {
        let (c, up) = (&self.columns[column], self.upscroll);
        let (x, w) = (c.x, c.w);
        let head = drawn(&c.head, time_ms).or_else(|| drawn(&c.tap, time_ms));
        // no tail art means the head's, flipped like a tail, as stable and lazer both do
        let tail = drawn(&c.tail, time_ms);
        let head_end = self.map_y(head_y);
        let tail_end = self.map_y(tail_y);
        let head_h = head.map_or((w * 0.3).max(10.0), |f| f.h);
        // A blank placeholder tail (a 1x1 transparent png) has no box. Sized off its aspect it would
        // grow a lane-wide invisible cap, and the body cascading from its far edge would eat the
        // gap to the next note.
        let art_top = tail.and_then(|f| f.top);
        let tail_h = match (tail, art_top) {
            (Some(f), Some(_)) => f.h,
            _ => 0.0,
        };
        let g = hold_boxes(up, head_end, tail_end, head_h, tail_h);
        let (mut body_top, mut body_bottom) = (g.body_top, g.body_bottom);
        if self.lazer_bodies {
            // lazer runs the body half a tail past the end line, blank cap or not
            let half = tail.map_or(head_h, |f| f.h) / 2.0;
            if up {
                body_bottom = tail_end + half;
            } else {
                body_top = tail_end - half;
            }
        }

        // The body stops at the tail cap's centre, where round art is widest. Caps whose art sits
        // shy of the centre (StepMania roofs end at 48%) get the body a pixel past their edge.
        let (mut visible_top, mut visible_bottom) = (body_top, body_bottom);
        if let Some(art_top) = art_top.filter(|_| tail_h > 0.0 && !self.lazer_bodies) {
            let centre = g.tail_top + tail_h / 2.0;
            let edge = g.tail_top + if up { art_top } else { 1.0 - art_top } * tail_h;
            if up {
                visible_bottom = visible_bottom.min(centre.max(edge + 1.0));
            } else {
                visible_top = visible_top.max(centre.min(edge - 1.0));
            }
        }

        if visible_bottom > visible_top {
            if let Some(b) = &c.body {
                // flipped whenever the end the cascade counts from (the tail, or the head for
                // style 2) sits at the bottom, which also turns the art to face it
                let anchor_bottom = if b.style == 2.0 && !self.lazer_bodies { !up } else { up };
                let how = Draw {
                    flip: anchor_bottom.then_some(body_top + body_bottom),
                    clip: Some([x, visible_top, w, visible_bottom - visible_top]),
                    ..Default::default()
                };
                if b.style == 0.0 {
                    out.push(Op::Image { sprite: b.sprite, src: [0.0, 0.0, b.sw, b.sh], dst: [x, body_top, w, body_bottom - body_top], how });
                } else {
                    for t in body_tiles(body_top, body_bottom, b.sh, b.row) {
                        out.push(Op::Image { sprite: b.sprite, src: [0.0, 0.0, b.sw, t.rows], dst: [x, t.top, w, t.height], how });
                    }
                }
            } else {
                let rect = [x + w * 0.16, visible_top, w * 0.68, visible_bottom - visible_top];
                out.push(Op::Fill { rect, color: self.body_color, alpha: 0.9 });
            }
        }

        if let Some(f) = tail.filter(|_| tail_h > 0.0) {
            // lazer masks the hold at the head's centre
            let clip = self.lazer_bodies.then(|| {
                let centre = if up { head_end + head_h / 2.0 } else { head_end - head_h / 2.0 };
                if up {
                    [x, centre, w, HEIGHT - centre]
                } else {
                    [x, 0.0, w, centre]
                }
            });
            // the tail's texture is flipped relative to the notes: flipped on downscroll
            let flip = (!up).then_some(g.tail_top * 2.0 + tail_h);
            out.push(self.image(f.sprite.unwrap(), [x, g.tail_top, w, tail_h], Draw { flip, clip, ..Default::default() }));
        }

        match head {
            Some(f) => {
                let how = if up { Draw::flipped(g.head_top * 2.0 + head_h) } else { Draw::default() };
                out.push(self.image(f.sprite.unwrap(), [x, g.head_top, w, head_h], how));
            }
            None => out.push(Op::RoundRect { rect: [x + 1.0, g.head_top, w - 2.0, head_h], radius: 4.0, color: c.head_color }),
        }
    }

    // the synthetic pattern frozen, the picture the TS draws
    pub fn png(&self) -> Result<Vec<u8>, String> {
        let pressed = self.pattern.pressed;
        let mut notes = vec![];
        for h in &self.pattern.holds {
            self.hold(h.column, h.head_y, h.tail_y, h.time_ms, &mut notes);
        }
        for t in &self.pattern.taps {
            self.tap(t.column, t.y, t.time_ms, &mut notes);
        }
        let keys: Vec<&Op> = self
            .columns
            .iter()
            .enumerate()
            .flat_map(|(i, c)| if i == pressed { &c.key_down } else { &c.key_up })
            .collect();
        let mut ops: Vec<&Op> = vec![];
        ops.extend(&self.base);
        ops.extend(&self.columns[pressed].light);
        ops.extend(&self.hint);
        if self.keys_under_notes {
            ops.extend(keys.iter().copied());
        }
        ops.extend(&notes);
        ops.extend(&self.line);
        if !self.keys_under_notes {
            ops.extend(keys.iter().copied());
        }
        ops.extend(&self.over);
        ops.extend(&self.columns[pressed].lighting);
        ops.extend(&self.top);

        let mut c = Canvas::new(WIDTH as u32, HEIGHT as u32);
        for op in ops {
            self.paint(&mut c, op);
        }
        c.pixmap.encode_png().map_err(|e| e.to_string())
    }

    fn paint(&self, c: &mut Canvas, op: &Op) {
        match op {
            Op::Fill { rect: [x, y, w, h], color, alpha } => c.fill_rect(*x, *y, *w, *h, *color, *alpha),
            Op::Clear { rect: [x, y, w, h] } => c.clear_rect(*x, *y, *w, *h),
            Op::Poly { points, color } => {
                let mut pb = PathBuilder::new();
                for (i, [x, y]) in points.iter().enumerate() {
                    if i == 0 {
                        pb.move_to(*x as f32, *y as f32);
                    } else {
                        pb.line_to(*x as f32, *y as f32);
                    }
                }
                pb.close();
                if let Some(path) = pb.finish() {
                    c.fill_path(&path, *color);
                }
            }
            Op::RoundRect { rect: [x, y, w, h], radius, color } => c.round_rect(*x, *y, *w, *h, *radius, *color),
            Op::Image { sprite, src, dst, how } => c.image_part(&self.sprites[*sprite], *src, *dst, *how),
        }
    }

    pub fn json(&self) -> Value {
        let frames = |f: &[Frame]| Value::Array(f.iter().map(Frame::json).collect());
        let columns: Vec<Value> = self
            .columns
            .iter()
            .map(|c| {
                json!({
                    "x": c.x,
                    "w": c.w,
                    "tap": frames(&c.tap),
                    "head": frames(&c.head),
                    "tail": frames(&c.tail),
                    "body": c.body.as_ref().map(|b| json!({ "s": b.sprite, "sw": b.sw, "sh": b.sh, "style": b.style, "row": b.row })),
                    "tapColor": c.tap_color,
                    "headColor": c.head_color,
                    "light": ops_json(&c.light),
                    "keyUp": ops_json(&c.key_up),
                    "keyDown": ops_json(&c.key_down),
                    "lighting": ops_json(&c.lighting),
                })
            })
            .collect();
        let p = &self.pattern;
        json!({
            "width": WIDTH,
            "height": HEIGHT,
            "upscroll": self.upscroll,
            "lazerBodies": self.lazer_bodies,
            "keysUnderNotes": self.keys_under_notes,
            "hitY": self.hit_y,
            "hitUnits": self.hit_units,
            "cutNow": self.cut_now,
            "own": self.own,
            "bodyColor": self.body_color,
            "sprites": self.sprites.iter().map(|t| json!([t.width(), t.height(), t.id])).collect::<Vec<_>>(),
            "base": ops_json(&self.base),
            "hint": ops_json(&self.hint),
            "line": ops_json(&self.line),
            "over": ops_json(&self.over),
            "top": ops_json(&self.top),
            "columns": columns,
            "frozen": {
                "taps": p.taps.iter().map(|t| json!({ "c": t.column, "y": t.y, "t": t.time_ms })).collect::<Vec<_>>(),
                "holds": p.holds.iter().map(|h| json!({ "c": h.column, "head": h.head_y, "tail": h.tail_y, "t": h.time_ms })).collect::<Vec<_>>(),
                "pressed": p.pressed,
            },
        })
    }

    pub fn sprite_png(&self, id: usize) -> Option<Vec<u8>> {
        let tex = self.sprites.get(id)?;
        self.encoded[id].get_or_init(|| tex.pixmap.encode_png().ok()).clone()
    }
}
