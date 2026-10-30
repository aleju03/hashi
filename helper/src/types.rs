use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub connection: Option<Connection>,
    #[serde(default)]
    pub key: Option<Key>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Settings>,
    // the last tosu seen running, it can be anywhere
    #[serde(default, rename = "tosuPath", skip_serializing_if = "Option::is_none")]
    pub tosu_path: Option<std::path::PathBuf>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    #[serde(default)]
    pub issuer: Value,
    #[serde(default)]
    pub environment: Value,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Value,
    #[serde(default)]
    pub installation_id: Value,
    #[serde(default)]
    pub user_id: Value,
    #[serde(default)]
    pub username: Value,
    #[serde(default)]
    pub connected_at: Value,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Key {
    pub private_key_pem: String,
    pub jwk: Jwk,
    pub thumbprint: String,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub y: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Settings {
    #[serde(default)]
    pub presence: bool,
    #[serde(default, rename = "tosuAutostart")]
    pub tosu_autostart: bool,
    #[serde(default, rename = "startInTray")]
    pub start_in_tray: bool,
    // only the live status, finished plays aren't sent
    #[serde(default, rename = "presenceOnly")]
    pub presence_only: bool,
    // dans are rough, some would rather not see them
    #[serde(default, rename = "hideDans", skip_serializing_if = "std::ops::Not::not")]
    pub hide_dans: bool,
    // turns off analysis, goals and the profile, and the work behind them
    #[serde(default, rename = "hideAnalysis", skip_serializing_if = "std::ops::Not::not")]
    pub hide_analysis: bool,
    #[serde(default, rename = "onTop", skip_serializing_if = "std::ops::Not::not")]
    pub on_top: bool,
    // the hand whose thumb plays the middle column of 5K, 7K, 9K. unset is the left
    #[serde(default, rename = "thumbRight", skip_serializing_if = "std::ops::Not::not")]
    pub thumb_right: bool,
    // the site's theme color: hue 0-359, saturation 0-100. unset is the default pink
    #[serde(default, rename = "themeHue", skip_serializing_if = "Option::is_none")]
    pub theme_hue: Option<u16>,
    #[serde(default, rename = "themeSat", skip_serializing_if = "Option::is_none")]
    pub theme_sat: Option<u8>,
    // the version whose changelog was read. unset on configs from before the changelog, so those see it
    #[serde(default, rename = "notesRead", skip_serializing_if = "Option::is_none")]
    pub notes_read: Option<String>,
    #[serde(default, rename = "guideRead", skip_serializing_if = "std::ops::Not::not")]
    pub guide_read: bool,
    // the finger on each column, by key count ("4": ["ring", "index", "index", "middle"]). unset is the usual layout
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub fingers: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    #[serde(default)]
    pub session_id: String,
    pub captured_at: String,
    #[serde(default)]
    pub detected_at_ms: i64,
    #[serde(default)]
    pub binding: Option<Binding>,
    pub play: Play,
    pub phase: String,
    #[serde(default)]
    pub replay: Option<Blob>,
    #[serde(default)]
    pub chart: Option<Blob>,
    #[serde(default)]
    pub submission_id: Option<String>,
    #[serde(default)]
    pub receipt: Option<Value>,
    #[serde(default)]
    pub chart_sent: bool,
    #[serde(default)]
    pub error: Option<ItemError>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub next_attempt_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart_needed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll_delay: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_from: Option<String>,
    // the player held it back, it waits until they send it
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub on_hold: bool,
    // a play of a practice map Hashi made: kept here for its analysis, never sent
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub practice: bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    #[serde(default)]
    pub issuer: Value,
    #[serde(default)]
    pub environment: Value,
    #[serde(default)]
    pub user_id: Value,
    #[serde(default)]
    pub installation_id: Value,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Play {
    #[serde(default)]
    pub client: Option<String>,
    pub beatmap_md5: String,
    #[serde(default)]
    pub artist: Value,
    #[serde(default)]
    pub title: Value,
    #[serde(default)]
    pub version: Value,
    #[serde(default)]
    pub set: Value,
    #[serde(default)]
    pub keys: Value,
    #[serde(default)]
    pub cover: Option<Cover>,
    #[serde(default)]
    pub chart_file: Option<String>,
    // the song, where tosu said it was when the play finished: lazer keeps it under a hash
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_file: Option<String>,
    #[serde(default)]
    pub mods: String,
    #[serde(default)]
    pub player_name: Value,
    #[serde(default)]
    pub score: Value,
    #[serde(default)]
    pub accuracy: Value,
    #[serde(default)]
    pub result_created_at: Value,
}

impl Play {
    pub fn is_lazer(&self) -> bool {
        self.client.as_deref() == Some("lazer")
    }
}

// relative to Songs (folder + file), or to lazer's files/ (lazer: true)
#[derive(Serialize, Deserialize, Clone)]
pub struct Cover {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lazer: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    pub file: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Blob {
    pub sha256: String,
    pub byte_length: u64,
    #[serde(default)]
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub played_at: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ItemError {
    pub code: String,
    #[serde(default)]
    pub message: Option<String>,
}

impl ItemError {
    pub fn new(code: &str, message: impl Into<String>) -> Option<Self> {
        Some(Self { code: code.into(), message: Some(message.into()) })
    }
}

// items that don't parse are kept as they were, not dropped on the next save
#[derive(Default)]
pub struct Queue {
    pub items: Vec<Item>,
    pub unparsed: Vec<Value>,
}

impl Queue {
    pub fn from_value(v: Value) -> Self {
        let mut q = Queue::default();
        for raw in v.get("items").and_then(Value::as_array).cloned().unwrap_or_default() {
            match serde_json::from_value::<Item>(raw.clone()) {
                Ok(item) => q.items.push(item),
                Err(e) => {
                    crate::log!("queue: kept an item it can't read ({e})");
                    q.unparsed.push(raw);
                }
            }
        }
        q
    }

    pub fn to_value(&self) -> Value {
        let mut items: Vec<Value> = self.items.iter().map(|i| serde_json::to_value(i).unwrap_or(Value::Null)).collect();
        items.extend(self.unparsed.iter().cloned());
        serde_json::json!({ "items": items })
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

pub fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}
