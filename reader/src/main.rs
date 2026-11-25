// the reader as a program of its own, which Hashi runs. it prints a JSON line each time what osu!
// shows changes, and one a second while nothing does: { phase, reason, pid, frame }, frame in the
// shape tosu's websocket sent. it leaves once its stdin closes. on linux only this file needs
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
    let reader = osu_reader::spawn_osu_reader(Arc::new(HitEventLog::new()), Arc::new(ReplayFrameLog::default()));
    let mut last = String::new();
    let mut said = Instant::now() - HEARTBEAT;
    let mut config = Config::default();
    loop {
        let state = reader.state();
        let frame = match (state.phase.as_str(), state.snapshot, state.packet) {
            ("healthy", Some(snapshot), Some(packet)) => {
                let mut raw = frame(&snapshot, packet);
                config.fill(&snapshot, &mut raw);
                Some(raw)
            }
            _ => None,
        };
        let line = json!({ "phase": state.phase, "reason": state.reason, "pid": state.pid, "frame": frame }).to_string();
        if line != last || said.elapsed() >= HEARTBEAT {
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
                return;
            }
            last = line;
            said = Instant::now();
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

// what the game keeps in its own settings rather than where the reader looks: stable's skin, and
// lazer's account. read again every so often, stable writes its .cfg when a setting changes or it closes
#[derive(Default)]
struct Config {
    read_at: Option<Instant>,
    from: Option<PathBuf>,
    skin: Option<String>,
    name: Option<String>,
}

const CONFIG_EVERY: Duration = Duration::from_secs(10);

impl Config {
    fn fill(&mut self, snapshot: &Snapshot, raw: &mut Value) {
        let file = match snapshot.client {
            Some(Client::Stable) => snapshot.game_folder.as_deref().and_then(|dir| stable_cfg(Path::new(dir))),
            Some(Client::Lazer) => snapshot.songs_folder.as_deref().and_then(|files| Path::new(files).parent()).map(|dir| dir.join("game.ini")),
            None => None,
        };
        if file != self.from || self.read_at.is_none_or(|at| at.elapsed() >= CONFIG_EVERY) {
            let text = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
            let value = |key: &str| {
                text.lines().find_map(|line| {
                    let (k, v) = line.split_once('=')?;
                    (k.trim() == key).then(|| v.trim().to_string()).filter(|v| !v.is_empty())
                })
            };
            self.skin = value("Skin");
            self.name = value("Username");
            self.from = file;
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
    let total = h.geki + h.n300 + h.katu + h.n100 + h.n50 + h.miss;
    if total == 0 {
        return 0.0;
    }
    let top = if perfect_counts_more { 305.0 } else { 300.0 };
    let points = top * f64::from(h.geki) + 300.0 * f64::from(h.n300) + 200.0 * f64::from(h.katu) + 100.0 * f64::from(h.n100) + 50.0 * f64::from(h.n50);
    100.0 * points / (top * f64::from(total))
}
