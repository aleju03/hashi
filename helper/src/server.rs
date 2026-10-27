use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, Response, StatusCode};
use axum::Router;
use bytes::Bytes;
use tokio::sync::{broadcast, oneshot};

use crate::auth::{self, Attempt};
use crate::{analysis, autostart, get_tosu, leaderboard, log, practice, presence, queue, replays, tosu, update, Bridge, COUNTER_PATH, DEFAULT_PORT};

// built in for releases, read from disk in debug builds so edits show up right away
#[derive(rust_embed::Embed)]
#[folder = "../counter/"]
struct Counter;

const WINDOW_ACTIONS: [&str; 6] = ["show", "hide", "minimize", "close", "drag", "opened"];

fn counter_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" => "text/javascript",
        "json" => "application/json",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ttf" => "font/ttf",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn empty(code: u16) -> Response<Body> {
    Response::builder().status(code).body(Body::empty()).unwrap()
}

fn json(code: u16, value: &serde_json::Value) -> Response<Body> {
    Response::builder().status(code).header(header::CONTENT_TYPE, "application/json").body(Body::from(value.to_string())).unwrap()
}

fn saved(b: &Bridge, ok: bool) -> Response<Body> {
    if ok {
        json(200, &b.snapshot())
    } else {
        json(500, &serde_json::json!({ "error": "Couldn't save that. It works until Hashi closes, check that its settings folder can be written to." }))
    }
}

fn redirect(to: &str) -> Response<Body> {
    Response::builder().status(StatusCode::FOUND).header(header::LOCATION, to).body(Body::empty()).unwrap()
}

fn stream(first: Option<String>, mut rx: broadcast::Receiver<Arc<str>>, b: Option<Arc<Bridge>>) -> Response<Body> {
    let body = async_stream::stream! {
        if let Some(first) = first {
            yield Ok::<Bytes, Infallible>(Bytes::from(first));
        }
        let mut ping = tokio::time::interval(Duration::from_secs(15));
        ping.tick().await;
        loop {
            tokio::select! {
                m = rx.recv() => match m {
                    Ok(text) => yield Ok(Bytes::from(text.to_string())),
                    // fell behind, a fresh snapshot covers it
                    Err(broadcast::error::RecvError::Lagged(_)) => if let Some(b) = &b {
                        yield Ok(Bytes::from(format!("data: {}\n\n", b.snapshot())));
                    },
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = ping.tick() => yield Ok(Bytes::from_static(b": ping\n\n")),
            }
        }
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(body))
        .unwrap()
}

async fn handle(State(b): State<Arc<Bridge>>, req: Request) -> Response<Body> {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
    let origin = req.headers().get(header::ORIGIN).and_then(|h| h.to_str().ok()).map(String::from);
    let port = b.env.port;
    // dns rebinding / random sites
    let allowed_host = host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}");
    if !allowed_host || origin.as_ref().is_some_and(|o| !b.env.counter_origins.contains(o)) {
        return empty(403);
    }
    let mut res = route(&b, req, origin.is_some()).await;
    if let Some(o) = origin.and_then(|o| HeaderValue::from_str(&o).ok()) {
        res.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
        res.headers_mut().insert(header::VARY, HeaderValue::from_static("origin"));
    }
    res
}

async fn route(b: &Arc<Bridge>, req: Request, has_origin: bool) -> Response<Body> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    if method == Method::OPTIONS {
        return Response::builder()
            .status(204)
            .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST")
            .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "content-type")
            .header(header::ACCESS_CONTROL_MAX_AGE, "600")
            .body(Body::empty())
            .unwrap();
    }

    if method == Method::GET {
        if path == "/" {
            return redirect(COUNTER_PATH);
        }
        if let Some(rel) = path.strip_prefix(COUNTER_PATH) {
            let rel = percent_encoding::percent_decode_str(rel).decode_utf8_lossy().into_owned();
            let query = req.uri().query().unwrap_or("");
            if rel.is_empty() && b.env.port != DEFAULT_PORT && !query.split('&').any(|p| p.starts_with("bridge=")) {
                let sep = if query.is_empty() { "" } else { "&" };
                return redirect(&format!("{COUNTER_PATH}?{query}{sep}bridge={}", b.env.port));
            }
            let rel = if rel.is_empty() { "index.html".to_string() } else { rel };
            if rel.split(['/', '\\']).any(|p| p == "..") {
                return empty(404);
            }
            return match Counter::get(&rel) {
                Some(file) => Response::builder()
                    .header(header::CONTENT_TYPE, counter_type(&rel))
                    .header(header::CACHE_CONTROL, "no-cache")
                    .body(Body::from(file.data.into_owned()))
                    .unwrap(),
                None => empty(404),
            };
        }
        if let Some(id) = path.strip_prefix("/cover/") {
            let item = b.st().queue.items.iter().find(|i| i.id == id).cloned();
            let Some(item) = item else { return empty(404) };
            let Some(file) = replays::cover_file(b, &item).await else { return empty(404) };
            let Ok(bytes) = crate::read_capped(&file, crate::MAX_IMAGE_BYTES).await else { return empty(404) };
            let Some(kind) = replays::image_type(&file).or_else(|| replays::sniff_image(&bytes)) else { return empty(404) };
            return Response::builder()
                .header(header::CONTENT_TYPE, kind)
                .header(header::CACHE_CONTROL, "max-age=3600")
                .body(Body::from(bytes))
                .unwrap();
        }
        if let Some(md5) = path.strip_prefix("/leaderboard/") {
            if !leaderboard::valid_md5(md5) {
                return empty(404);
            }
            let Some((speed, mods)) = leaderboard::parse_variant(req.uri().query().unwrap_or("")) else { return empty(400) };
            let (status, body) = leaderboard::board(b, md5, speed, mods).await;
            let mut res = json(status, &body);
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            return res;
        }
        if let Some(id) = path.strip_prefix("/analysis/") {
            let (status, body) = analysis::for_play(b, id).await;
            let mut res = json(status, &body);
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            return res;
        }
        if path == "/profile" {
            let mut res = json(200, &crate::profile::summary(b).await);
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            return res;
        }
        if path == "/state" {
            let mut res = json(200, &b.snapshot());
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            return res;
        }
        if path == "/events" {
            let first = format!("retry: 2000\ndata: {}\n\n", b.snapshot());
            return stream(Some(first), b.events.subscribe(), Some(b.clone()));
        }
        return empty(404);
    }

    // POSTs need the counter's origin
    if method == Method::POST && has_origin {
        if let Some(name) = path.strip_prefix("/window/") {
            if !WINDOW_ACTIONS.contains(&name) {
                return empty(404);
            }
            b.window_action(name);
            return empty(204);
        }
        if path == "/presence/on" || path == "/presence/off" {
            let ok = presence::set_presence_setting(b, path == "/presence/on").await;
            return saved(b, ok);
        }
        // /theme/<hue>/<saturation>
        if let Some(rest) = path.strip_prefix("/theme/") {
            let parsed = rest.split_once('/').and_then(|(h, v)| Some((h.parse::<u16>().ok()?, v.parse::<u8>().ok()?)));
            let Some((hue, sat)) = parsed.filter(|(h, v)| *h < 360 && *v <= 100) else { return empty(400) };
            let ok = b.set_theme(hue, sat).await;
            return saved(b, ok);
        }
        if path == "/tosu-autostart/on" || path == "/tosu-autostart/off" {
            let ok = tosu::set_tosu_autostart(b, path == "/tosu-autostart/on").await;
            return saved(b, ok);
        }
        // /sharing/both, /sharing/plays, /sharing/live
        if let Some(choice) = path.strip_prefix("/sharing/") {
            let Some((plays, live)) = (match choice {
                "both" => Some((true, true)),
                "plays" => Some((true, false)),
                "live" => Some((false, true)),
                _ => None,
            }) else {
                return empty(404);
            };
            let plays_saved = b.set_send_plays(plays).await;
            let live_saved = presence::set_presence_setting(b, live).await;
            return saved(b, plays_saved && live_saved);
        }
        if path == "/tosu-dashboard/on" || path == "/tosu-dashboard/off" {
            return match tosu::set_dashboard(b, path == "/tosu-dashboard/on").await {
                Ok(()) => json(200, &b.snapshot()),
                Err(e) => json(500, &serde_json::json!({ "error": e })),
            };
        }
        if path == "/notes/read" {
            let ok = b.read_notes().await;
            return saved(b, ok);
        }
        // /practice/<play id>?from=58&to=66&repeats=4&slowest=0.85&lead=2&goal=96.9
        if let Some(id) = path.strip_prefix("/practice/") {
            let Some(options) = practice::Options::from_query(req.uri().query().unwrap_or("")) else { return empty(400) };
            let (status, body) = practice::make(b, id, options).await;
            return json(status, &body);
        }
        // /goals/<id>/target/<percent>
        if let Some((goal, value)) = path.strip_prefix("/goals/").and_then(|r| r.split_once("/target/")) {
            let Some(target) = value.parse::<f64>().ok().filter(|t| (1.0..=100.0).contains(t)) else { return empty(400) };
            return if crate::goals::set_target(b, goal, target).await { json(200, &b.snapshot()) } else { empty(404) };
        }
        // /goals/<id>/dismiss
        if let Some(goal) = path.strip_prefix("/goals/").and_then(|r| r.strip_suffix("/dismiss")) {
            return if crate::goals::dismiss(b, goal).await { json(200, &b.snapshot()) } else { empty(404) };
        }
        // /finger/4/0/ring
        if let Some(rest) = path.strip_prefix("/finger/") {
            let parts: Vec<&str> = rest.split('/').collect();
            let [keys, column, finger] = parts[..] else { return empty(404) };
            let (Ok(keys), Ok(column)) = (keys.parse::<usize>(), column.parse::<usize>()) else { return empty(400) };
            if !(1..=18).contains(&keys) || column >= keys || !analysis::FINGERS.contains(&finger) {
                return empty(400);
            }
            let ok = b.set_finger(keys, column, finger).await;
            return saved(b, ok);
        }
        if path == "/guide/read" {
            let ok = b.read_guide().await;
            return saved(b, ok);
        }
        if path == "/show-dans/on" || path == "/show-dans/off" {
            let ok = b.set_show_dans(path == "/show-dans/on").await;
            return saved(b, ok);
        }
        if path == "/start-in-tray/on" || path == "/start-in-tray/off" {
            let ok = b.set_start_in_tray(path == "/start-in-tray/on").await;
            return saved(b, ok);
        }
        if path == "/thumb/left" || path == "/thumb/right" {
            let ok = b.set_thumb(path == "/thumb/right").await;
            return saved(b, ok);
        }
        if path == "/on-top/on" || path == "/on-top/off" {
            let ok = b.set_on_top(path == "/on-top/on").await;
            return saved(b, ok);
        }
        if path == "/update/download" {
            update::open_download_page(b);
            return json(200, &b.snapshot());
        }
        if path == "/autostart/on" || path == "/autostart/off" {
            return match autostart::set(b, path == "/autostart/on").await {
                Ok(()) => json(200, &b.snapshot()),
                Err(e) => json(500, &serde_json::json!({ "error": format!("Couldn't change that: {e}") })),
            };
        }
        // /plays/<id>/hold, /plays/<id>/send
        if let Some((id, what)) = path.strip_prefix("/plays/").and_then(|r| r.rsplit_once('/')).filter(|(_, w)| ["hold", "send"].contains(w)) {
            return if queue::hold(b, id, what == "hold") { json(200, &b.snapshot()) } else { empty(404) };
        }
        // /plays/<id>/remove
        if let Some(id) = path.strip_prefix("/plays/").and_then(|r| r.strip_suffix("/remove")) {
            return if queue::remove(b, id) { json(200, &b.snapshot()) } else { empty(404) };
        }
        if path == "/get-tosu" {
            get_tosu::start(b);
            return json(200, &b.snapshot());
        }
        if path == "/update/install" {
            update::install(b);
            return json(200, &b.snapshot());
        }
        if path == "/update/later" {
            update::dismiss(b);
            return json(200, &b.snapshot());
        }
        if ["/connect", "/cancel", "/disconnect"].contains(&path.as_str()) {
            let result = match path.as_str() {
                "/connect" => auth::start_connect(b).await,
                "/cancel" => {
                    let attempt = b.st().connect_attempt.clone();
                    if let Some(a) = attempt {
                        a.cancel();
                    }
                    Ok(())
                }
                _ => {
                    auth::disconnect(b).await;
                    Ok(())
                }
            };
            return match result {
                Ok(()) => json(200, &b.snapshot()),
                Err(e) => {
                    log!("{path}: {e}");
                    let status = {
                        let s = b.st();
                        let has_token = s.config.connection.as_ref().is_some_and(|c| c.refresh_token.is_some());
                        if has_token { s.connection.status.clone() } else { "offline".into() }
                    };
                    auth::set_connection(b, &status, Some(e.message().into()));
                    json(502, &serde_json::json!({ "error": e.message() }))
                }
            };
        }
    }
    empty(404)
}

pub(crate) fn serve(b: &Arc<Bridge>, listener: tokio::net::TcpListener) {
    let app = Router::new().fallback(handle).with_state(b.clone());
    log!("counter API on http://127.0.0.1:{}", b.env.port);
    b.spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            log!("counter server stopped: {e}");
        }
    });
}

pub(crate) fn serve_callback(b: &Arc<Bridge>, listener: tokio::net::TcpListener, callback_path: &'static str, state: String, attempt: Arc<Attempt>, stop: oneshot::Receiver<()>) {
    let port = b.env.port;
    let app = Router::new().fallback(move |req: Request| {
        let state = state.clone();
        let attempt = attempt.clone();
        async move {
            if req.uri().path() != callback_path {
                return empty(404);
            }
            let q: std::collections::HashMap<String, String> =
                url::form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).into_owned().collect();
            if q.get("state") != Some(&state) {
                return Response::builder()
                    .status(400)
                    .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                    .body(Body::from("This does not match the current connection attempt."))
                    .unwrap();
            }
            let error = q.get("error").cloned();
            let ok = error.is_none();
            auth::settle_callback(&attempt, error, q.get("code").cloned());
            redirect(&auth::done_url(port, ok))
        }
    });
    b.spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stop.await;
            })
            .await;
    });
}

pub(crate) fn watch_counter(b: &Arc<Bridge>) {
    if !cfg!(debug_assertions) {
        return;
    }
    use notify::Watcher;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("counter");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let Ok(mut watcher) = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
        // inotify also reports every read, serving a file would reload the window forever
        if e.is_ok_and(|e| e.kind.is_create() || e.kind.is_modify() || e.kind.is_remove()) {
            let _ = tx.send(());
        }
    }) else {
        return;
    };
    if watcher.watch(&dir, notify::RecursiveMode::Recursive).is_err() {
        return;
    }
    let b2 = b.clone();
    b.spawn(async move {
        let _keep = watcher;
        while rx.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(150)).await;
            while rx.try_recv().is_ok() {}
            let _ = b2.events.send(Arc::from("event: reload\ndata: {}\n\n"));
        }
    });
}
