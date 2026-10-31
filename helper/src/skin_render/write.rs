use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::edit::{self, Edits};
use super::{ini, load};

const PREFIX: &str = "hashi-";
// the copy of the score font with its % widened, for the whole skin
const SCORE_FONT: &str = "hashi-score";
const NOTHING: &str = "There's nothing to change.";

fn beside(ini: &Path, suffix: &str) -> PathBuf {
    let mut name = ini.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    ini.with_file_name(name)
}

// skin.ini as it was before Hashi first changed it, which edits always start from
pub(super) fn backup_of(ini: &Path) -> PathBuf {
    beside(ini, ".hashi-backup")
}

// what's applied, by keymode: { "4": { "edits": <the edits' json>, "state": <the page's own> } }
fn applied_of(ini: &Path) -> PathBuf {
    beside(ini, ".hashi-applied")
}

fn read_applied(ini: &Path) -> serde_json::Map<String, Value> {
    std::fs::read(applied_of(ini)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_applied(ini: &Path, applied: &serde_json::Map<String, Value>) -> Result<(), String> {
    if applied.is_empty() {
        let _ = std::fs::remove_file(applied_of(ini));
        return Ok(());
    }
    std::fs::write(applied_of(ini), Value::Object(applied.clone()).to_string()).map_err(|e| e.to_string())
}

// what's applied by keymode, and `_accuracy`: the % width in skin pixels while Hashi's score font is on
pub(crate) fn applied(dir: &Path) -> Value {
    let Some(ini) = load::ini_path(dir) else { return json!({}) };
    let mut out = read_applied(&ini);
    let ours = score_font_on(&std::fs::read(&ini).unwrap_or_default());
    let root = ini.parent().map(Path::to_path_buf).unwrap_or_default();
    let width = [("@2x", 2.0), ("", 1.0)].iter().find_map(|(two, scale)| image::image_dimensions(root.join(format!("{SCORE_FONT}-percent{two}.png"))).ok().map(|(w, _)| f64::from(w) / scale));
    if let (true, Some(w)) = (ours, width) {
        out.insert("_accuracy".into(), json!({ "percentWidth": w }));
    }
    Value::Object(out)
}

fn score_font_on(ini: &[u8]) -> bool {
    super::ini::parse(&String::from_utf8_lossy(ini)).fonts.get("ScorePrefix").is_some_and(|p| p.trim().eq_ignore_ascii_case(SCORE_FONT))
}

fn ini_of(dir: &Path) -> Result<PathBuf, String> {
    load::ini_path(dir).ok_or_else(|| "This skin has no skin.ini to change.".to_string())
}

pub(crate) fn edited(dir: &Path) -> bool {
    load::ini_path(dir).is_some_and(|ini| backup_of(&ini).exists())
}

// Writes the edited art next to skin.ini and points the keymode's [Mania] block at it. skin.ini is
// backed up the first time and the backup is never replaced, so undo always gets the original.
// Each apply replaces the last for its keymode: the block goes back to the original's first, so a
// value set before and not now is the skin's own again. Edits with nothing in them put the keymode
// back as it was.
pub(crate) fn apply(dir: &Path, keys: usize, edits: &str, state: Option<&str>) -> Result<Value, String> {
    let keys = keys.clamp(1, 10);
    let edits_text = edits;
    let edits = Edits::parse(edits);
    let skin = load::Skin::open(dir);
    let (_, profile) = load::import_from(&skin, keys)?;
    let (edited, made, swapped) = edit::apply(&profile, &edits, keys, &skin, &Default::default(), true);
    let hit = edits.hit_position.filter(|v| (0.0..=480.0).contains(v));
    let columns = edits.column_width.is_some() || edits.column_spacing.is_some();
    let score = edits.score_position.filter(|v| (0.0..=480.0).contains(v));
    let combo = edits.combo_position.filter(|v| (0.0..=480.0).contains(v));
    let nothing = made.is_empty()
        && swapped.is_empty()
        && hit.is_none()
        && edits.judgement_line.is_none()
        && edits.column_start.is_none()
        && !columns
        && score.is_none()
        && combo.is_none()
        && edits.combo_scale().is_none()
        && edits.percent_width().is_none();
    let ini = ini_of(dir)?;
    let root = ini.parent().ok_or("No folder for skin.ini.")?.to_path_buf();
    let text = std::fs::read(&ini).map_err(|e| e.to_string())?;
    let backup = backup_of(&ini);
    let mut applied = read_applied(&ini);
    let ours = format!("{PREFIX}{keys}k-");
    if nothing {
        // the accuracy is the whole skin's, so a keymode never applied can still take it off
        if applied.remove(&keys.to_string()).is_none() && !score_font_on(&text) {
            return Err(NOTHING.into());
        }
        let original = std::fs::read(&backup).unwrap_or_else(|_| text.clone());
        let new_text = original_section(&original_block(&text, &original, keys), &original, "Fonts");
        remove_files(&root, &format!("{SCORE_FONT}-"));
        std::fs::write(&ini, &new_text).map_err(|e| e.to_string())?;
        remove_stale(&root, &ours, &new_text, &[], &[]);
        write_applied(&ini, &applied)?;
        // nothing of Hashi's left: the skin is its own again
        if applied.is_empty() && new_text == original {
            let _ = std::fs::remove_file(&backup);
        }
        return Ok(json!({ "files": [], "keys": [] }));
    }
    if !backup.exists() {
        std::fs::write(&backup, &text).map_err(|e| format!("Couldn't back up skin.ini: {e}"))?;
    }
    let original = std::fs::read(&backup).map_err(|e| format!("Couldn't read the backed up skin.ini: {e}"))?;
    // [Fonts] as the original has it too, the accuracy's font set again below when it's moved
    let mut text = original_section(&original_block(&text, &original, keys), &original, "Fonts");
    let mut sets = vec![];
    let mut files = vec![];
    // a pure swap only points the line at the other image; one that's also cut or coloured below
    // gets its own made art instead
    let slash = separator(&text);
    for (c, part, reference) in &swapped {
        if !made.iter().any(|m| m.column == *c && m.part == *part) {
            sets.push((part.ini_key(*c), reference.replace(['\\', '/'], slash)));
        }
    }
    // columns that come out the same share their files
    let mut written: Vec<(u64, String)> = vec![];
    for m in &made {
        let two = if m.scale == 2.0 { "@2x" } else { "" };
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for f in &m.frames {
            (f.w, f.h, &f.rgba, two).hash(&mut hash);
        }
        let hash = hash.finish();
        if let Some((_, name)) = written.iter().find(|(h, _)| *h == hash) {
            sets.push((m.part.ini_key(m.column), name.clone()));
            continue;
        }
        let name = format!("{ours}{}-{}", m.part.name(), m.column);
        written.push((hash, name.clone()));
        for (i, frame) in m.frames.iter().enumerate() {
            let file = if m.frames.len() > 1 { format!("{name}-{i}{two}.png") } else { format!("{name}{two}.png") };
            write_png(&root.join(&file), frame)?;
            files.push(file);
        }
        sets.push((m.part.ini_key(m.column), name));
    }
    for c in 0..keys {
        let style = |p: &load::Profile| p.body_styles.get(c).copied().unwrap_or(1.0);
        if style(&profile) != style(&edited) {
            sets.push((format!("NoteBodyStyle{c}"), "1".into()));
        }
    }
    if let Some(v) = hit {
        sets.push(("HitPosition".into(), ini::round(v).to_string()));
    }
    if let Some(on) = edits.judgement_line {
        sets.push(("JudgementLine".into(), if on { "1" } else { "0" }.into()));
    }
    if let Some(v) = score {
        sets.push(("ScorePosition".into(), ini::round(v).to_string()));
    }
    if let Some(v) = combo {
        sets.push(("ComboPosition".into(), ini::round(v).to_string()));
    }
    if let Some(v) = edits.column_start {
        sets.push(("ColumnStart".into(), ini::round(v.clamp(0.0, edit::COLUMN_START_MAX)).to_string()));
    }
    // one value per column, and per gap between them
    if edits.column_width.is_some() {
        sets.push(("ColumnWidth".into(), vec![ini::round(edited.column_width).to_string(); keys].join(",")));
    }
    if edits.column_spacing.is_some() {
        sets.push(("ColumnSpacing".into(), vec![ini::round(edited.column_spacing).to_string(); keys.saturating_sub(1).max(1)].join(",")));
    }
    if let (Some(f), Some(c)) = (edits.combo_scale(), profile.combo.as_ref()) {
        let name = format!("{ours}combo");
        for (d, asset) in c.digits.iter().enumerate() {
            let Some((path, scale)) = asset.as_ref().and_then(|a| Some((a.path.clone()?, a.scale))) else { continue };
            let Some(raw) = load::raw(&path) else { continue };
            let two = if scale == 2.0 { "@2x" } else { "" };
            let file = format!("{name}-{d}{two}.png");
            write_png(&root.join(&file), &scaled_glyph(&raw, f, c.overlap * scale))?;
            files.push(file);
        }
        sets.push(("FontCombo".into(), name));
    }
    match (edits.percent_width(), profile.score_font.as_ref()) {
        (Some(w), Some(font)) => {
            write_score_font(&skin, &root, font, w)?;
            text = set_in_section(&text, "Fonts", "ScorePrefix", SCORE_FONT);
        }
        _ => remove_files(&root, &format!("{SCORE_FONT}-")),
    }
    let new_text = set_keys(&text, keys, &sets);
    std::fs::write(&ini, &new_text).map_err(|e| e.to_string())?;
    remove_stale(&root, &ours, &new_text, &written, &files);
    let state = state.and_then(|s| serde_json::from_str::<Value>(s).ok()).unwrap_or(Value::Null);
    applied.insert(keys.to_string(), json!({ "edits": edits_text, "state": state }));
    write_applied(&ini, &applied)?;
    Ok(json!({ "files": files, "keys": sets.iter().map(|(k, _)| k).collect::<Vec<_>>() }))
}

// A copy of the skin with the edits applied and its name changed, packed as an .osk in `out`. Lazer
// adds the file's name to an imported skin's unless they match, so the file is named after it.
pub(crate) fn pack(dir: &Path, keys: usize, edits: &str, name: &str, out: &Path) -> Result<PathBuf, String> {
    let work = out.join("skin");
    let _ = std::fs::remove_dir_all(out);
    copy_tree(dir, &work).map_err(|e| format!("Couldn't copy the skin: {e}"))?;
    let fixed = fix_tiffs(&work)?;
    if let Err(e) = apply(&work, keys, edits, None) {
        // the TIFFs saved as PNGs are worth a new skin on their own
        if fixed == 0 || e != NOTHING {
            return Err(e);
        }
    }
    let ini = ini_of(&work)?;
    let _ = std::fs::remove_file(backup_of(&ini));
    let _ = std::fs::remove_file(applied_of(&ini));
    let text = std::fs::read(&ini).map_err(|e| e.to_string())?;
    std::fs::write(&ini, set_name(&text, name)).map_err(|e| e.to_string())?;
    let file_name: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let file = out.join(format!("{}.osk", file_name.trim()));
    zip_tree(&work, &file).map_err(|e| format!("Couldn't pack the skin: {e}"))?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(file)
}

fn is_tiff(bytes: &[u8]) -> bool {
    bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*")
}

// Lazer reads PNG, JPEG, GIF, BMP and WebP, going by what's in the file. Stable also reads TIFF,
// which skins often ship under a .png name, so lazer draws nothing where those go.
pub(crate) fn tiffs(dir: &Path) -> Vec<String> {
    use std::io::Read;
    let skin = load::Skin::open(dir);
    let mut out = vec![];
    for (rel, path) in skin.files_under_root() {
        let lower = rel.to_lowercase();
        if !lower.ends_with(".png") && !lower.ends_with(".jpg") && !lower.ends_with(".jpeg") {
            continue;
        }
        let mut head = [0u8; 4];
        if std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && is_tiff(&head) {
            out.push(rel);
        }
    }
    out.sort();
    out
}

pub(crate) fn fix_tiffs(dir: &Path) -> Result<usize, String> {
    let root = load::ini_path(dir).and_then(|i| i.parent().map(Path::to_path_buf)).ok_or("This skin has no skin.ini.")?;
    let list = tiffs(dir);
    for rel in &list {
        let path = root.join(rel);
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let (rgba, w, h) = load::tiff(&bytes).ok_or_else(|| format!("Couldn't read {rel}."))?;
        let img = image::RgbaImage::from_raw(w, h, rgba).ok_or("bad image")?;
        // written beside it and moved over, so a failed write leaves the original
        let part = path.with_extension("hashi-part");
        img.save_with_format(&part, image::ImageFormat::Png).map_err(|e| format!("Couldn't save {rel}: {e}"))?;
        std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    }
    Ok(list.len())
}

pub(crate) fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let path = e.path();
        if e.file_type()?.is_dir() {
            copy_tree(&path, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&path, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

fn zip_tree(root: &Path, file: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::fs::File::create(file)?);
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(&dir)?.flatten() {
            let path = e.path();
            if e.file_type()?.is_dir() {
                dirs.push(path);
                continue;
            }
            let rel = path.strip_prefix(root)?.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            zip.start_file(rel, zip::write::SimpleFileOptions::default())?;
            zip.write_all(&std::fs::read(&path)?)?;
        }
    }
    zip.finish()?;
    Ok(())
}

// Sets Name in [General], adding the line or the section if they're missing
fn set_name(text: &[u8], name: &str) -> Vec<u8> {
    let crlf = text.windows(2).any(|w| w == b"\r\n");
    let eol: &[u8] = if crlf { b"\r\n" } else { b"\n" };
    let line = [format!("Name: {name}").as_bytes(), eol].concat();
    let mut lines: Vec<Vec<u8>> = text.split_inclusive(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
    let content = |l: &[u8]| String::from_utf8_lossy(l).trim_start_matches('\u{feff}').trim().to_string();
    let Some(general) = lines.iter().position(|l| content(l).eq_ignore_ascii_case("[General]")) else {
        let bom = text.starts_with(b"\xef\xbb\xbf");
        let rest = if bom { &text[3..] } else { text };
        return [if bom { b"\xef\xbb\xbf".as_slice() } else { b"" }, b"[General]", eol, &line, eol, rest].concat();
    };
    let end = (general + 1..lines.len()).find(|&j| content(&lines[j]).starts_with('[')).unwrap_or(lines.len());
    match (general + 1..end).find(|&j| content(&lines[j]).split_once(':').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("Name"))) {
        Some(j) => lines[j] = line,
        None => {
            if !lines[general].ends_with(b"\n") {
                lines[general].extend_from_slice(eol);
            }
            lines.insert(general + 1, line);
        }
    }
    lines.concat()
}

// the folder separator the skin's own image lines use, backslashes unless it only uses slashes
fn separator(text: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(text);
    let paths = text.lines().filter(|l| {
        let l = l.trim_start();
        l.starts_with("NoteImage") || l.starts_with("KeyImage") || l.starts_with("Stage")
    });
    let (back, forward) = paths.fold((0, 0), |(b, f), l| (b + l.matches('\\').count(), f + l.matches('/').count()));
    if forward > 0 && back == 0 {
        "/"
    } else {
        "\\"
    }
}

pub(crate) fn undo(dir: &Path) -> Result<(), String> {
    let ini = ini_of(dir)?;
    let backup = backup_of(&ini);
    if !backup.exists() {
        return Err("Hashi hasn't changed this skin.".into());
    }
    std::fs::copy(&backup, &ini).map_err(|e| e.to_string())?;
    if let Some(root) = ini.parent() {
        remove_files(root, PREFIX);
    }
    let _ = std::fs::remove_file(applied_of(&ini));
    std::fs::remove_file(&backup).map_err(|e| e.to_string())
}

// A combo digit `f` times its size. ComboOverlap stays as it is in skin.ini, so the image's width
// takes up the difference, keeping the digits as far apart for their size as they were
fn scaled_glyph(raw: &load::Raw, f: f64, overlap: f64) -> load::Raw {
    let (w, h) = (f64::from(raw.w), f64::from(raw.h));
    let (gw, gh) = ((w * f).round().max(1.0) as u32, (h * f).round().max(1.0) as u32);
    let glyph = resized(raw, gw, gh);
    let width = (w * f - overlap * (f - 1.0)).round().max(1.0) as u32;
    let mut rgba = vec![0u8; (width * gh * 4) as usize];
    // centred, cropped or padded the same each side
    let shift = (i64::from(width) - i64::from(gw)) / 2;
    for y in 0..gh as usize {
        for x in 0..gw as i64 {
            let to = x + shift;
            if to < 0 || to >= i64::from(width) {
                continue;
            }
            let (s, d) = ((y * gw as usize + x as usize) * 4, (y * width as usize + to as usize) * 4);
            rgba[d..d + 4].copy_from_slice(&glyph.rgba[s..s + 4]);
        }
    }
    load::Raw::new(rgba, width, gh)
}

// resampled premultiplied, so clear pixels don't darken the edges
fn resized(raw: &load::Raw, w: u32, h: u32) -> load::Raw {
    let mut pre = raw.rgba.clone();
    for p in pre.chunks_exact_mut(4) {
        for c in 0..3 {
            p[c] = ((u32::from(p[c]) * u32::from(p[3]) + 127) / 255) as u8;
        }
    }
    let Some(img) = image::RgbaImage::from_raw(raw.w, raw.h, pre) else { return load::Raw::new(raw.rgba.clone(), raw.w, raw.h) };
    let mut out = image::imageops::resize(&img, w, h, image::imageops::FilterType::CatmullRom);
    for p in out.pixels_mut() {
        let a = u32::from(p[3]);
        if a > 0 && a < 255 {
            for c in 0..3 {
                p[c] = ((u32::from(p[c]) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    load::Raw::new(out.into_raw(), w, h)
}

// The score font under SCORE_FONT, each glyph and its @2x copied as they are but the %, which is made
// `width` skin pixels wide: its own art kept at its left, the rest clear
fn write_score_font(skin: &load::Skin, root: &Path, font: &load::ScoreFont, width: f64) -> Result<(), String> {
    remove_files(root, &format!("{SCORE_FONT}-"));
    let prefix = font.prefix.replace('\\', "/");
    for glyph in ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "comma", "dot", "percent", "x"] {
        for (two, scale) in [("@2x", 2.0), ("", 1.0)] {
            let Some(path) = skin.file(&format!("{prefix}-{glyph}{two}.png")) else { continue };
            let to = root.join(format!("{SCORE_FONT}-{glyph}{two}.png"));
            if glyph != "percent" {
                std::fs::copy(&path, &to).map_err(|e| format!("Couldn't copy the score font: {e}"))?;
                continue;
            }
            let Some(raw) = load::raw(&path) else { continue };
            let w = (width * scale).round().max(1.0) as u32;
            let mut rgba = vec![0u8; (w * raw.h * 4) as usize];
            let keep = w.min(raw.w) as usize;
            for y in 0..raw.h as usize {
                let (s, d) = (y * raw.w as usize * 4, y * w as usize * 4);
                rgba[d..d + keep * 4].copy_from_slice(&raw.rgba[s..s + keep * 4]);
            }
            write_png(&to, &load::Raw::new(rgba, w, raw.h))?;
        }
    }
    Ok(())
}

// `text` with its [`name`] section as `original` has it, gone when the original has none
fn original_section(text: &[u8], original: &[u8], name: &str) -> Vec<u8> {
    let find = |lines: &[Vec<u8>]| {
        let start = lines.iter().position(|l| header(l).is_some_and(|h| h.eq_ignore_ascii_case(name)))?;
        let end = (start + 1..lines.len()).find(|&j| header(&lines[j]).is_some()).unwrap_or(lines.len());
        Some((start, end))
    };
    let mut lines = ini_lines(text);
    let from = ini_lines(original);
    let theirs = find(&from).map(|(a, b)| from[a..b].to_vec());
    match (find(&lines), theirs) {
        (Some((a, b)), Some(section)) => {
            lines.splice(a..b, section);
        }
        (Some((a, b)), None) => {
            lines.drain(a..b);
        }
        (None, Some(section)) => {
            if lines.last().is_some_and(|l| !l.ends_with(b"\n")) {
                lines.last_mut().unwrap().push(b'\n');
            }
            lines.extend(section);
        }
        (None, None) => {}
    }
    lines.concat()
}

// `key: value` in [`name`], the line replaced or added, the section added when there's none
fn set_in_section(text: &[u8], name: &str, key: &str, value: &str) -> Vec<u8> {
    let crlf = text.windows(2).any(|w| w == b"\r\n");
    let eol: &[u8] = if crlf { b"\r\n" } else { b"\n" };
    let mut lines = ini_lines(text);
    let line = [format!("{key}: {value}").as_bytes(), eol].concat();
    let Some(start) = lines.iter().position(|l| header(l).is_some_and(|h| h.eq_ignore_ascii_case(name))) else {
        if lines.last().is_some_and(|l| !l.ends_with(b"\n")) {
            lines.last_mut().unwrap().extend_from_slice(eol);
        }
        lines.push(eol.to_vec());
        lines.push([format!("[{name}]").as_bytes(), eol].concat());
        lines.push(line);
        return lines.concat();
    };
    let end = (start + 1..lines.len()).find(|&j| header(&lines[j]).is_some()).unwrap_or(lines.len());
    let key_of = |l: &[u8]| content(l).split_once(':').map(|(k, _)| k.trim().to_string());
    if let Some(i) = (start + 1..end).find(|&i| key_of(&lines[i]).is_some_and(|k| k.eq_ignore_ascii_case(key))) {
        lines[i] = line;
    } else {
        if !lines[start].ends_with(b"\n") {
            lines[start].extend_from_slice(eol);
        }
        lines.insert(start + 1, line);
    }
    lines.concat()
}

fn remove_files(root: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with(prefix) && name.ends_with(".png") && e.path().is_file() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

// Earlier applies' files that skin.ini no longer points at. A name written again this time also
// loses the frames and @2x copies it didn't get now, which the game would pick over the new file
fn remove_stale(root: &Path, ours: &str, text: &[u8], written: &[(u64, String)], files: &[String]) {
    let refs: Vec<String> = ini::parse(&String::from_utf8_lossy(text))
        .mania
        .iter()
        .flat_map(|b| b.values())
        .map(|v| {
            let v = v.trim_matches(['"', '\'']).replace('\\', "/").to_lowercase();
            v.strip_suffix(".png").map_or(v.clone(), str::to_string)
        })
        .filter(|v| v.starts_with(ours))
        .collect();
    let written: Vec<String> = written.iter().map(|(_, n)| n.to_lowercase()).collect();
    let files: Vec<String> = files.iter().map(|f| f.to_lowercase()).collect();
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with(ours) && e.path().is_file() && is_stale(&name, &refs, &written, &files) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn is_stale(file: &str, refs: &[String], written: &[String], files: &[String]) -> bool {
    let Some(stem) = file.strip_suffix(".png") else { return false };
    if files.iter().any(|f| f == file) {
        return false;
    }
    let stem = stem.strip_suffix("@2x").unwrap_or(stem);
    let of = |r: &String| stem == r || stem.strip_prefix(r.as_str()).and_then(|s| s.strip_prefix('-')).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    !refs.iter().any(|r| of(r) && !written.contains(r))
}

fn write_png(path: &Path, raw: &load::Raw) -> Result<(), String> {
    let img = image::RgbaImage::from_raw(raw.w, raw.h, raw.rgba.clone()).ok_or("bad image")?;
    img.save_with_format(path, image::ImageFormat::Png).map_err(|e| e.to_string())
}

// Sets keys in the first [Mania] block for `keys`, or a new one at the end. Only the lines it sets
// change; every other byte, the encoding and the line endings stay as they were.
// a line of skin.ini without its comment, BOM and spaces
fn content(line: &[u8]) -> String {
    let s = String::from_utf8_lossy(line).into_owned();
    let s = s.trim_start_matches('\u{feff}');
    let s = s.find("//").map_or(s, |i| &s[..i]);
    s.trim().to_string()
}

fn header(line: &[u8]) -> Option<String> {
    let c = content(line);
    (c.starts_with('[') && c.ends_with(']')).then(|| c[1..c.len() - 1].trim().to_string())
}

fn ini_lines(text: &[u8]) -> Vec<Vec<u8>> {
    text.split_inclusive(|&b| b == b'\n').map(<[u8]>::to_vec).collect()
}

// the [Mania] block for `keys` as lines start..end, its header first
fn mania_block(lines: &[Vec<u8>], keys: usize) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < lines.len() {
        if header(&lines[i]).as_deref() == Some("Mania") {
            let end = (i + 1..lines.len()).find(|&j| header(&lines[j]).is_some()).unwrap_or(lines.len());
            let declared = (i + 1..end).find_map(|j| {
                let c = content(&lines[j]);
                let (k, v) = c.split_once(':')?;
                (k.trim() == "Keys").then(|| super::ini::number(v)).flatten()
            });
            if declared.map(super::ini::round) == Some(keys as f64) {
                return Some((i, end));
            }
            i = end;
        } else {
            i += 1;
        }
    }
    None
}

// `text` with its block for `keys` as `original` has it, gone when the original has none
fn original_block(text: &[u8], original: &[u8], keys: usize) -> Vec<u8> {
    let mut lines = ini_lines(text);
    let from = ini_lines(original);
    let theirs = mania_block(&from, keys).map(|(a, b)| from[a..b].to_vec());
    match (mania_block(&lines, keys), theirs) {
        (Some((a, b)), Some(block)) => {
            lines.splice(a..b, block);
        }
        (Some((a, b)), None) => {
            lines.drain(a..b);
        }
        (None, Some(block)) => {
            if lines.last().is_some_and(|l| !l.ends_with(b"\n")) {
                lines.last_mut().unwrap().push(b'\n');
            }
            lines.extend(block);
        }
        (None, None) => {}
    }
    lines.concat()
}

fn set_keys(text: &[u8], keys: usize, sets: &[(String, String)]) -> Vec<u8> {
    let crlf = text.windows(2).any(|w| w == b"\r\n");
    let eol: &[u8] = if crlf { b"\r\n" } else { b"\n" };
    let mut lines = ini_lines(text);
    let key_of = |line: &[u8]| content(line).split_once(':').map(|(k, _)| k.trim().to_string());
    let block = mania_block(&lines, keys);
    let (start, mut end) = match block {
        Some(b) => b,
        None => {
            if lines.last().is_some_and(|l| !l.ends_with(b"\n")) {
                lines.last_mut().unwrap().extend_from_slice(eol);
            }
            lines.push(eol.to_vec());
            lines.push([b"[Mania]".as_slice(), eol].concat());
            lines.push([format!("Keys: {keys}").as_bytes(), eol].concat());
            (lines.len() - 2, lines.len())
        }
    };
    for (key, value) in sets {
        let mut found = false;
        for line in &mut lines[start + 1..end] {
            if key_of(line).as_deref() == Some(key.as_str()) {
                let indent: Vec<u8> = line.iter().take_while(|b| **b == b' ' || **b == b'\t').copied().collect();
                let ending: Vec<u8> = if line.ends_with(b"\r\n") { b"\r\n".to_vec() } else if line.ends_with(b"\n") { b"\n".to_vec() } else { vec![] };
                *line = [indent.as_slice(), format!("{key}: {value}").as_bytes(), &ending].concat();
                found = true;
            }
        }
        if !found {
            let at = (start + 1..end).rev().find(|&j| !content(&lines[j]).is_empty()).map_or(start + 1, |j| j + 1);
            if !lines[at - 1].ends_with(b"\n") {
                lines[at - 1].extend_from_slice(eol);
            }
            lines.insert(at, [format!("{key}: {value}").as_bytes(), eol].concat());
            end += 1;
        }
    }
    lines.concat()
}

#[cfg(test)]
mod tests {
    use super::{is_stale, pack, set_keys, set_name};

    #[test]
    fn sets_only_the_keys() {
        let ini = b"\xef\xbb\xbf[General]\r\nName: x\r\n\r\n[Mania]\r\nKeys: 4\r\nNoteImage0L: a // old\r\n\r\n[Mania]\r\nKeys: 7\r\n";
        let out = set_keys(ini, 4, &[("NoteImage0L".into(), "hashi-4k-body-0".into()), ("NoteImage1".into(), "n".into())]);
        let want = b"\xef\xbb\xbf[General]\r\nName: x\r\n\r\n[Mania]\r\nKeys: 4\r\nNoteImage0L: hashi-4k-body-0\r\nNoteImage1: n\r\n\r\n[Mania]\r\nKeys: 7\r\n";
        assert_eq!(String::from_utf8_lossy(&out), String::from_utf8_lossy(want));
        let out = set_keys(b"[Mania]\nKeys: 7", 4, &[("NoteImage0".into(), "n".into())]);
        assert_eq!(String::from_utf8_lossy(&out), "[Mania]\nKeys: 7\n\n[Mania]\nKeys: 4\nNoteImage0: n\n");
    }

    #[test]
    fn each_apply_replaces_the_last() {
        let dir = std::env::temp_dir().join(format!("hashi-apply-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = "[General]
Name: t

[Mania]
Keys: 4
HitPosition: 400

[Mania]
Keys: 7
HitPosition: 410
";
        std::fs::write(dir.join("skin.ini"), original).unwrap();
        let ini = || std::fs::read_to_string(dir.join("skin.ini")).unwrap();
        let block = |keys: usize| {
            let text = ini();
            let lines = super::ini_lines(text.as_bytes());
            let (a, b) = super::mania_block(&lines, keys).unwrap();
            String::from_utf8(lines[a..b].concat()).unwrap()
        };

        super::apply(&dir, 4, r#"{"hitPosition":420}"#, Some(r#"{"hit":420}"#)).unwrap();
        super::apply(&dir, 7, r#"{"columnStart":100}"#, None).unwrap();
        assert!(block(4).contains("HitPosition: 420"));
        // a second 4K apply replaces the first: HitPosition is the skin's own again
        super::apply(&dir, 4, r#"{"columnStart":120}"#, None).unwrap();
        assert!(block(4).contains("HitPosition: 400") && block(4).contains("ColumnStart: 120"), "{}", block(4));
        // and leaves 7K's alone
        assert!(block(7).contains("ColumnStart: 100") && block(7).contains("HitPosition: 410"));
        let applied = super::applied(&dir);
        assert_eq!(applied["4"]["edits"], r#"{"columnStart":120}"#);
        assert!(applied["7"].is_object());

        // nothing to apply puts a keymode back; with none left the skin is its own again
        super::apply(&dir, 4, "", None).unwrap();
        assert!(!block(4).contains("ColumnStart"));
        super::apply(&dir, 7, "", None).unwrap();
        assert_eq!(ini(), original);
        assert!(!super::backup_of(&dir.join("skin.ini")).exists());
        assert_eq!(super::applied(&dir), serde_json::json!({}));
        assert!(super::apply(&dir, 4, "", None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_a_bigger_combo_and_moves_the_accuracy() {
        use super::super::load::Raw;
        let dir = std::env::temp_dir().join(format!("hashi-fonts-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = "[Fonts]
ScorePrefix: score
ComboPrefix: combo
ComboOverlap: 4

[Mania]
Keys: 4
";
        std::fs::write(dir.join("skin.ini"), original).unwrap();
        let png = |name: &str, w: u32, h: u32, solid: u32| {
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            for y in 0..h {
                for x in 0..solid.min(w) {
                    let i = ((y * w + x) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
            super::write_png(&dir.join(name), &Raw::new(rgba, w, h)).unwrap();
        };
        for d in 0..10 {
            png(&format!("combo-{d}.png"), 20, 30, 20);
            png(&format!("score-{d}.png"), 10, 30, 10);
        }
        png("score-dot.png", 6, 30, 6);
        png("score-percent.png", 20, 30, 10);
        let size = |name: &str| image::image_dimensions(dir.join(name)).ok();
        let ini = || std::fs::read_to_string(dir.join("skin.ini")).unwrap();

        super::apply(&dir, 4, r#"{"comboScale":1.5}"#, None).unwrap();
        // 20 * 1.5 wide, less the overlap's growth of 4 * 0.5, so the digits sit as far apart for their size
        assert_eq!(size("hashi-4k-combo-0.png"), Some((28, 45)));
        assert!(ini().contains("FontCombo: hashi-4k-combo"), "{}", ini());

        super::apply(&dir, 4, r#"{"comboScale":1.5,"percentWidth":300}"#, None).unwrap();
        assert_eq!(size("hashi-score-percent.png"), Some((300, 30)));
        assert_eq!(size("hashi-score-7.png"), Some((10, 30)));
        assert!(ini().contains("ScorePrefix: hashi-score"), "{}", ini());
        // the % keeps its own art at its left
        let pct = image::open(dir.join("hashi-score-percent.png")).unwrap().to_rgba8();
        assert_eq!((pct.get_pixel(5, 10)[3], pct.get_pixel(200, 10)[3]), (255, 0));

        // the accuracy is the whole skin's: a keymode never applied takes it off, the 4K edits stay
        super::apply(&dir, 7, "", None).unwrap();
        assert!(ini().contains("ScorePrefix: score") && size("hashi-score-percent.png").is_none(), "{}", ini());
        assert_eq!(size("hashi-4k-combo-0.png"), Some((28, 45)));
        assert!(super::apply(&dir, 7, "", None).is_err());
        super::apply(&dir, 4, r#"{"comboScale":1.5,"percentWidth":300}"#, None).unwrap();

        // without the accuracy moved, the score font is the skin's again
        super::apply(&dir, 4, r#"{"comboScale":1.5}"#, None).unwrap();
        assert!(ini().contains("ScorePrefix: score") && size("hashi-score-percent.png").is_none(), "{}", ini());

        super::apply(&dir, 4, "", None).unwrap();
        assert_eq!(ini(), original);
        assert!(size("hashi-4k-combo-0.png").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_the_skin() {
        let name = |ini: &[u8]| String::from_utf8_lossy(&set_name(ini, "new")).into_owned();
        assert_eq!(name(b"\xef\xbb\xbf[General]\r\nname : old // x\r\nAuthor: a\r\n[Mania]\r\nName: no\r\n"), "\u{feff}[General]\r\nName: new\r\nAuthor: a\r\n[Mania]\r\nName: no\r\n");
        assert_eq!(name(b"[General]\nAuthor: a\n"), "[General]\nName: new\nAuthor: a\n");
        assert_eq!(name(b"[Mania]\nKeys: 4\n"), "[General]\nName: new\n\n[Mania]\nKeys: 4\n");
    }

    #[test]
    fn keeps_files_skin_ini_still_uses() {
        let v = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let refs = v(&["hashi-4k-note-0", "hashi-4k-note-1"]);
        let (written, files) = (v(&["hashi-4k-note-1"]), v(&["hashi-4k-note-1.png"]));
        // from an earlier apply, still pointed at
        assert!(!is_stale("hashi-4k-note-0.png", &refs, &written, &files));
        assert!(!is_stale("hashi-4k-note-0-3@2x.png", &refs, &written, &files));
        // written again now as one frame without @2x, so the old frames and @2x go
        assert!(!is_stale("hashi-4k-note-1.png", &refs, &written, &files));
        assert!(is_stale("hashi-4k-note-1-0.png", &refs, &written, &files));
        assert!(is_stale("hashi-4k-note-1@2x.png", &refs, &written, &files));
        // nothing points at it any more
        assert!(is_stale("hashi-4k-body-2.png", &refs, &written, &files));
    }

    #[test]
    fn packs_fixed_tiffs_without_edits() {
        let dir = std::env::temp_dir().join(format!("hashi-pack-test-{}", std::process::id()));
        let (skin, out) = (dir.join("in"), dir.join("out"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&skin).unwrap();
        std::fs::write(skin.join("skin.ini"), "[General]\nName: a\n\n[Mania]\nKeys: 4\n").unwrap();
        // a 1x1 uncompressed RGB TIFF: width, height, strip offset, samples, strip bytes
        let mut tiff = b"II*\0\x08\0\0\0\x05\0".to_vec();
        for (tag, kind, value) in [(256u16, 3u16, 1u32), (257, 3, 1), (273, 4, 74), (277, 3, 3), (279, 4, 3)] {
            tiff.extend(tag.to_le_bytes());
            tiff.extend(kind.to_le_bytes());
            tiff.extend(1u32.to_le_bytes());
            tiff.extend(value.to_le_bytes());
        }
        tiff.extend([0, 0, 0, 0, 255, 0, 0]);
        std::fs::write(skin.join("mania-note1.png"), tiff).unwrap();
        let file = pack(&skin, 4, r#"{"cut":[null,null,null,null]}"#, "b", &out).unwrap();
        assert!(file.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
