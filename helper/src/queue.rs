use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};

use crate::auth::{self, api, Fail, Req, Resp};
use crate::types::{Item, ItemError};
use crate::{log, now_ms, replays, Bridge};

const TERMINAL_STATES: [&str; 4] = ["accepted", "rejected", "expired", "deleted"];

#[derive(Default)]
pub(crate) struct PumpState {
    // 429
    paused_until: i64,
    pumping: bool,
    // woken while pumping, look once more before stopping
    again: bool,
}

fn title(item: &Item) -> String {
    crate::types::text(&item.play.title)
}

pub(crate) fn fail(b: &Arc<Bridge>, id: &str, phase: &str, code: &str, message: &str) {
    if let Some(item) = b.st().queue.get_mut(id) {
        item.phase = phase.into();
        item.error = ItemError::new(code, message);
        log!("{} [{}]: {phase} ({code}) {message}", title(item), crate::types::text(&item.play.version));
    }
    b.save_queue_later();
    b.broadcast();
}

// before it's completed on the site a play can still be taken back
const UNSENT: [&str; 5] = ["waiting_replay", "send", "held", "failed", "unbound"];

pub(crate) fn remove(b: &Arc<Bridge>, id: &str) -> bool {
    {
        let mut s = b.st();
        let before = s.queue.items.len();
        s.queue.items.retain(|i| !(i.id == id && UNSENT.contains(&i.phase.as_str())));
        if s.queue.items.len() == before {
            return false;
        }
    }
    log!("removed a play that wasn't sent");
    b.save_queue_later();
    b.broadcast();
    true
}

pub(crate) fn hold(b: &Arc<Bridge>, id: &str, on: bool) -> bool {
    {
        let mut s = b.st();
        let Some(item) = s.queue.get_mut(id).filter(|i| !i.practice && ["waiting_replay", "send"].contains(&i.phase.as_str())) else { return false };
        item.on_hold = on;
        log!("{}: {}", title(item), if on { "on hold" } else { "sending after all" });
    }
    b.save_queue_later();
    b.broadcast();
    if !on {
        wake_queue(b, 0);
    }
    true
}

fn retry_delay(attempts: u32) -> i64 {
    const STEPS: [i64; 6] = [5_000, 15_000, 60_000, 5 * 60_000, 15 * 60_000, 30 * 60_000];
    STEPS[(attempts as usize).min(STEPS.len() - 1)]
}

fn retry_later(item: &mut Item, message: &str, delay_ms: Option<i64>) {
    item.attempts += 1;
    let wait = delay_ms.unwrap_or_else(|| retry_delay(item.attempts - 1));
    item.next_attempt_at = now_ms() + wait;
    item.error = ItemError::new("retrying", message);
    log!("{}: retry in {}s ({message})", title(item), (wait as f64 / 1000.0).round());
}

pub(crate) fn wake_queue(b: &Arc<Bridge>, delay_ms: i64) {
    let gen = b.wake_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let b2 = b.clone();
    b.spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms.max(0) as u64)).await;
        if b2.wake_gen.load(Ordering::SeqCst) == gen {
            pump(&b2).await;
        }
    });
}

fn binding_matches(s: &crate::State, item: &Item) -> bool {
    match (&s.config.connection, &item.binding) {
        (Some(c), Some(bd)) => {
            bd.issuer == c.issuer && bd.environment == c.environment && bd.installation_id == c.installation_id && bd.user_id == c.user_id
        }
        _ => false,
    }
}

fn same_account(s: &crate::State, item: &Item) -> bool {
    match (&s.config.connection, &item.binding) {
        (Some(c), Some(bd)) => !bd.user_id.is_null() && bd.issuer == c.issuer && bd.environment == c.environment && bd.user_id == c.user_id,
        _ => false,
    }
}

fn next_due(b: &Arc<Bridge>) -> Option<Item> {
    let mut s = b.st();
    if s.connection.status != "connected" {
        return None;
    }
    let now = now_ms();
    if now < s.pump.paused_until {
        wake_queue(b, (s.pump.paused_until - now).min(60_000));
        return None;
    }
    let mut changed = false;
    let mut active: Vec<usize> = vec![];
    let sending = !s.sends_no_plays();
    for idx in 0..s.queue.items.len() {
        let i = &s.queue.items[idx];
        if !["send", "poll", "held"].contains(&i.phase.as_str()) || ((i.on_hold || !sending) && i.phase == "send") {
            continue;
        }
        let mut matches = binding_matches(&s, &s.queue.items[idx]);
        if !matches && same_account(&s, &s.queue.items[idx]) {
            let c = s.config.connection.as_ref().expect("same_account has a connection");
            let installation_id = c.installation_id.clone();
            let i = &mut s.queue.items[idx];
            if let Some(bd) = i.binding.as_mut() {
                bd.installation_id = installation_id;
            }
            log!("{}: from an earlier connection to the same account, going on under this one", title(i));
            matches = true;
            changed = true;
        }
        let i = &mut s.queue.items[idx];
        let account_changed = i.error.as_ref().is_some_and(|e| e.code == "account_changed");
        if !matches {
            if !account_changed {
                i.held_from = Some(i.phase.clone());
                i.phase = "held".into();
                i.error = ItemError::new("account_changed", "Played while connected to another account. It wasn't sent.");
                changed = true;
            }
            continue;
        }
        if account_changed {
            i.phase = i.held_from.take().unwrap_or("send".into());
            i.error = None;
            i.next_attempt_at = 0;
            changed = true;
        }
        active.push(idx);
    }
    if changed {
        b.save_queue_later();
        b.broadcast();
    }
    let due = active.iter().filter(|&&i| s.queue.items[i].next_attempt_at <= now).min_by_key(|&&i| s.queue.items[i].next_attempt_at);
    match due {
        Some(&i) => Some(s.queue.items[i].clone()),
        None => {
            if let Some(next) = active.iter().map(|&i| s.queue.items[i].next_attempt_at).min() {
                // the 60s tick covers the rest
                wake_queue(b, (next - now).clamp(250, 60_000));
            }
            None
        }
    }
}

pub(crate) async fn pump(b: &Arc<Bridge>) {
    {
        let mut s = b.st();
        if s.pump.pumping {
            s.pump.again = true;
            return;
        }
        s.pump.pumping = true;
        s.pump.again = false;
    }
    loop {
        pump_due(b).await;
        let mut s = b.st();
        if !std::mem::take(&mut s.pump.again) {
            s.pump.pumping = false;
            return;
        }
    }
}

async fn pump_due(b: &Arc<Bridge>) {
    while let Some(mut item) = next_due(b) {
        let result = step(b, &mut item).await;
        let problem = match result {
            Ok(()) => None,
            Err(Fail::Conn(status, message)) => Some((status, message)),
            Err(Fail::Other(message)) => {
                retry_later(&mut item, &message, None);
                None
            }
        };
        {
            let mut s = b.st();
            if item.phase == "done" {
                s.boards.forget(&item.play.beatmap_md5);
            }
            if let Some(slot) = s.queue.get_mut(&item.id) {
                // held while it was being sent: that holds until it's completed
                item.on_hold = slot.on_hold && item.phase == "send";
                *slot = item;
            }
        }
        if let Some((status, message)) = problem {
            auth::set_connection(b, status, Some(message));
            break;
        }
        b.save_queue().await;
        b.broadcast();
    }
}

fn state_of(receipt: &Value) -> &str {
    receipt.get("state").and_then(Value::as_str).unwrap_or("")
}

fn new_poll(item: &mut Item) {
    item.phase = "poll".into();
    item.poll_delay = Some(2000);
    item.next_attempt_at = now_ms() + 1000;
}

// reserving again is idempotent, so always start from the current receipt
async fn step(b: &Arc<Bridge>, item: &mut Item) -> Result<(), Fail> {
    if item.phase == "held" {
        item.phase = "send".into();
    }
    if item.phase == "poll" {
        return poll(b, item).await;
    }

    let Some(mut receipt) = reserve(b, item).await? else { return Ok(()) };
    for _ in 0..3 {
        if TERMINAL_STATES.contains(&state_of(&receipt)) {
            settle(item, receipt);
            return Ok(());
        }
        if state_of(&receipt) != "awaiting_assets" {
            new_poll(item);
            return Ok(());
        }
        // each answer is the latest receipt, another play can have sent the chart by now
        for asset in ["replay", "beatmap"] {
            if needs(&receipt, asset) {
                let Some(next) = upload(b, item, asset, &receipt).await? else { return Ok(()) };
                receipt = next;
            }
        }
        // removed, held or sending turned off while the files went up: completing is what makes it count
        if !still_wanted(b, &item.id) {
            return Ok(());
        }
        let id = item.submission_id.clone().unwrap_or_default();
        let res = api(b, &format!("/submissions/{id}/complete"), Req::method(reqwest::Method::POST)).await?;
        let body = res.json();
        if res.status == 202 || res.ok() {
            log!("{}: completed, {}", title(item), state_of(&body));
            item.receipt = Some(body);
            new_poll(item);
            item.error = None;
            item.attempts = 0;
            return Ok(());
        }
        let code = body.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
        if res.status == 409 && ["assets_missing", "wrong_state"].contains(&code) {
            // server lost a file, re-read and resend
            let Some(next) = read_receipt(b, item).await? else { return Ok(()) };
            receipt = next;
            continue;
        }
        handle_error(b, item, &res, &body, "complete")?;
        return Ok(());
    }
    retry_later(item, "The server kept asking for the files again.", None);
    Ok(())
}

fn still_wanted(b: &Arc<Bridge>, id: &str) -> bool {
    let s = b.st();
    !s.sends_no_plays() && s.queue.items.iter().any(|i| i.id == id && !i.on_hold)
}

fn manifest(item: &Item) -> Value {
    let replay = item.replay.as_ref();
    let chart = item.chart.as_ref();
    let mut m = json!({
        "protocol_version": 1,
        "client_version": auth::client_version(),
        "game_client": item.play.client.as_deref().unwrap_or("stable"),
        "capture_kind": "automatic_completed_play",
        "replay": { "sha256": replay.map(|r| r.sha256.clone()), "byte_length": replay.map(|r| r.byte_length) },
        "chart": { "md5": chart.and_then(|c| c.md5.clone()), "sha256": chart.map(|c| c.sha256.clone()), "byte_length": chart.map(|c| c.byte_length) },
    });
    if let Some(at) = replay.and_then(|r| r.played_at.clone()) {
        m["observed"] = json!({ "finished_at": at });
    }
    m
}

async fn reserve(b: &Arc<Bridge>, item: &mut Item) -> Result<Option<Value>, Fail> {
    let mut req = Req::json(reqwest::Method::POST, &manifest(item));
    req.headers.push(("idempotency-key", item.id.clone()));
    let res = api(b, "/submissions", req).await?;
    let body = res.json();
    if !res.ok() {
        return handle_error(b, item, &res, &body, "reserve");
    }
    let id = body.get("submission_id").and_then(Value::as_str).map(String::from);
    if item.submission_id.is_none() {
        log!(
            "{}: reserved {} (needs replay {}, chart {})",
            title(item),
            id.as_deref().unwrap_or("?"),
            body.get("needs_replay").unwrap_or(&Value::Null),
            body.get("needs_beatmap").unwrap_or(&Value::Null)
        );
    }
    item.submission_id = id;
    if item.chart_needed.is_none() {
        item.chart_needed = Some(body.get("needs_beatmap") == Some(&Value::Bool(true)));
    }
    item.receipt = Some(body.clone());
    Ok(Some(body))
}

async fn read_receipt(b: &Arc<Bridge>, item: &mut Item) -> Result<Option<Value>, Fail> {
    let id = item.submission_id.clone().unwrap_or_default();
    let res = api(b, &format!("/submissions/{id}"), Req::get()).await?;
    let body = res.json();
    if !res.ok() {
        return handle_error(b, item, &res, &body, "receipt");
    }
    item.receipt = Some(body.clone());
    Ok(Some(body))
}

// only paths the server gave us, kept under submissions/ on whichever host serves the api
fn upload_path(api_prefix: &str, p: &Value) -> Result<String, Fail> {
    let refuse = || Fail::Other(format!("Refusing upload path {}", crate::types::text(p)));
    let rest = p.as_str().and_then(|p| p.strip_prefix(api_prefix)).filter(|r| r.starts_with("/submissions/")).ok_or_else(refuse)?;
    let plain = rest.bytes().all(|c| c.is_ascii_alphanumeric() || b"/_-.".contains(&c));
    if !plain || rest.split('/').skip(1).any(|seg| seg.is_empty() || seg == "." || seg == "..") {
        return Err(refuse());
    }
    Ok(rest.to_string())
}

fn needs(receipt: &Value, asset: &str) -> bool {
    crate::types::truthy(receipt.get(format!("needs_{asset}").as_str()).unwrap_or(&Value::Null))
}

const STALL_SECS: u64 = if cfg!(test) { 1 } else { 15 };
const PART_BYTES: usize = 12 * 1024;
const PART_TRIES: u32 = 4;

fn put_req(body: Bytes, timeout: Duration) -> Req {
    Req {
        method: reqwest::Method::PUT,
        headers: vec![("content-type", "application/octet-stream".into())],
        body: Some(body),
        timeout,
    }
}

fn uploaded(b: &Arc<Bridge>, item: &mut Item, asset: &str, res: &Resp, body: Value) -> Result<Option<Value>, Fail> {
    if !res.ok() {
        return handle_error(b, item, res, &body, "upload");
    }
    // a part for a file the server no longer needs isn't kept
    if asset == "beatmap" && body.pointer("/upload/stored") != Some(&Value::Bool(false)) {
        item.chart_sent = true;
    }
    item.receipt = Some(body.clone());
    Ok(Some(body))
}

async fn upload(b: &Arc<Bridge>, item: &mut Item, asset: &str, receipt: &Value) -> Result<Option<Value>, Fail> {
    let blob = if asset == "replay" { item.replay.clone() } else { item.chart.clone() };
    let blob = blob.ok_or_else(|| Fail::Other("Nothing stored to upload.".into()))?;
    let bytes = Bytes::from(replays::read_blob(b, &blob).await.map_err(Fail::Other)?);
    let caps = auth::caps(b).await?;
    let path = receipt.get(format!("{asset}_upload_path").as_str()).unwrap_or(&Value::Null);
    let url = upload_path(caps.get("api_prefix").and_then(Value::as_str).unwrap_or(""), path)?;
    let part = caps.pointer("/chunked_upload/max_part_bytes").and_then(Value::as_u64).map(|m| (m as usize).min(PART_BYTES)).filter(|&m| m > 0);
    let Some(part) = part else {
        // server cuts uploads at 4 min
        let res = api(b, &url, put_req(bytes, Duration::from_secs(5 * 60))).await?;
        let body = res.json();
        return uploaded(b, item, asset, &res, body);
    };
    if !b.st().stalled {
        // 15s plus 32 KB/s, so a big chart on a slow line isn't taken for a stall
        let timeout = Duration::from_secs(STALL_SECS) + Duration::from_millis(bytes.len() as u64 / 32);
        match api(b, &url, put_req(bytes.clone(), timeout)).await {
            Err(Fail::Other(m)) if m == auth::TIMED_OUT => {
                log!("{}: the {asset} upload stalled, sending files in parts and every request on a new connection from now on", title(item));
                b.st().stalled = true;
                // another play can have sent it while this one hung
                let Some(now) = read_receipt(b, item).await? else { return Ok(None) };
                if !needs(&now, asset) {
                    return Ok(Some(now));
                }
            }
            res => {
                let res = res?;
                let body = res.json();
                return uploaded(b, item, asset, &res, body);
            }
        }
    }
    upload_parts(b, item, asset, &url, &bytes, part).await
}

// each part on a new connection, see State::stalled
async fn upload_parts(b: &Arc<Bridge>, item: &mut Item, asset: &str, url: &str, bytes: &Bytes, part: usize) -> Result<Option<Value>, Fail> {
    let mut offset = 0;
    let mut restarted = false;
    loop {
        let end = (offset + part).min(bytes.len());
        let res = send_part(b, &format!("{url}/parts/{offset}"), bytes.slice(offset..end)).await?;
        let body = res.json();
        // the server drops the parts it had
        if !res.ok() && !restarted && body.pointer("/error/code").and_then(Value::as_str) == Some("digest_mismatch") {
            restarted = true;
            offset = 0;
            continue;
        }
        if !res.ok() || body.pointer("/upload/complete") == Some(&Value::Bool(true)) {
            return uploaded(b, item, asset, &res, body);
        }
        if !needs(&body, asset) {
            item.receipt = Some(body.clone());
            return Ok(Some(body));
        }
        if end == bytes.len() {
            return Err(Fail::Other(format!("The server didn't get all of the {asset}.")));
        }
        offset = end;
    }
}

async fn send_part(b: &Arc<Bridge>, path: &str, body: Bytes) -> Result<Resp, Fail> {
    let mut tries = 1;
    loop {
        match api(b, path, put_req(body.clone(), Duration::from_secs(STALL_SECS))).await {
            Err(Fail::Other(m)) if tries < PART_TRIES => {
                log!("upload part {path}: {m}, sending it again");
                tries += 1;
            }
            res => return res,
        }
    }
}

async fn poll(b: &Arc<Bridge>, item: &mut Item) -> Result<(), Fail> {
    let Some(receipt) = read_receipt(b, item).await? else { return Ok(()) };
    let state = state_of(&receipt).to_string();
    if TERMINAL_STATES.contains(&state.as_str()) {
        settle(item, receipt);
        return Ok(());
    }
    item.error = None;
    if state == "awaiting_assets" {
        item.phase = "send".into();
        item.next_attempt_at = now_ms();
        return Ok(());
    }
    // deferred retries are 1-16 min apart
    let cap = if state == "deferred" { 60_000 } else { 30_000 };
    let delay = ((item.poll_delay.unwrap_or(2000) as f64 * 1.6).round() as i64).min(cap);
    item.poll_delay = Some(delay);
    item.next_attempt_at = now_ms() + delay;
    Ok(())
}

fn settle(item: &mut Item, receipt: Value) {
    let error = receipt.get("error").filter(|e| !e.is_null());
    item.error = error.map(|e| ItemError {
        code: e.get("code").and_then(Value::as_str).unwrap_or("").into(),
        message: e.get("message").and_then(Value::as_str).map(String::from),
    });
    let code = error.and_then(|e| e.get("code")).and_then(Value::as_str).map(|c| format!(" ({c})")).unwrap_or_default();
    log!("{}: {}{code}", title(item), state_of(&receipt));
    item.receipt = Some(receipt);
    item.phase = "done".into();
    item.next_attempt_at = 0;
}

fn handle_error(b: &Arc<Bridge>, item: &mut Item, res: &Resp, body: &Value, stage: &str) -> Result<Option<Value>, Fail> {
    let code = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .map(String::from)
        .unwrap_or_else(|| format!("http_{}", res.status));
    let message = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| body.get("error_description").and_then(Value::as_str))
        .map(String::from)
        .unwrap_or_else(|| format!("{stage} answered {}", res.status));
    let retry_after = res.header("retry-after").and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0);
    let retry_after_ms = retry_after.map(|s| (s.min(86_400.0) * 1000.0) as i64);

    if res.status == 401 {
        // token_expired already got its one refresh in api()
        if code == "token_expired" {
            b.st().access_token = None;
            retry_later(item, "The access token expired again right after a refresh.", None);
            return Ok(None);
        }
        if code == "installation_revoked" {
            return Err(Fail::Conn("revoked", "This connection was removed on Mania Tracker. Connect again.".into()));
        }
        if ["token_revoked", "invalid_token", "wrong_environment"].contains(&code.as_str()) {
            return Err(Fail::Conn("revoked", auth::GONE.into()));
        }
        // key_mismatch, invalid_dpop_proof, htu_mismatch, ...
        return Err(Fail::Conn("error", format!("The server refused this client's proof ({code}). Uploads are stopped.")));
    }
    if res.status == 403 && code == "account_not_allowed" {
        return Err(Fail::Conn("not_in_beta", message));
    }
    // reserving makes the submission, a 404 there is the site having trouble
    if res.status == 404 && stage != "reserve" {
        item.phase = "done".into();
        item.error = ItemError::new("gone", "The server no longer has this submission.");
        return Ok(None);
    }
    if res.status == 429 {
        let wait = retry_after_ms.unwrap_or(60_000);
        b.st().pump.paused_until = now_ms() + wait;
        retry_later(item, &message, Some(wait));
        return Ok(None);
    }
    if res.status == 409 && code == "chart_environment_conflict" {
        item.phase = "held".into();
        item.error = ItemError::new(&code, message);
        item.next_attempt_at = now_ms() + 30 * 60_000;
        return Ok(None);
    }
    if res.status == 504 && stage == "upload" {
        retry_later(item, &message, Some(10 * 60_000));
        return Ok(None);
    }
    if body.pointer("/error/retryable") == Some(&Value::Bool(true)) || res.status >= 500 || body.get("error").and_then(Value::as_str) == Some("server_error") {
        retry_later(item, &message, retry_after_ms);
        return Ok(None);
    }
    // digest_mismatch, not_mania, payload_too_large etc, permanent
    item.phase = "failed".into();
    let shown = if code == "unsupported_game_client" { "Mania Tracker doesn't take lazer plays from the app yet.".to_string() } else { message.clone() };
    item.error = ItemError::new(&code, shown);
    log!("{}: refused at {stage} ({} {code}) {message}", title(item), res.status);
    Ok(None)
}

fn play_url(r: &Value, origin: &str, username: Option<&str>) -> Option<String> {
    let state = r.get("state").and_then(Value::as_str)?;
    let duplicate = r.pointer("/error/code").and_then(Value::as_str) == Some("duplicate_play");
    if state != "accepted" && !(state == "rejected" && duplicate) {
        return None;
    }
    let encode = |s| percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC);
    let list = format!("{origin}/player/{}/recent", encode(username?));
    match r.get("local_score_id").and_then(Value::as_str).filter(|id| state == "accepted" && !id.is_empty()) {
        Some(id) => Some(format!("{list}?import={}", encode(id))),
        None => Some(list),
    }
}

fn rating(r: &Value) -> Value {
    let Some(x) = r.get("rating").filter(|_| state_of(r) == "accepted") else { return Value::Null };
    let msd = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|m| m.is_finite() && (0.0..=100.0).contains(m));
    let dan = x.get("dan").and_then(|d| {
        let label = d.get("label").and_then(Value::as_str).map(str::trim).filter(|l| !l.is_empty() && l.chars().count() <= 32)?;
        let ln = d.get("family").and_then(Value::as_str) == Some("ln");
        Some(json!({ "label": label, "ln": ln, "eligible": d.get("eligible").and_then(Value::as_bool).unwrap_or(true) }))
    });
    let msd = msd(x.get("msd"));
    if msd.is_none() && dan.is_none() {
        return Value::Null;
    }
    json!({ "msd": msd, "dan": dan })
}

pub(crate) fn public_item(item: &Item, origin: &str, username: Option<&str>) -> Value {
    let r = item.receipt.as_ref();
    json!({
        "id": item.id,
        "capturedAt": item.captured_at,
        "play": {
            "client": item.play.client.as_deref().unwrap_or("stable"),
            "artist": item.play.artist,
            "title": item.play.title,
            "version": item.play.version,
            "keys": item.play.keys,
            "mods": item.play.mods,
            "score": item.play.score,
            "accuracy": item.play.accuracy,
        },
        "phase": item.phase,
        "onHold": item.on_hold,
        "practice": item.practice,
        "retryAt": if item.error.as_ref().is_some_and(|e| e.code == "retrying") { Some(item.next_attempt_at) } else { None },
        "receipt": r.map(|r| json!({
            "state": r.get("state"),
            "submissionId": r.get("submission_id"),
            "error": r.get("error").cloned().unwrap_or(Value::Null),
        })),
        "cover": item.play.cover.is_some() || (item.chart.as_ref().is_some_and(|c| c.source.is_some()) && !item.play.is_lazer()),
        "chartNeeded": item.chart_needed,
        "chartSent": item.chart_sent,
        "error": item.error,
        "url": r.and_then(|r| play_url(r, origin, username)),
        "rating": r.map(rating).unwrap_or(Value::Null),
        "md5": item.play.beatmap_md5,
        "analysis": item.replay.is_some() && item.chart.is_some(),
        "rank": r.filter(|r| state_of(r) == "accepted").map(crate::leaderboard::rank).unwrap_or(Value::Null),
        // the site needed the map and put the play on no board: it has none for this map
        "noBoard": item.chart_needed == Some(true) && r.is_some_and(|r| state_of(r) == "accepted" && r.get("leaderboard").is_some_and(Value::is_null)),
    })
}

pub(crate) const KEEP_DAYS: i64 = 30;

// keep finished plays KEEP_DAYS. true if any went
pub(crate) async fn prune_queue(b: &Arc<Bridge>) -> bool {
    let cutoff = crate::iso_from_ms(now_ms() - KEEP_DAYS * 24 * 3600_000);
    let (keep, pruned): (std::collections::HashSet<String>, bool) = {
        let mut s = b.st();
        let before = s.queue.items.len();
        s.queue.items.retain(|i| !(["done", "failed", "unbound", "practice"].contains(&i.phase.as_str()) && i.captured_at < cutoff));
        let pruned = s.queue.items.len() != before;
        let mut keep: std::collections::HashSet<String> =
            s.queue.items.iter().flat_map(|i| [i.replay.as_ref(), i.chart.as_ref()]).flatten().map(|x| x.sha256.clone()).collect();
        for raw in &s.queue.unparsed {
            for k in ["/replay/sha256", "/chart/sha256"] {
                if let Some(h) = raw.pointer(k).and_then(Value::as_str) {
                    keep.insert(h.into());
                }
            }
        }
        (keep, pruned)
    };
    let Ok(mut dir) = tokio::fs::read_dir(&b.env.blob_dir).await else { return pruned };
    while let Ok(Some(e)) = dir.next_entry().await {
        if keep.contains(e.file_name().to_string_lossy().as_ref()) {
            continue;
        }
        // a replay being matched right now is stored before its play points at it
        let old = e.metadata().await.and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|d| d > Duration::from_secs(3600)));
        if old {
            let _ = tokio::fs::remove_file(e.path()).await;
        }
    }
    pruned
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn upload_paths() {
        let p = "/api/integrations/companella/v1";
        let path = |v: &str| super::upload_path(p, &json!(v)).ok();
        assert_eq!(path("/api/integrations/companella/v1/submissions/kf9PXHG_HhFN3Hvw-2lAQ/replay").as_deref(), Some("/submissions/kf9PXHG_HhFN3Hvw-2lAQ/replay"));
        for bad in [
            "/api/integrations/companella/v1/submissions/../../oauth/token",
            "/api/integrations/companella/v1/submissions/%2e%2e/x",
            "/api/integrations/companella/v1/submissions//x",
            "/api/integrations/companella/v1/submissions/a?b=c",
            "/api/integrations/companella/v1/me",
            "https://evil.example/api/integrations/companella/v1/submissions/a/replay",
            "/api/other/submissions/a/replay",
        ] {
            assert_eq!(path(bad), None, "{bad}");
        }
        assert!(super::upload_path(p, &json!(null)).is_err());
    }

    #[tokio::test]
    async fn stalled_upload_goes_in_parts_on_new_connections() {
        use axum::extract::{ConnectInfo, Path, State};
        use axum::routing::{get, put};
        use std::net::SocketAddr;
        use std::sync::{Arc, Mutex};

        #[derive(Default)]
        struct Seen {
            whole: u32,
            parts: Vec<(usize, u16)>,
            got: Vec<u8>,
            stalled_once: bool,
            held_already: bool,
            gets: Vec<u16>,
        }
        type S = State<Arc<Mutex<Seen>>>;
        let reply = |v: serde_json::Value| axum::response::Response::builder().header("content-type", "application/json").body(axum::body::Body::from(v.to_string())).unwrap();
        let app = axum::Router::new()
            .route("/api/submissions/s1", get(move |State(st): S, ConnectInfo(peer): ConnectInfo<SocketAddr>| async move {
                let mut s = st.lock().unwrap();
                s.gets.push(peer.port());
                reply(json!({ "state": "awaiting_assets", "needs_beatmap": !s.held_already }))
            }))
            .route("/api/submissions/s1/beatmap", put(move |State(st): S| async move {
                st.lock().unwrap().whole += 1;
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }))
            .route("/api/submissions/s1/beatmap/parts/{offset}", put(move |State(st): S, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(offset): Path<usize>, body: bytes::Bytes| async move {
                let stall = {
                    let mut s = st.lock().unwrap();
                    let first = offset > 0 && !s.stalled_once;
                    s.stalled_once |= first;
                    first
                };
                if stall {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
                let mut s = st.lock().unwrap();
                s.parts.push((offset, peer.port()));
                if s.held_already {
                    return reply(json!({ "state": "awaiting_assets", "needs_beatmap": false, "upload": { "asset": "beatmap", "complete": true, "stored": false } }));
                }
                s.got.truncate(offset);
                s.got.extend_from_slice(&body);
                let complete = s.got.len() == 30_000;
                reply(json!({ "state": "awaiting_assets", "needs_beatmap": !complete, "upload": { "asset": "beatmap", "received_bytes": s.got.len(), "byte_length": 30_000, "complete": complete, "stored": true } }))
            }));
        let seen = Arc::new(Mutex::new(Seen::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = app.with_state(seen.clone()).into_make_service_with_connect_info::<SocketAddr>();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (b, dir) = crate::auth::tests::test_bridge(&origin);
        let chart: Vec<u8> = (0..30_000u32).map(|i| (i % 251) as u8).collect();
        let sha = crate::replays::sha256_hex(&chart);
        std::fs::create_dir_all(&b.env.blob_dir).unwrap();
        std::fs::write(b.env.blob_dir.join(&sha), &chart).unwrap();
        {
            let mut s = b.st();
            s.config.key = Some(crate::auth::tests::new_key());
            s.access_token = Some("test-access".into());
            s.access_expires_at = crate::now_ms() + 3_600_000;
            s.caps = Some(json!({ "issuer": origin, "api_prefix": "/api", "chunked_upload": { "max_part_bytes": 16384 } }));
        }
        let mut item: crate::types::Item = serde_json::from_value(json!({
            "id": "i1", "capturedAt": "", "phase": "send", "submissionId": "s1", "play": { "beatmapMd5": "m" },
            "chart": { "sha256": sha, "byteLength": 30_000 },
        }))
        .unwrap();
        let receipt = json!({ "state": "awaiting_assets", "needs_beatmap": true, "beatmap_upload_path": "/api/submissions/s1/beatmap" });
        let gets = |b: &Arc<crate::Bridge>, mut item: crate::types::Item| {
            let (b, seen) = (b.clone(), seen.clone());
            async move {
                seen.lock().unwrap().gets.clear();
                for _ in 0..3 {
                    super::read_receipt(&b, &mut item).await.ok().unwrap();
                }
                seen.lock().unwrap().gets.iter().copied().collect::<std::collections::HashSet<u16>>().len()
            }
        };
        assert_eq!(gets(&b, item.clone()).await, 1, "kept alive while nothing stalled");

        let done = super::upload(&b, &mut item, "beatmap", &receipt).await.ok().unwrap();
        let done = done.unwrap_or_else(|| panic!("{}", serde_json::to_string(&item.error).unwrap()));
        assert_eq!(done["needs_beatmap"], json!(false));
        assert!(item.chart_sent);
        {
            let s = seen.lock().unwrap();
            assert_eq!(s.whole, 1);
            assert_eq!(s.got, chart);
            assert_eq!(s.parts.iter().map(|p| p.0).collect::<Vec<_>>(), [0, 12288, 24576]);
            let ports: std::collections::HashSet<u16> = s.parts.iter().map(|p| p.1).collect();
            assert_eq!(ports.len(), 3, "every part on its own connection");
        }
        assert_eq!(gets(&b, item.clone()).await, 3, "after a stall every request dials again");

        // the rest of the session goes straight to parts
        seen.lock().unwrap().parts.clear();
        super::upload(&b, &mut item, "beatmap", &receipt).await.ok().unwrap().unwrap();
        assert_eq!(seen.lock().unwrap().whole, 1);
        assert_eq!(seen.lock().unwrap().parts.len(), 3);

        // another play sent the chart meanwhile: the part isn't kept, so it wasn't this client's
        item.chart_sent = false;
        seen.lock().unwrap().held_already = true;
        let r = super::upload(&b, &mut item, "beatmap", &receipt).await.ok().unwrap().unwrap();
        assert_eq!(r["upload"]["stored"], json!(false));
        assert!(!item.chart_sent);

        // or while the whole upload hung, then no parts go out at all
        b.st().stalled = false;
        seen.lock().unwrap().parts.clear();
        let r = super::upload(&b, &mut item, "beatmap", &receipt).await.ok().unwrap().unwrap();
        assert_eq!(r["needs_beatmap"], json!(false));
        assert!(!item.chart_sent);
        assert!(seen.lock().unwrap().parts.is_empty());

        server.abort();
        std::fs::remove_dir_all(dir).ok();
    }

    // first `budget` bytes of each connection go through, then it hangs like the RU throttle
    async fn throttle_proxy(budget: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut c, _) = l.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    let mut head = Vec::new();
                    while !head.windows(2).any(|w| w == b"\r\n") {
                        let n = c.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        head.extend_from_slice(&buf[..n]);
                    }
                    let line = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_string();
                    println!("throttle: new connection, {}", line.split(' ').take(2).collect::<Vec<_>>().join(" "));
                    let Some(authority) = line.split(' ').nth(1).and_then(|u| u.strip_prefix("http://")).and_then(|r| r.split('/').next()).map(String::from) else { return };
                    let Ok(up) = tokio::net::TcpStream::connect(&authority).await else { return };
                    let (mut cr, mut cw) = c.into_split();
                    let (mut ur, mut uw) = up.into_split();
                    let up_task = async move {
                        let mut sent = 0;
                        let mut chunk = head;
                        loop {
                            let take = chunk.len().min(budget - sent);
                            if uw.write_all(&chunk[..take]).await.is_err() {
                                return;
                            }
                            sent += take;
                            if sent == budget {
                                println!("throttle: {authority} connection stalled after {budget} bytes");
                                while cr.read(&mut buf).await.unwrap_or(0) > 0 {}
                                return;
                            }
                            let n = cr.read(&mut buf).await.unwrap_or(0);
                            if n == 0 {
                                return;
                            }
                            chunk = buf[..n].to_vec();
                        }
                    };
                    tokio::select! {
                        _ = up_task => {}
                        _ = tokio::io::copy(&mut ur, &mut cw) => {}
                    }
                });
            }
        });
        format!("http://{addr}")
    }

    // a real play from a copy of a dev data dir connected to localhost:3000, through the throttle:
    // HASHI_REAL_DIR=/tmp/hashi-real [HASHI_REAL_PICK=title] [THROTTLE_BYTES=6000] [PART_BYTES=4096] cargo test real_upload -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn real_upload_through_a_throttle() {
        let Some(dir) = std::env::var_os("HASHI_REAL_DIR").map(std::path::PathBuf::from) else { return };
        let num = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let proxy = throttle_proxy(num("THROTTLE_BYTES", 6000)).await;
        let mut env = crate::Env::from_env();
        env.origin = "http://localhost:3000".into();
        env.data_dir = dir.clone();
        env.config_file = dir.join("config.json");
        env.queue_file = dir.join("queue.json");
        env.blob_dir = dir.join("blobs");
        let client = |fresh: bool| {
            let c = reqwest::Client::builder().proxy(reqwest::Proxy::http(&proxy).unwrap());
            if fresh { c.http1_only().pool_max_idle_per_host(0) } else { c }.build().unwrap()
        };
        let b = crate::auth::tests::bridge_with(env, client(false), client(true));
        b.st().config = serde_json::from_value(crate::read_json_file(&b.env.config_file).await.unwrap()).unwrap();
        let pick = std::env::var("HASHI_REAL_PICK").unwrap_or("666".into());
        let queue = crate::read_json_file(&b.env.queue_file).await.unwrap();
        let raw = queue["items"].as_array().unwrap().iter().rev().find(|i| i["play"]["title"].as_str().is_some_and(|t| t.contains(&pick)) && i["chart"].is_object()).unwrap();
        let mut item: crate::types::Item = serde_json::from_value(raw.clone()).unwrap();
        item.id = uuid::Uuid::new_v4().to_string();
        (item.submission_id, item.receipt, item.error, item.chart_sent, item.attempts) = (None, None, None, false, 0);
        item.phase = "send".into();
        println!("play: {} replay {} B, chart {} B", super::title(&item), item.replay.as_ref().unwrap().byte_length, item.chart.as_ref().unwrap().byte_length);
        b.st().queue.items.push(item.clone());
        let mut caps = crate::auth::load_capabilities(&b).await.ok().unwrap();
        caps["chunked_upload"]["max_part_bytes"] = json!(num("PART_BYTES", 4096));
        b.st().caps = Some(caps);

        let started = std::time::Instant::now();
        for _ in 0..20 {
            let r = if item.phase == "poll" {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                super::poll(&b, &mut item).await
            } else {
                super::step(&b, &mut item).await
            };
            if let Err(e) = r {
                println!("  error {e}, going on");
                continue;
            }
            let receipt = item.receipt.clone().unwrap_or_default();
            println!(
                "{:>5.1}s phase {} state {} needs replay {} chart {} chart_sent {} stalled {} error {}",
                started.elapsed().as_secs_f64(),
                item.phase,
                receipt["state"],
                receipt["needs_replay"],
                receipt["needs_beatmap"],
                item.chart_sent,
                b.st().stalled,
                serde_json::to_string(&item.error).unwrap()
            );
            if item.phase == "done" || item.phase == "failed" {
                break;
            }
        }
        println!("receipt: {}", item.receipt.unwrap_or_default());
    }

    #[test]
    fn play_links() {
        let o = "https://mania-tracker.com";
        let url = |r, name| super::play_url(&r, o, name);
        assert_eq!(url(json!({ "state": "accepted" }), Some("Aleju03")).as_deref(), Some("https://mania-tracker.com/player/Aleju03/recent"));
        assert_eq!(url(json!({ "state": "accepted" }), Some("a b")).as_deref(), Some("https://mania-tracker.com/player/a%20b/recent"));
        assert_eq!(
            url(json!({ "state": "accepted", "local_score_id": "kf9PXHG_HhFN3HvwyX2lAQ" }), Some("Aleju03")).as_deref(),
            Some("https://mania-tracker.com/player/Aleju03/recent?import=kf9PXHG%5FHhFN3HvwyX2lAQ")
        );
        assert_eq!(url(json!({ "state": "rejected", "local_score_id": "x1", "error": { "code": "duplicate_play" } }), Some("x")).as_deref(), Some("https://mania-tracker.com/player/x/recent"));
        assert_eq!(url(json!({ "state": "rejected", "error": { "code": "duplicate_play" } }), Some("x")).as_deref(), Some("https://mania-tracker.com/player/x/recent"));
        assert_eq!(url(json!({ "state": "rejected", "error": { "code": "bad_replay" } }), Some("x")), None);
        assert_eq!(url(json!({ "state": "queued" }), Some("x")), None);
        assert_eq!(url(json!({ "state": "accepted" }), None), None);
    }

    #[test]
    fn ratings() {
        let rating = |r| super::rating(&r);
        let dan = json!({ "label": "8+", "family": "dan", "raw": 8.21, "eligible": false });
        assert_eq!(
            rating(json!({ "state": "accepted", "rating": { "msd": 24.31, "dan": dan } })),
            json!({ "msd": 24.31, "dan": { "label": "8+", "ln": false, "eligible": false } })
        );
        assert_eq!(rating(json!({ "state": "accepted", "rating": { "msd": 24.31, "dan": null } })), json!({ "msd": 24.31, "dan": null }));
        // unrated: the chart's msd alone isn't the play's
        assert_eq!(rating(json!({ "state": "accepted", "rating": { "analysis_state": "unsupported", "msd": null, "chart_msd": 25.02, "dan": null } })), json!(null));
        assert_eq!(
            rating(json!({ "state": "accepted", "rating": { "msd": 30.5, "dan": { "label": "12", "family": "ln", "eligible": true } } })),
            json!({ "msd": 30.5, "dan": { "label": "12", "ln": true, "eligible": true } })
        );
        assert_eq!(rating(json!({ "state": "accepted", "rating": { "msd": null, "dan": null } })), json!(null));
        assert_eq!(rating(json!({ "state": "accepted", "rating": { "msd": 1e9, "dan": { "label": " " } } })), json!(null));
        assert_eq!(rating(json!({ "state": "accepted" })), json!(null));
        assert_eq!(rating(json!({ "state": "queued", "rating": { "msd": 24.31 } })), json!(null));
    }
}
