use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::edit;
use super::load::{self, Asset, Skin};

// subfolders a skin keeps alternative art in, deeper than this is something else
const MAX_DEPTH: usize = 3;
const MAX_IMAGES: usize = 1500;
const THUMB: u32 = 96;

// elements of the menus and other modes, which nothing in a [Mania] block points at
const NOT_MANIA: [&str; 26] = [
    "menu-", "ranking-", "score-", "selection-", "pause-", "cursor", "mode-", "button-", "play-", "scorebar-", "mania-hit", "inputoverlay-",
    "spinner-", "slider", "approachcircle", "hitcircle", "followpoint", "reversearrow", "fruit-", "taiko-", "default-", "section-", "arrow-",
    "particle", "multi-", "comboburst",
];
const NOT_MANIA_NAMES: [&str; 15] =
    ["hit0", "hit50", "hit100", "hit100k", "hit300", "hit300g", "hit300k", "go", "ready", "count1", "count2", "count3", "star", "star2", "welcome_text"];

struct Library {
    skin: Skin,
    thumbs: Mutex<HashMap<String, Option<Vec<u8>>>>,
}

// the page lists the images and then asks for thumbnails, one skin's worth is kept for those
static LIBRARY: Mutex<Option<(String, Arc<Library>)>> = Mutex::new(None);

fn library(dir: &Path, at: &str) -> Arc<Library> {
    let key = format!("{}\n{at}", dir.display());
    let mut held = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, lib)) = held.as_ref().filter(|(k, _)| *k == key) {
        return lib.clone();
    }
    let lib = Arc::new(Library { skin: Skin::open(dir), thumbs: Mutex::default() });
    *held = Some((key, lib.clone()));
    lib
}

fn strip_2x(stem: &str) -> (&str, bool) {
    match stem.len().checked_sub(3).filter(|&i| stem.is_char_boundary(i) && stem[i..].eq_ignore_ascii_case("@2x")) {
        Some(i) => (&stem[..i], true),
        None => (stem, false),
    }
}

// "name-3" as ("name", 3)
fn frame(stem: &str) -> Option<(&str, usize)> {
    let (base, n) = stem.rsplit_once('-')?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((base, n.parse().ok()?))
}

// how skin.ini names an asset's image: from skin.ini's folder, no extension, @2x or first frame
pub(super) fn name_of(skin: &Skin, asset: &Asset) -> Option<String> {
    let rel = asset.path.as_ref()?.strip_prefix(skin.root()?).ok()?.to_string_lossy().replace('\\', "/");
    let stem = rel.rsplit_once('.').map_or(rel.as_str(), |(s, _)| s);
    let stem = strip_2x(stem).0;
    let stem = frame(stem).filter(|(_, n)| *n == 0).map_or(stem, |(b, _)| b);
    Some(stem.replace('/', "\\"))
}

fn size(path: &Path) -> Option<(u32, u32)> {
    image::image_dimensions(path).ok().or_else(|| load::raw(path).map(|r| (r.w, r.h)))
}

// The skin's images as skin.ini would name them, with an animation's frames under one name
pub(crate) fn images(dir: &Path, at: &str) -> Result<Value, String> {
    let lib = library(dir, at);
    if lib.skin.root().is_none() {
        return Err("This skin has no skin.ini.".into());
    }
    // lowercase stem -> (stem as written, has @2x, the file the game would pick)
    let mut stems: HashMap<String, (String, bool, PathBuf)> = HashMap::new();
    for (rel, path) in lib.skin.files_under_root() {
        if rel.matches('/').count() > MAX_DEPTH {
            continue;
        }
        let Some((stem, ext)) = rel.rsplit_once('.') else { continue };
        if !["png", "jpg", "jpeg"].contains(&ext.to_ascii_lowercase().as_str()) {
            continue;
        }
        let (stem, hd) = strip_2x(stem);
        let file = stem.rsplit('/').next().unwrap_or(stem).to_ascii_lowercase();
        let base = frame(&file).map_or(file.as_str(), |(b, _)| b);
        if NOT_MANIA.iter().any(|p| file.starts_with(p)) || NOT_MANIA_NAMES.contains(&base) {
            continue;
        }
        let entry = stems.entry(stem.to_lowercase()).or_insert_with(|| (stem.to_string(), hd, path.clone()));
        if hd {
            *entry = (stem.to_string(), true, path.clone());
        }
    }
    let used = used_as(&lib.skin);
    let mut out = vec![];
    for (lower, (stem, hd, path)) in &stems {
        let animation = frame(lower).filter(|(b, _)| stems.contains_key(&format!("{b}-0")));
        let (name, frames) = match animation {
            Some((_, n)) if n != 0 => continue,
            Some((b, _)) => (frame(stem).map_or(stem.as_str(), |(s, _)| s), (0..).take_while(|i| stems.contains_key(&format!("{b}-{i}"))).count()),
            // the still image of something that also animates is shown as the animation
            None if stems.contains_key(&format!("{lower}-0")) => continue,
            None => (stem.as_str(), 1),
        };
        let (w, h) = size(path).unwrap_or((0, 0));
        let folder = name.rsplit_once('/').map_or("", |(f, _)| f);
        let roles = roles(name, w, h, &used);
        out.push(json!({ "path": name.replace('/', "\\"), "folder": folder.replace('/', "\\"), "w": w, "h": h, "hd": hd, "frames": frames, "roles": roles }));
    }
    let key = |v: &Value| (v["folder"].as_str().unwrap_or("").to_lowercase(), v["path"].as_str().unwrap_or("").to_lowercase());
    out.sort_by_key(key);
    out.truncate(MAX_IMAGES);
    Ok(json!({ "images": out }))
}

// what each image skin.ini points at is used as, in any keymode, by its name as written there
fn used_as(skin: &Skin) -> HashMap<String, Vec<&'static str>> {
    let mut used: HashMap<String, Vec<&'static str>> = HashMap::new();
    let Some(text) = skin.ini() else { return used };
    for block in super::ini::parse(&text).mania {
        for (key, value) in &block {
            let role = if let Some(rest) = key.strip_prefix("NoteImage") {
                match rest.trim_start_matches(|c: char| c.is_ascii_digit()) {
                    "" => "note",
                    "H" => "head",
                    "L" => "body",
                    "T" => "tail",
                    _ => continue,
                }
            } else if let Some(rest) = key.strip_prefix("KeyImage") {
                match rest.trim_start_matches(|c: char| c.is_ascii_digit()) {
                    "" => "key",
                    "D" => "keyDown",
                    _ => continue,
                }
            } else {
                continue;
            };
            let value = value.replace('\\', "/").to_lowercase();
            let value = value.strip_suffix(".png").unwrap_or(&value);
            let roles = used.entry(strip_2x(value).0.to_string()).or_default();
            if !roles.contains(&role) {
                roles.push(role);
            }
        }
    }
    used
}

// What an image is likely for: how skin.ini uses it, else osu!'s own name for it or words in its
// path, else its shape. Empty when nothing says.
fn roles(name: &str, w: u32, h: u32, used: &HashMap<String, Vec<&'static str>>) -> Vec<&'static str> {
    let lower = name.to_lowercase();
    if let Some(roles) = used.get(&lower) {
        return roles.clone();
    }
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    // mania-note1, mania-noteSH, mania-key2D
    if let Some(rest) = file.strip_prefix("mania-note") {
        return vec![match rest.trim_start_matches(['1', '2', 's']) {
            "h" => "head",
            "l" => "body",
            "t" => "tail",
            _ => "note",
        }];
    }
    if let Some(rest) = file.strip_prefix("mania-key") {
        return vec![if rest.ends_with('d') { "keyDown" } else { "key" }];
    }
    if file.starts_with("mania-") || file.starts_with("lighting") {
        return vec![];
    }
    // the file's own name says more than its shape, which says more than the folder it's in
    let (folder, file) = name.rsplit_once('/').unwrap_or(("", name));
    if let Some(role) = role_by_words(&words(file)) {
        return vec![role];
    }
    if w > 0 && h >= w.saturating_mul(3) && (h >= w.saturating_mul(6) || h >= 1000) {
        return vec!["body"];
    }
    // a receptor stands taller than it's wide, its ring at the bottom of the image
    if w > 0 && h * 10 >= w * 16 {
        return vec!["key"];
    }
    role_by_words(&words(folder)).into_iter().collect()
}

// split at anything not a letter and where case or digits change: "NoteL2" is note l 2
fn words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = vec![];
    let mut last: Option<char> = None;
    for c in name.chars() {
        let split = match last {
            None => true,
            Some(p) => !c.is_alphanumeric() || !p.is_alphanumeric() || p.is_ascii_digit() != c.is_ascii_digit() || (p.is_lowercase() && c.is_uppercase()),
        };
        if c.is_alphanumeric() {
            if split || words.is_empty() {
                words.push(String::new());
            }
            words.last_mut().unwrap().extend(c.to_lowercase());
        }
        last = Some(c);
    }
    words
}

fn role_by_words(words: &[String]) -> Option<&'static str> {
    let has = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    let noted = has(&["note", "notes", "n"]);
    if has(&["key", "keys", "receptor", "receptors"]) && !noted {
        return Some(if has(&["d", "down", "pressed", "press", "hit", "on"]) { "keyDown" } else { "key" });
    }
    if has(&["tail", "tails", "end"]) || (noted && has(&["t"])) {
        return Some("tail");
    }
    if has(&["head", "heads"]) || (noted && has(&["h"])) {
        return Some("head");
    }
    if has(&["body", "bodies", "hold", "holds", "ln", "long"]) || (noted && has(&["l"])) {
        return Some("body");
    }
    if noted || has(&["circle", "circles", "bar", "bars", "arrow", "arrows"]) {
        return Some("note");
    }
    None
}

// A small picture of an image. A tall strip such as a Percy body shows its first visible rows
// after the transparent lead-in, which is the part that tells one from another.
pub(crate) fn thumb(dir: &Path, at: &str, reference: &str) -> Option<Vec<u8>> {
    let lib = library(dir, at);
    let key = reference.to_lowercase();
    if let Some(png) = lib.thumbs.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return png.clone();
    }
    let png = make_thumb(&lib.skin, reference);
    lib.thumbs.lock().unwrap_or_else(|e| e.into_inner()).insert(key, png.clone());
    png
}

fn make_thumb(skin: &Skin, reference: &str) -> Option<Vec<u8>> {
    let asset = skin.swap(reference, true)?;
    let raw = load::raw(asset.path.as_ref()?)?;
    let (w, h) = (raw.w, raw.h);
    // a blank placeholder has nothing to show, the page labels it instead
    edit::lead_rows(&raw.rgba, w)?;
    let (top, rows) = if h >= w.saturating_mul(3) {
        let lead = edit::lead_rows(&raw.rgba, w).unwrap_or(0) as u32;
        (lead, (h - lead).min((w * 3 / 2).max(1)))
    } else {
        (0, h)
    };
    let row = w as usize * 4;
    let mut rgba = raw.rgba[top as usize * row..(top + rows) as usize * row].to_vec();
    // resized premultiplied, so transparent pixels don't bleed their colour into the edges
    for p in rgba.chunks_exact_mut(4) {
        for c in 0..3 {
            p[c] = ((p[c] as u32 * p[3] as u32 + 127) / 255) as u8;
        }
    }
    let scale = (THUMB as f64 / w as f64).min(THUMB as f64 / rows as f64).min(4.0);
    let (tw, th) = (((w as f64 * scale).round() as u32).max(1), ((rows as f64 * scale).round() as u32).max(1));
    let img = image::RgbaImage::from_raw(w, rows, rgba)?;
    let mut small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    for p in small.pixels_mut() {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in 0..3 {
                p[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let mut out = std::io::Cursor::new(vec![]);
    small.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}
