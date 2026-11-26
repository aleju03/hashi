// the reader as a program of its own, which Hashi runs. it prints a JSON line each time what osu!
// shows changes, and one a second while nothing does: { phase, reason, pid, frame }, frame in the
// shape tosu's websocket sent. the play's key presses go on lines of their own, { keys: { reset,
// frames: [[time, keys], ...] } }, reset when they start over. it leaves once its stdin closes. on linux only this file needs
// cap_sys_ptrace, not the app with its webview
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use osu_reader::osu::lazer::score::{HitEventLog, ReplayFrameLog};
use osu_reader::osu::model::{Client, Hits, Snapshot};
use serde_json::{json, Value};

const TICK: Duration = Duration::from_millis(100);
const HEARTBEAT: Duration = Duration::from_secs(1);
// osu!'s mod bit for ScoreV2, which weighs a perfect above a 300 as lazer does
const SCORE_V2: u64 = 1 << 29;

fn main() {
    std::thread::spawn(|| {
        for _ in std::io::stdin().lock().lines() {}
        std::process::exit(0);
    });
    let hits = Arc::new(HitEventLog::new());
    let presses = Arc::new(ReplayFrameLog::new());
    let reader = osu_reader::spawn_osu_reader(hits.clone(), presses.clone());
    let mut last = String::new();
    let mut said = Instant::now() - HEARTBEAT;
    let mut config = Config::default();
    let mut columns = Columns::default();
    let mut was_playing = false;
    loop {
        let state = reader.state();
        // a new play: the last one's counts go, before its own come in
        let playing = state.snapshot.as_ref().and_then(|s| s.state_name.as_deref()) == Some("play");
        if playing && !was_playing {
            columns.0.clear();
        }
        was_playing = playing;
        columns.take(&hits);
        let frame = match (state.phase.as_str(), state.snapshot, state.packet) {
            ("healthy", Some(snapshot), Some(packet)) => {
                let mut raw = frame(&snapshot, packet);
                config.fill(&snapshot, &mut raw);
                columns.fill(&mut raw);
                // when the song's time was read, for following it live. only in play, it changes with every read
                if playing && raw.pointer("/beatmap/time").is_some_and(Value::is_object) {
                    raw["beatmap"]["time"]["readAt"] = json!(state.frame_at_ms);
                }
                Some(raw)
            }
            _ => None,
        };
        let line = json!({ "phase": state.phase, "reason": state.reason, "pid": state.pid, "frame": frame }).to_string();
        let keys = presses.drain();
        let keys = (keys.reset || !keys.frames.is_empty()).then(|| {
            let frames: Vec<_> = keys.frames.iter().map(|f| json!([f.time, f.keys])).collect();
            json!({ "keys": { "reset": keys.reset, "frames": frames } }).to_string()
        });
        let changed = line != last || said.elapsed() >= HEARTBEAT;
        if changed || keys.is_some() {
            let mut out = std::io::stdout().lock();
            let mut write = |l: &str| writeln!(out, "{l}");
            let sent = (if changed { write(&line) } else { Ok(()) }).and_then(|_| keys.as_deref().map_or(Ok(()), &mut write));
            if sent.and_then(|_| out.flush()).is_err() {
                return;
            }
            if changed {
                last = line;
                said = Instant::now();
            }
        }
        std::thread::sleep(TICK);
    }
}

// what the reader leaves out that Hashi reads: the map's mode and key count, accuracy, whether the
// results are a replay's, and file paths with this system's separator
fn frame(snapshot: &Snapshot, mut raw: Value) -> Value {
    if let Some(header) = snapshot.beatmap_file.as_ref().map(|f| &f.header) {
        raw["beatmap"]["mode"] = json!({ "number": header.mode });
        if let Some(cs) = header.circle_size {
            raw["beatmap"]["stats"]["cs"] = json!({ "original": cs, "converted": cs });
        }
    }
    let v2 = |raw: &Value, key: &str| raw.pointer(&format!("/{key}/mods/number")).and_then(Value::as_u64).is_some_and(|m| m & SCORE_V2 != 0);
    let lazer = snapshot.client == Some(Client::Lazer);
    if let Some(hits) = &snapshot.play_hits {
        raw["play"]["accuracy"] = json!(accuracy(hits, lazer || v2(&raw, "play")));
    }
    if let Some(hits) = &snapshot.result_hits {
        raw["resultsScreen"]["accuracy"] = json!(accuracy(hits, lazer || v2(&raw, "resultsScreen")));
        if snapshot.replay {
            raw["resultsScreen"]["replay"] = json!(true);
        }
    }
    if snapshot.replay && snapshot.state_name.as_deref() == Some("play") {
        raw["play"]["replay"] = json!(true);
    }
    // stable counts retries, not replays watched: what tells the two apart when the results match
    if let Some(retries) = snapshot.retries.filter(|_| snapshot.state_name.as_deref() == Some("play")) {
        raw["play"]["retries"] = json!(retries);
    }
    if let Some(menu) = snapshot.pause_menu {
        raw["game"]["pauseMenu"] = json!(menu);
    }
    if let Some(health) = snapshot.health {
        raw["game"]["health"] = json!(health);
    }
    if snapshot.failed == Some(true) {
        raw["game"]["failed"] = json!(true);
    }
    if !cfg!(windows) {
        for key in ["/files/beatmap", "/files/background", "/files/audio"] {
            if let Some(path) = raw.pointer_mut(key) {
                if let Some(s) = path.as_str() {
                    *path = json!(s.replace('\\', "/"));
                }
            }
        }
    }
    raw
}

// the play's judgements so far, by column: [perfect, great, good, ok, meh, miss]
#[derive(Default)]
struct Columns(Vec<[u32; 6]>);

const MAX_COLUMNS: usize = 18;

impl Columns {
    fn take(&mut self, hits: &HitEventLog) {
        let batch = hits.drain();
        if batch.reset {
            self.0.clear();
        }
        for event in batch.events {
            // the game's own judgement numbers, 6 for a perfect down to 1 for a miss
            let (Ok(column), Ok(result)) = (usize::try_from(event.col), usize::try_from(event.result)) else { continue };
            if column >= MAX_COLUMNS || !(1..=6).contains(&result) {
                continue;
            }
            if self.0.len() <= column {
                self.0.resize(column + 1, [0; 6]);
            }
            self.0[column][6 - result] += 1;
        }
    }

    fn fill(&self, raw: &mut Value) {
        if raw.pointer("/state/name").and_then(Value::as_str) != Some("play") || self.0.is_empty() {
            return;
        }
        let keys = raw.pointer("/beatmap/stats/cs/converted").and_then(Value::as_f64).map_or(0, |k| k as usize);
        let mut columns = self.0.clone();
        if (1..=MAX_COLUMNS).contains(&keys) && columns.len() < keys {
            columns.resize(keys, [0; 6]);
        }
        raw["play"]["columns"] = json!(columns);
    }
}

// what the game keeps in its own settings rather than where the reader looks: stable's skin, and
// lazer's account. read again every so often, stable writes its .cfg when a setting changes or it closes
#[derive(Default)]
struct Config {
    read_at: Option<Instant>,
    // the game's folder it was read for
    from: Option<(Client, String)>,
    skin: Option<String>,
    name: Option<String>,
}

const CONFIG_EVERY: Duration = Duration::from_secs(10);

impl Config {
    fn fill(&mut self, snapshot: &Snapshot, raw: &mut Value) {
        let folder = match snapshot.client {
            Some(Client::Stable) => snapshot.game_folder.clone().map(|dir| (Client::Stable, dir)),
            Some(Client::Lazer) => snapshot.songs_folder.clone().map(|files| (Client::Lazer, files)),
            None => None,
        };
        if folder != self.from || self.read_at.is_none_or(|at| at.elapsed() >= CONFIG_EVERY) {
            let file = folder.as_ref().and_then(|(client, dir)| match client {
                Client::Stable => stable_cfg(Path::new(dir)),
                Client::Lazer => Path::new(dir).parent().map(|dir| dir.join("game.ini")),
            });
            let text = file.and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
            let value = |key: &str| {
                text.lines().find_map(|line| {
                    let (k, v) = line.split_once('=')?;
                    (k.trim() == key).then(|| v.trim().to_string()).filter(|v| !v.is_empty())
                })
            };
            self.skin = value("Skin");
            self.name = value("Username");
            self.from = folder;
            self.read_at = Some(Instant::now());
        }
        if snapshot.client == Some(Client::Stable) {
            if let Some(skin) = &self.skin {
                raw["folders"]["skin"] = json!(skin);
            }
        }
        // stable's .cfg keeps whatever was last typed into its login box, lazer's the account it signs in with
        if let Some(name) = self.name.as_ref().filter(|_| snapshot.client == Some(Client::Lazer)) {
            raw["profile"]["name"] = json!(name);
        }
    }
}

// osu!.<name>.cfg, the newest when several people have played on this copy
fn stable_cfg(game: &Path) -> Option<PathBuf> {
    std::fs::read_dir(game)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.starts_with("osu!.") && name.ends_with(".cfg") && name != "osu!.cfg"
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

// in percent. stable's scores a 300 and a perfect alike, lazer and ScoreV2 a perfect a little higher
fn accuracy(h: &Hits, perfect_counts_more: bool) -> f64 {
    let total: f64 = [h.geki, h.n300, h.katu, h.n100, h.n50, h.miss].into_iter().map(f64::from).sum();
    if total == 0.0 {
        return 0.0;
    }
    let top = if perfect_counts_more { 305.0 } else { 300.0 };
    let points = top * f64::from(h.geki) + 300.0 * f64::from(h.n300) + 200.0 * f64::from(h.katu) + 100.0 * f64::from(h.n100) + 50.0 * f64::from(h.n50);
    100.0 * points / (top * total)
}
