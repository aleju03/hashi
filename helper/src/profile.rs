use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Map, Value};

use crate::types::{text, Item};
use crate::{log, Bridge};

const FILE: &str = "profile.json";
// the parts of a play's analysis the profile adds up, none of them depend on the finger settings
const KEPT: [&str; 12] = ["keys", "mode", "rate", "length", "accuracy", "windows", "timing", "release", "columns", "patterns", "empty", "chordSpread"];
// reads saved by an older Hashi are read again when this goes up
const READ_VERSION: u64 = 2;

// play id -> { replay, read } or { replay, error }. a play can be matched to another replay later, so the replay is kept too
static READS: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();

fn reads(b: &Bridge) -> &'static Mutex<HashMap<String, Value>> {
    READS.get_or_init(|| Mutex::new(std::fs::read(b.env.data_dir.join(FILE)).ok().and_then(|x| serde_json::from_slice(&x).ok()).unwrap_or_default()))
}

fn trim(mut v: Value) -> Value {
    // each column's presses by whole ms early or late, [[ms, count]], far fewer numbers than the presses themselves
    let keys = v["keys"].as_u64().unwrap_or(0) as usize;
    let mut hist: Vec<BTreeMap<i64, u64>> = vec![BTreeMap::new(); keys];
    for h in v["hits"].as_array().into_iter().flatten() {
        let (Some(o), Some(c)) = (h[1].as_f64(), h[3].as_u64()) else { continue };
        if let Some(col) = hist.get_mut(c as usize) {
            *col.entry(o.round() as i64).or_default() += 1;
        }
    }
    let mut out = Map::new();
    out.insert("hist".into(), json!(hist.iter().map(|c| c.iter().map(|(o, n)| [*o, *n as i64]).collect::<Vec<_>>()).collect::<Vec<_>>()));
    for k in KEPT {
        if let Some(x) = v.get_mut(k).map(Value::take) {
            out.insert(k.into(), x);
        }
    }
    if let Some(columns) = out.get_mut("columns").and_then(Value::as_array_mut) {
        for c in columns.iter_mut().filter_map(Value::as_object_mut) {
            c.remove("hand");
            c.remove("finger");
        }
    }
    Value::Object(out)
}

async fn read_play(b: &Arc<Bridge>, item: &Item) -> Result<Value, String> {
    let (Some(replay), Some(chart)) = (&item.replay, &item.chart) else { return Err("no replay".into()) };
    let replay = crate::replays::read_blob(b, replay).await?;
    let chart = crate::replays::read_blob(b, chart).await?;
    let lazer = item.play.is_lazer();
    tokio::task::spawn_blocking(move || {
        let j = crate::analysis::judge_play(&replay, &chart, lazer)?;
        Ok(trim(crate::analysis::analyze(&j, true, None)))
    })
    .await
    .map_err(|e| e.to_string())?
}

// reads the plays it hasn't seen, drops the ones that are gone. replays don't change, so each is read once
async fn update(b: &Arc<Bridge>, items: &[Item]) {
    static UPDATING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _updating = UPDATING.lock().await;
    let missing: Vec<&Item> = {
        let known = reads(b).lock().unwrap_or_else(|e| e.into_inner());
        items
            .iter()
            .filter(|i| i.replay.as_ref().is_some_and(|r| known.get(&i.id).is_none_or(|k| k["replay"].as_str() != Some(&r.sha256) || k["v"].as_u64() != Some(READ_VERSION))))
            .collect()
    };
    let mut changed = false;
    for item in missing {
        let sha = item.replay.as_ref().map(|r| r.sha256.clone());
        let entry = match read_play(b, item).await {
            Ok(read) => json!({ "v": READ_VERSION, "replay": sha, "read": read }),
            Err(e) => json!({ "v": READ_VERSION, "replay": sha, "error": e }),
        };
        reads(b).lock().unwrap_or_else(|e| e.into_inner()).insert(item.id.clone(), entry);
        changed = true;
    }
    let list = {
        let mut known = reads(b).lock().unwrap_or_else(|e| e.into_inner());
        let before = known.len();
        known.retain(|id, _| items.iter().any(|i| &i.id == id));
        changed |= known.len() != before;
        json!(*known)
    };
    if changed {
        if let Err(e) = crate::write_json_file(&b.env.data_dir.join(FILE), &list).await {
            log!("profile: couldn't save ({e})");
        }
    }
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

// several plays' (count, mean, sd) as one
#[derive(Default)]
struct Pool {
    n: f64,
    sum: f64,
    squares: f64,
    early: f64,
}

impl Pool {
    fn add(&mut self, v: &Value, n: f64) {
        let (m, sd) = (f(&v["mean"]), f(&v["sd"]));
        self.n += n;
        self.sum += n * m;
        self.squares += n * (sd * sd + m * m);
        self.early += n * f(&v["early"]);
    }

    fn mean(&self) -> f64 {
        self.sum / self.n.max(1.0)
    }

    fn sd(&self) -> f64 {
        (self.squares / self.n.max(1.0) - self.mean().powi(2)).max(0.0).sqrt()
    }

    fn json(&self) -> Value {
        json!({ "n": self.n, "mean": r1(self.mean()), "sd": r1(self.sd()), "early": r2(self.early / self.n.max(1.0)) })
    }
}

// accuracies weighted by how many judgements each came from. stable and lazer count accuracy a little differently, so they can't be pooled from counts
#[derive(Default)]
struct Acc {
    weight: f64,
    sum: f64,
}

impl Acc {
    fn add(&mut self, v: &Value) {
        let Some(acc) = v["acc"].as_f64() else { return };
        let w: f64 = v["counts"].as_array().map_or(0.0, |c| c.iter().map(f).sum());
        self.weight += w;
        self.sum += w * acc;
    }

    fn get(&self) -> Option<f64> {
        (self.weight > 0.0).then(|| r2(self.sum / self.weight))
    }
}

#[derive(Default)]
struct Part {
    acc: Acc,
    timing: Pool,
    notes: f64,
    counts: [f64; 6],
}

impl Part {
    fn add(&mut self, v: &Value) {
        self.acc.add(v);
        self.timing.add(v, f(&v["hits"]));
        self.notes += f(&v["notes"]);
        for (sum, n) in self.counts.iter_mut().zip(v["counts"].as_array().into_iter().flatten()) {
            *sum += f(n);
        }
    }

    fn json(&self) -> Value {
        json!({ "acc": self.acc.get(), "mean": r1(self.timing.mean()), "sd": r1(self.timing.sd()), "hits": self.timing.n, "notes": self.notes, "counts": self.counts })
    }
}

fn key_mode(keys: usize, reads: &[&Value], fingers: &[(&str, &str)]) -> Value {
    let mut timing = Pool::default();
    let mut release = Pool::default();
    let mut columns: Vec<Part> = (0..keys).map(|_| Part::default()).collect();
    let mut hands: [Part; 2] = Default::default();
    let mut patterns: BTreeMap<String, Part> = BTreeMap::new();
    let (mut spread, mut chorded) = (0.0, 0.0);
    let (mut holds, mut breaks) = (0.0, 0.0);
    let mut accuracies = vec![];
    let mut clean = vec![];
    let mut hist: Vec<BTreeMap<i64, f64>> = vec![BTreeMap::new(); keys];
    // the windows depend on each map's OD, the chart draws them for the ones most plays had
    let mut windows: HashMap<String, usize> = HashMap::new();
    for r in reads {
        *windows.entry(r["windows"].to_string()).or_default() += 1;
        for (c, col) in r["hist"].as_array().into_iter().flatten().enumerate().take(keys) {
            for pair in col.as_array().into_iter().flatten() {
                *hist[c].entry(pair[0].as_i64().unwrap_or(0)).or_default() += f(&pair[1]);
            }
        }
        accuracies.push(f(&r["accuracy"]));
        timing.add(&r["timing"], f(&r["timing"]["n"]));
        if r["release"].is_object() {
            release.add(&r["release"], f(&r["release"]["n"]));
            holds += f(&r["release"]["holds"]);
            breaks += f(&r["release"]["breaks"]);
        }
        for (c, v) in r["columns"].as_array().into_iter().flatten().enumerate().take(keys) {
            columns[c].add(v);
            hands[(fingers[c].0 == "right") as usize].add(v);
        }
        for p in r["patterns"].as_array().into_iter().flatten() {
            patterns.entry(text(&p["name"])).or_default().add(p);
        }
        if let Some(s) = r["chordSpread"].as_f64() {
            spread += s;
            chorded += 1.0;
        }
        if r["empty"]["shown"] == json!(true) {
            clean.push(f(&r["empty"]["clean"]));
        }
    }
    let order = ["Singles", "Chords", "Jacks", "Long notes"];
    let mut patterns: Vec<(String, Part)> = patterns.into_iter().collect();
    patterns.sort_by_key(|(name, _)| order.iter().position(|o| o == name).unwrap_or(order.len()));
    let windows: Value = windows.into_iter().max_by_key(|(_, n)| *n).and_then(|(w, _)| serde_json::from_str(&w).ok()).unwrap_or(Value::Null);
    let mut timing_json = timing.json();
    timing_json["ur"] = r1(timing.sd() * 10.0).into();
    json!({
        "keys": keys,
        "plays": reads.len(),
        "accuracy": r2(accuracies.iter().sum::<f64>() / accuracies.len().max(1) as f64),
        "best": accuracies.iter().copied().fold(None, |m: Option<f64>, a| Some(m.map_or(a, |m| m.max(a)))),
        "timing": timing_json,
        "release": (release.n >= 10.0).then(|| {
            let mut v = release.json();
            v["holds"] = holds.into();
            v["breaks"] = breaks.into();
            v
        }),
        "columns": columns.iter().zip(fingers).map(|(c, (hand, finger))| {
            let mut v = c.json();
            v["hand"] = (*hand).into();
            v["finger"] = (*finger).into();
            v
        }).collect::<Vec<_>>(),
        "hands": { "left": hands[0].json(), "right": hands[1].json() },
        "patterns": patterns.iter().filter(|(_, p)| p.notes > 0.0).map(|(name, p)| {
            let mut v = p.json();
            v["name"] = name.as_str().into();
            v
        }).collect::<Vec<_>>(),
        "chordSpread": (chorded > 0.0).then(|| r1(spread / chorded)),
        "windows": windows,
        "hist": hist.iter().map(|c| c.iter().map(|(o, n)| json!([o, n])).collect::<Vec<_>>()).collect::<Vec<_>>(),
        "clean": (!clean.is_empty()).then(|| json!({ "plays": clean.len(), "acc": r2(clean.iter().sum::<f64>() / clean.len() as f64) })),
    })
}

pub(crate) async fn summary(b: &Arc<Bridge>) -> Value {
    let items: Vec<Item> = b.st().queue.items.clone();
    update(b, &items).await;
    let (thumb_left, own, username) = {
        let s = b.st();
        let settings = s.config.settings.as_ref();
        let username = s.config.connection.as_ref().map(|c| text(&c.username)).filter(|u| !u.is_empty() && u != "null");
        (!settings.is_some_and(|s| s.thumb_right), settings.map(|s| s.fingers.clone()).unwrap_or_default(), username)
    };
    let known = reads(b).lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut by_keys: BTreeMap<usize, Vec<&Value>> = BTreeMap::new();
    let mut plays = vec![];
    let mut sorted: Vec<&Item> = items.iter().collect();
    sorted.sort_by(|a, b| b.captured_at.cmp(&a.captured_at));
    for i in sorted {
        let read = known.get(&i.id).map(|k| &k["read"]).filter(|r| r.is_object());
        // practice maps are cut and slowed down, they'd skew everything
        if let Some(r) = read.filter(|_| !i.practice) {
            by_keys.entry(r["keys"].as_u64().unwrap_or(0) as usize).or_default().push(r);
        }
        let mut v = crate::queue::public_item(i, &b.env.origin, username.as_deref());
        v["read"] = match read {
            Some(r) => json!({
                "accuracy": r["accuracy"],
                "length": r["length"],
                "notes": r["columns"].as_array().map_or(0.0, |c| c.iter().map(|c| f(&c["notes"])).sum::<f64>()),
                "ur": r1(f(&r["timing"]["sd"]) * 10.0),
                "mean": r["timing"]["mean"],
            }),
            None => Value::Null,
        };
        plays.push(v);
    }
    let modes: Vec<Value> = by_keys
        .iter()
        .filter(|(k, _)| (1..=18).contains(*k))
        .map(|(&k, reads)| {
            let fingers = crate::analysis::column_fingers(k, thumb_left, own.get(&k.to_string()).map(Vec::as_slice));
            key_mode(k, reads, &fingers)
        })
        .collect();
    json!({ "plays": plays, "modes": modes, "keepDays": crate::queue::KEEP_DAYS })
}
