use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::auth::{self, Req};
use crate::{log, Bridge, VERSION};

const PATH: &str = "/bug-reports";
const BODY_MIN: usize = 10;
const BODY_MAX: usize = 4000;
// when the site doesn't say. the end of the log is what matters, so that's what's kept
const LOG_MAX: usize = 1 << 20;

fn log_file(b: &Bridge) -> Option<std::path::PathBuf> {
    b.opts.log_file.then(|| b.env.data_dir.join("bridge.log"))
}

fn read_log(b: &Bridge, max: usize) -> Option<String> {
    let mut f = std::fs::File::open(log_file(b)?).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(max as u64);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::with_capacity((len - from) as usize);
    f.read_to_end(&mut bytes).ok()?;
    Some(tail(&bytes, max, from > 0))
}

// lossy can grow it past max, so it's cut again, then at a line so the first one is whole
fn tail(bytes: &[u8], max: usize, cut: bool) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let text = &text[start..];
    if cut || start > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest).to_string()
    } else {
        text.to_string()
    }
}

pub(crate) fn log_preview(b: &Bridge) -> Value {
    match read_log(b, LOG_MAX) {
        Some(text) => {
            let lines: Vec<&str> = text.lines().collect();
            json!({ "bytes": text.len(), "tail": lines[lines.len().saturating_sub(200)..].join("\n") })
        }
        None => Value::Null,
    }
}

fn context(b: &Bridge) -> Value {
    let s = b.st();
    let count = |phase: &str| s.queue.items.iter().filter(|i| i.phase == phase).count();
    json!({
        "hashi": VERSION,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "osuClient": s.tosu.client.as_deref().unwrap_or("none"),
        "tosu": s.tosu.connected,
        "connection": s.connection.status,
        "sendPlays": !s.sends_no_plays(),
        "analysis": s.analyzes(),
        "waitingReplay": count("waiting_replay"),
        "failed": count("failed"),
    })
}

// Err is what to tell the player
pub(crate) async fn send(b: &Arc<Bridge>, text: &str, with_log: bool) -> Result<Value, String> {
    let text = text.trim();
    if text.chars().count() < BODY_MIN {
        return Err("Write a little more about what happened.".into());
    }
    let caps = auth::caps(b).await.map_err(|_| "Mania Tracker is not reachable. Try again in a bit.".to_string())?;
    let Some(spec) = caps.get("bug_reports").filter(|v| v.is_object()) else {
        return Err("Mania Tracker doesn't take reports right now.".into());
    };
    let max_log = spec.get("max_log_bytes").and_then(Value::as_u64).map_or(LOG_MAX, |n| n as usize);
    let mut body = json!({
        "body": text.chars().take(BODY_MAX).collect::<String>(),
        "kind": "bug",
        "context": context(b),
    });
    if with_log {
        if let Some(log) = tokio::task::spawn_blocking({
            let b = b.clone();
            move || read_log(&b, max_log)
        })
        .await
        .ok()
        .flatten()
        {
            body["log"] = log.into();
        }
    }
    let req = Req::json(reqwest::Method::POST, &body);
    let signed_in = b.st().config.connection.as_ref().is_some_and(|c| c.refresh_token.is_some());
    let res = if signed_in { auth::api(b, PATH, req).await } else { auth::api_open(b, PATH, req).await };
    let res = res.map_err(|e| {
        log!("report: {e}");
        "Couldn't reach Mania Tracker. Try again in a bit.".to_string()
    })?;
    let answer = res.json();
    if res.ok() {
        let id = answer.get("id").and_then(Value::as_str).unwrap_or_default();
        let log_lost = body.get("log").is_some() && answer.get("log_attached").and_then(Value::as_bool) != Some(true);
        log!("report sent ({id}{})", if log_lost { ", without the log" } else { "" });
        return Ok(json!({
            "id": id,
            "repliesUrl": answer.get("replies_url").and_then(Value::as_str).filter(|u| u.starts_with("https://") || u.starts_with("http://")),
            "logLost": log_lost,
        }));
    }
    let code = answer.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
    log!("report refused ({} {code})", res.status);
    Err(match (res.status, code) {
        (_, "too_many_reports") => "That's a lot of reports for one day. Try again tomorrow.",
        (429, _) => "That's a lot of reports in a row. Try again in an hour.",
        (413, _) => "The log is too big to send. Untick it and send the report without it.",
        (401 | 403, _) => "Mania Tracker didn't accept the connection. Connect again in the settings.",
        (_, "invalid_report") => "Mania Tracker didn't take that report. Write a little more about what happened.",
        _ => "Mania Tracker didn't take the report. Try again in a bit.",
    }
    .into())
}

#[cfg(test)]
mod tests {
    use axum::routing::{get, post};
    use axum::Router;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    #[test]
    fn log_tail_is_whole_lines_under_the_cap() {
        assert_eq!(super::tail(b"a\nb\n", 100, false), "a\nb\n");
        assert_eq!(super::tail(b"half line\nwhole\n", 100, true), "whole\n");
        // invalid bytes turn into 3 byte replacements, it still fits
        let mut bytes = b"x\n".to_vec();
        bytes.extend([0xff; 40]);
        bytes.extend(b"\nlast\n");
        let t = super::tail(&bytes, 48, false);
        assert!(t.len() <= 48);
        assert_eq!(t, "last\n");
    }

    #[tokio::test]
    async fn report_goes_to_the_site() {
        let got = Arc::new(Mutex::new(Value::Null));
        let seen = got.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let caps = json!({ "issuer": origin, "api_prefix": "/v1", "bug_reports": { "max_log_bytes": 1048576 } });
        let reply = |v: Value| axum::response::Response::builder().header("content-type", "application/json").body(axum::body::Body::from(v.to_string())).unwrap();
        let app = Router::new()
            .route("/api/integrations/companella/v1/capabilities", get(move || async move { reply(caps) }))
            .route("/v1/bug-reports", post(move |h: axum::http::HeaderMap, body: bytes::Bytes| async move {
                let body: Value = serde_json::from_slice(&body).unwrap();
                *seen.lock().unwrap() = json!({ "body": body, "auth": h.contains_key("authorization") });
                reply(json!({ "ok": true, "id": "r1", "duplicate": false, "log_attached": false, "replies_url": null }))
            }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (b, dir) = crate::auth::tests::test_bridge(&origin);

        assert!(super::send(&b, "short", true).await.is_err());
        let sent = super::send(&b, "  it got stuck on waiting for replay  ", true).await.unwrap();
        assert_eq!(sent, json!({ "id": "r1", "repliesUrl": null, "logLost": false }));
        let got = got.lock().unwrap().clone();
        assert_eq!(got["auth"], false);
        assert_eq!(got["body"]["body"], "it got stuck on waiting for replay");
        assert_eq!(got["body"]["kind"], "bug");
        assert_eq!(got["body"]["context"]["hashi"], crate::VERSION);
        let _ = std::fs::remove_dir_all(dir);
    }
}
