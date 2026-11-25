use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::types::{truthy, Binding, Cover, Item, Play};
use crate::{iso_now, log, now_ms, replays, Bridge, AUTOMATED_MODS};

// lazer showed song select for ~300ms between a score's results and its replay
const FROM_RESULTS_MS: i64 = 3000;
const SCORE_DATE_SLACK_MS: i64 = 60_000;
// song time this far before a finished play's last note means a new attempt or song select's preview
const RESTART_GAP_MS: f64 = 5000.0;

#[derive(Clone)]
pub(crate) struct Beatmap {
    md5: Option<String>,
    artist: Value,
    title: Value,
    version: Value,
    set: Value,
    keys: Value,
}

#[derive(Clone)]
pub(crate) struct LastPlay {
    client: String,
    mode: Value,
    // play.mode and resultsScreen.mode can say osu on a mania map, beatmap.mode doesn't
    beatmap_mode: Value,
    mods: Value,
    player_name: Value,
    // "playing", or someone else's play / a replay, which aren't sent
    whose: &'static str,
    beatmap: Beatmap,
    // lazer gives paths relative to files/, by hash
    chart_file: Option<String>,
    audio_file: Option<String>,
    // relative to Songs (or files/ on lazer), for the cover in the ui
    cover: Option<Cover>,
    // past the last note and not failed: quitting partway saves no score
    completed: bool,
    // whose says watching_replay only from where it started, a new score's date overrules that
    from_results: bool,
    started_at: i64,
}

pub(crate) struct PendingFinish {
    since: i64,
    play: LastPlay,
    viewed: Option<String>,
    stale: Option<String>,
}

// tosu keeps the socket open without osu!, it just stops sending
// blocked: { fix } when osu! runs but linux won't let hashi-reader read it
pub(crate) struct TosuState {
    pub connected: bool,
    pub osu: bool,
    pub client: Option<String>,
    pub state: Option<String>,
    pub player: Option<String>,
    // the mania map selected or being played, for its leaderboard
    pub map: Option<Value>,
    // its background, for the overlay
    pub map_cover: Option<Cover>,
    pub blocked: Option<String>,
    pub lazer_files_dir: PathBuf,
    // stable's folder and its Songs, wherever it's installed
    pub stable_dirs: Option<(PathBuf, PathBuf)>,
    // the skin's folder name, stable's
    pub skin: Option<String>,
    // as tosu last reported them, a wine drive other than Z: is looked up on disk
    stable_folders: Option<(String, String)>,
    pub last_frame_at: i64,
    pub last_frame: Option<Value>,
    prev_state: Option<String>,
    last_play: Option<LastPlay>,
    last_result_shown: Option<String>,
    viewed_result: Option<String>,
    // the results up are from a play finished here, a retry starts from those. not by result_key,
    // what tosu reads for them can change while they're up
    own_results: bool,
    // started from the results of some other score: stable's local scores open there first
    pub replay: bool,
    // the play is in torii, whose tosu-torii reports its results as play
    torii: bool,
    // the attempt's song time was seen before its first note. lazer always starts play before it,
    // so play that doesn't is something else: torii's song select preview, a moment of play on
    // lazer's results
    saw_start: bool,
    results_left_at: i64,
    play_started_at: i64,
    pending_finish: Option<PendingFinish>,
}

impl TosuState {
    pub fn new(lazer_files_dir: PathBuf) -> Self {
        TosuState {
            connected: false,
            osu: false,
            client: None,
            state: None,
            player: None,
            map: None,
            map_cover: None,
            blocked: None,
            lazer_files_dir,
            stable_dirs: None,
            skin: None,
            stable_folders: None,
            last_frame_at: 0,
            last_frame: None,
            prev_state: None,
            last_play: None,
            last_result_shown: None,
            viewed_result: None,
            own_results: false,
            replay: false,
            torii: false,
            saw_start: false,
            results_left_at: 0,
            play_started_at: 0,
            pending_finish: None,
        }
    }

    pub fn public(&self) -> Value {
        json!({
            "connected": self.connected,
            "osu": self.osu,
            "client": self.client,
            "state": self.state,
            "player": self.player,
            "map": self.map,
            "blocked": self.blocked.as_ref().map(|fix| json!({ "fix": fix })),
        })
    }

    fn gone(&mut self) {
        self.osu = false;
        self.client = None;
        self.state = None;
        self.player = None;
        self.map = None;
        self.map_cover = None;
    }
}

// a path tosu sees. stable runs under wine on linux, where only Z: is the real filesystem
fn local_path(p: &str) -> Option<PathBuf> {
    if p.is_empty() {
        return None;
    }
    if cfg!(windows) || p.starts_with('/') {
        return Some(PathBuf::from(p));
    }
    wine_path(p, &wine_prefixes())
}

// osu!.cfg's BeatmapDirectory can be relative to the game folder, and tosu passes that on as is
fn songs_path(game: &Path, p: &str) -> Option<PathBuf> {
    if p.is_empty() {
        return None;
    }
    if p.contains(':') || p.starts_with(['\\', '/']) {
        return local_path(p);
    }
    Some(game.join(p.replace('\\', "/")))
}

// X:\a\b through the prefix's dosdevices/x: link. osu-winello puts osu! on D:
fn wine_path(p: &str, prefixes: &[PathBuf]) -> Option<PathBuf> {
    let (drive, rest) = p.split_once(":\\")?;
    let drive = drive.to_ascii_lowercase();
    if drive.len() != 1 || !drive.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let rest = rest.replace('\\', "/");
    if drive == "z" {
        return Some(PathBuf::from(format!("/{rest}")));
    }
    prefixes.iter().map(|w| w.join("dosdevices").join(format!("{drive}:")).join(&rest)).find(|f| f.exists())
}

// tosu runs in osu!'s prefix, which isn't ours to ask about
fn wine_prefixes() -> Vec<PathBuf> {
    let home = crate::home();
    let data = crate::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or(home.join(".local").join("share"));
    let mut all: Vec<PathBuf> = crate::var("WINEPREFIX").map(PathBuf::from).into_iter().collect();
    all.push(data.join("wineprefixes").join("osu-wineprefix"));
    if let Ok(dirs) = std::fs::read_dir(data.join("wineprefixes")) {
        all.extend(dirs.flatten().map(|d| d.path()));
    }
    all.push(home.join(".wine"));
    all
}

pub(crate) fn mods_number(mods: &Value) -> i64 {
    mods.get("number").and_then(Value::as_i64).unwrap_or(0)
}

pub(crate) fn mods_name(mods: &Value) -> String {
    mods.get("name").and_then(Value::as_str).unwrap_or("").to_string()
}

fn is_mania(mode: &Value) -> bool {
    mode.get("number").and_then(Value::as_i64) == Some(3)
        || mode.get("name").and_then(Value::as_str).is_some_and(|n| n.to_lowercase().contains("mania"))
}

fn result_key(rs: &Value) -> String {
    let g = |k: &str| rs.get(k).cloned().unwrap_or(Value::Null);
    json!([g("createdAt"), g("score"), g("hits"), g("playerName")]).to_string()
}

// tosu filled in who and when, the reader only the judgements
fn populated(rs: &Value) -> bool {
    rs.get("hits").and_then(Value::as_object).is_some_and(|h| h.values().any(|n| n.as_u64().is_some_and(|n| n > 0)))
        || rs.get("playerName").is_some_and(|v| !v.is_null())
        || rs.get("createdAt").is_some_and(|v| !v.is_null())
        || rs.get("score").and_then(Value::as_f64).is_some_and(|s| s > 0.0)
}

fn str_of(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(String::from)
}

fn selected_map(raw: &Value) -> Option<Value> {
    let bm = raw.get("beatmap")?;
    if bm.pointer("/mode/number").and_then(Value::as_i64) != Some(3) {
        return None;
    }
    let md5 = bm.get("checksum").and_then(Value::as_str).filter(|c| crate::leaderboard::valid_md5(c))?.to_lowercase();
    let keys = bm.pointer("/stats/cs/original").and_then(Value::as_f64).map(f64::round);
    let raw_mods = raw.pointer("/play/mods").unwrap_or(&Value::Null);
    let acronyms = crate::presence::mod_acronyms(raw_mods);
    let (speed, mods) = crate::leaderboard::variant(&acronyms);
    let has = |m: &str| acronyms.iter().any(|a| a == m);
    Some(json!({
        "md5": md5,
        // osu!'s id for it, none for a map that was never submitted or one made here
        "id": bm.get("id").and_then(Value::as_i64).filter(|id| *id > 0),
        "artist": bm.get("artist"),
        "title": bm.get("title"),
        "version": bm.get("version"),
        "keys": keys,
        "speed": speed,
        "mods": mods,
        // for its dan, which goes by the exact rate and the judging
        "rate": crate::presence::mod_rate(raw_mods, &acronyms),
        "od": if has("HR") { Some("HR") } else if has("EZ") { Some("EZ") } else { None },
    }))
}

fn map_cover(raw: &Value, client: &str) -> Option<Cover> {
    let file = str_of(raw.pointer("/files/background")).filter(|x| !x.is_empty())?;
    if client == "lazer" {
        return Some(Cover { lazer: Some(true), folder: None, file });
    }
    let folder = str_of(raw.pointer("/folders/beatmap")).filter(|x| !x.is_empty())?;
    Some(Cover { lazer: None, folder: Some(folder), file })
}

pub(crate) fn on_tosu_message(b: &Arc<Bridge>, raw: Value) {
    let mut finished = None;
    {
        let mut s = b.st();
        let account = s.config.connection.as_ref().map(|c| crate::types::text(&c.username));
        let t = &mut s.tosu;
        t.last_frame_at = now_ms();
        if !t.osu {
            t.osu = true;
            t.blocked = None;
            b.broadcast();
        }
        let state = str_of(raw.pointer("/state/name"))
            .or_else(|| raw.pointer("/state/number").map(|n| n.to_string()))
            .unwrap_or_else(|| "unknown".into());
        let player = str_of(raw.pointer("/profile/name"));
        let client = if raw.get("client").and_then(Value::as_str) == Some("lazer") { "lazer" } else { "stable" };
        // lazer reports "." here, its skins have no folder
        if client == "stable" {
            t.skin = str_of(raw.pointer("/folders/skin")).filter(|x| !x.is_empty());
        }
        if client == "lazer" {
            if let Some(songs) = raw.pointer("/folders/songs").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                t.lazer_files_dir = PathBuf::from(songs);
            }
        } else {
            let folder = |k: &str| raw.pointer(&format!("/folders/{k}")).and_then(Value::as_str).unwrap_or_default().to_string();
            let folders = (folder("game"), folder("songs"));
            if t.stable_folders.as_ref() != Some(&folders) {
                let game = local_path(&folders.0);
                let songs = game.as_deref().and_then(|g| songs_path(g, &folders.1));
                if let (Some(game), Some(songs)) = (game, songs) {
                    if t.stable_dirs.as_ref() != Some(&(game.clone(), songs.clone())) && game.is_dir() {
                        log!("osu! is in {}, its songs in {}", game.display(), songs.display());
                        t.stable_dirs = Some((game, songs));
                    }
                }
                t.stable_folders = Some(folders);
            }
        }
        if t.state.as_deref() != Some(&state) || t.player != player || t.client.as_deref() != Some(client) {
            t.state = Some(state.clone());
            t.player = player;
            t.client = Some(client.into());
            b.broadcast();
        }

        let map = selected_map(&raw);
        if t.map != map {
            t.map = map;
            b.broadcast();
        }
        t.map_cover = t.map.as_ref().and_then(|_| map_cover(&raw, client));

        if t.prev_state.as_deref() == Some("resultScreen") && state != "resultScreen" {
            t.results_left_at = now_ms();
        }
        if state == "play" {
            if t.prev_state.as_deref() != Some("play") {
                // retry or watch replay from results, remember which score was up. lazer goes back
                // to song select for a moment on the way
                let from_results = t.prev_state.as_deref() == Some("resultScreen") || now_ms() - t.results_left_at < FROM_RESULTS_MS;
                t.viewed_result = if from_results { t.last_result_shown.clone() } else { None };
                t.replay = from_results && !t.own_results;
                t.play_started_at = now_ms();
                t.torii = client == "lazer" && t.lazer_files_dir.parent().is_some_and(crate::lazer::is_torii);
                t.saw_start = false;
            }
            let null = Value::Null;
            let bm = raw.get("beatmap").unwrap_or(&null);
            let g = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
            let files = raw.get("files").unwrap_or(&null);
            let background = str_of(files.get("background")).filter(|x| !x.is_empty());
            let at = |k: &str| bm.pointer(&format!("/time/{k}")).and_then(Value::as_f64).unwrap_or(0.0);
            let at_start = at("live") <= at("firstObject");
            t.saw_start |= at_start;
            // where tosu said it was, lazer's by hash under files/
            let map_file = |k: &str| match (str_of(files.get(k)).filter(|x| !x.is_empty()), client == "lazer") {
                (Some(f), true) => Some(t.lazer_files_dir.join(f).to_string_lossy().into_owned()),
                (Some(f), false) => match (str_of(raw.pointer("/folders/beatmap")).filter(|x| !x.is_empty()), &t.stable_dirs) {
                    (Some(folder), Some((_, songs))) => Some(songs.join(folder.replace('\\', "/")).join(f).to_string_lossy().into_owned()),
                    _ => None,
                },
                _ => None,
            };
            let play = LastPlay {
                client: client.into(),
                mode: raw.pointer("/play/mode").cloned().unwrap_or(Value::Null),
                beatmap_mode: g(bm, "mode"),
                mods: raw.pointer("/play/mods").cloned().unwrap_or(Value::Null),
                player_name: raw.pointer("/play/playerName").cloned().unwrap_or(Value::Null),
                whose: match raw.pointer("/play/playerName").and_then(Value::as_str).filter(|x| !x.is_empty()) {
                    _ if t.replay || raw.get("replay").and_then(Value::as_bool) == Some(true) => "watching_replay",
                    Some(who) => crate::presence::whose_play(&raw, who, account.as_deref()),
                    None => "playing",
                },
                beatmap: Beatmap {
                    md5: str_of(bm.get("checksum")).map(|c| c.to_lowercase()),
                    artist: g(bm, "artist"),
                    title: g(bm, "title"),
                    version: g(bm, "version"),
                    set: g(bm, "set"),
                    keys: bm
                        .pointer("/stats/cs/converted")
                        .filter(|v| !v.is_null())
                        .or(bm.pointer("/stats/cs/original"))
                        .cloned()
                        .unwrap_or(Value::Null),
                },
                chart_file: map_file("beatmap"),
                audio_file: map_file("audio"),
                cover: if client == "lazer" {
                    background.map(|file| Cover { lazer: Some(true), folder: None, file })
                } else {
                    match (str_of(raw.pointer("/folders/beatmap")).filter(|x| !x.is_empty()), background) {
                        (Some(folder), Some(file)) => Some(Cover { lazer: None, folder: Some(folder), file }),
                        _ => None,
                    }
                },
                completed: at("lastObject") > 0.0
                    && at("live") >= at("lastObject")
                    && raw.pointer("/play/failed").and_then(Value::as_bool) != Some(true)
                    && (t.saw_start || client != "lazer"),
                from_results: t.replay,
                started_at: t.play_started_at,
            };
            // a finished torii play is kept as it ended: tosu-torii reports torii's results as play, so
            // play again or going back to song select can come while it still says play
            let finished = t.last_play.as_ref().filter(|p| t.torii && p.completed && p.started_at == t.play_started_at).cloned();
            match finished {
                Some(done) if at("live") < at("lastObject") - RESTART_GAP_MS => {
                    t.pending_finish = Some(PendingFinish { since: now_ms(), play: done, viewed: None, stale: None });
                    t.play_started_at = now_ms();
                    t.saw_start = at_start;
                    t.last_play = Some(LastPlay { started_at: t.play_started_at, ..play });
                }
                Some(_) => {}
                None => t.last_play = Some(play),
            }
        }

        // multiplayer results aren't resultScreen: stable has its own ranking screens, and to tosu
        // lazer goes straight back to the room (or the playlist)
        let room_results = if client == "lazer" {
            !["play", "resultScreen"].contains(&state.as_str())
        } else {
            ["rankingVs", "rankingTeam", "rankingTagCoop"].contains(&state.as_str())
        };
        if room_results && t.prev_state.as_deref() == Some("play") {
            if let Some(play) = t.last_play.clone().filter(|p| p.completed) {
                t.pending_finish = Some(PendingFinish { since: now_ms(), play, viewed: None, stale: None });
            }
        }

        if state == "resultScreen" && t.prev_state.as_deref() != Some("resultScreen") {
            let finished = t.prev_state.as_deref() == Some("play");
            t.own_results = finished && t.last_play.as_ref().is_some_and(|p| p.whose == "playing");
            if let Some(play) = t.last_play.clone().filter(|p| finished && (p.completed || p.client != "lazer")) {
                t.pending_finish = Some(PendingFinish { since: now_ms(), play, viewed: t.viewed_result.clone(), stale: t.last_result_shown.clone() });
            }
        }

        if let Some(pending) = &t.pending_finish {
            // resultsScreen lags a few polls behind the state change
            let rs = raw.get("resultsScreen").cloned().unwrap_or(json!({}));
            let fresh = state == "resultScreen" && populated(&rs) && Some(result_key(&rs)) != pending.stale;
            if state != "resultScreen" || fresh || now_ms() - pending.since > 5000 {
                let finish = t.pending_finish.take().unwrap();
                finished = Some((finish, fresh.then_some(rs)));
            }
        }

        if state == "resultScreen" {
            if let Some(rs) = raw.get("resultsScreen").filter(|rs| populated(rs)) {
                t.last_result_shown = Some(result_key(rs));
            }
        }

        t.prev_state = Some(state);
        t.last_frame = Some(raw);
    }
    if let Some((finish, rs)) = finished {
        on_play_finished(b, finish, rs);
    }
}

fn on_play_finished(b: &Arc<Bridge>, finish: PendingFinish, rs: Option<Value>) {
    let PendingFinish { play, viewed, stale, .. } = finish;
    let bm = &play.beatmap;
    let label = format!("{} - {} [{}]", crate::types::text(&bm.artist), crate::types::text(&bm.title), crate::types::text(&bm.version));
    // a replay's results keep the date of the play it recorded
    let new_score = rs
        .as_ref()
        .and_then(|r| r.get("createdAt"))
        .and_then(Value::as_str)
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.timestamp_millis() >= play.started_at - SCORE_DATE_SLACK_MS);
    if new_score == Some(false) {
        return log!("skip (watched replay, an older score): {label}");
    }
    // the reader tells a replay by its results: lazer's ReplayPlayer, stable's name that isn't this computer's
    if rs.as_ref().and_then(|r| r.get("replay")).and_then(Value::as_bool) == Some(true) {
        return log!("skip (watched replay): {label}");
    }
    if play.whose != "playing" && !(play.from_results && new_score == Some(true)) {
        return log!("skip ({}, {}): {label}", play.whose.replace('_', " "), crate::types::text(&play.player_name));
    }
    // same results as before = watched the replay
    if rs.is_none() && viewed.is_some() && viewed == stale {
        return log!("skip (watched replay): {label}");
    }
    let rs = rs.unwrap_or(json!({}));
    let g = |k: &str| rs.get(k).cloned().unwrap_or(Value::Null);
    // the .osr header decides for real before anything is sent
    if ![g("mode"), play.mode.clone(), play.beatmap_mode.clone()].iter().any(is_mania) {
        return log!("skip (not mania): {label}");
    }
    let mods = rs.get("mods").filter(|m| !m.is_null()).cloned().unwrap_or(play.mods.clone());
    if mods_number(&mods) & AUTOMATED_MODS != 0 {
        return log!("skip (automated mods {}): {label}", mods_name(&mods));
    }
    let Some(md5) = bm.md5.clone() else {
        return log!("skip (tosu gave no beatmap checksum): {label}");
    };
    let created = g("createdAt");
    let practice = crate::practice::is_practice(b, &md5, &crate::types::text(&bm.version));

    let item = {
        let mut s = b.st();
        let kept = s.sends_no_plays() && !practice;
        if kept && !s.analyzes() {
            drop(s);
            return log!("skip (sending plays is off): {label}");
        }
        // watch replay right after playing
        if truthy(&created) && s.queue.items.iter().any(|i| i.play.beatmap_md5 == md5 && i.play.result_created_at == created) {
            drop(s);
            return log!("skip (results already captured): {label}");
        }
        let status = s.connection.status.clone();
        let binding = s.config.connection.as_ref().filter(|c| c.refresh_token.is_some() && ["connected", "offline", "not_in_beta"].contains(&status.as_str())).map(|c| Binding {
            issuer: c.issuer.clone(),
            environment: c.environment.clone(),
            user_id: c.user_id.clone(),
            installation_id: c.installation_id.clone(),
        });
        let or = |v: Value, w: Value| if v.is_null() { w } else { v };
        let item = Item {
            id: format!("cbt-{}", uuid::Uuid::new_v4()),
            session_id: b.session_id.clone(),
            captured_at: iso_now(),
            detected_at_ms: now_ms(),
            binding,
            play: Play {
                client: Some(play.client.clone()),
                beatmap_md5: md5,
                artist: bm.artist.clone(),
                title: bm.title.clone(),
                version: bm.version.clone(),
                set: bm.set.clone(),
                keys: bm.keys.clone(),
                cover: play.cover.clone(),
                chart_file: play.chart_file.clone(),
                audio_file: play.audio_file.clone(),
                mods: mods_name(&mods),
                rate: None,
                player_name: or(g("playerName"), play.player_name.clone()),
                score: g("score"),
                accuracy: g("accuracy"),
                result_created_at: created,
            },
            phase: "waiting_replay".into(),
            replay: None,
            chart: None,
            submission_id: None,
            receipt: None,
            chart_sent: false,
            error: None,
            attempts: 0,
            next_attempt_at: 0,
            chart_needed: None,
            poll_delay: None,
            held_from: None,
            on_hold: false,
            practice,
            kept,
        };
        s.queue.items.push(item.clone());
        item
    };
    b.save_queue_later();
    log!("finished: {label} {}; waiting for the replay file", if item.play.mods.is_empty() { "NM" } else { &item.play.mods });
    b.broadcast();
    let b2 = b.clone();
    b.spawn(async move { replays::match_replays(&b2).await });
}

// hashi-reader closed: nothing's read until it's back
pub(crate) fn reader_gone(b: &Arc<Bridge>) {
    {
        let mut s = b.st();
        let t = &mut s.tosu;
        t.connected = false;
        t.gone();
        t.prev_state = None;
        t.last_play = None;
        t.pending_finish = None;
    }
    b.broadcast();
}

// the reader found no osu! to read
pub(crate) fn osu_gone(b: &Arc<Bridge>) {
    let was = {
        let mut s = b.st();
        let was = s.tosu.osu;
        s.tosu.gone();
        was
    };
    if was {
        b.broadcast();
    }
}

// Some(the command that lets it read osu!) while linux keeps it out
pub(crate) fn set_blocked(b: &Arc<Bridge>, blocked: Option<String>) {
    let changed = {
        let mut s = b.st();
        let changed = s.tosu.blocked != blocked;
        if changed {
            if let Some(fix) = &blocked {
                log!("osu! is running but can't be read; fix: {fix}");
            }
            s.tosu.blocked = blocked;
        }
        changed
    };
    if changed {
        b.broadcast();
    }
}

pub(crate) fn watch_osu_frames(b: &Arc<Bridge>) {
    let b2 = b.clone();
    b.spawn(async move {
        let mut second = tokio::time::interval(Duration::from_secs(1));
        loop {
            second.tick().await;
            let stopped = {
                let mut s = b2.st();
                let stopped = s.tosu.osu && now_ms() - s.tosu.last_frame_at > 3000;
                if stopped {
                    s.tosu.gone();
                }
                stopped
            };
            if stopped {
                b2.broadcast();
            }

        }
    });
}

// for a command that gets copied into a terminal
pub(crate) fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:@%,=".contains(c)) {
        return s.into();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    #[test]
    fn paths() {
        if cfg!(not(windows)) {
            assert_eq!(super::local_path("Z:\\home\\a\\osu!"), Some("/home/a/osu!".into()));
            assert_eq!(super::local_path("/home/a/osu!"), Some("/home/a/osu!".into()));
        }
        assert_eq!(super::local_path(""), None);
    }

    #[cfg(unix)]
    #[test]
    fn wine_drives() {
        let root = std::env::temp_dir().join(format!("hashi-wine-{}", std::process::id()));
        let game = root.join("Games").join("osu!");
        std::fs::create_dir_all(game.join("Songs")).unwrap();
        let prefix = root.join("prefix");
        std::fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        std::os::unix::fs::symlink(&game, prefix.join("dosdevices").join("d:")).unwrap();
        let prefixes = [root.join("missing"), prefix.clone()];
        let d = prefix.join("dosdevices").join("d:");
        assert_eq!(super::wine_path("D:\\", &prefixes), Some(d.join("")));
        assert_eq!(super::wine_path("d:\\Songs", &prefixes), Some(d.join("Songs")));
        assert_eq!(super::wine_path("D:\\nope", &prefixes), None);
        assert_eq!(super::wine_path("C:\\osu!", &prefixes), None);
        assert_eq!(super::wine_path("Z:\\home\\a", &prefixes), Some("/home/a".into()));
        assert_eq!(super::wine_path("\\\\server\\x", &prefixes), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn songs_relative_to_the_game() {
        let game = std::path::Path::new("/games/osu!");
        assert_eq!(super::songs_path(game, "Songs"), Some(game.join("Songs")));
        assert_eq!(super::songs_path(game, "Maps\\Mania"), Some(game.join("Maps/Mania")));
        assert_eq!(super::songs_path(game, "/srv/songs"), Some("/srv/songs".into()));
        assert_eq!(super::songs_path(game, ""), None);
    }

    #[test]
    fn quoting() {
        assert_eq!(super::shell_quote("/home/a/.local/bin/tosu"), "/home/a/.local/bin/tosu");
        assert_eq!(super::shell_quote("/home/a/My Apps/tosu"), "'/home/a/My Apps/tosu'");
        assert_eq!(super::shell_quote("/x/it's/$tosu"), r"'/x/it'\''s/$tosu'");
    }

    fn frame(client: &str, state: &str, live: f64) -> Value {
        json!({
            "client": client,
            "state": { "name": state },
            "profile": { "name": "someone", "userStatus": { "name": "online" } },
            "play": { "playerName": "someone", "mode": { "number": 3 }, "mods": { "number": 0, "name": "" }, "failed": false },
            "beatmap": {
                "checksum": "0123456789abcdef0123456789abcdef",
                "mode": { "number": 3 },
                "time": { "live": live, "lastObject": 90_000.0 },
            },
        })
    }

    fn captured(frames: &[Value]) -> usize {
        let (b, dir) = crate::auth::tests::test_bridge("http://localhost:3000");
        for f in frames {
            super::on_tosu_message(&b, f.clone());
        }
        let n = b.st().queue.items.len();
        std::fs::remove_dir_all(dir).ok();
        n
    }

    #[tokio::test]
    async fn room_plays_are_captured_when_finished() {
        let lazer = |state, live| frame("lazer", state, live);
        assert_eq!(captured(&[lazer("lobby", 0.0), lazer("play", 0.0), lazer("play", 30_000.0), lazer("play", 91_000.0), lazer("lobby", 91_500.0)]), 1);
        let stable = |state, live| frame("stable", state, live);
        assert_eq!(captured(&[stable("matchSetup", 0.0), stable("play", 91_000.0), stable("rankingVs", 91_500.0)]), 1);
    }

    // tosu-torii says play on torii's results screen, with the song back at the preview after it
    #[tokio::test]
    async fn torii_plays_finished_into_results_read_as_play() {
        let root = std::env::temp_dir().join(format!("hashi-torii-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("files")).unwrap();
        std::fs::write(root.join("torii.ini"), b"").unwrap();
        let songs = root.join("files").to_string_lossy().into_owned();
        let torii = |state, live| {
            let mut f = frame("lazer", state, live);
            f["folders"] = json!({ "songs": songs });
            f
        };
        assert_eq!(captured(&[torii("selectPlay", 0.0), torii("play", 0.0), torii("play", 30_000.0), torii("play", 91_000.0), torii("play", 12_000.0), torii("selectPlay", 12_100.0)]), 1);
        // play again from those results never leaves play
        assert_eq!(
            captured(&[torii("selectPlay", 0.0), torii("play", 0.0), torii("play", 91_000.0), torii("play", 0.0), torii("play", 91_000.0), torii("play", 12_000.0), torii("selectPlay", 12_100.0)]),
            2
        );
        // or the second attempt is quit partway
        assert_eq!(captured(&[torii("selectPlay", 0.0), torii("play", 0.0), torii("play", 91_000.0), torii("play", 0.0), torii("play", 30_000.0), torii("selectPlay", 30_100.0)]), 1);
        // song select's preview starts partway in, it looping past the last note isn't another play
        let mut frames = vec![torii("selectPlay", 0.0), torii("play", 0.0), torii("play", 91_000.0), torii("play", 12_000.0)];
        frames.extend([torii("play", 91_000.0), torii("play", 12_000.0), torii("play", 91_000.0), torii("play", 12_000.0)]);
        assert_eq!(captured(&frames), 1);
        // and a real attempt still counts after it
        frames.extend([torii("play", 0.0), torii("play", 91_000.0), torii("play", 12_000.0)]);
        assert_eq!(captured(&frames), 2);
        std::fs::remove_dir_all(&root).unwrap();
    }

    // lazer's own replay seeked back after its end isn't a new play
    #[tokio::test]
    async fn lazer_plays_are_not_kept_past_their_end() {
        let root = std::env::temp_dir().join(format!("hashi-lazer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("files")).unwrap();
        let songs = root.join("files").to_string_lossy().into_owned();
        let lazer = |state, live| {
            let mut f = frame("lazer", state, live);
            f["folders"] = json!({ "songs": songs });
            f
        };
        assert_eq!(captured(&[lazer("selectPlay", 0.0), lazer("play", 91_000.0), lazer("play", 12_000.0), lazer("selectPlay", 12_100.0)]), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    // a lazer play counted twice, NM then NC, the second time with no replay left for it
    #[tokio::test]
    async fn lazer_results_are_one_play() {
        let lazer = |state, live| frame("lazer", state, live);
        let play = [lazer("selectPlay", 0.0), lazer("play", 0.0), lazer("play", 91_000.0), lazer("resultScreen", 91_000.0)];
        assert_eq!(captured(&[&play[..], &[lazer("selectPlay", 0.0)]].concat()), 1);
        // play again for a moment, still at the end
        let mut frames = play.to_vec();
        frames.extend([lazer("play", 91_000.0), lazer("resultScreen", 91_000.0), lazer("selectPlay", 0.0)]);
        assert_eq!(captured(&frames), 1);
        // the replay from the results, left a few seconds in
        let mut frames = play.to_vec();
        frames.extend([lazer("play", 0.0), lazer("play", 4_000.0), lazer("resultScreen", 4_000.0), lazer("selectPlay", 0.0)]);
        assert_eq!(captured(&frames), 1);
        // a retry played through is another
        let mut frames = play.to_vec();
        frames.extend([lazer("play", 0.0), lazer("play", 91_000.0), lazer("resultScreen", 91_000.0), lazer("selectPlay", 0.0)]);
        assert_eq!(captured(&frames), 2);
    }

    #[tokio::test]
    async fn room_plays_left_partway_are_not() {
        let lazer = |state, live| frame("lazer", state, live);
        assert_eq!(captured(&[lazer("lobby", 0.0), lazer("play", 30_000.0), lazer("lobby", 30_100.0)]), 0);
        let stable = |state, live| frame("stable", state, live);
        assert_eq!(captured(&[stable("matchSetup", 0.0), stable("play", 30_000.0), stable("matchSetup", 30_100.0)]), 0);
    }
}
