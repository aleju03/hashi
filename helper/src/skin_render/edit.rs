use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use super::ini::Rgba;
use super::load::{self, Asset, Profile, Raw, Skin};
use super::{LAZER_BODY_SPAN, UNIT};

// what the page asks to change in one keymode, applied on top of the skin as it is on disk
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct Edits {
    // skin units of transparent lead-in at the tail end of each column's body; None keeps the skin's
    pub cut: Vec<Option<f64>>,
    pub fade: Option<Fade>,
    pub colors: Colors,
    // which parts a `Colors::Columns` goes on
    pub parts: Parts,
    // other images of the skin's own per column, as skin.ini would name them
    pub swaps: Vec<Option<Swap>>,
    // skin.ini's HitPosition, 0-480 from the top
    #[serde(rename = "hitPosition")]
    pub hit_position: Option<f64>,
    #[serde(rename = "judgementLine")]
    pub judgement_line: Option<bool>,
    // skin.ini's ScorePosition and ComboPosition, where judgements and the combo show, 0-480 from the top
    #[serde(rename = "scorePosition")]
    pub score_position: Option<f64>,
    #[serde(rename = "comboPosition")]
    pub combo_position: Option<f64>,
    // skin.ini's ColumnStart, the stage's left edge in 480ths of the screen's height
    #[serde(rename = "columnStart")]
    pub column_start: Option<f64>,
    // how far up the receptors' art moves in its images, in HitPosition's 480ths. The game keeps the
    // images on the stage's bottom edge whatever HitPosition says
    #[serde(rename = "keysRaise")]
    pub keys_raise: Option<f64>,
    // the same for StageBottom's art, which the game sets on the stage's bottom edge with no setting
    // to place it
    #[serde(rename = "bottomRaise")]
    pub bottom_raise: Option<f64>,
    // skin.ini's ColumnWidth and ColumnSpacing, the same for every column; None keeps the skin's
    #[serde(rename = "columnWidth")]
    pub column_width: Option<f64>,
    #[serde(rename = "columnSpacing")]
    pub column_spacing: Option<f64>,
    // per column, how each part's art is reshaped
    pub shapes: Vec<Option<Shapes>>,
    // the combo's digits this many times their size, written as a font of the keymode's own
    #[serde(rename = "comboScale")]
    pub combo_scale: Option<f64>,
    // the score font's % this wide in skin pixels, which moves the accuracy along the top: it's
    // right-aligned, so a wider % pushes the numbers further left. For the whole skin
    #[serde(rename = "percentWidth")]
    pub percent_width: Option<f64>,
}

impl Edits {
    pub fn combo_scale(&self) -> Option<f64> {
        self.combo_scale.filter(|s| s.is_finite() && (s - 1.0).abs() >= 0.005).map(|s| s.clamp(0.25, 4.0))
    }

    pub fn percent_width(&self) -> Option<f64> {
        self.percent_width.filter(|w| w.is_finite() && *w >= 1.0).map(|w| w.min(8000.0))
    }
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Shapes {
    pub note: Option<Shape>,
    pub head: Option<Shape>,
    pub body: Option<Shape>,
    pub tail: Option<Shape>,
    pub key: Option<Shape>,
    pub key_down: Option<Shape>,
}

// a part's art made narrower or wider in its image, which the game stretches across the column, and
// outlined inside its edge
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub(crate) struct Shape {
    // of the art's own width, 1 as it is
    pub width: Option<f64>,
    pub outline: Option<Outline>,
    // the art's own outline: "keep" leaves it its colour when the part is coloured, "remove" paints
    // it over with the colour inside it
    pub rim: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum RimMode {
    Keep,
    Remove,
}

#[derive(Deserialize, Clone, Debug)]
pub(crate) struct Outline {
    // in ColumnWidth's units
    pub size: f64,
    pub color: String,
    // around the art's edge instead of just inside it
    #[serde(default)]
    pub outside: bool,
}

impl Shape {
    fn width(&self) -> Option<f64> {
        self.width.filter(|w| w.is_finite() && (w - 1.0).abs() >= 0.005).map(|w| w.clamp(0.1, 3.0))
    }

    fn outline(&self) -> Option<(f64, Rgba, bool)> {
        let o = self.outline.as_ref()?;
        Some((o.size.clamp(0.0, 40.0), hex(&o.color)?, o.outside)).filter(|(size, ..)| *size > 0.0)
    }

    fn rim(&self) -> Option<RimMode> {
        match self.rim.as_deref()? {
            "keep" => Some(RimMode::Keep),
            "remove" => Some(RimMode::Remove),
            _ => None,
        }
    }
}

fn hex(s: &str) -> Option<Rgba> {
    let hex = s.strip_prefix('#')?;
    let v = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6)?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255])
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Swap {
    pub note: Option<String>,
    pub head: Option<String>,
    pub body: Option<String>,
    pub tail: Option<String>,
    pub key: Option<String>,
    pub key_down: Option<String>,
}

impl Swap {
    fn parts(&self) -> impl Iterator<Item = (Part, &str)> {
        [
            (Part::Note, &self.note),
            (Part::Head, &self.head),
            (Part::Body, &self.body),
            (Part::Tail, &self.tail),
            (Part::Key, &self.key),
            (Part::KeyDown, &self.key_down),
        ]
        .into_iter()
        .filter_map(|(p, r)| Some((p, r.as_deref().map(str::trim).filter(|r| !r.is_empty())?)))
    }
}

#[derive(Deserialize, Clone, Copy)]
pub(crate) struct Fade {
    // skin units from the body's visible end to full opacity
    pub length: f64,
    // opacity at the visible end
    pub from: f64,
    // the visible end rounded off like a cap instead of cut straight across
    #[serde(default)]
    pub round: bool,
}

#[derive(Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum Colors {
    // one per column, on every part `parts` names; how edits applied before colours per part said it
    Columns(Vec<Option<String>>),
    ByPart(PartColors),
}

impl Default for Colors {
    fn default() -> Self {
        Colors::Columns(vec![])
    }
}

// one per column for each part
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub(crate) struct PartColors {
    pub notes: Vec<Option<String>>,
    pub heads: Vec<Option<String>>,
    pub bodies: Vec<Option<String>>,
    pub tails: Vec<Option<String>>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(default)]
pub(crate) struct Parts {
    pub notes: bool,
    pub heads: bool,
    pub bodies: bool,
    pub tails: bool,
}

impl Default for Parts {
    fn default() -> Self {
        Parts { notes: true, heads: true, bodies: true, tails: true }
    }
}

impl Edits {
    pub fn parse(json: &str) -> Edits {
        if json.is_empty() {
            return Edits::default();
        }
        serde_json::from_str(json).unwrap_or_default()
    }

    fn color(&self, part: Part, column: usize) -> Option<Rgba> {
        let list = match &self.colors {
            Colors::Columns(list) => {
                let on = match part {
                    Part::Note => self.parts.notes,
                    Part::Head => self.parts.heads,
                    Part::Body => self.parts.bodies,
                    Part::Tail => self.parts.tails,
                    _ => false,
                };
                if !on {
                    return None;
                }
                list
            }
            Colors::ByPart(p) => match part {
                Part::Note => &p.notes,
                Part::Head => &p.heads,
                Part::Body => &p.bodies,
                Part::Tail => &p.tails,
                _ => return None,
            },
        };
        hex(list.get(column)?.as_deref()?)
    }

    // a shape that changes something, for a part of a column
    fn shape(&self, column: usize, part: Part) -> Option<&Shape> {
        let s = self.shapes.get(column)?.as_ref()?;
        let shape = match part {
            Part::Note => &s.note,
            Part::Head => &s.head,
            Part::Body => &s.body,
            Part::Tail => &s.tail,
            Part::Key => &s.key,
            Part::KeyDown => &s.key_down,
            Part::StageBottom => return None,
        };
        shape.as_ref().filter(|s| s.width().is_some() || s.outline().is_some() || s.rim().is_some())
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Part {
    Note,
    Head,
    Body,
    Tail,
    Key,
    KeyDown,
    // the stage's, not a column's
    StageBottom,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Note => "note",
            Part::Head => "head",
            Part::Body => "body",
            Part::Tail => "tail",
            Part::Key => "key",
            Part::KeyDown => "keydown",
            Part::StageBottom => "stage-bottom",
        }
    }

    pub fn ini_key(self, column: usize) -> String {
        match self {
            Part::Note => format!("NoteImage{column}"),
            Part::Head => format!("NoteImage{column}H"),
            Part::Body => format!("NoteImage{column}L"),
            Part::Tail => format!("NoteImage{column}T"),
            Part::Key => format!("KeyImage{column}"),
            Part::KeyDown => format!("KeyImage{column}D"),
            Part::StageBottom => "StageBottom".into(),
        }
    }
}

pub(super) const COLUMN_START_MAX: f64 = 853.0;
// what the game reads ColumnWidth and ColumnSpacing between
pub(super) const COLUMN_WIDTH_MIN: f64 = 20.0;
pub(super) const COLUMN_WIDTH_MAX: f64 = 160.0;
pub(super) const COLUMN_SPACING_MAX: f64 = 40.0;

// a swap that resolved: the column, the part and the reference skin.ini gets
pub(super) type Swapped = (usize, Part, String);

// an image an edit made, as files to write: one per animation frame
pub(super) struct Made {
    pub column: usize,
    pub part: Part,
    pub frames: Vec<Raw>,
    pub scale: f64,
}

// decoded source images by file, kept with the loaded skin so each edit doesn't decode them again
pub(super) type Sources = Mutex<HashMap<PathBuf, Option<Arc<Raw>>>>;

fn source(sources: &Sources, path: &Path) -> Option<Arc<Raw>> {
    let mut map = sources.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(path.to_path_buf()).or_insert_with(|| load::raw(path).map(Arc::new)).clone()
}

// The skin with the edits drawn in. `full` also returns the images at the size they'd be written;
// otherwise a long body is only made as far as the texture the game would keep of it.
pub(super) fn apply(p: &Profile, edits: &Edits, keys: usize, skin: &Skin, sources: &Sources, full: bool) -> (Profile, Vec<Made>, Vec<Swapped>) {
    let mut out = p.clone();
    let mut made = vec![];
    let mut swapped = vec![];
    out.columns.resize_with(keys.max(out.columns.len()), Default::default);
    if let Some(v) = edits.hit_position.filter(|v| (0.0..=480.0).contains(v)) {
        out.hit_position = Some(load::hit_from_ini(v));
    }
    if let Some(on) = edits.judgement_line {
        out.judgement_line = on;
    }
    if let Some(v) = edits.score_position.filter(|v| (0.0..=480.0).contains(v)) {
        out.score_position = Some(load::hit_from_ini(v));
    }
    // a skin that pushed its combo off the stage to hide it shows it again where it's put
    if let Some(v) = edits.combo_position.filter(|v| (0.0..=480.0).contains(v)) {
        out.combo_position = Some(load::hit_from_ini(v));
        out.combo_hidden = false;
        out.combo_origin = 0.0;
    }
    if let Some(v) = edits.column_start {
        out.column_start = Some(v.clamp(0.0, COLUMN_START_MAX));
    }
    if let Some(w) = edits.column_width.filter(|w| w.is_finite()) {
        let w = w.clamp(COLUMN_WIDTH_MIN, COLUMN_WIDTH_MAX).round();
        out.column_width = w;
        out.column_widths = vec![w; keys];
    }
    if let Some(s) = edits.column_spacing.filter(|s| s.is_finite()) {
        out.column_spacing = s.clamp(0.0, COLUMN_SPACING_MAX).round();
    }
    if let Some(s) = edits.combo_scale() {
        out.combo_scale *= s;
    }
    if let Some(w) = edits.percent_width() {
        out.percent_width = Some(w);
    }
    // swaps first, so the cut, fade and colour work on the art swapped in
    for (c, swap) in edits.swaps.iter().enumerate().take(keys) {
        for (part, reference) in swap.iter().flat_map(Swap::parts) {
            let Some(asset) = skin.swap(reference, matches!(part, Part::Note | Part::Head | Part::Tail)) else { continue };
            let column = &mut out.columns[c];
            let slot = match part {
                Part::Note => &mut column.tap,
                Part::Head => &mut column.head,
                Part::Body => &mut column.body,
                Part::Tail => &mut column.tail,
                Part::Key => &mut column.key,
                Part::KeyDown => &mut column.key_down,
                Part::StageBottom => continue,
            };
            *slot = Some(asset);
            swapped.push((c, part, reference.to_string()));
        }
    }
    // the same art with the same edit in several columns is made once
    let mut done: HashMap<(PathBuf, String), (Asset, Option<Raw>)> = HashMap::new();
    for c in 0..keys {
        // no longer than the stage is tall, the rows it adds are allocated whole
        let cut = edits.cut.get(c).copied().flatten().map(|u| u.clamp(0.0, 480.0));
        let column = out.columns[c].clone();
        // the column's width in skin units, what its art is stretched across
        let units = out.column_widths.get(c).copied().unwrap_or(out.column_width);
        let shape = edits.shape(c, Part::Body);
        let tint = edits.color(Part::Body, c);
        if let Some(asset) = column.body.as_ref().filter(|_| cut.is_some() || edits.fade.is_some() || tint.is_some() || shape.is_some()) {
            let style = p.body_styles.get(c).copied().unwrap_or(1.0);
            let restyle = style == 0.0 && (cut.is_some() || edits.fade.is_some());
            let key = format!("body {cut:?} {:?} {tint:?} {} {units} {shape:?}", edits.fade.map(|f| (f.length, f.from, f.round)), restyle);
            // shaped and coloured first, so the cut and fade take the outline with them
            let make = |src: &Raw| {
                let pre = prep(src, tint, shape, units, false);
                body(&pre, asset, p.lazer_bodies, units, cut, edits.fade, None, full).or(Some((pre, None)))
            };
            if let Some((new, raw)) = redraw(asset, key, &mut done, sources, full, make) {
                if let Some(raw) = raw {
                    made.push(Made { column: c, part: Part::Body, frames: vec![raw], scale: asset.scale });
                }
                out.columns[c].body = Some(new);
                // a stretched body can't keep a cut, so it cascades like a Percy body
                if restyle {
                    if out.body_styles.is_empty() {
                        out.body_styles = vec![1.0; keys];
                    }
                    out.body_styles.resize(keys, 1.0);
                    out.body_styles[c] = 1.0;
                }
            }
        }
        let raise = edits.keys_raise.filter(|r| r.abs() >= 0.5 && r.abs() <= 480.0);
        for part in [Part::Note, Part::Head, Part::Tail, Part::Key, Part::KeyDown] {
            let tint = edits.color(part, c);
            let shape = edits.shape(c, part);
            let up = raise.filter(|_| matches!(part, Part::Key | Part::KeyDown));
            if tint.is_none() && shape.is_none() && up.is_none() {
                continue;
            }
            let Some(asset) = slot(&column, part) else { continue };
            let mut frames = vec![];
            let mut new = asset.clone();
            for (i, frame) in std::iter::once(asset).chain(&asset.frames).enumerate() {
                // a row of receptor art is a 768th of the stage, half of one at @2x
                let rows = up.map(|up| (up / UNIT * frame.scale).round() as i64);
                let key = format!("{} {tint:?} {shape:?} {units} {rows:?}", part.name());
                let drawn = redraw(frame, key, &mut done, sources, full, |src| {
                    let shaped = prep(src, tint, shape, units, true);
                    Some(match rows {
                        Some(rows) => raised(&shaped, rows),
                        None => (shaped, None),
                    })
                });
                let (drawn, raw) = drawn.unwrap_or_else(|| (frame.clone(), None));
                frames.extend(raw);
                if i == 0 {
                    new = Asset { frames: vec![], ..drawn };
                } else {
                    new.frames.push(drawn);
                }
            }
            if full && !frames.is_empty() {
                made.push(Made { column: c, part, frames, scale: asset.scale });
            }
            if let Some(slot) = slot_mut(&mut out.columns[c], part) {
                *slot = Some(new);
            }
        }
    }
    // a row of StageBottom's art is a 480th of the stage, half of one at @2x
    if let (Some(up), Some(asset)) = (edits.bottom_raise.filter(|r| r.abs() >= 0.5 && r.abs() <= 480.0), out.stage.bottom.clone()) {
        let mut frames = vec![];
        let mut new = asset.clone();
        for (i, frame) in std::iter::once(&asset).chain(&asset.frames).enumerate() {
            let rows = (up * frame.scale).round() as i64;
            let drawn = redraw(frame, format!("stage-bottom {rows}"), &mut done, sources, full, |src| Some(raised(src, rows)));
            let (drawn, raw) = drawn.unwrap_or_else(|| (frame.clone(), None));
            frames.extend(raw);
            if i == 0 {
                new = Asset { frames: vec![], ..drawn };
            } else {
                new.frames.push(drawn);
            }
        }
        if full && !frames.is_empty() {
            made.push(Made { column: 0, part: Part::StageBottom, frames, scale: asset.scale });
        }
        out.stage.bottom = Some(new);
    }
    (out, made, swapped)
}

fn slot(column: &load::Column, part: Part) -> Option<&Asset> {
    match part {
        Part::Note => column.tap.as_ref(),
        Part::Head => column.head.as_ref(),
        Part::Body => column.body.as_ref(),
        Part::Tail => column.tail.as_ref(),
        Part::Key => column.key.as_ref(),
        Part::KeyDown => column.key_down.as_ref(),
        Part::StageBottom => None,
    }
}

fn slot_mut(column: &mut load::Column, part: Part) -> Option<&mut Option<Asset>> {
    Some(match part {
        Part::Note => &mut column.tap,
        Part::Head => &mut column.head,
        Part::Body => &mut column.body,
        Part::Tail => &mut column.tail,
        Part::Key => &mut column.key,
        Part::KeyDown => &mut column.key_down,
        Part::StageBottom => return None,
    })
}

// The art reshaped, recoloured, then outlined, in that order so the outline keeps its own colour.
// `units` is the column's width in skin units, what the image is stretched across. `ends` outlines
// the image's top and bottom edges too, which a body's art repeats across
fn prep(src: &Raw, tint: Option<Rgba>, shape: Option<&Shape>, units: f64, ends: bool) -> Raw {
    let mut raw = match shape.and_then(Shape::width) {
        Some(f) => squeezed(src, f),
        None => Raw::new(src.rgba.clone(), src.w, src.h),
    };
    // found after the reshaping, which moves it with the art
    let mode = shape.and_then(Shape::rim);
    let found = mode.filter(|m| *m == RimMode::Remove || tint.is_some()).and_then(|_| rim(&raw, ends));
    match (mode, &found) {
        (Some(RimMode::Remove), Some(r)) => {
            unrim(&mut raw, r, ends);
            if let Some(t) = tint {
                raw = tinted(&raw, t).0;
            }
        }
        (Some(RimMode::Keep), Some(r)) => {
            if let Some(t) = tint {
                raw = tinted_keeping(&raw, t, r);
            }
        }
        _ => {
            if let Some(t) = tint {
                raw = tinted(&raw, t).0;
            }
        }
    }
    if let Some((size, color, outside)) = shape.and_then(Shape::outline) {
        let px = size * f64::from(raw.w) / units.max(1.0);
        outline(&mut raw, px, color, ends, outside);
    }
    raw
}

// The art `f` times as wide about the image's middle, the image the same size: narrower leaves
// clear sides, wider runs off them
fn squeezed(src: &Raw, f: f64) -> Raw {
    let (w, h) = (src.w as usize, src.h as usize);
    let mut rgba = vec![0u8; w * h * 4];
    let mid = w as f64 / 2.0;
    // premultiplied while it's mixed, so clear pixels don't darken the edges
    let take = |x: i64, y: usize| -> [f64; 4] {
        if x < 0 || x >= w as i64 {
            return [0.0; 4];
        }
        let i = (y * w + x as usize) * 4;
        let a = f64::from(src.rgba[i + 3]) / 255.0;
        [f64::from(src.rgba[i]) * a, f64::from(src.rgba[i + 1]) * a, f64::from(src.rgba[i + 2]) * a, a]
    };
    for x in 0..w {
        let from = (x as f64 + 0.5 - mid) / f + mid - 0.5;
        let left = from.floor();
        let t = from - left;
        let left = left as i64;
        for y in 0..h {
            let (p, q) = (take(left, y), take(left + 1, y));
            let mix = |k: usize| p[k] * (1.0 - t) + q[k] * t;
            let a = mix(3);
            if a <= 0.0 {
                continue;
            }
            let i = (y * w + x) * 4;
            for k in 0..3 {
                rgba[i + k] = (mix(k) / a).round().min(255.0) as u8;
            }
            rgba[i + 3] = (a * 255.0).round().min(255.0) as u8;
        }
    }
    Raw::new(rgba, src.w, src.h)
}

// A line `px` wide along the art's edge: just inside it, or around it in the image's clear space.
// The image can't grow, as the game scales it to the column, so what doesn't fit around the art goes
// inside it and the line keeps its shape the whole way round
fn outline(raw: &mut Raw, px: f64, color: Rgba, ends: bool, outside: bool) {
    if px <= 0.0 {
        return;
    }
    let around = if outside { px.min(room(raw, ends)) } else { 0.0 };
    if px - around > 0.0 {
        inside_line(raw, px - around, color, ends);
    }
    if around > 0.0 {
        around_line(raw, around, color);
    }
}

// clear pixels between the solid art and the image's nearest edge; a body's top and bottom don't
// count, as it repeats across them
fn room(raw: &Raw, ends: bool) -> f64 {
    let (w, h) = (raw.w as usize, raw.h as usize);
    let mut most = usize::MAX;
    for (i, p) in raw.rgba.chunks_exact(4).enumerate() {
        if p[3] < 128 {
            continue;
        }
        let (x, y) = (i % w, i / w);
        most = most.min(x).min(w - 1 - x);
        if ends {
            most = most.min(y).min(h - 1 - y);
        }
    }
    most as f64
}

// Faint art counts as art, so a body fading out keeps its line along its sides; the line is solid
// either way, or a soft edge leaves it see-through and a white line shows grey
fn inside_line(raw: &mut Raw, px: f64, color: Rgba, ends: bool) {
    let d = inside_distance(raw, ends);
    for (p, dist) in raw.rgba.chunks_exact_mut(4).zip(d) {
        if dist == 0.0 {
            continue;
        }
        let cover = (px + 1.0 - f64::from(dist)).clamp(0.0, 1.0);
        if cover <= 0.0 {
            continue;
        }
        for k in 0..3 {
            p[k] = (f64::from(p[k]) * (1.0 - cover) + f64::from(color[k]) * cover).round() as u8;
        }
        p[3] = p[3].max((cover * 255.0).round() as u8);
    }
}

// around the solid part of the art, drawn under its soft edge so the two meet without a gap
fn around_line(raw: &mut Raw, px: f64, color: Rgba) {
    let (w, h) = (raw.w as usize, raw.h as usize);
    let clear: Vec<bool> = raw.rgba.chunks_exact(4).map(|p| p[3] < 128).collect();
    let d = distance_from(&clear, w, h, f32::MAX / 2.0, f32::MAX / 2.0);
    for (p, dist) in raw.rgba.chunks_exact_mut(4).zip(d) {
        if dist == 0.0 {
            continue;
        }
        let cover = (px + 1.0 - f64::from(dist)).clamp(0.0, 1.0);
        if cover <= 0.0 {
            continue;
        }
        let a = f64::from(p[3]) / 255.0;
        let out = a + cover * (1.0 - a);
        for k in 0..3 {
            p[k] = ((f64::from(p[k]) * a + f64::from(color[k]) * cover * (1.0 - a)) / out).round().min(255.0) as u8;
        }
        p[3] = (out * 255.0).round() as u8;
    }
}

// An outline drawn into the art itself: a band along its edge in a colour of its own, the inside
// another. Each pixel's share of it, 1 in the band, a fraction where the band blends into the inside
pub(super) struct Rim {
    pub weight: Vec<f32>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub strength: f64,
    pub color: Rgba,
    pub px: usize,
}

const RIM_DIFF: f64 = 40.0;
// pixels of solid outline an edge needs to count as one
const RIM_STRENGTH: f64 = 3.0;

fn color_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

// The art's built-in outline, if it has one. The art is cut into rings by their distance in from
// the edge and each ring's colour averaged: an outline is outer rings that differ from the inside,
// alike among themselves, and a sharp change to the inside, which shading that grades into it isn't
pub(super) fn rim(raw: &Raw, ends: bool) -> Option<Rim> {
    let d = inside_distance(raw, ends);
    let deepest = d.iter().copied().filter(|v| *v < f32::MAX / 4.0).fold(0.0f32, f32::max);
    if deepest < 6.0 {
        return None;
    }
    // the widest outline looked for, and the band past it the inside's colour is taken from
    let most = ((deepest * 0.3) as usize).clamp(2, 24);
    let inner = (most + 1, (most + 6).min(deepest as usize).max(most + 1));
    let rings = inner.1 + 1;
    let mut sums = vec![[0.0f64; 4]; rings + 1];
    let mut counts = vec![0usize; rings + 1];
    for (p, &dist) in raw.rgba.chunks_exact(4).zip(&d) {
        if dist == 0.0 || dist >= f32::MAX / 4.0 {
            continue;
        }
        let k = dist.ceil() as usize;
        if k > rings {
            continue;
        }
        let a = f64::from(p[3]) / 255.0;
        for c in 0..3 {
            sums[k][c] += f64::from(p[c]) * a;
        }
        sums[k][3] += a;
        counts[k] += 1;
    }
    let mean = |s: &[f64; 4]| (s[3] > 0.0).then(|| [s[0] / s[3], s[1] / s[3], s[2] / s[3]]);
    let ring: Vec<Option<[f64; 3]>> = sums.iter().map(mean).collect();
    let mut inside = [0.0f64; 4];
    for s in &sums[inner.0..=inner.1] {
        for c in 0..4 {
            inside[c] += s[c];
        }
    }
    let inside = mean(&inside)?;
    let diff: Vec<f64> = ring.iter().map(|r| r.map_or(0.0, |r| color_distance(r, inside))).collect();
    // the band's colour from its second ring, as the first is often a faint anti-aliased edge
    let avg = ring.get(2).copied().flatten()?;
    let band_diff = color_distance(avg, inside);
    if band_diff < RIM_DIFF {
        return None;
    }
    let alike = |k: usize| ring.get(k).copied().flatten().is_some_and(|r| color_distance(r, avg) < band_diff * 0.25);
    // the band runs in from the edge while rings are its colour
    let mut px = 2;
    while px < most && alike(px + 1) {
        px += 1;
    }
    // thick: how many pixels of solid outline colour the edge holds, each ring by its opacity and by
    // how near its colour is to the band's. A solid line scores its width, a soft light edge little
    let along = [0, 1, 2].map(|c| avg[c] - inside[c]);
    let span = along.iter().map(|v| v * v).sum::<f64>().max(1.0);
    let strength: f64 = (1..=px + 2)
        .filter_map(|k| {
            let r = ring.get(k).copied().flatten()?;
            let t = (0..3).map(|c| (r[c] - inside[c]) * along[c]).sum::<f64>() / span;
            Some(sums[k][3] / counts[k].max(1) as f64 * t.clamp(0.0, 1.0))
        })
        .sum();
    if strength < RIM_STRENGTH {
        return None;
    }
    // sharp: a couple of rings past the band it's the inside's colour, where shading would still differ
    if diff.get(px + 3).is_some_and(|v| *v >= RIM_DIFF * 0.5) {
        return None;
    }
    // by colour, a pixel's place between the inside's and the band's, as the band's own width in
    // rings is only near even along a curve; past a couple of rings beyond it nothing counts
    let weight = raw
        .rgba
        .chunks_exact(4)
        .zip(&d)
        .map(|(p, &dist)| {
            if dist == 0.0 || dist >= f32::MAX / 4.0 || dist.ceil() as usize > px + 2 {
                return 0.0;
            }
            let t = (0..3).map(|c| (f64::from(p[c]) - inside[c]) * along[c]).sum::<f64>() / span;
            // a little off either colour still counts as that colour
            ((t - 0.15) / 0.7).clamp(0.0, 1.0) as f32
        })
        .collect();
    Some(Rim { weight, strength, color: [avg[0].round() as u8, avg[1].round() as u8, avg[2].round() as u8, 255], px })
}

// The outline painted over with the colour just inside it, so the art is the same size without it.
// Done from the inside out, each pixel taking the average of its neighbours further in
fn unrim(raw: &mut Raw, rim: &Rim, ends: bool) {
    let (w, h) = (raw.w as usize, raw.h as usize);
    let d = inside_distance(raw, ends);
    let mut order: Vec<usize> = (0..w * h).filter(|&i| rim.weight[i] > 0.0).collect();
    order.sort_by(|&a, &b| d[b].total_cmp(&d[a]));
    let src = raw.rgba.clone();
    let mut fill = src.clone();
    for &i in &order {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let mut sum = [0.0f64; 3];
        let mut n = 0.0;
        for dy in -1..=1i64 {
            for dx in -1..=1i64 {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                // further in: inside the band, or in it and already filled, as it's done deepest first
                if d[j] <= d[i] || fill[j * 4 + 3] < 25 {
                    continue;
                }
                for c in 0..3 {
                    sum[c] += f64::from(fill[j * 4 + c]);
                }
                n += 1.0;
            }
        }
        if n == 0.0 {
            continue;
        }
        let t = f64::from(rim.weight[i]);
        for c in 0..3 {
            let inside = sum[c] / n;
            fill[i * 4 + c] = (f64::from(src[i * 4 + c]) * (1.0 - t) + inside * t).round() as u8;
        }
    }
    raw.rgba = fill;
}

// Each pixel's distance in from the art's edge, 1 on the edge and 0 off the art, by a two pass
// distance transform
fn inside_distance(raw: &Raw, ends: bool) -> Vec<f32> {
    let (w, h) = (raw.w as usize, raw.h as usize);
    let art: Vec<bool> = raw.rgba.chunks_exact(4).map(|p| p[3] >= 25).collect();
    distance_from(&art, w, h, 0.0, if ends { 0.0 } else { f32::MAX / 2.0 })
}

// Each pixel of `region` its distance to the nearest pixel outside it, 0 outside. Past the image's
// sides counts as `sides` far, past its top and bottom as `ends`: 0 makes them an edge
fn distance_from(region: &[bool], w: usize, h: usize, sides: f32, ends: f32) -> Vec<f32> {
    let diagonal = std::f32::consts::SQRT_2;
    let mut d: Vec<f32> = region.iter().map(|&r| if r { f32::MAX / 2.0 } else { 0.0 }).collect();
    let edge_distance = |d: &[f32], x: i64, y: i64| {
        if x < 0 || x >= w as i64 {
            sides
        } else if y < 0 || y >= h as i64 {
            ends
        } else {
            d[y as usize * w + x as usize]
        }
    };
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let i = y as usize * w + x as usize;
            if d[i] == 0.0 {
                continue;
            }
            let at = |dx: i64, dy: i64| edge_distance(&d, x + dx, y + dy);
            let v = d[i].min(at(-1, 0) + 1.0).min(at(0, -1) + 1.0).min(at(-1, -1) + diagonal).min(at(1, -1) + diagonal);
            d[i] = v;
        }
    }
    for y in (0..h as i64).rev() {
        for x in (0..w as i64).rev() {
            let i = y as usize * w + x as usize;
            if d[i] == 0.0 {
                continue;
            }
            let at = |dx: i64, dy: i64| edge_distance(&d, x + dx, y + dy);
            let v = d[i].min(at(1, 0) + 1.0).min(at(0, 1) + 1.0).min(at(1, 1) + diagonal).min(at(-1, 1) + diagonal);
            d[i] = v;
        }
    }
    d
}

// The art `rows` higher in its image, which the game sets on the stage's bottom edge: empty rows
// added under it, or taken off the bottom to lower it
fn raised(src: &Raw, rows: i64) -> (Raw, Option<f64>) {
    let row_bytes = src.w as usize * 4;
    let h = (src.h as i64 + rows).max(1) as usize;
    let keep = h.min(src.h as usize);
    let mut rgba = vec![0u8; h * row_bytes];
    rgba[..keep * row_bytes].copy_from_slice(&src.rgba[..keep * row_bytes]);
    (Raw::new(rgba, src.w, h as u32), None)
}

// an asset with its art made again by `make`, and the made image when it's for writing
fn redraw(
    asset: &Asset,
    key: String,
    done: &mut HashMap<(PathBuf, String), (Asset, Option<Raw>)>,
    sources: &Sources,
    full: bool,
    make: impl FnOnce(&Raw) -> Option<(Raw, Option<f64>)>,
) -> Option<(Asset, Option<Raw>)> {
    let path = asset.path.clone()?;
    let id = (path.clone(), key);
    if let Some((a, raw)) = done.get(&id) {
        return Some((a.clone(), raw.as_ref().map(|r| Raw::new(r.rgba.clone(), r.w, r.h))));
    }
    let src = source(sources, &path)?;
    let (raw, full_height) = make(&src)?;
    let kept = full.then(|| Raw::new(raw.rgba.clone(), raw.w, raw.h));
    let (image, width, height, source_height) = load::texture(raw)?;
    let new = Asset {
        width: Some(width),
        height: Some(height),
        source_height: source_height.or(full_height),
        image: Some(image),
        frames: vec![],
        path: None,
        ..asset.clone()
    };
    done.insert(id, (new.clone(), kept.as_ref().map(|r| Raw::new(r.rgba.clone(), r.w, r.h))));
    Some((new, kept))
}

fn luma(p: &[u8]) -> f64 {
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

// The colour times each pixel's lightness against the art's brightest: white art takes the colour
// exactly, the way the game tints a sprite, and coloured art is recoloured with its shading kept
fn tint(rgba: &mut [u8], color: Rgba, max: f64) {
    for p in rgba.chunks_exact_mut(4) {
        let l = if max > 0.0 { (luma(p) / max).min(1.0) } else { 1.0 };
        for i in 0..3 {
            p[i] = (color[i] as f64 * l).round() as u8;
        }
    }
}

fn brightest(rgba: &[u8]) -> f64 {
    rgba.chunks_exact(4).filter(|p| p[3] >= 25).map(luma).fold(0.0, f64::max)
}

// Coloured with the built-in outline left as it is: only the inside counts toward the brightest,
// so it takes the colour exactly, and the outline's pixels keep theirs by their share of it
fn tinted_keeping(src: &Raw, color: Rgba, rim: &Rim) -> Raw {
    let max = src
        .rgba
        .chunks_exact(4)
        .zip(&rim.weight)
        .filter(|(p, w)| p[3] >= 25 && **w < 0.5)
        .map(|(p, _)| luma(p))
        .fold(0.0, f64::max);
    let mut rgba = src.rgba.clone();
    tint(&mut rgba, color, max);
    for ((p, o), &w) in rgba.chunks_exact_mut(4).zip(src.rgba.chunks_exact(4)).zip(&rim.weight) {
        if w > 0.0 {
            let t = f64::from(w);
            for c in 0..3 {
                p[c] = (f64::from(p[c]) * (1.0 - t) + f64::from(o[c]) * t).round() as u8;
            }
        }
    }
    Raw::new(rgba, src.w, src.h)
}

fn tinted(src: &Raw, color: Rgba) -> (Raw, Option<f64>) {
    let mut rgba = src.rgba.clone();
    let max = brightest(&rgba);
    tint(&mut rgba, color, max);
    (Raw::new(rgba, src.w, src.h), None)
}

// rows of body art per skin unit: stable gives a row 480/768 of a unit (half at @2x), lazer
// stretches the whole image over its span
pub(super) fn rows_per_unit(scale: f64, full_height: f64, lazer: bool) -> f64 {
    if lazer {
        full_height / LAZER_BODY_SPAN
    } else {
        scale / UNIT
    }
}

pub(super) fn lead_rows(rgba: &[u8], w: u32) -> Option<usize> {
    rgba.chunks_exact(w as usize * 4).position(|row| row.chunks_exact(4).any(|p| p[3] >= 25))
}

// The image stretched to `rows` tall, mixed premultiplied so clear rows don't darken their neighbours
fn taller(src: &Raw, rows: u32) -> Raw {
    let (w, h, new_h) = (src.w as usize, src.h as usize, rows as usize);
    let row_bytes = w * 4;
    let mut rgba = vec![0u8; new_h * row_bytes];
    for y in 0..new_h {
        let from = ((y as f64 + 0.5) * h as f64 / new_h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
        let top = from.floor() as usize;
        let bottom = (top + 1).min(h - 1);
        let t = from - top as f64;
        for x in 0..w {
            let (p, q) = ((top * w + x) * 4, (bottom * w + x) * 4);
            let (pa, qa) = (f64::from(src.rgba[p + 3]), f64::from(src.rgba[q + 3]));
            let a = pa * (1.0 - t) + qa * t;
            let i = y * row_bytes + x * 4;
            if a <= 0.0 {
                continue;
            }
            for c in 0..3 {
                let v = (f64::from(src.rgba[p + c]) * pa * (1.0 - t) + f64::from(src.rgba[q + c]) * qa * t) / a;
                rgba[i + c] = v.round().min(255.0) as u8;
            }
            rgba[i + 3] = a.round().min(255.0) as u8;
        }
    }
    Raw::new(rgba, src.w, rows)
}

// A Percy body: `cut` units of nothing at the tail end, then the skin's own art, fading in from the
// visible end. Art that isn't a strip already is tiled down far enough that a hold never repeats it.
#[allow(clippy::too_many_arguments)]
fn body(src: &Raw, asset: &Asset, lazer: bool, width: f64, cut: Option<f64>, fade: Option<Fade>, tint: Option<Rgba>, full: bool) -> Option<(Raw, Option<f64>)> {
    // lazer stretches the image over its whole span, so a short one has a row for hundreds of units
    // and a cut or fade shorter than that rounds away or smears over the hold. Made as tall as a
    // texture goes first, which the stretch doesn't show
    let taller_one;
    let src = if lazer && (cut.is_some() || fade.is_some()) && src.h < load::MAX_TEXTURE {
        taller_one = taller(src, load::MAX_TEXTURE);
        &taller_one
    } else {
        src
    };
    let (w, h) = (src.w, src.h as usize);
    let row_bytes = w as usize * 4;
    let old_lead = lead_rows(&src.rgba, w)?;
    let rows = rows_per_unit(asset.scale, h as f64, lazer);
    if cut.is_none() && fade.is_none() {
        return tint.map(|t| tinted(src, t));
    }
    let art = h - old_lead;
    let strip = old_lead > 0 || load::is_strip(w, src.h);
    let lead = cut.map_or(old_lead, |u| (u * rows).round() as usize);
    let total = if lazer {
        h
    } else if strip {
        lead + art
    } else {
        lead + art.max(4096)
    };
    let lead = lead.min(total.saturating_sub(1));
    // what the loader would keep of the file, unless all of it is wanted
    let made = if full || !load::is_strip(w, total as u32) { total } else { total.min(load::MAX_TEXTURE as usize) };
    let max = tint.map(|_| brightest(&src.rgba[old_lead * row_bytes..])).unwrap_or(0.0);
    // the cap is a half ellipse as wide as the art's opaque part; the body is stretched to the column's
    // width but keeps its rows, so its height in rows comes from the column width in units
    let cap = fade.filter(|f| f.round).and_then(|_| {
        // the widest the art gets near its end: a skin's own cap narrows its first rows
        let (left, right) = (old_lead..h.min(old_lead + 400))
            .filter_map(|y| {
                let row = &src.rgba[y * row_bytes..(y + 1) * row_bytes];
                Some((row.chunks_exact(4).position(|p| p[3] >= 25)?, row.chunks_exact(4).rposition(|p| p[3] >= 25)?))
            })
            .max_by_key(|(l, r)| r - l)?;
        let half = (right + 1 - left) as f64 / 2.0;
        let rows_tall = half / (w as f64 / width.max(1.0)) * rows;
        Some((left as f64 + half, half, rows_tall.max(1.0)))
    });
    let mut rgba = vec![0u8; made * row_bytes];
    for i in lead..made {
        let k = i - lead;
        let from = if k < art {
            old_lead + k
        } else if strip {
            h - 1
        } else {
            old_lead + k % art
        };
        let row = &mut rgba[i * row_bytes..(i + 1) * row_bytes];
        row.copy_from_slice(&src.rgba[from * row_bytes..(from + 1) * row_bytes]);
        if let Some(t) = tint {
            self::tint(row, t, max);
        }
        if let Some(f) = fade.filter(|f| f.length > 0.0) {
            let at = (k as f64 / rows / f.length).clamp(0.0, 1.0);
            let alpha = f.from.clamp(0.0, 1.0) + (1.0 - f.from.clamp(0.0, 1.0)) * at;
            for p in row.chunks_exact_mut(4) {
                p[3] = (p[3] as f64 * alpha).round() as u8;
            }
        }
        if let Some((mid, half, tall)) = cap.filter(|c| (k as f64) < c.2) {
            let dy = (tall - k as f64 - 0.5) / tall;
            // a soft edge a few pixels wide, like the hand-drawn caps
            let soft = (half * 0.12).max(1.0);
            for (x, p) in row.chunks_exact_mut(4).enumerate() {
                let dx = (x as f64 + 0.5 - mid) / half;
                let inside = (1.0 - (dx * dx + dy * dy).sqrt()) * half;
                let keep = (inside / soft + 0.5).clamp(0.0, 1.0);
                p[3] = (p[3] as f64 * keep).round() as u8;
            }
        }
    }
    let full_height = (made < total).then_some(total as f64);
    Some((Raw::new(rgba, w, made as u32), full_height))
}

#[cfg(test)]
mod tests {
    use super::*;

    // a disc of `fill` with a `band` px wide ring of `ring` at its edge, on clear
    fn disc(size: u32, fill: [u8; 3], ring: [u8; 3], band: f64) -> Raw {
        let mut rgba = vec![0u8; (size * size * 4) as usize];
        let r = size as f64 / 2.0 - 1.0;
        for y in 0..size {
            for x in 0..size {
                let d = ((x as f64 + 0.5 - size as f64 / 2.0).powi(2) + (y as f64 + 0.5 - size as f64 / 2.0).powi(2)).sqrt();
                if d > r {
                    continue;
                }
                let i = ((y * size + x) * 4) as usize;
                let c = if d > r - band { ring } else { fill };
                rgba[i..i + 3].copy_from_slice(&c);
                rgba[i + 3] = 255;
            }
        }
        Raw::new(rgba, size, size)
    }

    // a 40 px wide grey bar down the middle of an 80 px image, its sides fading over 4 px
    fn soft_bar() -> Raw {
        let (w, h) = (80u32, 40u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let inset = (x as i64 - 20).min(59 - x as i64);
                if inset < 0 {
                    continue;
                }
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 3].copy_from_slice(&[130, 128, 132]);
                rgba[i + 3] = ((inset + 1) as f64 / 4.0).min(1.0).mul_add(250.0, 0.0) as u8;
            }
        }
        Raw::new(rgba, w, h)
    }

    #[test]
    fn an_inside_outline_is_solid_on_a_soft_edge() {
        let mut raw = soft_bar();
        outline(&mut raw, 3.0, [255, 255, 255, 255], false, false);
        // the bar's faint first column, which a colour-only line left see-through
        let p = &raw.rgba[(20 * 80 + 20) * 4..][..4];
        assert_eq!(p, &[255, 255, 255, 255]);
        // well inside it, untouched
        assert_eq!(&raw.rgba[(20 * 80 + 40) * 4..][..3], &[130, 128, 132]);
    }

    #[test]
    fn an_outside_outline_goes_around_the_art() {
        let mut raw = soft_bar();
        let before = raw.rgba.clone();
        outline(&mut raw, 3.0, [255, 255, 255, 255], false, true);
        let at = |x: usize| &raw.rgba[(20 * 80 + x) * 4..][..4];
        // two pixels out from the bar's solid part: line
        assert_eq!(at(19)[3], 255);
        assert!(at(19)[..3].iter().all(|c| *c >= 250), "{:?}", at(19));
        // well clear of it: nothing
        assert_eq!(at(5)[3], 0);
        // the solid art itself is as it was
        assert_eq!(at(40), &before[(20 * 80 + 40) * 4..][..4]);
    }

    #[test]
    fn an_outside_outline_without_room_goes_inside() {
        // a disc that fills its image: no clear space around it but the corners
        let mut raw = disc(64, [100, 200, 100], [100, 200, 100], 0.0);
        outline(&mut raw, 4.0, [255, 255, 255, 255], true, true);
        let at = |x: usize, y: usize| &raw.rgba[(y * 64 + x) * 4..][..4];
        // the corners stay clear, so it doesn't turn into an octagon
        assert!(at(7, 7)[3] < 128, "{:?}", at(7, 7));
        // the line runs inside the edge instead
        assert_eq!(at(3, 32), &[255, 255, 255, 255]);
        assert_eq!(&at(32, 32)[..3], &[100, 200, 100]);
    }

    #[test]
    fn colours_go_on_their_own_part() {
        let rgb = |e: &Edits, part: Part, c: usize| e.color(part, c).map(|x| [x[0], x[1], x[2]]);
        let e = Edits::parse(r##"{"colors":{"notes":["#ff0000"],"heads":[null,"#00ff00"]}}"##);
        assert_eq!(rgb(&e, Part::Note, 0), Some([255, 0, 0]));
        assert_eq!(rgb(&e, Part::Head, 0), None);
        assert_eq!(rgb(&e, Part::Head, 1), Some([0, 255, 0]));
        assert_eq!(rgb(&e, Part::Body, 0), None);
        // one per column, on the parts named
        let old = Edits::parse(r##"{"colors":["#ff0000"],"parts":{"notes":true,"heads":false,"bodies":true,"tails":true}}"##);
        assert_eq!(rgb(&old, Part::Note, 0), Some([255, 0, 0]));
        assert_eq!(rgb(&old, Part::Head, 0), None);
        assert_eq!(rgb(&old, Part::Body, 0), Some([255, 0, 0]));
    }

    #[test]
    fn finds_a_white_rim_on_a_note() {
        let raw = disc(128, [255, 235, 165], [255, 255, 255], 5.0);
        let rim = rim(&raw, true).expect("a rim");
        assert!((4..=6).contains(&rim.px), "{}", rim.px);
        assert!(rim.color.iter().take(3).all(|c| *c >= 245));
    }

    #[test]
    fn no_rim_on_a_plain_or_shaded_note() {
        assert!(rim(&disc(128, [255, 235, 165], [255, 235, 165], 5.0), true).is_none());
        // darker toward the edge little by little, the way shading goes
        let mut shaded = disc(128, [200, 200, 200], [200, 200, 200], 0.0);
        for y in 0..128u32 {
            for x in 0..128u32 {
                let d = ((x as f64 - 63.5).powi(2) + (y as f64 - 63.5).powi(2)).sqrt();
                let i = ((y * 128 + x) * 4) as usize;
                let v = (240.0 - d * 2.5).max(0.0) as u8;
                shaded.rgba[i..i + 3].copy_from_slice(&[v, v, v]);
            }
        }
        assert!(rim(&shaded, true).is_none());
    }

    #[test]
    fn removing_the_rim_leaves_the_fill_and_the_size() {
        let mut raw = disc(128, [255, 235, 165], [255, 255, 255], 5.0);
        let alpha: Vec<u8> = raw.rgba.chunks_exact(4).map(|p| p[3]).collect();
        let r = rim(&raw, true).unwrap();
        unrim(&mut raw, &r, true);
        assert_eq!(alpha, raw.rgba.chunks_exact(4).map(|p| p[3]).collect::<Vec<_>>());
        for p in raw.rgba.chunks_exact(4).filter(|p| p[3] > 0) {
            assert!(color_distance([p[0] as f64, p[1] as f64, p[2] as f64], [255.0, 235.0, 165.0]) < 12.0, "{p:?}");
        }
    }

    #[test]
    fn keeping_the_rim_colours_only_the_inside() {
        let raw = disc(128, [255, 235, 165], [255, 255, 255], 5.0);
        let r = rim(&raw, true).unwrap();
        let out = tinted_keeping(&raw, [230, 30, 90, 255], &r);
        let at = |x: u32, y: u32| &out.rgba[((y * 128 + x) * 4) as usize..][..3];
        assert_eq!(at(64, 64), &[230, 30, 90]);
        // on the ring, partway along the left edge's band
        assert_eq!(at(3, 64), &[255, 255, 255]);
    }

    #[test]
    fn a_lazer_fade_spans_rows_of_a_short_body() {
        // a body 82 rows tall, as some skins have, stretched over lazer's whole span
        let src = Raw::new(vec![255u8; 256 * 82 * 4], 256, 82);
        let asset = Asset { width: None, height: None, source_height: None, scale: 1.0, image: None, frames: vec![], path: None };
        let fade = Fade { length: 150.0, from: 0.0, round: false };
        let (out, _) = body(&src, &asset, true, 100.0, None, Some(fade), None, true).unwrap();
        let rows = rows_per_unit(1.0, out.h as f64, true);
        let alpha = |units: f64| out.rgba[((units * rows) as usize * 256) * 4 + 3];
        assert!(alpha(0.0) < 10);
        assert!((90..170).contains(&alpha(75.0)), "{}", alpha(75.0));
        assert_eq!(alpha(300.0), 255);
    }

    // RIM_DIR=<folder> cargo test -p mania-bridge-helper --lib rims_in_a_folder -- --ignored --nocapture
    #[test]
    #[ignore]
    fn rims_in_a_folder() {
        let dir = std::env::var("RIM_DIR").expect("RIM_DIR");
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files.iter().filter(|f| f.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))) {
            let Some(raw) = load::raw(f) else { continue };
            let found = rim(&raw, true);
            println!("{:<28} {}x{} {}", f.file_name().unwrap().to_string_lossy(), raw.w, raw.h, found.as_ref().map_or("-".into(), |r| format!("rim {}px {:?} strength {:.2}", r.px, &r.color[..3], r.strength)));
            // RIM_OUT=<folder> also writes each outlined image coloured keeping it, and with it removed
            if let (Some(r), Ok(out)) = (found, std::env::var("RIM_OUT")) {
                let stem = f.file_stem().unwrap().to_string_lossy().to_string();
                let save = |raw: &Raw, tag: &str| {
                    image::RgbaImage::from_raw(raw.w, raw.h, raw.rgba.clone()).unwrap().save(Path::new(&out).join(format!("{stem}-{tag}.png"))).unwrap();
                };
                save(&tinted_keeping(&raw, [236, 30, 90, 255], &r), "keep");
                save(&tinted(&raw, [236, 30, 90, 255]).0, "plain");
                let mut gone = Raw::new(raw.rgba.clone(), raw.w, raw.h);
                unrim(&mut gone, &r, true);
                save(&gone, "removed");
            }
        }
    }
}
