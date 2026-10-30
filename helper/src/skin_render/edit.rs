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
    pub colors: Vec<Option<String>>,
    pub parts: Parts,
    // other images of the skin's own per column, as skin.ini would name them
    pub swaps: Vec<Option<Swap>>,
    // skin.ini's HitPosition, 0-480 from the top
    #[serde(rename = "hitPosition")]
    pub hit_position: Option<f64>,
    #[serde(rename = "judgementLine")]
    pub judgement_line: Option<bool>,
    // skin.ini's ColumnStart, the stage's left edge in 480ths of the screen's height
    #[serde(rename = "columnStart")]
    pub column_start: Option<f64>,
    // how far up the receptors' art moves in its images, in HitPosition's 480ths. The game keeps the
    // images on the stage's bottom edge whatever HitPosition says
    #[serde(rename = "keysRaise")]
    pub keys_raise: Option<f64>,
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

    fn color(&self, column: usize) -> Option<Rgba> {
        let hex = self.colors.get(column)?.as_deref()?.strip_prefix('#')?;
        let v = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6)?;
        Some([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255])
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
        }
    }
}

pub(super) const COLUMN_START_MAX: f64 = 853.0;

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
    if let Some(v) = edits.column_start {
        out.column_start = Some(v.clamp(0.0, COLUMN_START_MAX));
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
            };
            *slot = Some(asset);
            swapped.push((c, part, reference.to_string()));
        }
    }
    // the same art with the same edit in several columns is made once
    let mut done: HashMap<(PathBuf, String), (Asset, Option<Raw>)> = HashMap::new();
    for c in 0..keys {
        let tint = edits.color(c);
        let cut = edits.cut.get(c).copied().flatten().map(|u| u.max(0.0));
        let column = out.columns[c].clone();
        if let Some(asset) = column.body.as_ref().filter(|_| cut.is_some() || edits.fade.is_some() || (tint.is_some() && edits.parts.bodies)) {
            let tint = tint.filter(|_| edits.parts.bodies);
            let style = p.body_styles.get(c).copied().unwrap_or(1.0);
            let restyle = style == 0.0 && (cut.is_some() || edits.fade.is_some());
            let width = p.column_widths.get(c).copied().unwrap_or(p.column_width);
            let key = format!("body {cut:?} {:?} {tint:?} {} {width}", edits.fade.map(|f| (f.length, f.from, f.round)), restyle);
            if let Some((new, raw)) = redraw(asset, key, &mut done, sources, full, |src| body(src, asset, p.lazer_bodies, width, cut, edits.fade, tint, full)) {
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
        let Some(tint) = tint else { continue };
        for (part, on) in [(Part::Note, edits.parts.notes), (Part::Head, edits.parts.heads), (Part::Tail, edits.parts.tails)] {
            let slot = match part {
                Part::Note => &column.tap,
                Part::Head => &column.head,
                _ => &column.tail,
            };
            let Some(asset) = slot.as_ref().filter(|_| on) else { continue };
            let mut frames = vec![];
            let mut new = asset.clone();
            for (i, frame) in std::iter::once(asset).chain(&asset.frames).enumerate() {
                let key = format!("tint {tint:?}");
                let (drawn, raw) = redraw(frame, key, &mut done, sources, full, |src| Some(tinted(src, tint))).unwrap_or_else(|| (frame.clone(), None));
                frames.extend(raw);
                if i == 0 {
                    new = Asset { frames: vec![], ..drawn };
                } else {
                    new.frames.push(drawn);
                }
            }
            if full {
                made.push(Made { column: c, part, frames, scale: asset.scale });
            }
            let target = &mut out.columns[c];
            match part {
                Part::Note => target.tap = Some(new),
                Part::Head => target.head = Some(new),
                _ => target.tail = Some(new),
            }
        }
    }
    if let Some(up) = edits.keys_raise.filter(|r| r.abs() >= 0.5 && r.abs() <= 480.0) {
        for c in 0..keys {
            for part in [Part::Key, Part::KeyDown] {
                let column = out.columns[c].clone();
                let Some(asset) = (if part == Part::Key { column.key.as_ref() } else { column.key_down.as_ref() }) else { continue };
                // a row of art is a 768th of the stage, half of one at @2x
                let rows = (up / UNIT * asset.scale).round() as i64;
                let Some((new, raw)) = redraw(asset, format!("raise {rows}"), &mut done, sources, full, |src| Some(raised(src, rows))) else { continue };
                if let Some(raw) = raw {
                    made.push(Made { column: c, part, frames: vec![raw], scale: asset.scale });
                }
                if part == Part::Key {
                    out.columns[c].key = Some(new);
                } else {
                    out.columns[c].key_down = Some(new);
                }
            }
        }
    }
    (out, made, swapped)
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

// A Percy body: `cut` units of nothing at the tail end, then the skin's own art, fading in from the
// visible end. Art that isn't a strip already is tiled down far enough that a hold never repeats it.
#[allow(clippy::too_many_arguments)]
fn body(src: &Raw, asset: &Asset, lazer: bool, width: f64, cut: Option<f64>, fade: Option<Fade>, tint: Option<Rgba>, full: bool) -> Option<(Raw, Option<f64>)> {
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
