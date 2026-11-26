use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::{log, now_ms, Bridge};

// supporters' avatar frames and name styles, from Mania Tracker's public lists
const NAMES_FRESH_MS: i64 = 60_000;
const FRAMES_FRESH_MS: i64 = 10 * 60_000;
// coming back to the window asks again, at most this often
const MINE_AGAIN_MS: i64 = 20_000;
const NAMES_PATH: &str = "/api/supporters/name-styles";
// these two from the site itself, where the frames' geometry lives
const FRAMES_PATH: &str = "/supporter-frames.json";
pub(crate) const SHEET_PATH: &str = "/supporter-names.css";

#[derive(Default)]
pub(crate) struct Looks {
    names: Option<(i64, Value)>,
    frames: Option<(i64, Value)>,
    // the connected player's (their id, then whether they're a supporter and their look)
    pub mine: Option<(String, Value)>,
    mine_asked: i64,
}

async fn get(b: &Arc<Bridge>, url: &str) -> Option<Value> {
    match b.client().get(url).timeout(Duration::from_secs(10)).send().await {
        Ok(res) if res.status().is_success() => res.json().await.ok(),
        Ok(res) => {
            log!("looks: {url} answered {}", res.status());
            None
        }
        Err(e) => {
            log!("looks: {}", crate::auth::net_message(&e));
            None
        }
    }
}

// a failed fetch keeps the last good copy, and isn't tried again until it would have gone stale
async fn cached(b: &Arc<Bridge>, url: Option<String>, fresh_ms: i64, slot: fn(&mut Looks) -> &mut Option<(i64, Value)>) -> Value {
    if let Some((at, v)) = slot(&mut b.st().looks) {
        if now_ms() - *at < fresh_ms {
            return v.clone();
        }
    }
    let got = match url {
        Some(url) => get(b, &url).await,
        None => None,
    };
    let mut s = b.st();
    let slot = slot(&mut s.looks);
    let v = got.or_else(|| slot.as_ref().map(|(_, v)| v.clone())).unwrap_or(Value::Null);
    *slot = Some((now_ms(), v.clone()));
    v
}

async fn names(b: &Arc<Bridge>) -> Value {
    let url = crate::auth::backend(b).await.map(|base| format!("{base}{NAMES_PATH}"));
    cached(b, url, NAMES_FRESH_MS, |l| &mut l.names).await
}

async fn frames(b: &Arc<Bridge>) -> Value {
    cached(b, Some(format!("{}{FRAMES_PATH}", b.env.origin)), FRAMES_FRESH_MS, |l| &mut l.frames).await
}

// it ends up in a class name
fn is_id(s: &str) -> bool {
    (1..=48).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

// six hex digits, then 1 to 3 for a thinner line
fn is_outline(s: &str) -> bool {
    matches!(s.len(), 6 | 7) && s[..6].bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) && (s.len() == 6 || (b'1'..=b'3').contains(&s.as_bytes()[6]))
}

fn name_look(style: Option<&Value>, font: Option<&Value>, outline: Option<&Value>) -> Option<Value> {
    let pick = |v: Option<&Value>, ok: fn(&str) -> bool| v.and_then(Value::as_str).filter(|s| ok(s)).map(String::from);
    let (style, font, outline) = (pick(style, is_id), pick(font, is_id), pick(outline, is_outline));
    (style.is_some() || font.is_some() || outline.is_some()).then(|| json!({ "style": style, "font": font, "outline": outline }))
}

// only art from the site Hashi is connected to, or from its own subdomains
fn art_url(v: Option<&Value>, site_host: &str) -> Option<String> {
    let raw = v?.as_str()?;
    let u = url::Url::parse(raw).ok()?;
    let host = u.host_str()?;
    let ok = match u.scheme() {
        "https" => host == site_host || host.ends_with(&format!(".{site_host}")),
        "http" => host == "localhost" || host == "127.0.0.1",
        _ => false,
    };
    ok.then(|| raw.to_string())
}

fn numbers(v: Option<&Value>, lo: f64, hi: f64) -> Option<[f64; 4]> {
    let l = v?.as_array()?;
    if l.len() != 4 {
        return None;
    }
    let mut out = [0.0; 4];
    for (o, x) in out.iter_mut().zip(l) {
        *o = x.as_f64().filter(|n| (lo..=hi).contains(n))?;
    }
    Some(out)
}

// it goes into a style attribute
fn is_filter(s: &str) -> bool {
    s.len() <= 120 && !s.contains("url") && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"().,%#- ".contains(&c))
}

fn frame_look(manifest: &Value, id: &str, site_host: &str) -> Option<Value> {
    let f = manifest.get("frames")?.get(id)?;
    let art = |k: &str| art_url(f.get(k), site_host);
    Some(json!({
        "id": id,
        "box": numbers(f.get("box"), -5.0, 5.0)?,
        "overhang": numbers(f.get("overhang"), 0.0, 5.0)?,
        "still": art("still")?,
        "round": art("round"),
        "video": art("video"),
        "roundVideo": art("roundVideo"),
        "square": f.get("square").and_then(Value::as_bool).unwrap_or(false),
        "avatarFilter": f.get("avatarFilter").and_then(Value::as_str).filter(|s| is_filter(s)),
    }))
}

fn site_host(b: &Bridge) -> String {
    url::Url::parse(&b.env.origin).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default()
}

fn look(name: Option<Value>, frame: Option<Value>) -> Option<Value> {
    (name.is_some() || frame.is_some()).then(|| json!({ "name": name, "frame": frame }))
}

// each row's look, from the map the site's own lists use
pub(crate) async fn dress(b: &Arc<Bridge>, page: &mut Value) {
    let map = names(b).await;
    let of = |k: &str, id: &str| map.get(k).and_then(|m| m.get(id)).cloned();
    let id_of = |e: &Value| e.get("userId").map(crate::types::text).filter(|id| !id.is_empty());
    let rows: Vec<&Value> = page.get("entries").and_then(Value::as_array).into_iter().flatten().chain(page.get("self")).collect();
    let framed = rows.iter().filter_map(|e| id_of(e)).any(|id| of("frames", &id).is_some());
    let manifest = if framed { frames(b).await } else { Value::Null };
    let host = site_host(b);
    let dress_one = |e: &mut Value| {
        let Some(id) = id_of(e) else { return };
        let name = name_look(of("styles", &id).as_ref(), of("fonts", &id).as_ref(), of("outlines", &id).as_ref());
        let frame = of("frames", &id).and_then(|f| frame_look(&manifest, f.as_str()?, &host));
        if let Some(l) = look(name, frame) {
            e["look"] = l;
        }
    };
    if let Some(list) = page.get_mut("entries").and_then(Value::as_array_mut) {
        list.iter_mut().for_each(dress_one);
    }
    if let Some(me) = page.get_mut("self").filter(|v| v.is_object()) {
        dress_one(me);
    }
}

async fn load_mine(b: &Arc<Bridge>) {
    let Some(id) = b.st().config.connection.as_ref().map(|c| crate::types::text(&c.user_id)).filter(|id| !id.is_empty()) else {
        return;
    };
    let Some(base) = crate::auth::backend(b).await else { return };
    let Some(body) = get(b, &format!("{base}/api/supporters/look/{id}")).await else { return };
    let frame = match body.get("frame").and_then(Value::as_str) {
        Some(f) => frame_look(&frames(b).await, f, &site_host(b)),
        None => None,
    };
    let name = name_look(body.get("nameStyle"), body.get("nameFont"), body.get("nameOutline"));
    let mine = (id, json!({ "supporter": body.get("supporter") == Some(&Value::Bool(true)), "look": look(name, frame) }));
    let changed = {
        let mut s = b.st();
        let changed = s.looks.mine.as_ref() != Some(&mine);
        s.looks.mine = Some(mine);
        changed
    };
    if changed {
        b.broadcast();
    }
}

// on connecting, and when the window is back in front: a look just picked on the site, or supporter
// just bought through the boat, shows then
pub(crate) fn refresh_mine(b: &Arc<Bridge>, force: bool) {
    {
        let mut s = b.st();
        if !force && now_ms() - s.looks.mine_asked < MINE_AGAIN_MS {
            return;
        }
        s.looks.mine_asked = now_ms();
    }
    let b2 = b.clone();
    b.spawn(async move { load_mine(&b2).await });
}

// theirs only while it's the account that's connected
pub(crate) fn mine_for<'a>(s: &'a crate::State, user_id: &str) -> Option<&'a Value> {
    s.looks.mine.as_ref().filter(|(id, _)| id == user_id).map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines_and_ids_are_checked_before_they_reach_a_class_or_style() {
        assert!(is_outline("ff0000") && is_outline("ff00002"));
        assert!(!is_outline("ff00004") && !is_outline("FF0000") && !is_outline("ff00"));
        assert!(is_id("solid-ff66aa") && is_id("terminal-bare"));
        assert!(!is_id("a b") && !is_id("x\"") && !is_id(""));
        assert!(is_filter("saturate(1.2) brightness(0.9)") && !is_filter("url(#x)") && !is_filter("a;b"));
    }

    #[test]
    fn frame_art_must_come_from_the_site() {
        let host = "mania-tracker.com";
        let ok = |u: &str| art_url(Some(&json!(u)), host).is_some();
        assert!(ok("https://mania-tracker.com/images/supporter-frames/crown.png"));
        assert!(ok("https://cdn.mania-tracker.com/frame.webm"));
        assert!(!ok("https://evil.com/x.png") && !ok("https://mania-tracker.com.evil.com/x.png") && !ok("http://mania-tracker.com/x.png"));
        assert!(!ok("javascript:alert(1)"));
    }

    #[test]
    fn a_frame_needs_its_geometry_and_still() {
        let manifest = json!({ "frames": {
            "crown": { "box": [-0.37, -0.64, 1.66, 1.05], "overhang": [0.3, 0.5, 0, 0], "still": "https://mania-tracker.com/c.png" },
            "broken": { "box": [0, 0, 1], "overhang": [0, 0, 0, 0], "still": "https://mania-tracker.com/b.png" },
        }});
        let f = frame_look(&manifest, "crown", "mania-tracker.com").unwrap();
        assert_eq!(f["still"], "https://mania-tracker.com/c.png");
        assert!(f["video"].is_null());
        assert!(frame_look(&manifest, "broken", "mania-tracker.com").is_none());
        assert!(frame_look(&manifest, "missing", "mania-tracker.com").is_none());
    }
}
