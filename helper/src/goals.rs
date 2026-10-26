use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{log, Bridge};

const FILE: &str = "goals.json";
const DONE_SHOWN_MS: i64 = 3 * 24 * 3600_000;
const MAX_TRIES: usize = 50;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Goal {
    // kind-md5
    pub id: String,
    // "clean" or "practice"
    pub kind: String,
    pub md5: String,
    pub title: String,
    pub version: String,
    pub target: f64,
    pub tries: Vec<Try>,
    pub created_at: String,
    #[serde(default)]
    pub done_at: Option<String>,
    #[serde(default)]
    pub dismissed: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Try {
    pub at: String,
    pub play: String,
    pub value: f64,
}

pub(crate) struct Seen<'a> {
    pub kind: &'a str,
    pub md5: &'a str,
    pub title: &'a str,
    pub version: &'a str,
    pub target: f64,
    pub play: Option<(&'a str, &'a str, f64)>,
    pub opens: bool,
}

static GOALS: OnceLock<Mutex<Vec<Goal>>> = OnceLock::new();

fn goals(b: &Bridge) -> &'static Mutex<Vec<Goal>> {
    GOALS.get_or_init(|| Mutex::new(std::fs::read(b.env.data_dir.join(FILE)).ok().and_then(|x| serde_json::from_slice(&x).ok()).unwrap_or_default()))
}

async fn save(b: &Arc<Bridge>) {
    // held through the write, or an older copy can land on disk after a newer one
    static WRITING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _writing = WRITING.lock().await;
    let list = goals(b).lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Err(e) = crate::write_json_file(&b.env.data_dir.join(FILE), &json!(list)).await {
        log!("goals: couldn't save ({e})");
    }
    b.broadcast();
}

// the same play twice counts once
pub(crate) async fn record(b: &Arc<Bridge>, s: Seen<'_>) {
    let id = format!("{}-{}", s.kind, s.md5);
    let changed = {
        let mut list = goals(b).lock().unwrap_or_else(|e| e.into_inner());
        let at = list.iter().position(|g| g.id == id);
        match at {
            None if !s.opens => false,
            None => {
                let mut g = Goal {
                    id,
                    kind: s.kind.into(),
                    md5: s.md5.into(),
                    title: s.title.into(),
                    version: s.version.into(),
                    target: s.target,
                    tries: vec![],
                    created_at: s.play.map_or_else(crate::iso_now, |(_, when, _)| when.to_string()),
                    done_at: None,
                    dismissed: false,
                };
                if let Some((play, when, value)) = s.play {
                    add_try(&mut g, play, when, value);
                }
                log!("goal set: {} on {} [{}], {:.0}%", g.kind, g.title, g.version, g.target);
                list.push(g);
                true
            }
            Some(i) => {
                let g = &mut list[i];
                match s.play {
                    Some((play, when, value)) if !g.dismissed && g.done_at.is_none() && !g.tries.iter().any(|t| t.play == play) => {
                        add_try(g, play, when, value);
                        true
                    }
                    _ => false,
                }
            }
        }
    };
    if changed {
        save(b).await;
    }
}

// a play from before the goal goes on its track but can't reach it
fn reaches(g: &Goal, when: &str, value: f64) -> bool {
    value >= g.target && when >= g.created_at.as_str()
}

fn add_try(g: &mut Goal, play: &str, when: &str, value: f64) {
    g.tries.push(Try { at: when.into(), play: play.into(), value: (value * 100.0).round() / 100.0 });
    g.tries.sort_by(|a, b| a.at.cmp(&b.at));
    if g.tries.len() > MAX_TRIES {
        g.tries.remove(1);
    }
    if reaches(g, when, value) && g.done_at.is_none() {
        g.done_at = Some(crate::iso_now());
        log!("goal reached: {} on {} [{}], {value:.2}%", g.kind, g.title, g.version);
    }
}

pub(crate) async fn set_target(b: &Arc<Bridge>, id: &str, target: f64) -> bool {
    let practice_md5 = {
        let mut list = goals(b).lock().unwrap_or_else(|e| e.into_inner());
        let Some(g) = list.iter_mut().find(|g| g.id == id) else { return false };
        g.target = (target * 10.0).round() / 10.0;
        let reached = g.tries.iter().any(|t| reaches(g, &t.at, t.value));
        g.done_at = match (reached, g.done_at.take()) {
            (true, Some(at)) => Some(at),
            (true, None) => Some(crate::iso_now()),
            (false, _) => None,
        };
        log!("goal changed: {} on {} [{}], now {:.1}%", g.kind, g.title, g.version, g.target);
        (g.kind == "practice").then(|| (g.md5.clone(), g.target))
    };
    if let Some((md5, target)) = practice_md5 {
        crate::practice::set_goal(b, &md5, target).await;
    }
    save(b).await;
    true
}

pub(crate) async fn dismiss(b: &Arc<Bridge>, id: &str) -> bool {
    let found = {
        let mut list = goals(b).lock().unwrap_or_else(|e| e.into_inner());
        list.iter_mut().find(|g| g.id == id).map(|g| g.dismissed = true).is_some()
    };
    if found {
        save(b).await;
    }
    found
}

fn public_goal(g: &Goal) -> Value {
    let values: Vec<f64> = g.tries.iter().map(|t| t.value).collect();
    json!({
        "id": g.id,
        "kind": g.kind,
        "md5": g.md5,
        "title": g.title,
        "version": g.version,
        "target": g.target,
        "start": values.first(),
        "best": values.iter().copied().reduce(f64::max),
        "last": values.last(),
        "previous": values.len().checked_sub(2).map(|i| values[i]),
        "tries": values.len(),
        "values": values,
        "latestPlay": g.tries.last().map(|t| t.play.clone()),
        "done": g.done_at.is_some(),
        "doneAt": g.done_at,
    })
}

pub(crate) fn public(b: &Bridge) -> Value {
    let recent = crate::iso_from_ms(crate::now_ms() - DONE_SHOWN_MS);
    let list = goals(b).lock().unwrap_or_else(|e| e.into_inner());
    let mut shown: Vec<&Goal> = list.iter().filter(|g| !g.dismissed && g.done_at.as_ref().is_none_or(|d| *d >= recent)).collect();
    shown.sort_by(|a, b| b.tries.last().map(|t| &t.at).cmp(&a.tries.last().map(|t| &t.at)).then(b.created_at.cmp(&a.created_at)));
    json!(shown.into_iter().map(public_goal).collect::<Vec<_>>())
}

pub(crate) fn for_map(b: &Bridge, md5: &str) -> Value {
    let list = goals(b).lock().unwrap_or_else(|e| e.into_inner());
    list.iter().filter(|g| g.md5 == md5 && !g.dismissed).last().map(public_goal).unwrap_or(Value::Null)
}

pub(crate) async fn on_play(b: &Arc<Bridge>, id: &str) {
    let Some(item) = b.st().queue.items.iter().find(|i| i.id == id).cloned() else { return };
    let (Some(replay), Some(chart)) = (&item.replay, &item.chart) else { return };
    let (Ok(replay), Ok(chart)) = (crate::replays::read_blob(b, replay).await, crate::replays::read_blob(b, chart).await) else { return };
    let practice = crate::practice::find(b, &item.play.beatmap_md5);
    let lazer = item.play.is_lazer();
    let p2 = practice.clone();
    let worked = tokio::task::spawn_blocking(move || {
        let j = crate::analysis::judge_play(&replay, &chart, lazer).ok()?;
        let full = p2.as_ref().and_then(|p| full_speed(&crate::practice::breakdown(p, &j)));
        Some((crate::analysis::clean_stats(&j), full))
    })
    .await
    .ok()
    .flatten();
    let Some((clean, full)) = worked else { return };
    let (title, version) = (crate::types::text(&item.play.title), crate::types::text(&item.play.version));
    match practice {
        Some(p) => {
            if let (Some(goal), Some(value)) = (p.goal, full) {
                record(b, Seen { kind: "practice", md5: &p.md5, title: &p.title, version: &p.version, target: goal, play: Some((&item.id, &item.captured_at, value)), opens: true }).await;
            }
        }
        None => {
            record(b, Seen { kind: "clean", md5: &item.play.beatmap_md5, title: &title, version: &version, target: clean.goal, play: Some((&item.id, &item.captured_at, clean.clean)), opens: clean.shown }).await;
        }
    }
}

pub(crate) fn full_speed(breakdown: &Value) -> Option<f64> {
    breakdown["repeats"].as_array()?.iter().filter(|r| r["speed"].as_f64().is_some_and(|s| s >= 0.999)).filter_map(|r| r["acc"].as_f64()).reduce(f64::max)
}
