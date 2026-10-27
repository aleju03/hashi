use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::edit::{self, Edits};
use super::load;

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
    if made.is_empty() && swapped.is_empty() {
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
    std::fs::write(&ini, set_keys(&text, keys, &sets)).map_err(|e| e.to_string())?;
    Ok(json!({ "files": files, "keys": sets.iter().map(|(k, _)| k).collect::<Vec<_>>() }))
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
    use super::set_keys;

    #[test]
    fn sets_only_the_keys() {
        let ini = b"\xef\xbb\xbf[General]\r\nName: x\r\n\r\n[Mania]\r\nKeys: 4\r\nNoteImage0L: a // old\r\n\r\n[Mania]\r\nKeys: 7\r\n";
        let out = set_keys(ini, 4, &[("NoteImage0L".into(), "hashi-4k-body-0".into()), ("NoteImage1".into(), "n".into())]);
        let want = b"\xef\xbb\xbf[General]\r\nName: x\r\n\r\n[Mania]\r\nKeys: 4\r\nNoteImage0L: hashi-4k-body-0\r\nNoteImage1: n\r\n\r\n[Mania]\r\nKeys: 7\r\n";
        assert_eq!(String::from_utf8_lossy(&out), String::from_utf8_lossy(want));
        let out = set_keys(b"[Mania]\nKeys: 7", 4, &[("NoteImage0".into(), "n".into())]);
        assert_eq!(String::from_utf8_lossy(&out), "[Mania]\nKeys: 7\n\n[Mania]\nKeys: 4\nNoteImage0: n\n");
    }
}
