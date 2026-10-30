use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::edit::{self, Edits};
use super::{ini, load};

const PREFIX: &str = "hashi-";

fn backup_of(ini: &Path) -> PathBuf {
    let mut name = ini.file_name().unwrap_or_default().to_os_string();
    name.push(".hashi-backup");
    ini.with_file_name(name)
}

fn ini_of(dir: &Path) -> Result<PathBuf, String> {
    load::ini_path(dir).ok_or_else(|| "This skin has no skin.ini to change.".to_string())
}

pub(crate) fn edited(dir: &Path) -> bool {
    load::ini_path(dir).is_some_and(|ini| backup_of(&ini).exists())
}

// Writes the edited art next to skin.ini and points the keymode's [Mania] block at it. skin.ini is
// backed up the first time and the backup is never replaced, so undo always gets the original.
pub(crate) fn apply(dir: &Path, keys: usize, edits: &str) -> Result<Value, String> {
    let keys = keys.clamp(1, 10);
    let edits = Edits::parse(edits);
    let skin = load::Skin::open(dir);
    let (_, profile) = load::import_from(&skin, keys)?;
    let (edited, made, swapped) = edit::apply(&profile, &edits, keys, &skin, &Default::default(), true);
    let hit = edits.hit_position.filter(|v| (0.0..=480.0).contains(v));
    if made.is_empty() && swapped.is_empty() && hit.is_none() && edits.judgement_line.is_none() && edits.column_start.is_none() {
        return Err("There's nothing to change.".into());
    }
    let ini = ini_of(dir)?;
    let root = ini.parent().ok_or("No folder for skin.ini.")?.to_path_buf();
    let text = std::fs::read(&ini).map_err(|e| e.to_string())?;
    let backup = backup_of(&ini);
    if !backup.exists() {
        std::fs::write(&backup, &text).map_err(|e| format!("Couldn't back up skin.ini: {e}"))?;
    }
    let ours = format!("{PREFIX}{keys}k-");
    remove_files(&root, &ours);
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
    if let Some(v) = edits.column_start {
        sets.push(("ColumnStart".into(), ini::round(v.clamp(0.0, edit::COLUMN_START_MAX)).to_string()));
    }
    std::fs::write(&ini, set_keys(&text, keys, &sets)).map_err(|e| e.to_string())?;
    Ok(json!({ "files": files, "keys": sets.iter().map(|(k, _)| k).collect::<Vec<_>>() }))
}

// A copy of the skin with the edits applied and its name changed, packed as an .osk in `out`. Lazer
// adds the file's name to an imported skin's unless they match, so the file is named after it.
pub(crate) fn pack(dir: &Path, keys: usize, edits: &str, name: &str, out: &Path) -> Result<PathBuf, String> {
    let work = out.join("skin");
    let _ = std::fs::remove_dir_all(out);
    copy_tree(dir, &work).map_err(|e| format!("Couldn't copy the skin: {e}"))?;
    let fixed = fix_tiffs(&work)?;
    if !edits.is_empty() || fixed == 0 {
        apply(&work, keys, edits)?;
    }
    let ini = ini_of(&work)?;
    let _ = std::fs::remove_file(backup_of(&ini));
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

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
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
    std::fs::remove_file(&backup).map_err(|e| e.to_string())
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

fn write_png(path: &Path, raw: &load::Raw) -> Result<(), String> {
    let img = image::RgbaImage::from_raw(raw.w, raw.h, raw.rgba.clone()).ok_or("bad image")?;
    img.save_with_format(path, image::ImageFormat::Png).map_err(|e| e.to_string())
}

// Sets keys in the first [Mania] block for `keys`, or a new one at the end. Only the lines it sets
// change; every other byte, the encoding and the line endings stay as they were.
fn set_keys(text: &[u8], keys: usize, sets: &[(String, String)]) -> Vec<u8> {
    let crlf = text.windows(2).any(|w| w == b"\r\n");
    let eol: &[u8] = if crlf { b"\r\n" } else { b"\n" };
    let mut lines: Vec<Vec<u8>> = text.split_inclusive(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
    let content = |line: &[u8]| {
        let s = String::from_utf8_lossy(line).into_owned();
        let s = s.trim_start_matches('\u{feff}');
        let s = s.find("//").map_or(s, |i| &s[..i]);
        s.trim().to_string()
    };
    let key_of = |line: &[u8]| content(line).split_once(':').map(|(k, _)| k.trim().to_string());
    let header = |line: &[u8]| {
        let c = content(line);
        (c.starts_with('[') && c.ends_with(']')).then(|| c[1..c.len() - 1].trim().to_string())
    };
    let mut block = None;
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
                block = Some((i, end));
                break;
            }
            i = end;
        } else {
            i += 1;
        }
    }
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
    use super::{set_keys, set_name};

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
    fn names_the_skin() {
        let name = |ini: &[u8]| String::from_utf8_lossy(&set_name(ini, "new")).into_owned();
        assert_eq!(name(b"\xef\xbb\xbf[General]\r\nname : old // x\r\nAuthor: a\r\n[Mania]\r\nName: no\r\n"), "\u{feff}[General]\r\nName: new\r\nAuthor: a\r\n[Mania]\r\nName: no\r\n");
        assert_eq!(name(b"[General]\nAuthor: a\n"), "[General]\nName: new\nAuthor: a\n");
        assert_eq!(name(b"[Mania]\nKeys: 4\n"), "[General]\nName: new\n\n[Mania]\nKeys: 4\n");
    }
}
