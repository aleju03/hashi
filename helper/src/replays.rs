use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use md5::{Digest as _, Md5};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::types::{Blob, Item, ItemError};
use crate::{iso_from_ms, log, now_ms, osr, queue, Bridge, AUTOMATED_MODS, REPLAY_TICK_TOLERANCE_MS, REPLAY_WAIT_MS};

// lazer saves the replay under its hash with no name, so look at what got written since the play
const LAZER_SCAN_MS: i64 = 2000;

pub(crate) struct Fresh {
    name: String,
    // lazer: the full path, stable: in Data/r
    path: Option<PathBuf>,
    md5: String,
    // stable: from the file name, lazer: mtime
    at: i64,
    seen_at: i64,
}

#[derive(Default)]
pub(crate) struct Replays {
    seen: HashSet<String>,
    fresh: Vec<Fresh>,
    // Data/r's mtime at the last full look
    dir_stamp: Option<std::time::SystemTime>,
    dir: Option<PathBuf>,
    // path -> mtime
    seen_lazer: HashMap<PathBuf, i64>,
    fresh_lazer: Vec<Fresh>,
    last_lazer_scan: i64,
    matching: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChartEntry {
    size: u64,
    mtime_ms: f64,
    md5: String,
}

pub(crate) fn md5_hex(bytes: &[u8]) -> String {
    hex(&Md5::digest(bytes))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn mtime_ms(meta: &std::fs::Metadata) -> f64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

fn scan_replay_dir(b: &Arc<Bridge>, initial: bool) {
    // a new file bumps the folder's mtime. a second one in the same tick wouldn't, so a fresh
    // mtime gets looked at again
    let path = b.replay_dir();
    // once tosu says where stable is, what's already in there is old
    let initial = {
        let mut s = b.st();
        let moved = s.replays.dir.as_ref() != Some(&path);
        if moved {
            s.replays.dir = Some(path.clone());
            s.replays.dir_stamp = None;
        }
        initial || moved
    };
    let Ok(stamp) = std::fs::metadata(&path).and_then(|m| m.modified()) else { return };
    if b.st().replays.dir_stamp == Some(stamp) {
        return;
    }
    let Ok(dir) = std::fs::read_dir(&path) else { return };
    let names: Vec<String> = dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    let settled = stamp.elapsed().is_ok_and(|d| d > Duration::from_secs(2));
    let mut s = b.st();
    s.replays.dir_stamp = settled.then_some(stamp);
    for name in names {
        if s.replays.seen.contains(&name) {
            continue;
        }
        let Some((md5, ended_at)) = osr::parse_replay_name(&name) else { continue };
        s.replays.seen.insert(name.clone());
        if initial {
            continue;
        }
        // imported replays keep their old time
        if (now_ms() - ended_at).abs() > REPLAY_WAIT_MS {
            log!("ignore {name}: its time is not recent (an imported replay?)");
            continue;
        }
        s.replays.fresh.push(Fresh { path: Some(path.join(&name)), name, md5, at: ended_at, seen_at: now_ms() });
    }
}

pub(crate) fn watch_replay_dir(b: &Arc<Bridge>) {
    scan_replay_dir(b, true);
    let b2 = b.clone();
    // polled, fs events miss stuff under wine anyway
    b.spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            scan_replay_dir(&b2, false);
            let b3 = b2.clone();
            b2.spawn(async move { match_replays(&b3).await });
        }
    });
}

fn scan_lazer_files(files: &Path, since: f64, seen: &HashMap<PathBuf, i64>) -> Vec<Fresh> {
    let mut found = vec![];
    let Ok(tops) = std::fs::read_dir(files) else { return found };
    for a in tops.flatten().filter(|a| a.file_name().len() == 1) {
        let Ok(subs) = std::fs::read_dir(a.path()) else { continue };
        for ab in subs.flatten() {
            // a new file bumps its folder's mtime, so most folders are skipped here
            let Ok(st) = ab.metadata() else { continue };
            if mtime_ms(&st) < since {
                continue;
            }
            let Ok(names) = std::fs::read_dir(ab.path()) else { continue };
            for f in names.flatten() {
                let file = f.path();
                if seen.contains_key(&file) {
                    continue;
                }
                let Ok(fst) = f.metadata() else { continue };
                if !fst.is_file() || mtime_ms(&fst) < since {
                    continue;
                }
                let md5 = read_head(&file, 39).and_then(|h| osr::lazer_replay_md5(&h));
                found.push(Fresh {
                    name: f.file_name().to_string_lossy().into_owned(),
                    path: Some(file),
                    md5: md5.unwrap_or_default(),
                    at: mtime_ms(&fst) as i64,
                    seen_at: now_ms(),
                });
            }
        }
    }
    found
}

fn read_head(file: &Path, n: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = vec![0; n];
    let mut f = std::fs::File::open(file).ok()?;
    let mut got = 0;
    while got < n {
        match f.read(&mut buf[got..]).ok()? {
            0 => break,
            k => got += k,
        }
    }
    buf.truncate(got);
    Some(buf)
}

pub(crate) async fn match_replays(b: &Arc<Bridge>) {
    let (waiting, lazer_since, files) = {
        let mut s = b.st();
        if s.replays.matching {
            return;
        }
        s.replays.matching = true;
        let now = now_ms();
        s.replays.fresh.retain(|f| now - f.seen_at <= REPLAY_WAIT_MS);
        s.replays.fresh_lazer.retain(|f| now - f.seen_at <= REPLAY_WAIT_MS);
        let waiting: Vec<Item> = s.queue.items.iter().filter(|i| i.phase == "waiting_replay").cloned().collect();
        let lazer_since = waiting.iter().filter(|i| i.play.is_lazer()).map(|i| i.detected_at_ms).min();
        let due = now - s.replays.last_lazer_scan >= LAZER_SCAN_MS;
        if lazer_since.is_some() && due {
            s.replays.last_lazer_scan = now;
        }
        (waiting, lazer_since.filter(|_| due), s.tosu.lazer_files_dir.clone())
    };

    if let Some(since) = lazer_since {
        let since = since - REPLAY_TICK_TOLERANCE_MS;
        let seen = {
            let mut s = b.st();
            // the oldest waiting play only gets newer, so files from before it never come up again
            s.replays.seen_lazer.retain(|_, at| *at >= since);
            s.replays.seen_lazer.clone()
        };
        let found = tokio::task::spawn_blocking(move || scan_lazer_files(&files, since as f64, &seen)).await.unwrap_or_default();
        let mut s = b.st();
        for f in found {
            s.replays.seen_lazer.insert(f.path.clone().unwrap(), f.at);
            if !f.md5.is_empty() {
                s.replays.fresh_lazer.push(f);
            }
        }
    }

    let mut matched: HashSet<&str> = HashSet::new();
    for item in &waiting {
        let lazer = item.play.is_lazer();
        let file = {
            let mut s = b.st();
            let pool = if lazer { &mut s.replays.fresh_lazer } else { &mut s.replays.fresh };
            let idx = closest_replay(pool, item, waiting.iter().filter(|o| o.id != item.id && !matched.contains(o.id.as_str())));
            idx.map(|i| pool.remove(i))
        };
        if let Some(file) = file {
            matched.insert(&item.id);
            if let Err(e) = attach_replay(b, &item.id, file).await {
                queue::fail(b, &item.id, "failed", "replay_unreadable", &e);
            }
        } else if now_ms() - item.detected_at_ms > REPLAY_WAIT_MS {
            queue::fail(b, &item.id, "failed", "no_replay", "osu! saved no replay for this play.");
        }
    }
    b.st().replays.matching = false;
}

fn could_be(f: &Fresh, item: &Item) -> bool {
    f.md5 == item.play.beatmap_md5
        && if item.play.is_lazer() {
            // lazer's file has no time in its name, it just has to be written after the play
            f.at >= item.detected_at_ms - REPLAY_TICK_TOLERANCE_MS
        } else {
            (f.at - item.detected_at_ms).abs() <= REPLAY_TICK_TOLERANCE_MS
        }
}

// a quick retry of the same map has two plays and two files close together, each play takes the
// nearest file that isn't nearer to another play
fn closest_replay<'a>(pool: &[Fresh], item: &Item, others: impl Iterator<Item = &'a Item> + Clone) -> Option<usize> {
    let gap = |f: &Fresh, i: &Item| (f.at - i.detected_at_ms).abs();
    pool.iter()
        .enumerate()
        .filter(|(_, f)| could_be(f, item))
        .filter(|(_, f)| !others.clone().any(|o| o.play.is_lazer() == item.play.is_lazer() && could_be(f, o) && gap(f, o) < gap(f, item)))
        .min_by_key(|(_, f)| gap(f, item))
        .map(|(i, _)| i)
}

async fn attach_replay(b: &Arc<Bridge>, id: &str, file: Fresh) -> Result<(), String> {
    let full = file.path.clone().unwrap_or_else(|| b.replay_dir().join(&file.name));
    // osu! might still be writing it: same size and time twice in a row, then read it once
    let stamp = || async {
        let st = tokio::fs::metadata(&full).await.map_err(|e| e.to_string())?;
        Ok::<_, String>((st.len(), st.modified().ok()))
    };
    let mut got = None;
    for _ in 0..20 {
        let a = stamp().await?;
        tokio::time::sleep(Duration::from_millis(250)).await;
        let c = stamp().await?;
        if a == c {
            if c.0 > crate::MAX_REPLAY_BYTES {
                return Err(format!("{} is bigger than {} MB.", file.name, crate::MAX_REPLAY_BYTES >> 20));
            }
            let bytes = crate::read_capped(&full, crate::MAX_REPLAY_BYTES).await.map_err(|e| e.to_string())?;
            if let Ok(header) = osr::parse_osr(&bytes) {
                if bytes.len() as u64 == c.0 && header.complete {
                    got = Some((bytes, header));
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let Some((bytes, header)) = got else {
        return Err(format!("{} never became a complete replay.", file.name));
    };
    let Some(item) = b.st().queue.items.iter().find(|i| i.id == id).cloned() else { return Ok(()) };
    if header.mode != 3 {
        return Ok(queue::fail(b, id, "failed", "not_mania", "The replay is not a mania play."));
    }
    if header.beatmap_md5.as_deref().map(str::to_lowercase).as_deref() != Some(item.play.beatmap_md5.as_str()) {
        return Ok(queue::fail(b, id, "failed", "replay_mismatch", "The replay header names a different chart."));
    }
    if header.mods & AUTOMATED_MODS != 0 {
        return Ok(queue::fail(b, id, "failed", "automated", "The replay carries automated mods."));
    }

    let mut replay = keep_blob(b, &bytes).await?;
    replay.source = Some(full.to_string_lossy().into_owned());
    replay.played_at = Some(iso_from_ms(header.played_at));
    log!("replay {} matched ({} bytes)", file.name, bytes.len());

    let lazer = item.play.is_lazer();
    let chart_file = if lazer {
        item.play.chart_file.clone().map(PathBuf::from).filter(|f| f.exists())
    } else {
        find_chart(b, &item.play.beatmap_md5, item.play.set.as_i64(), item.play.title.as_str().map(String::from)).await?
    };
    let Some(chart_file) = chart_file else {
        let msg = if lazer { "Couldn't find lazer's copy of this chart." } else { "No .osu in Songs hashes to this chart's MD5." };
        update(b, id, |i| {
            i.replay = Some(replay);
            if let Some(name) = &header.player_name {
                i.play.player_name = name.clone().into();
            }
        });
        return Ok(queue::fail(b, id, "failed", "chart_not_found", msg));
    };
    let chart_bytes = crate::read_capped(&chart_file, crate::MAX_CHART_BYTES).await.map_err(|e| e.to_string())?;
    if md5_hex(&chart_bytes) != item.play.beatmap_md5 {
        return Ok(queue::fail(b, id, "failed", "chart_changed", "The chart changed while it was being read."));
    }
    let mut chart = keep_blob(b, &chart_bytes).await?;
    chart.md5 = Some(item.play.beatmap_md5.clone());
    chart.source = Some(chart_file.to_string_lossy().into_owned());
    log!("chart {} matched", chart_file.file_name().unwrap_or_default().to_string_lossy());

    update(b, id, |i| {
        i.replay = Some(replay);
        if let Some(name) = &header.player_name {
            i.play.player_name = name.clone().into();
        }
        i.chart = Some(chart);
        i.phase = if i.practice {
            "practice".into()
        } else if i.kept {
            "kept".into()
        } else if i.binding.is_some() {
            "send".into()
        } else {
            "unbound".into()
        };
        if i.binding.is_none() && !i.practice && !i.kept {
            i.error = ItemError::new("not_connected", "Not connected when this play finished.");
        }
    });
    b.save_queue().await;
    b.broadcast();
    queue::wake_queue(b, 0);
    // spawned so sending isn't held up
    let (b2, id2) = (b.clone(), id.to_string());
    b.spawn(async move { crate::goals::on_play(&b2, &id2).await });
    Ok(())
}

fn update(b: &Arc<Bridge>, id: &str, f: impl FnOnce(&mut Item)) {
    if let Some(item) = b.st().queue.get_mut(id) {
        f(item);
    }
}

// set folder first, then everything. md5s cached by size+mtime
async fn find_chart(b: &Arc<Bridge>, md5: &str, set: Option<i64>, title: Option<String>) -> Result<Option<PathBuf>, String> {
    let cached = b.st().chart_index.take();
    let index = match cached {
        Some(i) => i,
        None => crate::read_json_file(&b.env.chart_index_file).await.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
    };
    let songs = b.songs_dir();
    let md5 = md5.to_string();
    let (found, index, dirty) = tokio::task::spawn_blocking(move || search_songs(&songs, &md5, set, title, index))
        .await
        .map_err(|e| e.to_string())??;
    if dirty {
        let v = serde_json::to_value(&index).unwrap_or_default();
        let _ = crate::write_json_file(&b.env.chart_index_file, &v).await;
    }
    b.st().chart_index = Some(index);
    Ok(found)
}

type SearchResult = Result<(Option<PathBuf>, HashMap<String, ChartEntry>, bool), String>;

fn search_songs(songs: &Path, md5: &str, set: Option<i64>, title: Option<String>, mut index: HashMap<String, ChartEntry>) -> SearchResult {
    let folders: Vec<String> = std::fs::read_dir(songs)
        .map_err(|e| format!("Cannot read {}: {e}", songs.display()))?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let mut preferred: Vec<&String> = vec![];
    if let Some(set) = set.filter(|s| *s > 0) {
        let prefix = format!("{set} ");
        preferred.extend(folders.iter().filter(|f| f.starts_with(&prefix) || **f == set.to_string()));
    }
    if let Some(title) = title.map(|t| t.to_lowercase()) {
        let more: Vec<&String> = folders.iter().filter(|f| f.to_lowercase().contains(&title) && !preferred.contains(f)).collect();
        preferred.extend(more);
    }
    let rest = folders.iter().filter(|f| !preferred.contains(f));
    let ordered: Vec<&String> = preferred.iter().copied().chain(rest).collect();
    let mut dirty = false;
    for folder in ordered {
        let Ok(dir) = std::fs::read_dir(songs.join(folder)) else { continue };
        for e in dir.flatten() {
            let file = e.path();
            if !e.file_type().is_ok_and(|t| t.is_file()) || !e.file_name().to_string_lossy().to_lowercase().ends_with(".osu") {
                continue;
            }
            let Ok(st) = e.metadata() else { continue };
            let key = file.to_string_lossy().into_owned();
            let (size, mtime) = (st.len(), mtime_ms(&st));
            let hash = match index.get(&key) {
                Some(c) if c.size == size && (c.mtime_ms - mtime).abs() < 1.0 => c.md5.clone(),
                _ => {
                    let Ok(bytes) = crate::read_capped_sync(&file, crate::MAX_CHART_BYTES) else { continue };
                    let h = md5_hex(&bytes);
                    index.insert(key, ChartEntry { size, mtime_ms: mtime, md5: h.clone() });
                    dirty = true;
                    h
                }
            };
            if hash == md5 {
                return Ok((Some(file), index, dirty));
            }
        }
    }
    Ok((None, index, dirty))
}

// keep a copy so retries send the exact same bytes
async fn keep_blob(b: &Bridge, bytes: &[u8]) -> Result<Blob, String> {
    let digest = sha256_hex(bytes);
    let file = b.env.blob_dir.join(&digest);
    crate::create_private_dir(&b.env.blob_dir).await.map_err(|e| e.to_string())?;
    // whole or not there, a half-written one would be kept for good
    if !file.exists() {
        let tmp = file.with_extension("tmp");
        let written = match crate::write_private(&tmp, bytes).await {
            Ok(()) => tokio::fs::rename(&tmp, &file).await,
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e.to_string());
        }
    }
    Ok(Blob {
        sha256: digest,
        byte_length: bytes.len() as u64,
        file: file.to_string_lossy().into_owned(),
        md5: None,
        source: None,
        played_at: None,
    })
}

pub(crate) async fn read_blob(b: &Bridge, blob: &Blob) -> Result<Vec<u8>, String> {
    // by hash, the stored path can point at an old data folder
    let bytes = crate::read_capped(&b.env.blob_dir.join(&blob.sha256), crate::MAX_REPLAY_BYTES).await.map_err(|e| e.to_string())?;
    if sha256_hex(&bytes) != blob.sha256 {
        return Err("The stored copy no longer matches its hash.".into());
    }
    Ok(bytes)
}

pub(crate) fn image_type(file: &Path) -> Option<&'static str> {
    match file.extension()?.to_string_lossy().to_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

pub(crate) fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

fn inside(root: &Path, file: &Path) -> bool {
    match (std::fs::canonicalize(root), std::fs::canonicalize(file)) {
        (Ok(r), Ok(f)) => f.starts_with(&r) && f != r,
        _ => false,
    }
}

fn no_dots(rel: &str) -> bool {
    !rel.replace('\\', "/").split('/').any(|p| p == "..")
}

pub(crate) async fn cover_file(b: &Arc<Bridge>, item: &Item) -> Option<PathBuf> {
    if let Some(cover) = item.play.cover.as_ref().filter(|c| c.lazer == Some(true)) {
        let root = b.st().tosu.lazer_files_dir.clone();
        let file = root.join(&cover.file);
        return (no_dots(&cover.file) && inside(&root, &file)).then_some(file);
    }
    let songs = &b.songs_dir();
    let mut tries: Vec<PathBuf> = vec![];
    if let Some(cover) = &item.play.cover {
        if let Some(folder) = &cover.folder {
            tries.push(songs.join(folder.replace('\\', "/")).join(&cover.file));
        }
    }
    // lazer's chart sits alone in files/, its background isn't next to it
    if let (Some(source), false) = (item.chart.as_ref().and_then(|c| c.source.clone()), item.play.is_lazer()) {
        if let Ok(bytes) = crate::read_capped(Path::new(&source), crate::MAX_CHART_BYTES).await {
            if let Some(bg) = events_background(&String::from_utf8_lossy(&bytes)) {
                tries.push(Path::new(&source).parent().unwrap_or(Path::new("")).join(bg.replace('\\', "/")));
            }
        }
    }
    for want in tries {
        if !no_dots(&want.to_string_lossy()) || image_type(&want).is_none() {
            continue;
        }
        if want.exists() {
            if inside(songs, &want) {
                return Some(want);
            }
            continue;
        }
        // .osu files written on windows don't care about case
        let (Some(dir), Some(name)) = (want.parent(), want.file_name()) else { continue };
        let lower = name.to_string_lossy().to_lowercase();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().to_lowercase() == lower && inside(songs, &e.path()) {
                    return Some(e.path());
                }
            }
        }
    }
    None
}

// 0,0,"bg.jpg" under [Events]
fn events_background(text: &str) -> Option<String> {
    let mut in_events = false;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('[') {
            in_events = in_events || line == "[Events]";
            continue;
        }
        if in_events {
            if let Some(rest) = line.strip_prefix("0,0,\"") {
                if let Some(end) = rest.find('"') {
                    return Some(rest[..end].to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{closest_replay, Fresh};
    use crate::types::Item;

    const MD5: &str = "0123456789abcdef0123456789abcdef";

    fn play(id: &str, at: i64) -> Item {
        serde_json::from_value(serde_json::json!({
            "id": id, "capturedAt": "", "phase": "waiting_replay", "detectedAtMs": at, "play": { "beatmapMd5": MD5 },
        }))
        .unwrap()
    }

    fn file(at: i64) -> Fresh {
        Fresh { name: format!("{MD5}-{at}.osr"), path: None, md5: MD5.into(), at, seen_at: 0 }
    }

    #[test]
    fn retries_keep_their_own_replay() {
        let (a, b) = (play("a", 100_000), play("b", 190_000));
        // listed in the wrong order
        let pool = [file(190_500), file(100_400)];
        assert_eq!(closest_replay(&pool, &a, [&b].into_iter()), Some(1));
        assert_eq!(closest_replay(&pool, &b, [&a].into_iter()), Some(0));
        // only b's is there yet, a waits for its own
        let pool = [file(190_500)];
        assert_eq!(closest_replay(&pool, &a, [&b].into_iter()), None);
        assert_eq!(closest_replay(&pool, &b, [&a].into_iter()), Some(0));
    }
}
