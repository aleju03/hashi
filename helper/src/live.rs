// live spectating: while osu! is open the play goes to Mania Tracker as it happens, key presses,
// the song's clock, counts and map changes, so anyone can watch it from the player's profile

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};

use crate::auth::{api_direct, direct_base, Req, Resp};
use crate::{log, now_ms, Bridge, State, LIVE_SCOPE};

// what the site allows when its capabilities don't say
const MAX_FRAMES_PER_EVENT: usize = 4096;
const MAX_EVENTS_PER_BATCH: usize = 256;
const MAX_BODY_BYTES: usize = 1 << 20;
const MAX_CHART_BYTES: u64 = 4 << 20;
// the site forgets a stream 30s after its last batch
const KEEPALIVE_MS: i64 = 10_000;
const GIVE_UP_MS: i64 = 25_000;
// the song's clock standing still this long is a pause
const PAUSED_AFTER_MS: i64 = 400;
const SEED_WAIT_MS: i64 = 2000;
const EASY_EMPTY_MS: i64 = 1500;
// a song for a map the site has none for, sent a little ahead of the play, not all of it at once
const MAX_SONG_BYTES: u64 = 64 << 20;
const SONG_PIECE: u64 = 256 << 10;
const SONG_BYTES_PER_S: f64 = (256 << 10) as f64;
const SONG_AHEAD_MS: i64 = 20_000;
// browsers read an Ogg's length from its end
const OGG_TAIL: u64 = 64 << 10;

const HIT_KEYS: [(&str, &str); 6] = [("geki", "perfect"), ("300", "great"), ("katu", "good"), ("100", "ok"), ("50", "meh"), ("0", "miss")];

struct Stream {
    id: String,
    key: String,
    seq: u64,
    // sent or not, until the site acks them
    outbox: VecDeque<Value>,
    sent_at: i64,
    failing_since: Option<i64>,
    frames_capped: bool,
}

struct Play {
    id: String,
    md5: String,
    start: Value,
    // how many of LiveState.frames went out
    sent: usize,
    // time, paused, counting down to resume
    clock: Option<(i64, bool, bool)>,
    moved_at: i64,
    // lazer holds the song at 0 while a play loads, that isn't a pause
    started: bool,
    opened_at: i64,
    had_health: bool,
    // when the bar ran empty: the time and where in the map
    empty_since: Option<(i64, i64)>,
    stats: Option<Value>,
}

// off | unsupported | needs_reconnect | waiting | live | error
pub(crate) struct LiveState {
    pub status: &'static str,
    pub message: Option<String>,
    pub viewers: u64,
    pub watch_url: Option<String>,
    // the play's key presses: [time in ms, a bit per held column, leftmost first]
    pub frames: Vec<(f64, u32)>,
    // the newest frame osu! recorded, pressed or not, and when it came: the play's own time
    // before the song starts, when the song's clock doesn't move yet
    last_raw: Option<(f64, i64)>,
    // a reset came in, the frames are another attempt's
    restarted: bool,
    stream: Option<Stream>,
    play: Option<Play>,
    state_key: Option<String>,
    state: Option<Value>,
    busy: bool,
    paused_until: i64,
    charts: HashSet<String>,
    max_per_batch: usize,
    plays: u64,
    // a Random play waits a moment for its seed to be read
    seed_wait_since: Option<i64>,
    pause_menu: Option<bool>,
    failed_seen: bool,
    still_seen: bool,
    reset_pending: bool,
    // the play failed and lazer's fail screen is up, the same attempt isn't a new play
    failed_attempt: bool,
    // the song the site last asked for: the chart's md5 and how much it holds from the start
    song: Option<(String, u64)>,
    // the play a song is going out for, and one it was given up on
    song_task: Option<String>,
    song_given_up: Option<String>,
}

impl Default for LiveState {
    fn default() -> Self {
        LiveState {
            status: "off",
            message: None,
            viewers: 0,
            watch_url: None,
            frames: vec![],
            last_raw: None,
            restarted: false,
            stream: None,
            play: None,
            state_key: None,
            state: None,
            busy: false,
            paused_until: 0,
            charts: HashSet::new(),
            max_per_batch: MAX_EVENTS_PER_BATCH,
            plays: 0,
            seed_wait_since: None,
            pause_menu: None,
            failed_seen: false,
            still_seen: false,
            reset_pending: false,
            failed_attempt: false,
            song: None,
            song_task: None,
            song_given_up: None,
        }
    }
}

pub(crate) fn on_keys(b: &Arc<Bridge>, keys: &Value) {
    let raw: Vec<(f64, u32)> = keys.get("frames").and_then(Value::as_array).into_iter().flatten().filter_map(|f| {
        let time = f.get(0)?.as_f64()?;
        let held = u32::try_from(f.get(1)?.as_u64()?).ok()?;
        Some((time, held))
    }).collect();
    let mut s = b.st();
    if !turned_on(&s) {
        return;
    }
    if let Some(last) = raw.last() {
        s.live.last_raw = Some((last.0, now_ms()));
    }
    let reset = keys.get("reset").and_then(Value::as_bool) == Some(true);
    // the reader starts over now and then without osu! doing so: what comes next tells
    if reset && raw.is_empty() {
        s.live.reset_pending = true;
        if s.live.frames.is_empty() {
            s.live.last_raw = None;
        }
        return;
    }
    if reset || s.live.reset_pending {
        s.live.reset_pending = false;
        let fresh = changes(&[], &raw);
        if same_play(&s.live.frames, &fresh) {
            if fresh.len() > s.live.frames.len() {
                s.live.frames = fresh;
            }
            return;
        }
        s.live.frames = fresh;
        s.live.restarted = true;
        return;
    }
    let more = changes(&s.live.frames, &raw);
    s.live.frames.extend(more);
}

// read over from the start, the same presses: all of them again, or the first part of a long
// play (the reader reads so many at a time). a retry starts over with only a moment of them
fn same_play(had: &[(f64, u32)], fresh: &[(f64, u32)]) -> bool {
    let (Some(first), Some(last)) = (had.first(), fresh.last()) else { return false };
    fresh.starts_with(had) || (had.starts_with(fresh) && last.0 - first.0 > 5000.0)
}

// osu! records a frame every few ms, only the ones where a key goes up or down matter
fn changes(before: &[(f64, u32)], frames: &[(f64, u32)]) -> Vec<(f64, u32)> {
    let mut last = before.last().map(|f| f.1);
    let mut out = vec![];
    for &f in frames {
        if last != Some(f.1) {
            out.push(f);
            last = Some(f.1);
        }
    }
    out
}

pub(crate) fn live_supported(s: &State) -> bool {
    s.caps.as_ref().and_then(|c| c.get("scopes_supported")).and_then(Value::as_array).is_some_and(|l| l.iter().any(|x| x.as_str() == Some(LIVE_SCOPE)))
        && s.caps.as_ref().and_then(|c| c.pointer("/live/enabled")).and_then(Value::as_bool) == Some(true)
}

fn granted(s: &State) -> bool {
    s.config.connection.as_ref().and_then(|c| c.scope.as_str()).unwrap_or("").split(' ').any(|x| x == LIVE_SCOPE)
}

fn turned_on(s: &State) -> bool {
    s.config.settings.as_ref().is_some_and(|x| x.live)
}

fn limit(s: &State, key: &str, default: usize) -> usize {
    s.caps.as_ref().and_then(|c| c.pointer(&format!("/live/{key}"))).and_then(Value::as_u64).map_or(default, |n| n as usize)
}

fn set_status(b: &Arc<Bridge>, s: &mut State, status: &'static str, message: Option<String>) {
    if s.live.status != status || s.live.message != message {
        s.live.status = status;
        s.live.message = message;
        let b = b.clone();
        tokio::spawn(async move { b.broadcast() });
    }
}

enum Next {
    Nothing,
    Open,
    Close(String),
    Send(String, String, Vec<Value>),
}

pub(crate) async fn live_tick(b: &Arc<Bridge>) {
    let next = {
        let mut s = b.st();
        if s.live.busy {
            return;
        }
        let on = turned_on(&s);
        let connected = s.connection.status == "connected" && s.config.connection.as_ref().is_some_and(|c| c.refresh_token.is_some());
        let status = if !on {
            "off"
        } else if !connected {
            "waiting"
        } else if !live_supported(&s) {
            "unsupported"
        } else if !granted(&s) {
            "needs_reconnect"
        } else {
            ""
        };
        if !status.is_empty() || !s.reader.osu {
            let status = if status.is_empty() { "waiting" } else { status };
            if s.live.status != "error" || !on {
                set_status(b, &mut s, status, None);
            }
            match s.live.stream.take() {
                Some(stream) if connected => {
                    s.live.busy = true;
                    Next::Close(stream.id)
                }
                _ => Next::Nothing,
            }
        } else if now_ms() < s.live.paused_until {
            Next::Nothing
        } else if s.live.stream.is_none() {
            // claimed here, or another tick could open a second stream before this one is up
            s.live.busy = true;
            Next::Open
        } else {
            follow(&mut s);
            batch(&mut s)
        }
    };
    match next {
        Next::Nothing => {}
        Next::Open => open(b).await,
        Next::Close(id) => close(b, &id).await,
        Next::Send(id, key, events) => send(b, id, key, events).await,
    }
}

// turns what the reader shows now into events for the stream
fn follow(s: &mut State) {
    let Some(body) = crate::presence::presence_body(s) else { return };
    let state = body.get("state").and_then(Value::as_str).unwrap_or("idle").to_string();
    let beatmap = body.get("beatmap").cloned().unwrap_or(Value::Null);
    let client = if s.reader.client.as_deref() == Some("lazer") { "lazer" } else { "stable" };
    // not which map is selected, spectators only see what's played
    let state_event = json!({ "type": "state", "state": state, "client": client });
    let key = state_event.to_string();
    if s.live.state_key.as_deref() != Some(&key) {
        s.live.state_key = Some(key);
        s.live.state = Some(state_event.clone());
        push(s, state_event);
    }

    let playing = state == "playing";
    let raw = s.reader.last_frame.clone().unwrap_or(Value::Null);
    let md5 = beatmap.get("md5").and_then(Value::as_str).map(String::from);
    // the log is also cleared as a play opens, before its first press
    let retry = playing && s.live.restarted && s.live.play.as_ref().is_some_and(|p| p.sent > 0);
    let menu = raw.pointer("/game/pauseMenu").and_then(Value::as_bool);
    let stood_still = stable_still(&raw);
    let other_map = s.live.play.as_ref().is_some_and(|p| Some(&p.md5) != md5.as_ref());
    if s.live.play.is_some() && (!playing || retry || other_map) {
        let reason = if retry {
            "retry"
        } else if state == "results" {
            "passed"
        } else {
            "quit"
        };
        end_play(s, reason);
    }
    if !playing || s.live.restarted {
        s.live.failed_attempt = false;
    }
    // a retry's first moments can still show the last attempt's fail screen
    let failed = raw.pointer("/game/failed").and_then(Value::as_bool) == Some(true) || stable_out_of_health(s, &raw);
    if failed && s.live.play.as_ref().is_some_and(|p| now_ms() - p.opened_at >= 1000) {
        // where it failed: lazer slows the song to a stop after, so as read, not carried forward
        let at = s.live.play.as_ref().and_then(|p| p.empty_since.map(|(_, t)| t)).or_else(|| raw.pointer("/beatmap/time/live").and_then(Value::as_f64).map(|t| t.round() as i64));
        end_play_at(s, "failed", at);
        s.live.failed_attempt = true;
    }
    s.live.restarted = false;
    if !playing {
        s.live.seed_wait_since = None;
        // the last play's presses, so the next one doesn't start with them
        s.live.frames.clear();
        return;
    }
    let Some(md5) = md5 else { return };
    if s.live.failed_attempt {
        return;
    }
    // a map made for another mode, played as mania: spectators are told, nothing of the play is sent
    let convert = raw.pointer("/beatmap/mode/number").and_then(Value::as_u64).is_some_and(|m| m != 3);
    if s.live.play.is_none() {
        let Some(start) = play_start(s, &raw, &beatmap, &md5, &body, convert) else { return };
        s.live.play = Some(Play { id: start["play_id"].as_str().unwrap_or("").into(), md5, start: start.clone(), sent: 0, clock: None, moved_at: now_ms(), started: false, opened_at: now_ms(), had_health: false, empty_since: None, stats: None });
        push(s, start);
    }
    if convert {
        return;
    }
    send_frames(s);
    let now = now_ms();
    // until the first press the clock can still be song select's preview
    let time = raw.pointer("/beatmap/time/live").and_then(Value::as_f64).map(|t| t.round() as i64).filter(|_| !s.live.frames.is_empty());
    let stats = stats_of(&raw);
    // the time can be a quarter second old by now, it's sent as where the song is now
    let read_at = raw.pointer("/beatmap/time/readAt").and_then(Value::as_i64).unwrap_or(s.reader.last_frame_at);
    let age = now - read_at;
    let rate = play_rate(&s.live.play);
    // lazer's song clock stays at 0 through the lead-in, its presses carry the play's own time
    // (stable's clock counts the lead-in itself). lazer records frames all the time, none coming
    // in is a pause: held at the last one
    let lazer = raw.get("client").and_then(Value::as_str) == Some("lazer");
    let lead_in = s.live.last_raw.filter(|(t, _)| lazer && *t < 0.0).map(|(t, at)| {
        let paused = menu == Some(true) || now - at > 500;
        let time = if paused { t } else { t + (now - at) as f64 * rate };
        (time.round() as i64, paused)
    });
    let health = raw.pointer("/game/health").and_then(Value::as_f64);
    let play = s.live.play.as_mut().expect("just set");
    let id = play.id.clone();
    let mut out = vec![];
    if let Some((at, paused)) = lead_in {
        if play.clock != Some((at, paused, false)) {
            play.clock = Some((at, paused, false));
            out.push(clock_event(&id, at, paused, false, 0, rate, health));
        }
    } else if let Some(time) = time {
        let last = play.clock.map(|(t, ..)| t);
        if last != Some(time) {
            play.moved_at = now;
        }
        play.started |= last.is_some_and(|l| time > l);
        // lazer's pause menu says so at once, otherwise it's the song standing still
        let still = last == Some(time) && now - play.moved_at >= PAUSED_AFTER_MS;
        let paused = play.started && (menu == Some(true) || still || stood_still);
        let resuming = paused && menu == Some(false);
        if play.clock != Some((time, paused, resuming)) {
            play.clock = Some((time, paused, resuming));
            out.push(clock_event(&id, time, paused, resuming, age, rate, health));
        }
    }
    if let Some(mut stats) = stats.filter(|x| Some(x) != play.stats.as_ref()) {
        play.stats = Some(stats.clone());
        stats["type"] = json!("stats");
        stats["play_id"] = json!(id);
        out.push(stats);
    }
    for e in out {
        push(s, e);
    }
}

fn play_start(s: &mut State, raw: &Value, beatmap: &Value, md5: &str, body: &Value, convert: bool) -> Option<Value> {
    let keys = beatmap.get("key_count").and_then(Value::as_u64).or_else(|| raw.pointer("/beatmap/stats/cs/converted").and_then(Value::as_f64).map(|k| k.round() as u64))?;
    let text = |k: &str| beatmap.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mods = body.get("mods").cloned().unwrap_or(json!([]));
    let random = mods.as_array().is_some_and(|m| m.iter().any(|x| x.as_str() == Some("RD")));
    let since = *s.live.seed_wait_since.get_or_insert_with(now_ms);
    if random && !convert && seed(raw).is_none() && now_ms() - since < SEED_WAIT_MS {
        return None;
    }
    s.live.seed_wait_since = None;
    s.live.plays += 1;
    let mut start = json!({
        "type": "play_start",
        "play_id": format!("{}-{}", now_ms(), s.live.plays),
        "client": if s.reader.client.as_deref() == Some("lazer") { "lazer" } else { "stable" },
        "beatmap": { "md5": md5, "title": text("title"), "artist": text("artist"), "creator": text("creator"), "version": text("version"), "keys": keys },
        "mods": mods,
        "rate": body.get("rate").cloned().unwrap_or(json!(1)),
        "seed": if random { seed(raw) } else { None },
    });
    if convert {
        start["convert"] = json!(true);
    }
    Some(start)
}

// how Random shuffled the columns: stable keeps it on the score, lazer in the mod's settings
fn seed(raw: &Value) -> Option<i64> {
    // stable shows 0 where it has none to read
    raw.pointer("/play/seed").and_then(Value::as_i64).filter(|s| *s != 0).or_else(|| {
        let mods = raw.pointer("/play/mods/array")?.as_array()?;
        mods.iter().find(|m| m.get("acronym").and_then(Value::as_str) == Some("RD"))?.pointer("/settings/seed")?.as_i64()
    })
}

fn clock_event(id: &str, time: i64, paused: bool, resuming: bool, age: i64, rate: f64, health: Option<f64>) -> Value {
    let at = if paused { time } else { time + (age.clamp(0, 1000) as f64 * rate).round() as i64 };
    let mut e = json!({ "type": "clock", "play_id": id, "time": at, "paused": paused });
    if let Some(health) = health {
        e["health"] = json!(health);
    }
    if resuming {
        e["resuming"] = json!(true);
    }
    e
}

// lazer's pause menu opening or closing, or the play failing, goes out now, not on the next
// tick: spectators count down to the resume from when it arrives, and stop where it failed
pub(crate) fn on_game(b: &Arc<Bridge>, raw: &Value) {
    let menu = raw.pointer("/game/pauseMenu").and_then(Value::as_bool);
    let stable_empty = raw.get("client").and_then(Value::as_str) == Some("stable") && raw.pointer("/game/health").and_then(Value::as_f64).is_some_and(|h| h <= 0.0);
    let failed = raw.pointer("/game/failed").and_then(Value::as_bool) == Some(true) || stable_empty;
    let still = stable_still(raw);
    let changed = {
        let mut s = b.st();
        let changed = (s.live.pause_menu != menu || s.live.failed_seen != failed || s.live.still_seen != still) && s.live.play.is_some();
        s.live.pause_menu = menu;
        s.live.failed_seen = failed;
        s.live.still_seen = still;
        changed
    };
    if changed {
        let b = b.clone();
        // a batch still on its way holds the next one back, so wait it out
        tokio::spawn(async move {
            for _ in 0..20 {
                if !b.st().live.busy {
                    return live_tick(&b).await;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        });
    }
}

// stable has no pause menu to read: the reader says when the song's time didn't move between two
// of its reads, a quarter second apart
fn stable_still(raw: &Value) -> bool {
    raw.get("client").and_then(Value::as_str) == Some("stable") && raw.pointer("/game/paused").and_then(Value::as_bool) == Some(true)
}

// stable fails the moment health runs out, unless No Fail keeps it going; Easy refills it a few
// times, so with it the bar has to stay empty. only once the bar was seen above 0, it can start
// empty while the play loads
fn stable_out_of_health(s: &mut State, raw: &Value) -> bool {
    let Some(play) = s.live.play.as_mut() else { return false };
    let Some(health) = raw.pointer("/game/health").and_then(Value::as_f64) else { return false };
    if raw.get("client").and_then(Value::as_str) != Some("stable") {
        return false;
    }
    play.had_health |= health > 0.0;
    // a quick retry reads 0 while the next attempt loads, its time back near the start
    let time = raw.pointer("/beatmap/time/live").and_then(Value::as_f64);
    let went_back = play.clock.zip(time).is_some_and(|((t, ..), now)| now < t as f64 - 1000.0);
    if s.live.restarted || went_back {
        return false;
    }
    let has = |code: &str| play.start["mods"].as_array().is_some_and(|m| m.iter().any(|x| x.as_str() == Some(code)));
    if health > 0.0 || !play.had_health || has("NF") {
        play.empty_since = None;
        return false;
    }
    let (since, _) = *play.empty_since.get_or_insert((now_ms(), time.map_or(0, |t| t.round() as i64)));
    !has("EZ") || now_ms() - since >= EASY_EMPTY_MS
}

fn play_rate(play: &Option<Play>) -> f64 {
    play.as_ref().and_then(|p| p.start["rate"].as_f64()).filter(|r| *r > 0.0).unwrap_or(1.0)
}

fn stats_of(raw: &Value) -> Option<Value> {
    let play = raw.get("play")?;
    let counts = play.get("hits")?;
    let mut hits = serde_json::Map::new();
    for (from, to) in HIT_KEYS {
        hits.insert(to.into(), json!(counts.get(from).and_then(Value::as_u64).unwrap_or(0)));
    }
    let acc = play.get("accuracy").and_then(Value::as_f64).unwrap_or(100.0).clamp(0.0, 100.0);
    Some(json!({
        "hits": hits,
        "acc": (acc * 10_000.0).round() / 10_000.0,
        "columns": play.get("columns").cloned().unwrap_or(json!([])),
    }))
}

fn end_play(s: &mut State, reason: &str) {
    end_play_at(s, reason, None);
}

fn end_play_at(s: &mut State, reason: &str, time: Option<i64>) {
    let Some(play) = s.live.play.take() else { return };
    // after a retry the frames are already the next attempt's
    if !s.live.restarted {
        send_frames_of(s, &play);
    }
    let stats = play.stats.unwrap_or_else(|| json!({ "hits": {}, "acc": 0 }));
    let mut end = json!({ "type": "play_end", "play_id": play.id, "reason": reason, "hits": stats["hits"], "acc": stats["acc"] });
    if let Some(time) = time {
        end["time"] = json!(time);
    }
    push(s, end);
}

fn send_frames(s: &mut State) {
    let Some(mut play) = s.live.play.take() else { return };
    play.sent = send_frames_of(s, &play);
    s.live.play = Some(play);
}

// the frames not sent yet, in events the site takes. returns how many are sent now
fn send_frames_of(s: &mut State, play: &Play) -> usize {
    let total = s.live.frames.len();
    if s.live.stream.as_ref().is_some_and(|x| x.frames_capped) || play.sent >= total {
        return total.max(play.sent);
    }
    let per = limit(s, "max_frames_per_batch", MAX_FRAMES_PER_EVENT).max(1);
    let chunks: Vec<Value> = s.live.frames[play.sent..]
        .chunks(per)
        .enumerate()
        .map(|(i, c)| {
            let frames: Vec<Value> = c.iter().map(|(t, k)| json!([t.round() as i64, k])).collect();
            json!({ "type": "frames", "play_id": play.id, "reset": play.sent == 0 && i == 0, "frames": frames })
        })
        .collect();
    for e in chunks {
        push(s, e);
    }
    total
}

fn push(s: &mut State, mut event: Value) {
    let Some(stream) = s.live.stream.as_mut() else { return };
    stream.seq += 1;
    event["seq"] = json!(stream.seq);
    stream.outbox.push_back(event);
}

// what a new stream starts with, so viewers see the same as on the old one
fn resend_all(s: &mut State) {
    if let Some(state) = s.live.state.clone() {
        push(s, state);
    }
    let Some(mut play) = s.live.play.take() else { return };
    push(s, play.start.clone());
    play.sent = 0;
    play.sent = send_frames_of(s, &play);
    if let Some((time, paused, resuming)) = play.clock {
        push(s, clock_event(&play.id, time, paused, resuming, 0, 1.0, None));
    }
    if let Some(mut stats) = play.stats.clone() {
        stats["type"] = json!("stats");
        stats["play_id"] = json!(play.id);
        push(s, stats);
    }
    s.live.play = Some(play);
}

fn batch(s: &mut State) -> Next {
    let max_events = limit(s, "max_events_per_batch", MAX_EVENTS_PER_BATCH).min(s.live.max_per_batch).max(1);
    let max_bytes = limit(s, "max_body_bytes", MAX_BODY_BYTES);
    let stream = s.live.stream.as_mut().expect("checked");
    if stream.outbox.is_empty() && now_ms() - stream.sent_at < KEEPALIVE_MS {
        return Next::Nothing;
    }
    let mut events = vec![];
    let mut bytes = 16;
    for e in &stream.outbox {
        let len = e.to_string().len() + 1;
        if events.len() >= max_events || (!events.is_empty() && bytes + len > max_bytes) {
            break;
        }
        bytes += len;
        events.push(e.clone());
    }
    s.live.busy = true;
    Next::Send(stream.id.clone(), stream.key.clone(), events)
}

async fn open(b: &Arc<Bridge>) {
    b.st().live.busy = true;
    let res = api_direct(b, "/live", Req::json(reqwest::Method::POST, &json!({}))).await;
    let mut s = b.st();
    s.live.busy = false;
    match res {
        Ok(res) if res.ok() => {
            let body = res.json();
            let text = |k: &str| body.get(k).and_then(Value::as_str).map(String::from);
            let (Some(id), Some(key)) = (text("stream_id"), text("stream_key")) else {
                s.live.paused_until = now_ms() + 60_000;
                return set_status(b, &mut s, "error", Some("Mania Tracker answered oddly.".into()));
            };
            s.live.watch_url = text("watch_url");
            s.live.stream = Some(Stream { id, key, seq: 0, outbox: VecDeque::new(), sent_at: 0, failing_since: None, frames_capped: false });
            s.live.charts.clear();
            s.live.viewers = 0;
            s.live.song = None;
            resend_all(&mut s);
            log!("live: watchable at {}", s.live.watch_url.as_deref().unwrap_or("the profile"));
            set_status(b, &mut s, "live", None);
        }
        Ok(res) => answer(b, &mut s, &res),
        Err(e) => {
            s.live.paused_until = now_ms() + 15_000;
            set_status(b, &mut s, "error", Some(e.message().into()));
        }
    }
}

async fn close(b: &Arc<Bridge>, id: &str) {
    b.st().live.busy = true;
    let _ = api_direct(b, &format!("/live/{id}"), Req::method(reqwest::Method::DELETE)).await;
    let mut s = b.st();
    s.live.busy = false;
    s.live.play = None;
    s.live.state = None;
    s.live.state_key = None;
    s.live.viewers = 0;
    s.live.song = None;
}

// for when Hashi quits, the site would wait 30s otherwise
pub(crate) async fn stop(b: &Arc<Bridge>) {
    let id = b.st().live.stream.take().map(|x| x.id);
    if let Some(id) = id {
        close(b, &id).await;
    }
}

async fn send(b: &Arc<Bridge>, id: String, key: String, events: Vec<Value>) {
    let base = b.st().caps.as_ref().and_then(direct_base);
    let res = match base {
        Some(base) => {
            let body = json!({ "events": events }).to_string();
            let req = b.client().post(format!("{base}/live/{id}/events")).header("authorization", format!("Live {key}")).header("content-type", "application/json").timeout(Duration::from_secs(10)).body(body);
            req.send().await
        }
        None => {
            let mut s = b.st();
            s.live.busy = false;
            s.live.stream = None;
            s.live.paused_until = now_ms() + 10 * 60_000;
            return set_status(b, &mut s, "unsupported", None);
        }
    };
    let res = match res {
        Ok(r) => {
            let (status, headers) = (r.status().as_u16(), r.headers().clone());
            r.bytes().await.map(|body| Resp { status, headers, body })
        }
        Err(e) => Err(e),
    };
    let (chart, song) = {
        let mut s = b.st();
        s.live.busy = false;
        let Some(stream) = s.live.stream.as_mut().filter(|x| x.id == id) else { return };
        match res {
            Ok(res) if res.ok() => {
                let body = res.json();
                let ack = body.get("ack").and_then(Value::as_u64).unwrap_or(0);
                while stream.outbox.front().is_some_and(|e| e["seq"].as_u64().unwrap_or(0) <= ack) {
                    stream.outbox.pop_front();
                }
                stream.sent_at = now_ms();
                stream.failing_since = None;
                stream.frames_capped |= body.get("frames_capped").and_then(Value::as_bool) == Some(true);
                let viewers = body.get("viewers").and_then(Value::as_u64).unwrap_or(0);
                let need = body.get("need_chart").and_then(Value::as_str).map(String::from);
                s.live.song = body.get("need_song").and_then(|n| Some((n.get("md5")?.as_str()?.to_lowercase(), n.get("have").and_then(Value::as_u64).unwrap_or(0))));
                if s.live.viewers != viewers {
                    s.live.viewers = viewers;
                    let b = b.clone();
                    tokio::spawn(async move { b.broadcast() });
                }
                set_status(b, &mut s, "live", None);
                let chart = need.filter(|md5| s.live.charts.insert(md5.clone()));
                (chart, song_to_start(&mut s))
            }
            Ok(res) if res.status == 413 => {
                s.live.max_per_batch = (events.len() / 2).max(1);
                (None, None)
            }
            Ok(res) if res.status == 400 => {
                // the site won't take these, sending them again won't change that
                log!("live: a batch was refused: {}", String::from_utf8_lossy(&res.body));
                let last = events.last().and_then(|e| e["seq"].as_u64()).unwrap_or(0);
                stream.outbox.retain(|e| e["seq"].as_u64().unwrap_or(0) > last);
                (None, None)
            }
            Ok(res) => {
                answer(b, &mut s, &res);
                (None, None)
            }
            Err(e) => {
                let since = *stream.failing_since.get_or_insert(now_ms());
                // the site has let it go by now, start again with everything
                if now_ms() - since >= GIVE_UP_MS {
                    s.live.stream = None;
                }
                set_status(b, &mut s, "error", Some(crate::auth::net_message(&e)));
                (None, None)
            }
        }
    };
    if let Some(md5) = chart {
        let (b, id, key) = (b.clone(), id.clone(), key.clone());
        tokio::spawn(async move { upload_chart(&b, &id, &key, &md5).await });
    }
    if let Some((play_id, md5)) = song {
        let b = b.clone();
        tokio::spawn(async move { upload_song(b, id, key, play_id, md5).await });
    }
}

// the site doesn't have this map (or not at this rate): the .osu, for drawing it while the play is on
async fn upload_chart(b: &Arc<Bridge>, id: &str, key: &str, md5: &str) {
    let (base, max) = {
        let s = b.st();
        (s.caps.as_ref().and_then(direct_base), limit(&s, "max_chart_bytes", MAX_CHART_BYTES as usize) as u64)
    };
    let Some(base) = base else { return };
    let Some(file) = crate::skin::selected_chart(b) else { return };
    let Ok(bytes) = crate::read_capped(&file, max).await else { return };
    if crate::replays::md5_hex(&bytes) != md5 {
        b.st().live.charts.remove(md5);
        return;
    }
    let req = b.client().put(format!("{base}/live/{id}/chart/{md5}")).header("authorization", format!("Live {key}")).header("content-type", "text/plain").timeout(Duration::from_secs(30)).body(Bytes::from(bytes));
    match req.send().await {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => log!("live: the map wasn't taken ({})", r.status().as_u16()),
        Err(e) => {
            b.st().live.charts.remove(md5);
            log!("live: couldn't send the map: {}", crate::auth::net_message(&e));
        }
    }
}

fn song_to_start(s: &mut State) -> Option<(String, String)> {
    let (md5, _) = s.live.song.as_ref()?;
    let play = s.live.play.as_ref().filter(|p| p.md5.eq_ignore_ascii_case(md5))?;
    if s.live.song_task.is_some() || s.live.song_given_up.as_deref() == Some(play.id.as_str()) {
        return None;
    }
    s.live.song_task = Some(play.id.clone());
    Some((play.id.clone(), md5.clone()))
}

#[derive(PartialEq)]
enum SongEnd {
    // not asked for now, it can be again
    Stopped,
    GaveUp,
}

async fn upload_song(b: Arc<Bridge>, id: String, key: String, play_id: String, md5: String) {
    let end = send_song(&b, &id, &key, &play_id, &md5).await;
    let mut s = b.st();
    if s.live.song_task.as_deref() == Some(play_id.as_str()) {
        s.live.song_task = None;
    }
    if end == SongEnd::GaveUp {
        s.live.song_given_up = Some(play_id);
    }
}

// the site has no song for this map: its file, a piece at a time and only a little ahead of
// where the play is, so a short play of a long song doesn't send all of it
async fn send_song(b: &Arc<Bridge>, id: &str, key: &str, play_id: &str, md5: &str) -> SongEnd {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Some(base) = b.st().caps.as_ref().and_then(direct_base) else { return SongEnd::GaveUp };
    let Some(path) = crate::practice::selected_audio(b, md5) else {
        log!("live: couldn't find the song to send");
        return SongEnd::GaveUp;
    };
    let Ok(mut file) = tokio::fs::File::open(&path).await else { return SongEnd::GaveUp };
    let total = file.metadata().await.map_or(0, |m| m.len());
    if total == 0 || total > MAX_SONG_BYTES {
        return SongEnd::GaveUp;
    }
    let mut magic = [0u8; 4];
    let ogg = file.read_exact(&mut magic).await.is_ok() && &magic == b"OggS";
    let length = song_length_ms(b, md5, &path).await;
    let mut have = 0;
    let mut tail_due = ogg;
    loop {
        let wanted = {
            let s = b.st();
            let play = s.live.play.as_ref().filter(|p| p.id == play_id && s.live.stream.as_ref().is_some_and(|x| x.id == id));
            let need = s.live.song.as_ref().filter(|(m, _)| m == md5);
            play.zip(need).map(|(p, (_, have))| (*have, p.clock.map_or(0, |c| c.0)))
        };
        let Some((acked, time)) = wanted else { return SongEnd::Stopped };
        have = have.max(acked);
        if have >= total {
            return SongEnd::Stopped;
        }
        let ahead = length.map_or(total, |len| (((time.max(0) + SONG_AHEAD_MS) as f64 / len) * total as f64).ceil() as u64);
        let Some((start, end, tail)) = next_piece(have, total, ahead, tail_due) else {
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        };
        let mut piece = vec![0u8; (end - start + 1) as usize];
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() || file.read_exact(&mut piece).await.is_err() {
            return SongEnd::GaveUp;
        }
        let began = Instant::now();
        let req = song_http()
            .put(format!("{base}/live/{id}/song/{md5}"))
            .header("authorization", format!("Live {key}"))
            .header("content-range", format!("bytes {start}-{end}/{total}"))
            .header("content-type", "application/octet-stream")
            .timeout(Duration::from_secs(30))
            .body(piece);
        let wait = match req.send().await {
            Ok(r) => {
                let status = r.status().as_u16();
                let retry_after = r.headers().get("retry-after").and_then(|h| h.to_str().ok()).and_then(|h| h.trim().parse::<f64>().ok());
                let body: Value = r.json().await.unwrap_or(Value::Null);
                match status {
                    200..=299 => {
                        if let Some(h) = body.get("have").and_then(Value::as_u64) {
                            have = have.max(h);
                        }
                        tail_due &= !tail;
                        None
                    }
                    429 => Some(Duration::from_secs_f64(retry_after.filter(|r| r.is_finite() && *r > 0.0).unwrap_or(1.0).min(30.0))),
                    // another play's map now, the stream's gone, or the site restarting
                    409 | 401 | 404 => return SongEnd::Stopped,
                    400 | 413 | 507 => {
                        log!("live: the song wasn't taken ({status} {})", body.pointer("/error/code").and_then(Value::as_str).unwrap_or(""));
                        return SongEnd::GaveUp;
                    }
                    _ => Some(Duration::from_secs(2)),
                }
            }
            Err(_) => Some(Duration::from_secs(2)),
        };
        let pace = Duration::from_secs_f64((end - start + 1) as f64 / SONG_BYTES_PER_S);
        tokio::time::sleep(wait.unwrap_or(pace.saturating_sub(began.elapsed()))).await;
    }
}

// the next bytes to send, ends included: an Ogg's end once its start is there, then on from what
// the site holds up to `ahead`
fn next_piece(have: u64, total: u64, ahead: u64, tail_due: bool) -> Option<(u64, u64, bool)> {
    if tail_due && have > 0 && have + OGG_TAIL < total {
        return Some((total - OGG_TAIL, total - 1, true));
    }
    (have < ahead.min(total)).then(|| (have, (have + SONG_PIECE).min(total) - 1, false))
}

// pieces go on their own connection, not in line with the batches on the shared one
fn song_http() -> &'static reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(format!("mania-bridge/{}", crate::VERSION))
            .http1_only()
            .build()
            .unwrap_or_default()
    })
}

// from the game when it says, else the file's own header, else the map's last note: too short
// only sends it sooner
async fn song_length_ms(b: &Bridge, md5: &str, path: &Path) -> Option<f64> {
    let (game, last) = {
        let s = b.st();
        let raw = s.reader.last_frame.as_ref().filter(|r| r.pointer("/beatmap/checksum").and_then(Value::as_str).is_some_and(|c| c.eq_ignore_ascii_case(md5)));
        let time = |k: &str| raw.and_then(|r| r.pointer(&format!("/beatmap/time/{k}"))).and_then(Value::as_f64).filter(|t| *t > 0.0);
        (time("mp3Length"), time("lastObject"))
    };
    if game.is_some() {
        return game;
    }
    let path = path.to_path_buf();
    let file = tokio::task::spawn_blocking(move || file_length_ms(&path)).await.ok().flatten();
    file.or(last)
}

fn file_length_ms(path: &Path) -> Option<f64> {
    use symphonia::core::{formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint};
    let src = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());
    let probed = symphonia::default::get_probe().format(&Hint::new(), mss, &FormatOptions::default(), &MetadataOptions::default()).ok()?;
    let params = &probed.format.default_track()?.codec_params;
    let ms = params.n_frames? as f64 * 1000.0 / params.sample_rate? as f64;
    (ms > 0.0).then_some(ms)
}

fn answer(b: &Arc<Bridge>, s: &mut State, res: &Resp) {
    let body = res.json();
    let code = body.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
    let retry_after = body
        .pointer("/error/retry_after")
        .and_then(Value::as_f64)
        .or(res.header("retry-after").and_then(|h| h.trim().parse().ok()))
        .filter(|r| r.is_finite() && *r > 0.0);
    match res.status {
        // a new stream next time, the site restarted or the key is old
        404 | 401 if s.live.stream.is_some() => {
            s.live.stream = None;
        }
        // the capabilities offer it, so a backend that's restarting: try again soon
        404 if live_supported(s) => {
            s.live.paused_until = now_ms() + 15_000;
            set_status(b, s, "error", Some(format!("Mania Tracker answered {}.", res.status)));
        }
        404 => {
            s.live.paused_until = now_ms() + 10 * 60_000;
            set_status(b, s, "unsupported", None);
        }
        403 if code == "insufficient_scope" => set_status(b, s, "needs_reconnect", None),
        429 => s.live.paused_until = now_ms() + retry_after.map_or(5_000, |r| (r * 1000.0) as i64),
        _ => {
            s.live.paused_until = now_ms() + retry_after.map_or(60_000, |r| (r * 1000.0) as i64);
            let message = body.pointer("/error/message").and_then(Value::as_str).map(String::from).unwrap_or(format!("Mania Tracker answered {}.", res.status));
            set_status(b, s, "error", Some(message));
        }
    }
}

pub(crate) async fn set_live_setting(b: &Arc<Bridge>, on: bool) -> bool {
    {
        let mut s = b.st();
        s.config.settings.get_or_insert_with(Default::default).live = on;
        s.live.paused_until = 0;
    }
    let saved = b.save_config().await;
    log!("live watching {}", if on { "on" } else { "off" });
    let b2 = b.clone();
    let _ = tokio::spawn(async move { live_tick(&b2).await }).await;
    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_over_is_the_same_play_and_a_retry_isnt() {
        let play: Vec<(f64, u32)> = (0..400).map(|i| (-1600.0 + 50.0 * i as f64, i % 3)).collect();
        assert!(same_play(&play[..300], &play));
        assert!(same_play(&play, &play[..200]));
        assert!(!same_play(&play, &play[..3]));
        assert!(!same_play(&[], &play));
        assert_eq!(changes(&[(0.0, 1)], &[(1.0, 1), (2.0, 0), (3.0, 0), (4.0, 2)]), [(2.0, 0), (4.0, 2)]);
    }

    #[test]
    fn a_song_goes_a_piece_at_a_time_up_to_where_it_is_wanted() {
        let mb = 1 << 20;
        assert_eq!(next_piece(0, mb, 100, false), Some((0, SONG_PIECE - 1, false)));
        assert_eq!(next_piece(SONG_PIECE, mb, 100, false), None);
        // an Ogg's end right after its first piece
        assert_eq!(next_piece(SONG_PIECE, mb, mb, true), Some((mb - OGG_TAIL, mb - 1, true)));
        assert_eq!(next_piece(0, mb, mb, true), Some((0, SONG_PIECE - 1, false)));
        assert_eq!(next_piece(mb - 10, mb, mb, true), Some((mb - 10, mb - 1, false)));
        assert_eq!(next_piece(mb, mb, mb * 2, false), None);
    }

    fn stream() -> Stream {
        Stream { id: "s1".into(), key: "k".into(), seq: 0, outbox: VecDeque::new(), sent_at: 0, failing_since: None, frames_capped: false }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_song_is_sent_as_the_play_goes_and_stops_when_whole() {
        use axum::{extract::Path as At, http::HeaderMap, routing::put, Router};
        use std::sync::Mutex;
        type Got = Arc<Mutex<(Vec<(u64, u64)>, Vec<bool>)>>;
        let total: u64 = 600 << 10;
        let got: Got = Arc::new(Mutex::new((vec![], vec![false; total as usize])));
        let app = Router::new().route("/live/{id}/song/{md5}", put({
            let got = got.clone();
            move |At((_, _)): At<(String, String)>, h: HeaderMap, body: Bytes| {
                let got = got.clone();
                async move {
                    let range = h["content-range"].to_str().unwrap().trim_start_matches("bytes ").to_string();
                    let (span, all) = range.split_once('/').unwrap();
                    let (a, z) = span.split_once('-').unwrap();
                    let (a, z, all): (u64, u64, u64) = (a.parse().unwrap(), z.parse().unwrap(), all.parse().unwrap());
                    assert_eq!(z - a + 1, body.len() as u64);
                    let mut g = got.lock().unwrap();
                    g.0.push((a, z));
                    g.1[a as usize..=z as usize].fill(true);
                    let have = g.1.iter().position(|x| !x).unwrap_or(g.1.len());
                    ([("content-type", "application/json")], json!({ "have": have, "total": all }).to_string())
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (b, dir) = crate::auth::tests::test_bridge(&base);
        std::fs::create_dir_all(&dir).unwrap();
        let mut song = b"OggS".to_vec();
        song.resize(total as usize, 7);
        std::fs::write(dir.join("song"), &song).unwrap();
        let md5 = "0123456789abcdef0123456789abcdef";
        {
            let mut s = b.st();
            s.caps = Some(json!({ "api_base_url": base }));
            s.reader.client = Some("lazer".into());
            s.reader.lazer_files_dir = dir.clone();
            s.reader.last_frame = Some(json!({ "client": "lazer", "beatmap": { "checksum": md5, "time": { "lastObject": 100_000.0 } }, "files": { "audio": "song" } }));
            s.live.stream = Some(stream());
            s.live.play = Some(Play { id: "p1".into(), md5: md5.into(), start: json!({}), sent: 0, clock: Some((0, false, false)), moved_at: 0, started: true, opened_at: 0, had_health: false, empty_since: None, stats: None });
            s.live.song = Some((md5.into(), 0));
        }
        let task = tokio::spawn({
            let b = b.clone();
            async move { send_song(&b, "s1", "k", "p1", md5).await }
        });
        // at the start: the first 20 s of a 100 s song is one piece, then the Ogg's end
        tokio::time::sleep(Duration::from_millis(1600)).await;
        assert_eq!(got.lock().unwrap().0, [(0, SONG_PIECE - 1), (total - OGG_TAIL, total - 1)]);
        // most of the way in, the rest goes, and then it's done
        b.st().live.play.as_mut().unwrap().clock = Some((80_000, false, false));
        let end = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap();
        assert!(end == SongEnd::Stopped);
        let g = got.lock().unwrap();
        assert!(g.1.iter().all(|x| *x));
        assert_eq!(g.0[2], (SONG_PIECE, 2 * SONG_PIECE - 1));
        let _ = std::fs::remove_dir_all(dir);
    }
}
