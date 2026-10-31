use std::path::PathBuf;

use serde_json::{json, Map, Value};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fps {
    Vsync,
    Times(u32),
    Unlimited,
}

pub const FPS: [(Fps, &str); 5] = [(Fps::Vsync, "VSync"), (Fps::Times(2), "2×"), (Fps::Times(4), "4×"), (Fps::Times(8), "8×"), (Fps::Unlimited, "Unlimited")];

#[derive(Clone, PartialEq)]
pub struct Game {
    pub auto: bool,
    // None: like the client osu! is
    pub lazer: Option<bool>,
    // by key count, KeyCode names left to right
    pub binds: Map<String, Value>,
    // scroll speed as osu! counts it, None: the skin page's
    pub speed: Option<f64>,
    pub volume: f64,
    pub offset: f64,
    pub mute_osu: bool,
    pub fps: Fps,
}

pub const SPEED: (f64, f64) = (1.0, 40.0);

// the ms a note takes at a scroll speed to fall to a judgement line `hit` down the 480 tall stage.
// Lazer keeps the notes' speed whatever HitPosition is, 402 its default
pub fn speed_ms(speed: f64, hit: f64) -> f64 {
    11485.0 / speed * hit / 402.0
}

// by key count, KeyCode names, which are KeyboardEvent.code's too
const DEFAULT_KEYS: [&[&str]; 11] = [
    &[],
    &["Space"],
    &["KeyF", "KeyJ"],
    &["KeyF", "Space", "KeyJ"],
    &["KeyD", "KeyF", "KeyJ", "KeyK"],
    &["KeyD", "KeyF", "Space", "KeyJ", "KeyK"],
    &["KeyS", "KeyD", "KeyF", "KeyJ", "KeyK", "KeyL"],
    &["KeyS", "KeyD", "KeyF", "Space", "KeyJ", "KeyK", "KeyL"],
    &["KeyA", "KeyS", "KeyD", "KeyF", "KeyJ", "KeyK", "KeyL", "Semicolon"],
    &["KeyA", "KeyS", "KeyD", "KeyF", "Space", "KeyJ", "KeyK", "KeyL", "Semicolon"],
    &["KeyA", "KeyS", "KeyD", "KeyF", "KeyV", "KeyN", "KeyJ", "KeyK", "KeyL", "Semicolon"],
];

fn file() -> PathBuf {
    mania_bridge::Env::from_env().data_dir.join("game-window.json")
}

impl Game {
    pub fn load() -> Game {
        let v: Value = std::fs::read(file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let fps = match v.get("fps").and_then(Value::as_str).unwrap_or("x2") {
            "vsync" => Fps::Vsync,
            "unlimited" => Fps::Unlimited,
            x => Fps::Times(x.trim_start_matches('x').parse().unwrap_or(2).clamp(1, 8)),
        };
        Game {
            auto: v.get("mode").and_then(Value::as_str) != Some("play"),
            lazer: v.get("judge").and_then(Value::as_str).map(|j| j == "lazer"),
            binds: v.get("binds").and_then(Value::as_object).cloned().unwrap_or_default(),
            speed: v.get("speed").and_then(Value::as_f64).map(|s| s.round().clamp(SPEED.0, SPEED.1)),
            volume: v.get("volume").and_then(Value::as_f64).unwrap_or(60.0).clamp(0.0, 100.0),
            offset: v.get("offset").and_then(Value::as_f64).unwrap_or(0.0),
            mute_osu: v.get("muteOsu").and_then(Value::as_bool).unwrap_or(true),
            fps,
        }
    }

    pub fn save(&self) {
        let fps = match self.fps {
            Fps::Vsync => "vsync".to_string(),
            Fps::Unlimited => "unlimited".to_string(),
            Fps::Times(n) => format!("x{n}"),
        };
        let v = json!({
            "mode": if self.auto { "auto" } else { "play" },
            "judge": self.lazer.map(|l| if l { "lazer" } else { "stable" }),
            "binds": self.binds,
            "speed": self.speed,
            "volume": self.volume,
            "offset": self.offset,
            "muteOsu": self.mute_osu,
            "fps": fps,
        });
        let _ = std::fs::write(file(), serde_json::to_vec_pretty(&v).unwrap_or_default());
    }

    pub fn keys_for(&self, keys: usize) -> Vec<String> {
        match self.binds.get(&keys.to_string()).and_then(Value::as_array) {
            Some(list) if list.len() == keys => list.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
            _ => DEFAULT_KEYS.get(keys).map_or(vec![], |k| k.iter().map(|s| s.to_string()).collect()),
        }
    }

    pub fn own_keys(&self, keys: usize) -> bool {
        self.binds.contains_key(&keys.to_string())
    }
}

pub fn key_name(code: &str) -> String {
    let named = match code {
        "Space" => "Space",
        "Semicolon" => ";",
        "Quote" => "'",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Backslash" => "\\",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Minus" => "-",
        "Equal" => "=",
        "Backquote" => "`",
        "ShiftLeft" => "L Shift",
        "ShiftRight" => "R Shift",
        "ControlLeft" => "L Ctrl",
        "ControlRight" => "R Ctrl",
        "AltLeft" => "L Alt",
        "AltRight" => "R Alt",
        "CapsLock" => "Caps",
        "Tab" => "Tab",
        "Enter" => "Enter",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    let s = code.strip_prefix("Key").or_else(|| code.strip_prefix("Digit")).unwrap_or(code);
    s.strip_prefix("Numpad").map_or_else(|| s.strip_prefix("Arrow").unwrap_or(s).to_string(), |n| format!("Num {n}"))
}
