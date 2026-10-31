// PUT on every change and every 30s while osu! is open, DELETE when it closes.
// the site drops it 90s after the last one

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};

use crate::auth::{api, presence_supported, Req, Resp};
use crate::{iso_now, log, now_ms, Bridge, State, PRESENCE_SCOPE};

pub(crate) const PRESENCE_TICK_MS: u64 = 500;
const PRESENCE_MIN_GAP_MS: i64 = 2000;
const PRESENCE_HEARTBEAT_MS: i64 = 30_000;
const PRESENCE_SETTLE_MS: i64 = 1500;
const PRESENCE_TEXT_MAX: usize = 256;
const RULESETS: [&str; 4] = ["osu", "taiko", "fruits", "mania"];

fn presence_state(tosu_state: &str) -> &'static str {
    match tosu_state {
        "menu" => "menu",
        "selectPlay" | "selectDrawings" => "song_select",
        "play" => "playing",
        "resultScreen" => "results",
        "edit" | "selectEdit" => "editing",
        "selectMulti" | "lobby" | "matchSetup" => "multiplayer",
        _ => "idle",
    }
}

// tosu has no replay or spectating flag, only the name on the play and the one osu! is signed in
// as. offline lazer plays live as "Guest", and nobody can spectate while offline
pub(crate) fn whose_play(raw: &Value, who: &str, account: Option<&str>) -> &'static str {
    let same = |name: Option<&str>| name.is_some_and(|n| n.eq_ignore_ascii_case(who));
    let Some(osu) = raw.pointer("/profile/name").and_then(Value::as_str).filter(|n| !n.is_empty()) else { return "playing" };
    let status = raw.pointer("/profile/userStatus/name").and_then(Value::as_str);
    let online = !osu.eq_ignore_ascii_case("guest") && !matches!(status, Some("offline" | "reconnecting" | "connecting"));
    if same(Some(osu)) {
        "playing"
    } else if same(account) || !online {
        "watching_replay"
    } else {
        "spectating"
    }
}

fn site_has_state(s: &State, state: &str) -> bool {
    s.caps
        .as_ref()
        .and_then(|c| c.pointer("/presence/states"))
        .and_then(Value::as_array)
        .is_some_and(|l| l.iter().any(|x| x.as_str() == Some(state)))
}

// the site ignores a map in these
fn no_map(state: &str) -> bool {
    ["menu", "multiplayer", "idle"].contains(&state)
}

pub(crate) struct Sent {
    key: String,
    at: i64,
    state: String,
    body: Value,
}

// off | unsupported | needs_reconnect | sharing | paused | error
pub(crate) struct PresenceState {
    pub status: &'static str,
    pub message: Option<String>,
    // the last PUT, None after a DELETE
    pub sent: Option<Sent>,
    seen_key: Option<String>,
    seen_at: i64,
    started_at: Option<String>,
    busy: bool,
    paused_until: i64,
}

impl Default for PresenceState {
    fn default() -> Self {
        PresenceState { status: "off", message: None, sent: None, seen_key: None, seen_at: 0, started_at: None, busy: false, paused_until: 0 }
    }
}

fn set_presence(b: &Arc<Bridge>, s: &mut State, status: &'static str, message: Option<String>) {
    if s.presence.status == status && s.presence.message == message {
        return;
    }
    s.presence.status = status;
    s.presence.message = message;
    b.broadcast();
}

fn presence_granted(s: &State) -> bool {
    s.config.connection.as_ref().and_then(|c| c.scope.as_str()).unwrap_or("").split(' ').any(|x| x == PRESENCE_SCOPE)
}

fn sharing_on(s: &State) -> bool {
    s.config.settings.as_ref().is_some_and(|x| x.presence)
}

fn clip(v: Option<&Value>) -> Value {
    match v.and_then(Value::as_str).filter(|x| !x.is_empty()) {
        Some(x) => x.chars().take(PRESENCE_TEXT_MAX).collect::<String>().into(),
        None => Value::Null,
    }
}

// the regex /10K|[1-9]K|SV2|[A-Z]{2}/g over a name like "HDDT"
pub(crate) fn split_mods(name: &str) -> Vec<String> {
    let c: Vec<char> = name.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < c.len() {
        let at = |k: usize| c.get(i + k).copied().unwrap_or('\0');
        let len = if at(0) == '1' && at(1) == '0' && at(2) == 'K' {
            3
        } else if ('1'..='9').contains(&at(0)) && at(1) == 'K' {
            2
        } else if at(0) == 'S' && at(1) == 'V' && at(2) == '2' {
            3
        } else if at(0).is_ascii_uppercase() && at(1).is_ascii_uppercase() {
            2
        } else {
            0
        };
        if len == 0 {
            i += 1;
        } else {
            out.push(c[i..i + len].iter().collect());
            i += len;
        }
    }
    out
}

pub(crate) fn mod_acronyms(mods: &Value) -> Vec<String> {
    let list: Vec<String> = match mods.get("array").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        Some(a) => a.iter().filter_map(|m| m.as_str().or_else(|| m.get("acronym").and_then(Value::as_str)).map(String::from)).collect(),
        None => split_mods(&crate::tosu::mods_name(mods)),
    };
    list.into_iter().filter(|m| !m.is_empty() && m.len() <= 8).take(32).collect()
}

fn mod_rate(mods: &Value, acronyms: &[String]) -> f64 {
    let has = |l: &[&str]| acronyms.iter().any(|m| l.contains(&m.as_str()));
    let rate = match mods.get("rate").and_then(Value::as_f64).filter(|r| *r > 0.0) {
        Some(r) => r,
        None if has(&["DT", "NC"]) => 1.5,
        None if has(&["HT", "DC"]) => 0.75,
        None => 1.0,
    };
    (rate.clamp(0.5, 2.0) * 100.0).round() / 100.0
}

fn presence_body(s: &mut State) -> Option<Value> {
    let raw = s.tosu.last_frame.as_ref()?;
    if !s.tosu.osu {
        return None;
    }
    let null = Value::Null;
    let bm = raw.get("beatmap").unwrap_or(&null);
    let mut state = presence_state(s.tosu.state.as_deref().unwrap_or(""));
    let who = raw.pointer("/play/playerName").and_then(Value::as_str).filter(|x| !x.is_empty());
    if state == "playing" && s.tosu.replay {
        state = "watching_replay";
    } else if let (true, Some(who)) = (state == "playing", who) {
        let account = s.config.connection.as_ref().map(|c| crate::types::text(&c.username));
        state = whose_play(raw, who, account.as_deref());
    }
    // older sites refuse states they don't list
    if state == "watching_replay" && !site_has_state(s, state) {
        state = "spectating";
    }

    let play_mode = raw.pointer("/play/mode/number").and_then(Value::as_u64);
    let map_mode = bm.pointer("/mode/number").and_then(Value::as_u64);
    let mode = if state == "playing" { play_mode.or(map_mode) } else { map_mode.or(play_mode) };
    let ruleset = mode.and_then(|m| RULESETS.get(m as usize)).copied();

    let md5 = bm
        .get("checksum")
        .and_then(Value::as_str)
        .filter(|c| c.len() == 32 && c.chars().all(|x| x.is_ascii_hexdigit()))
        .map(str::to_lowercase);
    let keys = bm
        .pointer("/stats/cs/converted")
        .and_then(Value::as_f64)
        .or(bm.pointer("/stats/cs/original").and_then(Value::as_f64))
        .unwrap_or(0.0)
        .round() as i64;
    let mods_raw = raw.pointer("/play/mods").unwrap_or(&null);
    let mods = mod_acronyms(mods_raw);
    let rate = mod_rate(mods_raw, &mods);
    let beatmap = match md5 {
        Some(md5) if !no_map(state) => json!({
            "md5": md5,
            "beatmap_id": bm.get("id").filter(|v| v.as_f64().is_some_and(|i| i > 0.0)),
            "artist": clip(bm.get("artist")),
            "title": clip(bm.get("title")),
            "version": clip(bm.get("version")),
            "creator": clip(bm.get("mapper")),
            "key_count": if ruleset == Some("mania") && (1..=10).contains(&keys) { Some(keys) } else { None },
        }),
        _ => Value::Null,
    };
    let mut body = json!({
        "game_client": s.tosu.client.as_deref().unwrap_or("stable"),
        "state": state,
        "ruleset": ruleset,
        "beatmap": beatmap,
        "mods": mods,
        "rate": rate,
    });
    if matches!(state, "spectating" | "watching_replay") {
        body["player"] = json!({ "username": clip(who.map(Value::from).as_ref()) });
    }

    if state == "playing" {
        s.presence.started_at.get_or_insert_with(iso_now);
    } else {
        s.presence.started_at = None;
    }
    if let Some(at) = &s.presence.started_at {
        body["started_at"] = at.clone().into();
    }
    Some(body)
}

enum Next {
    Nothing,
    Clear,
    Put(String, Value),
}

pub(crate) async fn presence_tick(b: &Arc<Bridge>) {
    let next = {
        let mut s = b.st();
        if s.presence.busy {
            return;
        }
        let on = sharing_on(&s);
        let connected = s.connection.status == "connected" && s.config.connection.as_ref().is_some_and(|c| c.refresh_token.is_some());
        if !on || !connected {
            set_presence(b, &mut s, if on { "paused" } else { "off" }, None);
            if s.presence.sent.is_some() && connected {
                Next::Clear
            } else {
                Next::Nothing
            }
        } else if !presence_supported(&s) {
            set_presence(b, &mut s, "unsupported", None);
            Next::Nothing
        } else if !presence_granted(&s) {
            set_presence(b, &mut s, "needs_reconnect", None);
            Next::Nothing
        } else if now_ms() < s.presence.paused_until {
            Next::Nothing
        } else {
            match presence_body(&mut s) {
                None if s.presence.sent.is_some() => Next::Clear,
                None => {
                    set_presence(b, &mut s, "paused", None);
                    Next::Nothing
                }
                Some(body) => decide(&mut s, body),
            }
        }
    };
    match next {
        Next::Nothing => {}
        Next::Clear => clear_presence(b).await,
        Next::Put(key, body) => put(b, key, body).await,
    }
}

fn decide(s: &mut State, body: Value) -> Next {
    let now = now_ms();
    let key = body.to_string();
    if s.presence.seen_key.as_deref() != Some(&key) {
        s.presence.seen_key = Some(key.clone());
        s.presence.seen_at = now;
    }
    let p = &s.presence;
    let since = p.sent.as_ref().map_or(i64::MAX, |x| now - x.at);
    let changed = p.sent.as_ref().is_none_or(|x| x.key != key);
    let state = body.get("state").and_then(Value::as_str).unwrap_or("");
    let browsing = changed && p.sent.as_ref().is_some_and(|x| x.state == "song_select") && state == "song_select";
    let settled = now - p.seen_at >= PRESENCE_SETTLE_MS;
    if since < PRESENCE_MIN_GAP_MS {
        return Next::Nothing;
    }
    let out = if changed && (!browsing || settled) {
        (key, body)
    } else if since >= PRESENCE_HEARTBEAT_MS {
        match (browsing, &p.sent) {
            (true, Some(sent)) => (sent.key.clone(), sent.body.clone()),
            _ => (key, body),
        }
    } else {
        return Next::Nothing;
    };
    s.presence.busy = true;
    Next::Put(out.0, out.1)
}

async fn put(b: &Arc<Bridge>, key: String, body: Value) {
    let req = Req {
        method: reqwest::Method::PUT,
        headers: vec![("content-type", "application/json".into())],
        body: Some(Bytes::from(body.to_string())),
        timeout: std::time::Duration::from_secs(30),
    };
    match api(b, "/presence", req).await {
        Ok(res) => {
            presence_answer(b, &res);
            if res.ok() {
                let mut s = b.st();
                let state = body.get("state").and_then(Value::as_str).unwrap_or("").to_string();
                s.presence.sent = Some(Sent { key, at: now_ms(), state, body });
                set_presence(b, &mut s, "sharing", None);
            }
        }
        Err(e) => {
            let mut s = b.st();
            s.presence.paused_until = now_ms() + 15_000;
            set_presence(b, &mut s, "error", Some(e.message().into()));
        }
    }
    b.st().presence.busy = false;
}

pub(crate) async fn clear_presence(b: &Arc<Bridge>) {
    b.st().presence.busy = true;
    match api(b, "/presence", Req::method(reqwest::Method::DELETE)).await {
        Ok(res) if res.ok() || res.status == 404 => b.st().presence.sent = None,
        Ok(res) => presence_answer(b, &res),
        // it expires on the site anyway
        Err(_) => b.st().presence.sent = None,
    }
    let mut s = b.st();
    s.presence.busy = false;
    let status = if sharing_on(&s) { "paused" } else { "off" };
    set_presence(b, &mut s, status, None);
}

fn presence_answer(b: &Arc<Bridge>, res: &Resp) {
    if res.ok() {
        return;
    }
    let body = res.json();
    let code = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .map(String::from)
        .unwrap_or_else(|| format!("http_{}", res.status));
    let mut s = b.st();
    if res.status == 404 {
        // a site that lists the scope but has no endpoint, treat it as not there
        s.presence.paused_until = now_ms() + 10 * 60_000;
        return set_presence(b, &mut s, "unsupported", None);
    }
    if res.status == 403 && code == "insufficient_scope" {
        return set_presence(b, &mut s, "needs_reconnect", None);
    }
    // retry_after in the body, Retry-After as a header, both in seconds
    let retry_after = body
        .get("retry_after")
        .and_then(Value::as_f64)
        .or(body.pointer("/error/retry_after").and_then(Value::as_f64))
        .or(res.header("retry-after").and_then(|h| h.trim().parse().ok()))
        .filter(|r| r.is_finite() && *r > 0.0);
    s.presence.paused_until = now_ms() + retry_after.map_or(60_000, |r| (r * 1000.0) as i64);
    if res.status == 429 {
        return;
    }
    let message = body.pointer("/error/message").and_then(Value::as_str).map(String::from).unwrap_or(format!("The site answered {}.", res.status));
    set_presence(b, &mut s, "error", Some(message));
}

pub(crate) async fn set_presence_setting(b: &Arc<Bridge>, on: bool) -> bool {
    {
        let mut s = b.st();
        s.config.settings.get_or_insert_with(Default::default).presence = on;
        s.presence.paused_until = 0;
        if !on {
            set_presence(b, &mut s, "off", None);
        }
    }
    let saved = b.save_config().await;
    log!("sharing live status {}", if on { "on" } else { "off" });
    // a task of its own: the page can leave mid-request, and a dropped one would leave it busy for good
    let b2 = b.clone();
    let _ = tokio::spawn(async move { presence_tick(&b2).await }).await;
    saved
}
