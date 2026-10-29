use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use image::ImageDecoder;
use moxcms::{Chromaticity, ColorPrimaries, ColorProfile, Layout, ToneReprCurve, TransformOptions, XyY};
use tiny_skia::{IntSize, Pixmap};

use super::canvas::Texture;
use super::ini::{self, clamp_int, color, color_alpha, hex, number_list, opt_number, Block, Rgba};

const MAX_KEYS: usize = 10;
const MAX_FRAMES: usize = 60;
// past this the GPU refuses the texture, and the TS this mirrors caps it before the renderer sees it
pub(super) const MAX_TEXTURE: u32 = 4096;
// stretched this far past square the art is a Percy-style LN body, cropped rather than scaled so
// its cap keeps its size
const STRIP_ASPECT: f64 = 8.0;
const LAZER_MARKERS: [&str; 4] = ["skininfo.json", "mainhudcomponents.json", "songselect.json", "playfield.json"];

#[derive(Clone)]
pub(super) struct Asset {
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub source_height: Option<f64>,
    pub scale: f64,
    pub image: Option<Arc<Texture>>,
    pub frames: Vec<Asset>,
    // the file, for edits that redraw it
    pub path: Option<PathBuf>,
}

impl Asset {
    pub fn native_width(&self, tex: &Texture) -> f64 {
        self.width.unwrap_or(tex.width()) / self.scale
    }

    pub fn native_height(&self, tex: &Texture) -> f64 {
        self.height.unwrap_or(tex.height()) / self.scale
    }
}

#[derive(Clone, Default)]
pub(super) struct Column {
    pub tap: Option<Asset>,
    pub head: Option<Asset>,
    pub body: Option<Asset>,
    pub tail: Option<Asset>,
    pub key: Option<Asset>,
    pub key_down: Option<Asset>,
}

#[derive(Clone, Default)]
pub(super) struct Stage {
    pub left: Option<Asset>,
    pub right: Option<Asset>,
    pub bottom: Option<Asset>,
    pub hint: Option<Asset>,
    pub light: Option<Asset>,
    pub lighting: Option<Asset>,
    pub lighting_widths: Vec<f64>,
    pub light_colors: Vec<Option<Rgba>>,
}

#[derive(Clone)]
pub(super) struct Combo {
    pub overlap: f64,
    pub digits: Vec<Option<Asset>>,
}

#[derive(Clone)]
pub(super) struct Profile {
    pub tap_color: Rgba,
    pub tap_colors: Vec<Option<Rgba>>,
    pub head_color: Rgba,
    pub head_colors: Vec<Option<Rgba>>,
    pub column_width: f64,
    pub column_spacing: f64,
    pub column_widths: Vec<f64>,
    pub line_widths: Vec<f64>,
    pub line_color: Option<Rgba>,
    pub backgrounds: Vec<Option<Rgba>>,
    pub judgement_line: bool,
    pub judgement_line_color: Option<Rgba>,
    pub column_start: Option<f64>,
    pub light_position: Option<f64>,
    pub hit_position: Option<f64>,
    pub score_position: Option<f64>,
    pub combo_position: Option<f64>,
    pub combo_hidden: bool,
    pub keys_under_notes: bool,
    pub body_styles: Vec<f64>,
    pub lazer_bodies: bool,
    pub combo_scale: f64,
    pub note_height_scale: f64,
    pub columns: Vec<Column>,
    pub judgement: Option<Asset>,
    pub combo: Option<Combo>,
    pub stage: Stage,
}

impl Profile {
    fn base(tap_color: Rgba, head_color: Rgba, width: f64, spacing: f64, note_height_scale: f64) -> Self {
        Profile {
            tap_color,
            tap_colors: vec![],
            head_color,
            head_colors: vec![],
            column_width: width,
            column_spacing: spacing,
            column_widths: vec![],
            line_widths: vec![],
            line_color: None,
            backgrounds: vec![],
            judgement_line: true,
            judgement_line_color: None,
            column_start: None,
            light_position: None,
            hit_position: None,
            score_position: None,
            combo_position: None,
            combo_hidden: false,
            keys_under_notes: false,
            body_styles: vec![],
            lazer_bodies: false,
            combo_scale: 1.0,
            note_height_scale,
            columns: vec![],
            judgement: None,
            combo: None,
            stage: Stage::default(),
        }
    }

    // mania-hub's replay viewer ships its own 4K layout, which is what a skin without any 4K art
    // falls back to
    fn default_4k() -> Self {
        let mut p = Profile::base(hex("#9cf2ae"), hex("#dfffe6"), 75.0, 2.0, 50.0);
        p.head_colors = vec![Some(hex("#e3a5de")); 5];
        p
    }
}

pub(super) struct Settings {
    pub upscroll: bool,
    pub hit_position: f64,
    pub score_position: f64,
    pub combo_position: f64,
    pub ln_body_color: Rgba,
}

pub(super) struct Skin {
    ini: Option<PathBuf>,
    files: HashMap<String, PathBuf>,
    // walk order, for the lookups that take the first match
    order: Vec<String>,
    cache: Mutex<HashMap<String, Option<Arc<Loaded>>>>,
}

struct Loaded {
    image: Option<Arc<Texture>>,
    width: Option<f64>,
    height: Option<f64>,
    source_height: Option<f64>,
    path: PathBuf,
}

fn normalize(path: &str) -> String {
    let mut s = path.replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.trim_start_matches('/').to_string();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '/' && out.ends_with('/') {
            continue;
        }
        out.push(c);
    }
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if depth < 8 {
                walk(root, &path, out, depth + 1);
            }
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.push((normalize(&rel.to_string_lossy()).to_lowercase(), path));
            }
        }
    }
}

impl Skin {
    pub fn open(dir: &Path) -> Self {
        let mut found = vec![];
        walk(dir, dir, &mut found, 0);
        found.sort_by(|a, b| a.0.cmp(&b.0));
        let mut files = HashMap::new();
        let mut order = vec![];
        for (rel, path) in &found {
            if files.insert(rel.clone(), path.clone()).is_none() {
                order.push(rel.clone());
            }
        }
        // the game reads the shallowest skin.ini, and references resolve from its folder
        let ini = found
            .iter()
            .filter(|(rel, _)| rel == "skin.ini" || rel.ends_with("/skin.ini"))
            .min_by_key(|(rel, _)| rel.matches('/').count());
        if let Some((rel, _)) = ini {
            let root = &rel[..rel.len() - "skin.ini".len()];
            if !root.is_empty() {
                for (rel, path) in &found {
                    if let Some(short) = rel.strip_prefix(root) {
                        if !files.contains_key(short) {
                            files.insert(short.to_string(), path.clone());
                            order.push(short.to_string());
                        }
                    }
                }
            }
        }
        let ini = ini.map(|(_, path)| path.clone());
        Skin { ini, files, order, cache: Mutex::default() }
    }

    // where skin.ini's references start from
    pub fn root(&self) -> Option<&Path> {
        self.ini.as_deref()?.parent()
    }

    // every file under the root once, by its path from there in its own case
    pub fn files_under_root(&self) -> Vec<(String, &PathBuf)> {
        let Some(root) = self.root() else { return vec![] };
        let mut seen = HashSet::new();
        let mut out = vec![];
        for path in self.order.iter().map(|r| &self.files[r]) {
            if let Ok(rel) = path.strip_prefix(root) {
                if seen.insert(path) {
                    out.push((rel.to_string_lossy().replace('\\', "/"), path));
                }
            }
        }
        out
    }

    // what a part pointed at `reference` shows, resolved the way skin.ini's own lines are
    pub fn swap(&self, reference: &str, animated: bool) -> Option<Asset> {
        if animated {
            self.frames_of(reference).or_else(|| self.resolve_str(reference))
        } else {
            self.resolve_str(reference).or_else(|| self.resolve_str(&format!("{reference}-0")))
        }
    }

    pub(super) fn ini(&self) -> Option<String> {
        let bytes = std::fs::read(self.ini.as_ref()?).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn find_named(&self, name: &str) -> Option<&PathBuf> {
        self.files.get(name).or_else(|| {
            let suffix = format!("/{name}");
            self.order.iter().find(|r| r.ends_with(&suffix)).map(|r| &self.files[r])
        })
    }

    fn is_lazer(&self, ini: &str) -> bool {
        self.order.iter().any(|r| LAZER_MARKERS.contains(&r.rsplit('/').next().unwrap_or(r))) || ini.contains("automatically added by osu!")
    }

    // @2x first, then the plain name, over png, jpg and jpeg unless the reference names its own
    fn resolve(&self, reference: Option<&str>) -> Option<Asset> {
        let reference = reference?.trim();
        let reference = reference.strip_prefix(['"', '\'']).unwrap_or(reference);
        let reference = reference.strip_suffix(['"', '\'']).unwrap_or(reference);
        let clean = normalize(reference);
        if clean.is_empty() || clean.contains("..") {
            return None;
        }
        let ext = clean.rfind('.').map(|i| &clean[i + 1..]).filter(|e| !e.is_empty() && e.bytes().all(|c| c.is_ascii_alphanumeric()));
        let (base, exts) = match ext {
            Some(e) => (&clean[..clean.len() - e.len() - 1], vec![e.to_ascii_lowercase()]),
            None => (clean.as_str(), vec!["png".into(), "jpg".into(), "jpeg".into()]),
        };
        let scaled = base.to_ascii_lowercase().ends_with("@2x");
        let mut candidates = vec![];
        for e in &exts {
            if !scaled {
                candidates.push((format!("{base}@2x.{e}"), 2.0));
            }
            candidates.push((format!("{base}.{e}"), if scaled { 2.0 } else { 1.0 }));
        }
        let (key, scale) = candidates.into_iter().map(|(p, s)| (normalize(&p).to_lowercase(), s)).find(|(p, _)| self.files.contains_key(p))?;
        let path = &self.files[&key];
        let cache_key = path.to_string_lossy().to_lowercase();
        let loaded = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&cache_key).cloned();
        let loaded = match loaded {
            Some(l) => l,
            None => {
                let l = load(path).map(Arc::new);
                self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(cache_key, l.clone());
                l
            }
        }?;
        Some(Asset {
            width: loaded.width,
            height: loaded.height,
            source_height: loaded.source_height,
            scale,
            image: loaded.image.clone(),
            frames: vec![],
            path: Some(loaded.path.clone()),
        })
    }

    fn resolve_str(&self, reference: &str) -> Option<Asset> {
        self.resolve(Some(reference))
    }

    // reference, its first animation frame, then stable's default name and its first frame
    fn with_defaults(&self, reference: Option<&str>, fallback: &str) -> Option<Asset> {
        self.resolve(reference)
            .or_else(|| reference.and_then(|r| self.resolve_str(&format!("{r}-0"))))
            .or_else(|| self.resolve_str(fallback))
            .or_else(|| self.resolve_str(&format!("{fallback}-0")))
    }

    // note art animates from "-N" frames, which outrank the static image
    fn animated(&self, reference: Option<&str>, fallback: &str) -> Option<Asset> {
        for base in reference.into_iter().chain([fallback]) {
            if let Some(first) = self.frames_of(base) {
                return Some(first);
            }
            if let Some(single) = self.resolve_str(base) {
                return Some(single);
            }
        }
        None
    }

    fn frames_of(&self, base: &str) -> Option<Asset> {
        let mut first = self.resolve_str(&format!("{base}-0"))?;
        for i in 1..MAX_FRAMES {
            let Some(frame) = self.resolve_str(&format!("{base}-{i}")) else { break };
            first.frames.push(frame);
        }
        Some(first)
    }
}

// straight RGBA as decoded and colour-converted, before the texture cap
pub(super) struct Raw {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
    // a TIFF strip is scaled nearest-neighbour by the TS this mirrors
    nearest: bool,
}

impl Raw {
    pub fn new(rgba: Vec<u8>, w: u32, h: u32) -> Self {
        Raw { rgba, w, h, nearest: false }
    }
}

// None for a missing or empty file, Some(None) for one that doesn't decode
fn read(path: &Path) -> Option<Option<Raw>> {
    let bytes = std::fs::read(path).ok().filter(|b| !b.is_empty())?;
    // skins ship Photoshop TIFFs renamed to .png, Percy LN bodies especially, and stable decodes by
    // content
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some(tiff(&bytes).map(|(rgba, w, h)| Raw { rgba, w, h, nearest: true }));
    }
    Some(decode(&bytes).map(|(rgba, w, h)| Raw::new(rgba, w, h)))
}

pub(super) fn raw(path: &Path) -> Option<Raw> {
    read(path).flatten()
}

fn load(path: &Path) -> Option<Loaded> {
    let empty = Loaded { image: None, width: None, height: None, source_height: None, path: path.to_path_buf() };
    let Some(raw) = read(path)? else { return Some(empty) };
    let (image, width, height, source_height) = texture(raw)?;
    Some(Loaded { image: Some(image), width: Some(width), height: Some(height), source_height, path: path.to_path_buf() })
}

// what the game gets for an image: capped to the texture limit, a strip cropped rather than scaled,
// as a premultiplied texture; with the width and height the asset reports and the strip's full height
pub(super) fn texture(raw: Raw) -> Option<(Arc<Texture>, f64, f64, Option<f64>)> {
    let Raw { rgba, w, h, nearest } = raw;
    let (mut width, mut height, mut source_height) = (w, h, None);
    let (rgba, tw, th) = if w > MAX_TEXTURE || h > MAX_TEXTURE {
        let (cw, ch) = (w.min(MAX_TEXTURE), h.min(MAX_TEXTURE));
        if is_strip(w, h) {
            if ch < h {
                source_height = Some(h as f64);
            }
            (width, height) = (cw, ch);
            let mut out = Vec::with_capacity(cw as usize * ch as usize * 4);
            for row in rgba.chunks_exact(w as usize * 4).take(ch as usize) {
                out.extend_from_slice(&row[..cw as usize * 4]);
            }
            (out, cw, ch)
        } else if nearest {
            let mut out = Vec::with_capacity(cw as usize * ch as usize * 4);
            for y in 0..ch as u64 {
                let sy = ((y * h as u64) / ch as u64).min(h as u64 - 1);
                for x in 0..cw as u64 {
                    let sx = ((x * w as u64) / cw as u64).min(w as u64 - 1);
                    let i = ((sy * w as u64 + sx) * 4) as usize;
                    out.extend_from_slice(&rgba[i..i + 4]);
                }
            }
            (out, cw, ch)
        } else {
            let img = image::RgbaImage::from_raw(w, h, rgba)?;
            let small = image::imageops::resize(&img, cw, ch, image::imageops::FilterType::Triangle);
            (small.into_raw(), cw, ch)
        }
    } else {
        (rgba, w, h)
    };
    let mut data = rgba;
    for p in data.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    let pixmap = Pixmap::from_vec(data, IntSize::from_wh(tw, th)?)?;
    Some((Arc::new(Texture::new(pixmap)), width.min(MAX_TEXTURE) as f64, height.min(MAX_TEXTURE) as f64, source_height))
}

pub(super) fn is_strip(w: u32, h: u32) -> bool {
    w.max(h) as f64 / w.min(h).max(1) as f64 >= STRIP_ASPECT
}

fn decode(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_decoder().ok()?;
    let icc = decoder.icc_profile().ok().flatten();
    let img = image::DynamicImage::from_decoder(decoder).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    let mut rgba = img.into_raw();
    let profile = match icc {
        Some(icc) => ColorProfile::new_from_slice(&icc).ok(),
        None => png_colorimetry(bytes),
    };
    if let Some(profile) = profile {
        to_srgb(&profile, &mut rgba);
    }
    Some((rgba, w, h))
}

// Chrome reads a PNG without iCCP as sRGB unless it has both cHRM and gAMA and no sRGB chunk
fn png_colorimetry(bytes: &[u8]) -> Option<ColorProfile> {
    let mut at = bytes.starts_with(b"\x89PNG\r\n\x1a\n").then_some(8)?;
    let (mut chrm, mut gama) = (None, None);
    while let Some(head) = bytes.get(at..at + 8) {
        let len = u32::from_be_bytes(head[..4].try_into().ok()?) as usize;
        let data = bytes.get(at + 8..at + 8 + len)?;
        let fixed = |i: usize| u32::from_be_bytes(data[i * 4..i * 4 + 4].try_into().unwrap()) as f64 / 100_000.0;
        match &head[4..] {
            b"IDAT" => break,
            b"sRGB" => return None,
            b"cHRM" if len == 32 => chrm = Some([0, 1, 2, 3, 4, 5, 6, 7].map(fixed)),
            b"gAMA" if len == 4 => gama = Some(fixed(0)).filter(|g| *g > 0.0),
            _ => {}
        }
        at += 12 + len;
    }
    let ([wx, wy, rx, ry, gx, gy, bx, by], gamma) = (chrm?, gama?);
    let xy = |x: f64, y: f64| Chromaticity { x: x as f32, y: y as f32 };
    let mut profile = ColorProfile::new_srgb();
    profile.update_rgb_colorimetry(XyY { x: wx, y: wy, yb: 1.0 }, ColorPrimaries { red: xy(rx, ry), green: xy(gx, gy), blue: xy(bx, by) });
    let curve = ToneReprCurve::Parametric(vec![(1.0 / gamma) as f32]);
    (profile.red_trc, profile.green_trc, profile.blue_trc) = (Some(curve.clone()), Some(curve.clone()), Some(curve));
    Some(profile)
}

fn to_srgb(profile: &ColorProfile, rgba: &mut [u8]) {
    let srgb = ColorProfile::new_srgb();
    let Ok(transform) = profile.create_transform_8bit(Layout::Rgba, &srgb, Layout::Rgba, TransformOptions::default()) else { return };
    // an sRGB profile under another name changes nothing in Chrome, so don't let rounding change it here
    let ramp: Vec<u8> = (0..=255u8).flat_map(|v| [v, v.wrapping_mul(7), v.wrapping_mul(13), 255]).collect();
    let mut out = ramp.clone();
    if transform.transform(&ramp, &mut out).is_err() || out == ramp {
        return;
    }
    let src = rgba.to_vec();
    let _ = transform.transform(&src, rgba);
}

// 8-bit RGB/RGBA, chunky, uncompressed strips: what Photoshop writes by default
pub(super) fn tiff(b: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let le = b.starts_with(b"II");
    let u16_at = |o: usize| -> Option<u32> {
        let s: [u8; 2] = b.get(o..o + 2)?.try_into().ok()?;
        Some(if le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) } as u32)
    };
    let u32_at = |o: usize| -> Option<u32> {
        let s: [u8; 4] = b.get(o..o + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    let mut tags: HashMap<u32, Vec<u32>> = HashMap::new();
    for i in 0..count {
        let entry = ifd + 2 + i * 12;
        if entry + 12 > b.len() {
            return None;
        }
        let kind = u16_at(entry + 2)?;
        let n = u32_at(entry + 4)? as usize;
        let size = match kind {
            1 => 1,
            3 => 2,
            4 => 4,
            _ => continue,
        };
        if n == 0 || n > 65536 {
            continue;
        }
        let at = if size * n <= 4 { entry + 8 } else { u32_at(entry + 8)? as usize };
        if at + size * n > b.len() {
            continue;
        }
        let values = (0..n)
            .map(|j| match size {
                1 => b[at + j] as u32,
                2 => u16_at(at + j * 2).unwrap_or(0),
                _ => u32_at(at + j * 4).unwrap_or(0),
            })
            .collect();
        tags.insert(u16_at(entry)?, values);
    }
    let first = |t: u32| tags.get(&t).and_then(|v| v.first().copied());
    let (w, h) = (first(256).unwrap_or(0) as usize, first(257).unwrap_or(0) as usize);
    let bits = tags.get(&258).cloned().unwrap_or_default();
    let samples = first(277).map_or(bits.len(), |s| s as usize);
    let premultiplied = first(338) == Some(1);
    let offsets = tags.get(&273).cloned().unwrap_or_default();
    let counts = tags.get(&279).cloned().unwrap_or_default();
    let rows_per_strip = first(278).map_or(h, |r| r as usize);
    if first(259).unwrap_or(1) != 1 || first(262).unwrap_or(2) != 2 || first(284).unwrap_or(1) != 1 {
        return None;
    }
    if !(samples == 3 || samples == 4) || bits.iter().any(|&b| b != 8) {
        return None;
    }
    if w == 0 || h == 0 || w > 65000 || h > 65000 || w * h > 50_000_000 || offsets.is_empty() || offsets.len() != counts.len() {
        return None;
    }
    let mut rgba = vec![0u8; w * h * 4];
    let row_bytes = w * samples;
    let mut row = 0;
    for (&start, &len) in offsets.iter().zip(&counts) {
        if row >= h {
            break;
        }
        let start = start as usize;
        let available = (len as usize).min(b.len().saturating_sub(start));
        let rows = (available / row_bytes).min(rows_per_strip).min(h - row);
        if rows == 0 {
            return None;
        }
        for r in 0..rows {
            let src = &b[start + r * row_bytes..][..row_bytes];
            let dst = &mut rgba[(row + r) * w * 4..][..w * 4];
            for (px, out) in src.chunks_exact(samples).zip(dst.chunks_exact_mut(4)) {
                out[..3].copy_from_slice(&px[..3]);
                out[3] = if samples == 4 { px[3] } else { 255 };
            }
        }
        row += rows;
    }
    if row < h {
        return None;
    }
    if premultiplied && samples == 4 {
        for p in rgba.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
    Some((rgba, w as u32, h as u32))
}

// stable's default column layout, 1 and 2 mirrored from the edges in with S in the middle of odd
// keymodes: 4K is 1,2,2,1 and 7K is 1,2,1,S,1,2,1
fn default_suffix(keys: usize, col: usize) -> &'static str {
    if keys % 2 == 1 && col == keys / 2 {
        return "S";
    }
    let mirrored = if (col as f64) < keys as f64 / 2.0 { col } else { keys - 1 - col };
    if mirrored % 2 == 0 {
        "1"
    } else {
        "2"
    }
}

// skin.ini positions are 480ths of the stage from the top; the TS keeps them as 768ths from the
// bottom. Anything off the stage means "not here" rather than the nearest edge.
fn stage_position(raw: Option<&String>) -> Option<f64> {
    let v = opt_number(raw)?;
    if !(0.0..=480.0).contains(&v) {
        return None;
    }
    Some(ini::round((480.0 - ini::round(v)) * 1.6))
}

fn get<'a>(block: &'a Block, key: &str) -> Option<&'a str> {
    block.get(key).map(String::as_str).filter(|s| !s.is_empty())
}

fn opaque(c: Option<Rgba>) -> Option<Rgba> {
    c.filter(|c| c[3] == 255)
}

fn build(skin: &Skin, block: &Block, fonts: &HashMap<String, String>, base: &Profile, keys: usize) -> (Profile, usize) {
    let widths = number_list(block.get("ColumnWidth"));
    let spacings = number_list(block.get("ColumnSpacing"));
    let line_widths = match block.get("ColumnLineWidth") {
        Some(raw) => {
            let parsed = number_list(Some(raw));
            (0..=keys).map(|i| clamp_int(parsed.get(i).copied().unwrap_or(2.0), 0.0, 20.0)).collect()
        }
        None => vec![],
    };
    let block_style = opt_number(block.get("NoteBodyStyle"));
    let column_styles: Vec<Option<f64>> = (0..keys).map(|c| opt_number(block.get(&format!("NoteBodyStyle{c}")))).collect();
    let body_styles = if block_style.is_none() && column_styles.iter().all(Option::is_none) {
        vec![]
    } else {
        column_styles.iter().map(|s| clamp_int(s.or(block_style).unwrap_or(1.0), 0.0, 2.0)).collect()
    };
    let column_width = clamp_int(
        widths.first().copied().or_else(|| opt_number(block.get("ColumnWidth"))).unwrap_or(base.column_width),
        20.0,
        160.0,
    );
    let column_spacing = clamp_int(
        spacings.first().copied().or_else(|| opt_number(block.get("ColumnSpacing"))).unwrap_or(base.column_spacing),
        0.0,
        40.0,
    );
    let smallest = widths.iter().copied().reduce(f64::min);
    let note_height_scale =
        clamp_int(opt_number(block.get("WidthForNoteHeightScale")).or(smallest).unwrap_or(column_width), 10.0, 200.0);
    let tap_colors: Vec<Option<Rgba>> = (0..keys).map(|i| base.tap_colors.get(i).copied().flatten()).collect();
    let head_colors: Vec<Option<Rgba>> = (0..keys).map(|i| base.head_colors.get(i).copied().flatten()).collect();

    let mut columns = vec![];
    let mut found = 0;
    for col in 0..keys {
        let s = default_suffix(keys, col);
        let r = |k: String| get(block, &k);
        let column = Column {
            tap: skin.animated(r(format!("NoteImage{col}")), &format!("mania-note{s}")),
            head: skin.animated(r(format!("NoteImage{col}H")), &format!("mania-note{s}H")),
            body: skin.with_defaults(r(format!("NoteImage{col}L")), &format!("mania-note{s}L")),
            tail: skin.animated(r(format!("NoteImage{col}T")), &format!("mania-note{s}T")),
            key: skin.with_defaults(r(format!("KeyImage{col}")), &format!("mania-key{s}")),
            key_down: skin.with_defaults(r(format!("KeyImage{col}D")), &format!("mania-key{s}D")),
        };
        found += [&column.tap, &column.head, &column.body, &column.tail, &column.key, &column.key_down]
            .iter()
            .filter(|a| a.is_some())
            .count();
        columns.push(column);
    }

    // the max judgement as the skin defines it; hit300 only stands in when there is no 300g file
    let judgement = ["Hit300g", "Hit300"].iter().find_map(|key| {
        let reference = get(block, key);
        let name = format!("mania-hit{}", &key[3..]);
        skin.resolve(reference)
            .or_else(|| reference.and_then(|r| skin.resolve_str(&format!("{r}-0"))))
            .or_else(|| skin.resolve_str(&format!("{name}-0")))
            .or_else(|| skin.resolve_str(&name))
    });

    let stage = Stage {
        left: skin.with_defaults(get(block, "StageLeft"), "mania-stage-left"),
        right: skin.with_defaults(get(block, "StageRight"), "mania-stage-right"),
        bottom: skin.with_defaults(get(block, "StageBottom"), "mania-stage-bottom"),
        hint: skin.with_defaults(get(block, "StageHint"), "mania-stage-hint"),
        light: skin.with_defaults(get(block, "StageLight"), "mania-stage-light"),
        lighting: skin
            .with_defaults(get(block, "LightingN"), "lightingN")
            .or_else(|| skin.with_defaults(get(block, "LightingL"), "lightingL")),
        lighting_widths: number_list(block.get("LightingNWidth")).into_iter().take(keys).map(|w| w.clamp(1.0, 400.0)).collect(),
        light_colors: (0..keys).map(|i| color(block.get(&format!("ColourLight{}", i + 1)))).collect(),
    };

    // stable's ComboPrefix defaults to "score", and a FontCombo pointing at digits that aren't
    // there falls through to the next font rather than leaving the counter blank
    let mut prefixes: Vec<&str> = vec![];
    for p in [get(block, "FontCombo"), fonts.get("ComboPrefix").map(String::as_str).filter(|s| !s.is_empty()), Some("score")]
        .into_iter()
        .flatten()
    {
        if !prefixes.contains(&p) {
            prefixes.push(p);
        }
    }
    let combo = prefixes.iter().find_map(|p| {
        let digits: Vec<Option<Asset>> = (0..10).map(|d| skin.resolve_str(&format!("{p}-{d}"))).collect();
        let x = skin.resolve_str(&format!("{p}-x"));
        (digits.iter().any(Option::is_some) || x.is_some()).then(|| Combo {
            overlap: clamp_int(opt_number(fonts.get("ComboOverlap")).unwrap_or(0.0), -80.0, 80.0),
            digits,
        })
    });

    let profile = Profile {
        tap_color: tap_colors.iter().flatten().next().copied().unwrap_or(base.tap_color),
        tap_colors,
        head_color: head_colors.iter().flatten().next().copied().unwrap_or(base.head_color),
        head_colors,
        column_width,
        column_spacing,
        column_widths: widths.iter().take(MAX_KEYS).map(|w| w.clamp(20.0, 160.0)).collect(),
        line_widths,
        line_color: color_alpha(block.get("ColourColumnLine")),
        // the TS only keeps an opaque Colour{n}, a translucent one leaves the default black
        backgrounds: (0..keys).map(|i| opaque(color_alpha(block.get(&format!("Colour{}", i + 1))))).collect(),
        judgement_line: opt_number(block.get("JudgementLine")).is_none_or(|v| v != 0.0),
        judgement_line_color: color_alpha(block.get("ColourJudgementLine")),
        column_start: opt_number(block.get("ColumnStart")).map(|v| v.clamp(0.0, 853.0)),
        light_position: opt_number(block.get("LightPosition")).map(|v| v.clamp(0.0, 480.0)),
        hit_position: stage_position(block.get("HitPosition")),
        score_position: stage_position(block.get("ScorePosition")),
        combo_position: stage_position(block.get("ComboPosition")),
        combo_hidden: opt_number(block.get("ComboPosition")).is_some_and(|v| v > 480.0),
        keys_under_notes: opt_number(block.get("KeysUnderNotes")) == Some(1.0),
        body_styles,
        lazer_bodies: false,
        combo_scale: base.combo_scale,
        note_height_scale,
        columns,
        judgement,
        combo,
        stage,
    };
    (profile, found)
}

struct HudCombo {
    scale: Option<f64>,
    position: Option<f64>,
}

// lazer keeps HUD editor tweaks in MainHUDComponents.json, which stable ignores
fn hud_combo(skin: &Skin) -> Option<HudCombo> {
    let bytes = std::fs::read(skin.find_named("mainhudcomponents.json")?).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let json: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let entry = json.pointer("/DrawableInfo/mania")?.as_array()?.iter().find(|item| {
        item.get("Type").and_then(|t| t.as_str()).is_some_and(|t| t.contains("LegacyManiaComboCounter"))
    })?;
    let scale = entry.get("Scale").and_then(|s| s.get("y").and_then(|v| v.as_f64()).or_else(|| s.get("x").and_then(|v| v.as_f64())));
    let scale = scale.filter(|s| s.is_finite() && *s > 0.0 && *s != 1.0).map(|s| s.clamp(0.1, 4.0));
    let anchor = entry.get("Anchor").and_then(|a| a.as_f64()).unwrap_or(0.0) as i64;
    let position = entry.pointer("/Position/y").and_then(|v| v.as_f64()).and_then(|y| {
        // osu!framework anchors: 1 top, 2 vertical centre, 4 bottom
        let origin = if anchor & 4 != 0 { 768.0 } else if anchor & 2 != 0 { 384.0 } else { 0.0 };
        let from_bottom = 768.0 - (origin + y);
        (from_bottom > 0.0 && from_bottom < 768.0).then(|| ini::round(from_bottom))
    });
    (scale.is_some() || position.is_some()).then_some(HudCombo { scale, position })
}

pub(super) fn ini_path(dir: &Path) -> Option<PathBuf> {
    Skin::open(dir).ini
}

pub(super) fn import(dir: &Path, keys: usize) -> Result<(Settings, Profile), String> {
    import_from(&Skin::open(dir), keys)
}

pub(super) fn import_from(skin: &Skin, keys: usize) -> Result<(Settings, Profile), String> {
    let text = skin.ini().ok_or("No skin.ini was found in this skin.")?;
    let parsed = ini::parse(&text);
    let mut seen = HashSet::new();
    let blocks: Vec<(usize, &Block)> = parsed
        .mania
        .iter()
        .filter_map(|b| {
            let k = ini::round(opt_number(b.get("Keys"))?);
            (1.0..=MAX_KEYS as f64).contains(&k).then_some((k as usize, b))
        })
        .filter(|(k, _)| seen.insert(*k))
        .collect();
    if blocks.is_empty() {
        return Err("No [Mania] section with a valid Keys value was found.".into());
    }
    let selected = blocks.iter().find(|(k, _)| *k == keys).unwrap_or(&blocks[0]);

    let default_base = |k: usize| if k == 4 { Profile::default_4k() } else { Profile::base(hex("#9cf2ae"), hex("#dfffe6"), 50.0, 0.0, 50.0) };
    let declared = |k: usize| blocks.iter().find(|(bk, _)| *bk == k).map(|(_, b)| build(&skin, b, &parsed.fonts, &default_base(k), k).0);
    // stable plays undeclared keymodes from the default filenames, so a 4K-only skin still has
    // its art on 7K
    let mut kept = true;
    let mut profile = match declared(keys) {
        Some(p) => p,
        None => {
            let (p, found) = build(&skin, &Block::new(), &parsed.fonts, &default_base(keys), keys);
            kept = found > 0 || keys == 4;
            if found > 0 {
                p
            } else if keys == 4 {
                Profile::default_4k()
            } else {
                // mania-hub falls back to the selected block's layout and colours, with no art
                let s = declared(selected.0).expect("the selected block is declared");
                let mut p = Profile::base(s.tap_color, s.head_color, s.column_width, s.column_spacing, s.note_height_scale);
                (p.tap_colors, p.head_colors) = (s.tap_colors, s.head_colors);
                p
            }
        }
    };
    let hud = hud_combo(&skin);
    if kept {
        if let Some(scale) = hud.as_ref().and_then(|h| h.scale) {
            profile.combo_scale = scale;
        }
        profile.lazer_bodies = skin.is_lazer(&text);
    }

    let block = selected.1;
    let settings = Settings {
        upscroll: block.get("UpsideDown").is_some_and(|v| v == "1"),
        hit_position: stage_position(block.get("HitPosition")).unwrap_or(48.0),
        score_position: stage_position(block.get("ScorePosition")).unwrap_or(438.0),
        combo_position: hud.and_then(|h| h.position).or_else(|| stage_position(block.get("ComboPosition"))).unwrap_or(485.0),
        ln_body_color: color(block.get("ColourHold")).unwrap_or(hex("#8b8b93")),
    };
    Ok((settings, profile))
}
