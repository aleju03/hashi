use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::Bridge;

#[derive(Clone)]
pub(crate) struct Found {
    pub dir: PathBuf,
    pub name: String,
    pub client: &'static str,
    // read out of lazer's database into Hashi's own folder, so edits can't be saved back
    pub stored: bool,
}

// lazer's skin editor's "Edit externally" copies the skin to the temp folder under its hash, and
// takes the folder back in when that's finished. A lazer that closed without finishing leaves its
// folder behind, so only one made since this lazer started counts
fn lazer_mount(lazer: &Path) -> Option<PathBuf> {
    let started = lazer_started(lazer)?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(std::env::temp_dir()).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.len() != 64 || !name.bytes().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let dir = entry.path();
        if !has_file(&dir, "skin.ini") && !has_file(&dir, "skininfo.json") {
            continue;
        }
        let at = entry.metadata().and_then(|m| m.created().or_else(|_| m.modified())).unwrap_or(std::time::UNIX_EPOCH);
        if at >= started && best.as_ref().is_none_or(|(t, _)| at > *t) {
            best = Some((at, dir));
        }
    }
    best.map(|(_, d)| d)
}

// each run of lazer starts its logs under the unix time it started at
fn lazer_started(lazer: &Path) -> Option<std::time::SystemTime> {
    let secs = std::fs::read_dir(lazer.join("logs"))
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".runtime.log")?.parse::<u64>().ok())
        .max()?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

fn has_file(dir: &Path, name: &str) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut it| it.any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))))
}

fn ini_name(dir: &Path) -> Option<String> {
    let file = std::fs::read_dir(dir).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("skin.ini"))?;
    let text = String::from_utf8_lossy(&std::fs::read(file.path()).ok()?).into_owned();
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim().eq_ignore_ascii_case("name")).then(|| v.trim().to_string()).filter(|v| !v.is_empty())
    })
}

// with osu! closed tosu reports nothing, stable keeps the skin in osu!.<user>.cfg
fn cfg_skin(game: &Path) -> Option<String> {
    let cfg = std::fs::read_dir(game)
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            n.starts_with("osu!.") && n.ends_with(".cfg") && n != "osu!.cfg"
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
    let text = String::from_utf8_lossy(&std::fs::read(cfg.path()).ok()?).into_owned();
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "Skin").then(|| v.trim().to_string()).filter(|v| !v.is_empty())
    })
}

pub(crate) fn current(b: &Bridge) -> Result<Found, &'static str> {
    let (client, last, skin, game) = {
        let s = b.st();
        let last = s.tosu.last_frame.as_ref().and_then(|f| f.get("client")?.as_str().map(str::to_string));
        (s.tosu.client.clone(), last, s.tosu.skin.clone(), s.tosu.stable_dirs.as_ref().map(|(g, _)| g.clone()))
    };
    if client.as_deref() == Some("lazer") {
        let lazer = b.st().tosu.lazer_files_dir.parent().map(Path::to_path_buf);
        if let Some(dir) = lazer.as_deref().and_then(lazer_mount) {
            let name = ini_name(&dir).unwrap_or_else(|| "lazer skin".into());
            return Ok(Found { dir, name, client: "lazer", stored: false });
        }
        return lazer_stored(b);
    }
    // with osu! closed, the skin of the one played last
    if client.is_none() && last.as_deref() == Some("lazer") {
        if let Ok(found) = lazer_stored(b) {
            return Ok(found);
        }
    }
    let game = game.unwrap_or_else(|| b.env.osu_dir.clone());
    let stable = match skin.or_else(|| cfg_skin(&game)).filter(|s| s.trim_matches('.') != "") {
        Some(skin) if game.join("Skins").join(&skin).is_dir() => return Ok(Found { dir: game.join("Skins").join(&skin), name: skin, client: "stable", stored: false }),
        Some(_) => "missing",
        None => "unknown",
    };
    lazer_stored(b).map_err(|_| stable)
}

// with every skin's name, for naming a copy
struct Stored {
    key: (uuid::Uuid, Option<std::time::SystemTime>),
    found: Found,
    names: Vec<String>,
}

static STORED: Mutex<Option<Stored>> = Mutex::new(None);

// the skin lazer is set to, out of its database into a folder of links to lazer's own files
fn lazer_stored(b: &Bridge) -> Result<Found, &'static str> {
    let files = b.st().tosu.lazer_files_dir.clone();
    let root = files.parent().ok_or("lazer")?;
    let id = lazer_skin_id(root).ok_or("lazer")?;
    let realm = root.join("client.realm");
    let key = (id, std::fs::metadata(&realm).and_then(|m| m.modified()).ok());
    let mut cache = STORED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache.as_ref().filter(|c| c.key == key && c.found.dir.is_dir()) {
        return Ok(c.found.clone());
    }
    // lazer may be mid-write, a second read usually lands after it
    let (skin, names) = (0..3)
        .find_map(|_| {
            let r = crate::realm::Realm::open(std::fs::read(&realm).ok()?)?;
            Some((crate::realm::lazer_skin(&r, id)?, crate::realm::skin_names(&r)?))
        })
        .ok_or("lazer")?;
    if skin.files.is_empty() {
        return Err("builtin");
    }
    let dir = mirror(&b.env.data_dir.join("lazer skins"), &files, id, &skin.files).ok_or("lazer")?;
    let found = Found { dir, name: skin.name, client: "lazer", stored: true };
    *cache = Some(Stored { key, found: found.clone(), names });
    Ok(found)
}

// a name for a copy of the skin that no lazer skin has yet: "name (Hashi)", then "(Hashi 2)" and on
fn new_name(found: &Found) -> String {
    let taken = STORED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| c.names.clone()).unwrap_or_default();
    // a copy of a copy is numbered from the original's name
    let base = found
        .name
        .strip_suffix(')')
        .and_then(|rest| rest.rfind(" (Hashi").map(|i| (i, &rest[i + 7..])))
        .filter(|(_, n)| n.is_empty() || n.strip_prefix(' ').is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit())))
        .map_or(found.name.as_str(), |(i, _)| &found.name[..i]);
    (1..)
        .map(|n| if n == 1 { format!("{base} (Hashi)") } else { format!("{base} (Hashi {n})") })
        .find(|n| !taken.contains(n))
        .unwrap_or_default()
}

pub(crate) fn apply_as_new(b: &Bridge, found: &Found, keys: usize, edits: &str, name: &str) -> Result<Value, String> {
    let name = match name.trim() {
        "" => new_name(found),
        n => n.to_string(),
    };
    let file = crate::skin_render::pack(&found.dir, keys, edits, &name, &std::env::temp_dir().join("hashi lazer skin"))?;
    crate::practice::open_in_osu(b, true, &file);
    Ok(json!({ "name": name }))
}

fn lazer_skin_id(root: &Path) -> Option<uuid::Uuid> {
    let text = std::fs::read_to_string(root.join("game.ini")).ok()?;
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == "Skin").then(|| v.trim().parse().ok()).flatten()
    })
}

// lazer keeps each file under its hash; the skin's folder is rebuilt from hard links, or copies
// where links can't be made, and named for its contents so a changed skin gets a new one
fn mirror(base: &Path, files: &Path, id: uuid::Uuid, list: &[(String, String)]) -> Option<PathBuf> {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for (name, hash) in list {
        h.update(name.as_bytes());
        h.update([0]);
        h.update(hash.as_bytes());
        h.update([0]);
    }
    let tag: String = h.finalize()[..6].iter().map(|b| format!("{b:02x}")).collect();
    let dir = base.join(format!("{id}-{tag}"));
    if dir.is_dir() {
        return Some(dir);
    }
    let part = base.join(format!("{id}-{tag}.part"));
    let _ = std::fs::remove_dir_all(&part);
    for (name, hash) in list {
        let parts: Vec<&str> = name.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
        if parts.is_empty() || parts.iter().any(|p| *p == "." || *p == ".." || p.contains(':')) || hash.len() < 2 || !hash.is_ascii() {
            continue;
        }
        let to = parts.iter().fold(part.clone(), |d, p| d.join(p));
        std::fs::create_dir_all(to.parent()?).ok()?;
        let from = files.join(&hash[..1]).join(&hash[..2]).join(hash);
        if std::fs::hard_link(&from, &to).is_err() {
            let _ = std::fs::copy(&from, &to);
        }
    }
    for e in std::fs::read_dir(base).ok()?.flatten() {
        if e.path() != part {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    std::fs::rename(&part, &dir).ok()?;
    Some(dir)
}

pub(crate) fn info(b: &Bridge) -> Value {
    match current(b) {
        Ok(f) => json!({
            "found": true,
            "name": f.name,
            "client": f.client,
            "edited": crate::skin_render::edited(&f.dir),
            "stored": f.stored,
            "unreadable": if f.client == "lazer" { crate::skin_render::tiffs(&f.dir) } else { vec![] },
            "newName": f.stored.then(|| new_name(&f)),
            // changes when Edit externally starts or ends
            "folder": f.dir.to_string_lossy(),
        }),
        Err(why) => json!({ "found": false, "why": why }),
    }
}

// the map osu! has selected, from tosu's last report
fn selected_chart(b: &Bridge) -> Option<PathBuf> {
    let s = b.st();
    let raw = s.tosu.last_frame.as_ref()?;
    let file = raw.pointer("/files/beatmap").and_then(Value::as_str).filter(|f| !f.is_empty())?;
    if raw.get("client").and_then(Value::as_str) == Some("lazer") {
        return Some(s.tosu.lazer_files_dir.join(file));
    }
    let folder = raw.pointer("/folders/beatmap").and_then(Value::as_str).filter(|f| !f.is_empty())?;
    Some(s.tosu.stable_dirs.as_ref()?.1.join(folder.replace('\\', "/")).join(file))
}

// notes for the preview to play: [column, time, end or null] in the map's own milliseconds
pub(crate) async fn chart(b: &std::sync::Arc<Bridge>, play: Option<&str>) -> (u16, Value) {
    let bytes = match play {
        Some(id) => {
            let chart = b.st().queue.items.iter().find(|i| i.id == id).and_then(|i| i.chart.clone());
            let Some(chart) = chart else { return (404, json!({ "error": "That play has no chart here." })) };
            crate::replays::read_blob(b, &chart).await.ok()
        }
        None => match selected_chart(b) {
            Some(path) => tokio::fs::read(path).await.ok(),
            None => return (404, json!({ "error": "No map is selected in osu!." })),
        },
    };
    let Some(bytes) = bytes else { return (404, json!({ "error": "Couldn't read the map." })) };
    let text = String::from_utf8_lossy(&bytes);
    let chart = match crate::beatmap::parse_chart(&text) {
        Ok(c) if (1..=10).contains(&c.keys) && !c.notes.is_empty() => c,
        Ok(_) => return (422, json!({ "error": "The preview plays 1K to 10K maps." })),
        Err(_) => return (422, json!({ "error": "That isn't a mania map." })),
    };
    let meta = |k: &str| text.lines().find_map(|l| l.trim().strip_prefix(k)).map(|v| v.trim().to_string());
    let notes: Vec<Value> = chart.notes.iter().map(|n| json!([n.column, n.time, if n.is_hold { json!(n.end_time) } else { Value::Null }])).collect();
    (200, json!({ "keys": chart.keys, "title": meta("Title:"), "version": meta("Version:"), "notes": notes }))
}

// each play's long note releases as (column, ms late), judging a replay takes a moment
static RELEASES: Mutex<Vec<(String, Arc<Vec<(usize, f64)>>)>> = Mutex::new(Vec::new());

async fn releases(b: &Arc<Bridge>, item: &crate::types::Item) -> Arc<Vec<(usize, f64)>> {
    if let Some((_, r)) = RELEASES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(id, _)| *id == item.id) {
        return r.clone();
    }
    let (Some(replay), Some(chart)) = (&item.replay, &item.chart) else { return Arc::default() };
    let (Ok(replay), Ok(chart)) = (crate::replays::read_blob(b, replay).await, crate::replays::read_blob(b, chart).await) else { return Arc::default() };
    let lazer = item.play.is_lazer();
    let judged = tokio::task::spawn_blocking(move || {
        let j = crate::analysis::judge_play(&replay, &chart, lazer).ok()?;
        if j.mods.iter().any(|m| m == "NR") {
            return Some(vec![]);
        }
        // the same offsets the analysis reports: real milliseconds, late positive
        Some(j.notes.iter().zip(&j.states).filter(|(n, s)| n.is_hold && s.pressed).filter_map(|(n, s)| Some((n.column, s.release? / j.rate))).collect())
    })
    .await;
    let r = Arc::new(judged.ok().flatten().unwrap_or_default());
    let mut cache = RELEASES.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 64 {
        cache.remove(0);
    }
    cache.push((item.id.clone(), r.clone()));
    r
}

// how the latest plays at this key count let go of long notes, or one play's when asked for
pub(crate) async fn timing(b: &Arc<Bridge>, keys: usize, play: Option<&str>) -> Value {
    let items: Vec<crate::types::Item> = {
        let s = b.st();
        let fits = |i: &&crate::types::Item| i.replay.is_some() && i.chart.is_some() && i.play.keys.as_f64().map(|k| k.round() as usize) == Some(keys);
        match play.and_then(|id| s.queue.items.iter().find(|i| i.id == id)).filter(fits) {
            Some(i) => vec![i.clone()],
            None => {
                let mut all: Vec<_> = s.queue.items.iter().filter(fits).cloned().collect();
                all.sort_by(|a, b| b.captured_at.cmp(&a.captured_at));
                all.truncate(10);
                all
            }
        }
    };
    let own = play.is_some_and(|id| items.len() == 1 && items[0].id == id);
    let mut all = vec![];
    let mut plays = 0;
    for item in &items {
        let r = releases(b, item).await;
        plays += usize::from(!r.is_empty());
        all.extend(r.iter().copied());
    }
    let mean = |v: &[f64]| (!v.is_empty()).then(|| (v.iter().sum::<f64>() / v.len() as f64 * 10.0).round() / 10.0);
    let columns: Vec<Value> = (0..keys)
        .map(|c| {
            let v: Vec<f64> = all.iter().filter(|(col, _)| *col == c).map(|(_, r)| *r).collect();
            json!({ "n": v.len(), "mean": mean(&v) })
        })
        .collect();
    let every: Vec<f64> = all.iter().map(|(_, r)| *r).collect();
    json!({ "plays": plays, "fromPlay": own, "n": every.len(), "mean": mean(&every), "columns": columns })
}
