use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::auth::{api, Req};
use crate::{now_ms, Bridge};

// the site caches its boards for a minute too
const FRESH_MS: i64 = 60_000;
const KEEP: usize = 24;

#[derive(Default)]
pub(crate) struct Boards {
    fetched: HashMap<String, (i64, Value)>,
}

impl Boards {
    // a new play on the map is on its boards as soon as it's accepted
    pub fn forget(&mut self, md5: &str) {
        self.fetched.retain(|k, _| !k.starts_with(md5));
    }
}

pub(crate) fn valid_md5(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}

// mods with a board of their own. the rest are ignored like mirror, or keep a play off every board
const OWN_BOARD: [&str; 3] = ["HO", "IN", "NR"];

// which board some mods pick: speed (none is the usual board, nomod and DT together) and the note mods
pub(crate) fn variant(mods: &[String]) -> (Option<&'static str>, Vec<&'static str>) {
    let has = |l: &[&str]| mods.iter().any(|m| l.contains(&m.as_str()));
    let speed = if has(&["HT", "DC"]) {
        Some("ht")
    } else if has(&["DT", "NC"]) {
        Some("dt")
    } else {
        None
    };
    (speed, OWN_BOARD.into_iter().filter(|m| has(&[m])).collect())
}

// from the page's query, None if it names a board that can't exist
pub(crate) fn parse_variant(query: &str) -> Option<(Option<String>, Vec<String>)> {
    let mut speed = None;
    let mut mods = vec![];
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match &*k {
            "speed" if v == "ht" || v == "dt" => speed = Some(v.into_owned()),
            "speed" if v.is_empty() => {}
            "mods" => {
                for m in v.split(',').filter(|m| !m.is_empty()) {
                    let m = m.to_uppercase();
                    if !OWN_BOARD.contains(&m.as_str()) {
                        return None;
                    }
                    mods.push(m);
                }
            }
            // the filters, read by parse_filters
            "offset" | "country" | "q" | "friends" => {}
            _ => return None,
        }
    }
    mods.sort();
    mods.dedup();
    Some((speed, mods))
}

pub(crate) const PAGE: u32 = 50;
const MAX_OFFSET: u32 = 5000;

// which players, and which page of them: a country's or the friends loaded on the site re-ranked
// among themselves, or a name search that keeps everyone's place
#[derive(Default, Debug, PartialEq)]
pub(crate) struct Filters {
    pub offset: u32,
    pub country: Option<String>,
    pub q: Option<String>,
    pub friends: bool,
}

pub(crate) fn parse_filters(query: &str) -> Option<Filters> {
    let mut f = Filters::default();
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match &*k {
            "offset" => f.offset = v.parse().ok().filter(|o| o % PAGE == 0 && *o <= MAX_OFFSET)?,
            "country" if v.is_empty() => {}
            "country" => f.country = Some(v.to_uppercase()).filter(|c| c.len() == 2 && c.chars().all(|c| c.is_ascii_alphabetic())),
            "q" => f.q = Some(v.trim().to_string()).filter(|q| !q.is_empty() && q.chars().count() <= 32),
            "friends" => f.friends = v == "1",
            _ => {}
        }
        if k == "country" && !v.is_empty() && f.country.is_none() {
            return None;
        }
    }
    Some(f)
}

// (status, body) for the page
pub(crate) async fn board(b: &Arc<Bridge>, md5: &str, speed: Option<String>, mods: Vec<String>, f: Filters) -> (u16, Value) {
    let md5 = md5.to_lowercase();
    if b.st().connection.status != "connected" {
        return (409, json!({ "error": "Connect to Mania Tracker to see leaderboards." }));
    }
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let query = [
        speed.map(|sp| format!("speed={sp}")),
        (!mods.is_empty()).then(|| format!("mods={}", mods.join(","))),
        (f.offset > 0).then(|| format!("offset={}", f.offset)),
        f.country.map(|c| format!("country={c}")),
        f.q.map(|q| format!("q={}", enc(&q))),
        f.friends.then(|| "friends=1".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("&");
    let key = format!("{md5}?{query}");
    if let Some((at, v)) = b.st().boards.fetched.get(&key) {
        if now_ms() - at < FRESH_MS {
            return (200, v.clone());
        }
    }
    let path = if query.is_empty() { format!("/charts/{md5}/leaderboard") } else { format!("/charts/{md5}/leaderboard?{query}") };
    let res = match api(b, &path, Req::get()).await {
        Ok(res) => res,
        Err(e) => return (502, json!({ "error": e.message() })),
    };
    let body = res.json();
    let code = body.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
    if res.status == 404 && code == "unknown_chart" {
        return (200, json!({ "unknown": true }));
    }
    if res.status == 404 {
        return (200, json!({ "unsupported": true }));
    }
    if res.status == 409 && code == "friends_not_loaded" {
        return (200, json!({ "friendsNotLoaded": true }));
    }
    if res.status == 429 {
        return (429, json!({ "error": "Opened too many leaderboards in a minute. Try again in a moment." }));
    }
    if !res.ok() {
        let message = body.pointer("/error/message").and_then(Value::as_str).map(String::from);
        return (502, json!({ "error": message.unwrap_or_else(|| format!("Mania Tracker answered {}", res.status)) }));
    }
    let page = public_board(&body, &b.env.origin);
    let mut s = b.st();
    let fetched = &mut s.boards.fetched;
    if fetched.len() >= KEEP {
        let oldest = fetched.iter().min_by_key(|(_, (at, _))| *at).map(|(k, _)| k.clone());
        if let Some(k) = oldest {
            fetched.remove(&k);
        }
    }
    fetched.insert(key, (now_ms(), page.clone()));
    (200, page)
}

fn str_field(v: &Value, k: &str) -> Value {
    v.get(k).and_then(Value::as_str).map(Value::from).unwrap_or(Value::Null)
}

fn num_field(v: &Value, k: &str) -> Value {
    v.get(k).and_then(Value::as_f64).filter(|n| n.is_finite()).map(Value::from).unwrap_or(Value::Null)
}

fn https(v: &Value) -> Value {
    v.as_str().filter(|u| u.starts_with("https://")).map(Value::from).unwrap_or(Value::Null)
}

// only links into the site Hashi is connected to
fn on_site(v: &Value, origin: &str) -> Value {
    v.as_str().filter(|u| u.strip_prefix(origin).is_some_and(|rest| rest.starts_with('/'))).map(Value::from).unwrap_or(Value::Null)
}

fn entry(e: &Value) -> Option<Value> {
    let user = e.get("user")?;
    let username = user.get("username").and_then(Value::as_str)?;
    let mods: Vec<&str> = e.get("mods").and_then(Value::as_array).map(|m| m.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    Some(json!({
        "position": e.get("position").and_then(Value::as_u64),
        "userId": user.get("id"),
        "username": username,
        "avatarUrl": https(user.get("avatar_url").unwrap_or(&Value::Null)),
        "country": str_field(user, "country_code"),
        // a fraction on the wire
        "accuracy": e.get("accuracy").and_then(Value::as_f64).filter(|a| a.is_finite()).map(|a| a * 100.0),
        "grade": str_field(e, "grade"),
        "mods": mods,
        "maxCombo": num_field(e, "max_combo"),
        "score": num_field(e, "total_score"),
        "pp": num_field(e, "pp"),
        "lazer": e.get("is_lazer").and_then(Value::as_bool).unwrap_or(false),
        "playedAt": str_field(e, "played_at"),
        "app": str_field(e, "app"),
        "rate": num_field(e, "rate"),
    }))
}

fn public_board(body: &Value, origin: &str) -> Value {
    let entries: Vec<Value> = body.get("entries").and_then(Value::as_array).map(|l| l.iter().filter_map(entry).collect()).unwrap_or_default();
    json!({
        "beatmapId": body.get("beatmap_id"),
        "artist": str_field(body, "artist"),
        "title": str_field(body, "title"),
        "version": str_field(body, "version"),
        "status": str_field(body, "status"),
        "url": on_site(body.get("url").unwrap_or(&Value::Null), origin),
        "total": body.get("total").and_then(Value::as_u64).unwrap_or(entries.len() as u64),
        "entries": entries,
        "self": body.get("self").and_then(entry),
        "speed": board_speed(body),
        "mods": board_mods(body),
        "offset": body.get("offset").and_then(Value::as_u64).unwrap_or(0),
        "country": country(body.get("country")),
        "q": str_field(body, "q"),
        "friends": body.get("friends").and_then(Value::as_bool).unwrap_or(false),
        // only on the first page of the whole board
        "countries": body.get("countries").and_then(Value::as_array).map(|l| {
            l.iter()
                .filter_map(|c| Some(json!({ "code": country(c.get("code")).as_str()?, "count": c.get("count").and_then(Value::as_u64)? })))
                .collect::<Vec<_>>()
        }),
        "selfCountry": country(body.get("self_country")),
    })
}

fn country(v: Option<&Value>) -> Value {
    v.and_then(Value::as_str).filter(|c| c.len() == 2 && c.chars().all(|c| c.is_ascii_alphabetic())).map(|c| Value::from(c.to_uppercase())).unwrap_or(Value::Null)
}

fn board_speed(v: &Value) -> Value {
    v.get("speed").and_then(Value::as_str).filter(|s| ["ht", "dt"].contains(s)).map(Value::from).unwrap_or(Value::Null)
}

fn board_mods(v: &Value) -> Value {
    let mods: Vec<&str> = v.get("mods").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).filter(|m| OWN_BOARD.contains(m)).collect()).unwrap_or_default();
    json!(mods)
}

// the play's spot on its map's board, from the receipt
pub(crate) fn rank(receipt: &Value) -> Value {
    let Some(l) = receipt.get("leaderboard").filter(|l| l.is_object()) else { return Value::Null };
    let Some(position) = l.get("position").and_then(Value::as_u64).filter(|p| *p > 0) else { return Value::Null };
    json!({
        "position": position,
        "total": l.get("total").and_then(Value::as_u64),
        "best": l.get("personal_best").and_then(Value::as_bool).unwrap_or(true),
        "speed": board_speed(l),
        "mods": board_mods(l),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn ranks() {
        assert_eq!(
            super::rank(&json!({ "leaderboard": { "position": 7, "total": 41, "personal_best": true } })),
            json!({ "position": 7, "total": 41, "best": true, "speed": null, "mods": [] })
        );
        let r = super::rank(&json!({ "leaderboard": { "position": 2, "total": 5, "personal_best": true, "speed": "dt", "mods": ["IN", "XX"] } }));
        assert_eq!((&r["speed"], &r["mods"]), (&json!("dt"), &json!(["IN"])));
        assert_eq!(super::rank(&json!({ "leaderboard": { "position": 3, "total": 9, "personal_best": false } }))["best"], json!(false));
        assert_eq!(super::rank(&json!({ "leaderboard": null })), json!(null));
        assert_eq!(super::rank(&json!({ "leaderboard": { "position": 0 } })), json!(null));
        assert_eq!(super::rank(&json!({})), json!(null));
    }

    #[test]
    fn boards() {
        let body = json!({
            "beatmap_id": 5, "title": "T", "url": "javascript:alert(1)", "total": 2,
            "entries": [
                { "position": 1, "user": { "id": 1, "username": "a", "avatar_url": "https://a.ppy.sh/1" }, "accuracy": 0.991, "mods": ["DT", 3], "app": "mania-bridge" },
                { "position": 2, "user": { "id": 2 } },
            ],
            "self": null,
        });
        let page = super::public_board(&body, "https://mania-tracker.com");
        assert_eq!(page["url"], json!(null));
        assert_eq!(super::public_board(&json!({ "url": "https://mania-tracker.com/maps?map=5" }), "https://mania-tracker.com")["url"], json!("https://mania-tracker.com/maps?map=5"));
        assert_eq!(super::public_board(&json!({ "url": "https://mania-tracker.com.evil/maps" }), "https://mania-tracker.com")["url"], json!(null));
        assert!((page["entries"][0]["accuracy"].as_f64().unwrap() - 99.1).abs() < 1e-9);
        assert_eq!(page["entries"][0]["app"], json!("mania-bridge"));
        assert_eq!(page["entries"].as_array().unwrap().len(), 1);
        assert_eq!(page["entries"][0]["mods"], json!(["DT"]));
        assert_eq!(page["self"], json!(null));
    }

    #[test]
    fn variants() {
        let v = |l: &[&str]| super::variant(&l.iter().map(|m| m.to_string()).collect::<Vec<_>>());
        assert_eq!(v(&[]), (None, vec![]));
        assert_eq!(v(&["MR", "HD"]), (None, vec![]));
        assert_eq!(v(&["DC"]), (Some("ht"), vec![]));
        assert_eq!(v(&["NC", "NR", "IN"]), (Some("dt"), vec!["IN", "NR"]));
        let p = super::parse_variant;
        assert_eq!(p(""), Some((None, vec![])));
        assert_eq!(p("speed=ht&mods=nr,IN,IN"), Some((Some("ht".into()), vec!["IN".into(), "NR".into()])));
        assert_eq!(p("speed=x"), None);
        assert_eq!(p("mods=EZ"), None);
        assert_eq!(p("mods=IN&offset=50&country=cr&q=a%20b&friends=1"), Some((None, vec!["IN".into()])));
    }

    #[test]
    fn filters() {
        let f = super::parse_filters;
        assert_eq!(f(""), Some(super::Filters::default()));
        assert_eq!(
            f("offset=100&country=cr&q=%20pl0x%20&friends=1"),
            Some(super::Filters { offset: 100, country: Some("CR".into()), q: Some("pl0x".into()), friends: true })
        );
        assert_eq!(f("offset=25"), None);
        assert_eq!(f("offset=5050"), None);
        assert_eq!(f("country=CRI"), None);
        assert_eq!(f("country="), Some(super::Filters::default()));
        assert_eq!(f("q=%20%20").map(|x| x.q), Some(None));
    }
}
