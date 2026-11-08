use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use bytes::Bytes;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::types::{Connection, Jwk, Key};
use crate::{iso_now, log, now_ms, queue, Bridge, Env, State, APP_NAME, CALLBACK_PATH, CLIENT_ID, OLD_CALLBACK_PATH, COUNTER_PATH, PRESENCE_SCOPE, SCOPES, VERSION};

pub(crate) enum Fail {
    // ConnectionProblem: the connection itself is in trouble, uploads stop
    Conn(&'static str, String),
    Other(String),
}

impl Fail {
    pub fn message(&self) -> &str {
        match self {
            Fail::Conn(_, m) | Fail::Other(m) => m,
        }
    }
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

pub(crate) struct Resp {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Bytes,
}

impl Resp {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).and_then(|v| v.to_str().ok()).map(String::from)
    }
}

pub(crate) struct Req {
    pub method: reqwest::Method,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<Bytes>,
    pub timeout: Duration,
}

impl Req {
    pub fn get() -> Self {
        Req { method: reqwest::Method::GET, headers: vec![], body: None, timeout: Duration::from_secs(30) }
    }

    pub fn json(method: reqwest::Method, body: &Value) -> Self {
        Req {
            method,
            headers: vec![("content-type", "application/json".into())],
            body: Some(Bytes::from(body.to_string())),
            timeout: Duration::from_secs(30),
        }
    }

    pub fn method(method: reqwest::Method) -> Self {
        Req { method, ..Req::get() }
    }
}

pub(crate) fn b64(bytes: impl AsRef<[u8]>) -> String {
    B64.encode(bytes)
}

fn sha256(bytes: impl AsRef<[u8]>) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn random_b64(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    b64(buf)
}

fn create_key() -> Key {
    let secret = SigningKey::random(&mut rand::rngs::OsRng);
    let point = secret.verifying_key().to_encoded_point(false);
    let jwk = Jwk {
        kty: "EC".into(),
        crv: "P-256".into(),
        x: b64(point.x().expect("uncompressed point")),
        y: b64(point.y().expect("uncompressed point")),
    };
    Key {
        private_key_pem: secret.to_pkcs8_pem(LineEnding::LF).expect("pkcs8").to_string(),
        thumbprint: jwk_thumbprint(&jwk),
        jwk,
        created_at: iso_now(),
    }
}

// rfc 7638
fn jwk_thumbprint(jwk: &Jwk) -> String {
    let canonical = json!({ "crv": jwk.crv, "kty": jwk.kty, "x": jwk.x, "y": jwk.y }).to_string();
    b64(sha256(canonical))
}

fn dpop_proof(s: &mut State, method: &str, url: &str, access_token: Option<&str>, nonce: Option<&str>) -> Result<String, Fail> {
    let key = s.config.key.as_ref().ok_or_else(|| Fail::Other("No key to sign with.".into()))?;
    if s.signing_key.is_none() {
        s.signing_key = Some(SigningKey::from_pkcs8_pem(&key.private_key_pem).map_err(|e| Fail::Other(format!("Bad stored key: {e}")))?);
    }
    let header = json!({ "typ": "dpop+jwt", "alg": "ES256", "jwk": key.jwk });
    let u = url::Url::parse(url).map_err(|e| Fail::Other(format!("Bad url {url}: {e}")))?;
    let mut payload = json!({
        "jti": uuid::Uuid::new_v4().to_string(),
        "htm": method.to_uppercase(),
        "htu": format!("{}{}", u.origin().ascii_serialization(), u.path()),
        "iat": now_ms() / 1000,
    });
    if let Some(n) = nonce {
        payload["nonce"] = n.into();
    }
    if let Some(t) = access_token {
        payload["ath"] = b64(sha256(t)).into();
    }
    let input = format!("{}.{}", b64(header.to_string()), b64(payload.to_string()));
    // jws wants raw r||s, not DER
    let sig: Signature = s.signing_key.as_ref().unwrap().sign(input.as_bytes());
    Ok(format!("{input}.{}", b64(sig.to_bytes())))
}

pub(crate) async fn load_capabilities(b: &Arc<Bridge>) -> Result<Value, Fail> {
    let url = format!("{}/api/integrations/companella/v1/capabilities", b.env.origin);
    let res = b.client().get(&url).timeout(Duration::from_secs(10)).send().await.map_err(|e| Fail::Other(net_message(&e)))?;
    if !res.status().is_success() {
        return Err(Fail::Other(format!("capabilities answered {}", res.status().as_u16())));
    }
    let caps: Value = res.json().await.map_err(|e| Fail::Other(e.to_string()))?;
    b.st().caps = Some(caps.clone());
    Ok(caps)
}

pub(crate) async fn caps(b: &Arc<Bridge>) -> Result<Value, Fail> {
    let have = b.st().caps.clone();
    match have {
        Some(c) => Ok(c),
        None => load_capabilities(b).await,
    }
}

fn cap(caps: &Value, k: &str) -> String {
    caps.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

pub(crate) fn api_base(caps: &Value) -> String {
    format!("{}{}", cap(caps, "issuer"), cap(caps, "api_prefix"))
}

// the backend's own host, so a stuck site doesn't hold up plays
fn direct_base(caps: &Value) -> Option<String> {
    let raw = caps.get("api_base_url").and_then(Value::as_str)?;
    let u = url::Url::parse(raw).ok()?;
    let fine = matches!(u.scheme(), "https" | "http") && u.host().is_some() && u.query().is_none() && u.fragment().is_none();
    fine.then(|| raw.trim_end_matches('/').to_string())
}

// the profile endpoint is on the same host, outside api_prefix
fn backend_origin(caps: &Value) -> Option<String> {
    Some(url::Url::parse(&direct_base(caps)?).ok()?.origin().ascii_serialization())
}

const DIRECT_PAUSE_MS: i64 = 5 * 60_000;

// direct first while it answers, the site always last
fn routes(b: &Bridge, caps: &Value, path: &str, site: String) -> Vec<String> {
    let mut urls = Vec::with_capacity(2);
    if let Some(d) = direct_base(caps) {
        if b.st().direct_down_until <= now_ms() {
            urls.push(format!("{d}{path}"));
        }
    }
    urls.push(site);
    urls
}

// each proof is signed for the url it goes to, so falling back re-signs
async fn send_routed(b: &Arc<Bridge>, urls: &[String], req: &Req, auth: bool) -> Result<Resp, Fail> {
    let (site, direct) = urls.split_last().expect("the site is always there");
    for url in direct {
        match send(b, url, req, auth).await {
            Ok(res) => return Ok(res),
            Err(e) => {
                b.st().direct_down_until = now_ms() + DIRECT_PAUSE_MS;
                log!("api host: {e}, going through {} for {} min", b.env.origin, DIRECT_PAUSE_MS / 60_000);
            }
        }
    }
    send(b, site, req, auth).await
}

pub(crate) const TIMED_OUT: &str = "Network error: timed out";

pub(crate) fn net_message(e: &reqwest::Error) -> String {
    let mut root: &dyn std::error::Error = e;
    while let Some(next) = root.source() {
        root = next;
    }
    if e.is_timeout() {
        TIMED_OUT.into()
    } else {
        format!("Network error: {root}")
    }
}

async fn attempt(b: &Arc<Bridge>, url: &str, req: &Req, auth: bool, nonce: Option<String>) -> Result<Resp, Fail> {
    let (token, proof) = {
        let mut s = b.st();
        let token = if auth { s.access_token.clone() } else { None };
        let proof = dpop_proof(&mut s, req.method.as_str(), url, token.as_deref(), nonce.as_deref())?;
        (token, proof)
    };
    let mut rb = b.client().request(req.method.clone(), url).timeout(req.timeout).header("dpop", proof);
    for (k, v) in &req.headers {
        rb = rb.header(*k, v);
    }
    if let Some(t) = token {
        rb = rb.header("authorization", format!("DPoP {t}"));
    }
    if let Some(body) = &req.body {
        rb = rb.body(body.clone());
    }
    let res = rb.send().await.map_err(|e| Fail::Other(net_message(&e)))?;
    let status = res.status().as_u16();
    let headers = res.headers().clone();
    let body = res.bytes().await.map_err(|e| Fail::Other(net_message(&e)))?;
    Ok(Resp { status, headers, body })
}

// fresh proof per request, retry once on a nonce challenge
pub(crate) async fn send(b: &Arc<Bridge>, url: &str, req: &Req, auth: bool) -> Result<Resp, Fail> {
    let nonce = b.st().dpop_nonce.clone();
    let mut res = attempt(b, url, req, auth, nonce).await?;
    let challenge = res.header("dpop-nonce");
    if let Some(c) = &challenge {
        b.st().dpop_nonce = Some(c.clone());
        let asks = res.status == 401 || (res.status == 400 && res.json().get("error").and_then(Value::as_str) == Some("use_dpop_nonce"));
        if asks {
            res = attempt(b, url, req, auth, Some(c.clone())).await?;
            if let Some(again) = res.header("dpop-nonce") {
                b.st().dpop_nonce = Some(again);
            }
        }
    }
    Ok(res)
}

fn not_in_beta(payload: &Value) -> Option<String> {
    let desc = payload.get("error_description").and_then(Value::as_str).unwrap_or("");
    (payload.get("error").and_then(Value::as_str) == Some("invalid_grant") && desc.to_lowercase().contains("not in the beta")).then(|| desc.to_string())
}

pub(crate) async fn refresh_access(b: &Arc<Bridge>) -> Result<(), Fail> {
    let refresh = b.st().config.connection.as_ref().and_then(|c| c.refresh_token.clone());
    let Some(refresh) = refresh else {
        return Err(Fail::Conn("disconnected", "Not connected.".into()));
    };
    let caps = caps(b).await.map_err(|_| Fail::Conn("offline", format!("Could not reach {}.", b.env.origin)))?;
    let req = Req::json(reqwest::Method::POST, &json!({ "grant_type": "refresh_token", "refresh_token": refresh }));
    let urls = routes(b, &caps, "/oauth/token", cap(&caps, "token_endpoint"));
    let res = send_routed(b, &urls, &req, false)
        .await
        .map_err(|_| Fail::Conn("offline", format!("Could not reach {}.", b.env.origin)))?;
    let payload = res.json();
    if !res.ok() {
        if let Some(desc) = not_in_beta(&payload) {
            return Err(Fail::Conn("not_in_beta", desc));
        }
        let err = payload.get("error").and_then(Value::as_str).unwrap_or("");
        if ["invalid_grant", "invalid_dpop_proof", "invalid_client"].contains(&err) {
            // the site doesn't say whether it was removed, ran out or never existed
            let desc = payload.get("error_description").and_then(Value::as_str).unwrap_or(GONE);
            return Err(Fail::Conn("revoked", desc.into()));
        }
        return Err(Fail::Conn("offline", format!("Refresh failed ({} {err}).", res.status).replace(" )", ")")));
    }
    let changed = {
        let mut s = b.st();
        s.access_token = payload.get("access_token").and_then(Value::as_str).map(String::from);
        s.access_expires_at = now_ms() + payload.get("expires_in").and_then(Value::as_i64).unwrap_or(300) * 1000;
        // not rotated, but keep it if it ever changes
        match (payload.get("refresh_token").and_then(Value::as_str), s.config.connection.as_mut()) {
            (Some(new), Some(conn)) if conn.refresh_token.as_deref() != Some(new) => {
                conn.refresh_token = Some(new.into());
                true
            }
            _ => false,
        }
    };
    if changed {
        b.save_config().await;
    }
    Ok(())
}

async fn ensure_access(b: &Arc<Bridge>) -> Result<(), Fail> {
    let stale = {
        let s = b.st();
        s.access_token.is_none() || now_ms() > s.access_expires_at - 30_000
    };
    if stale {
        refresh_access(b).await?;
    }
    Ok(())
}

// path is relative to the api base, e.g. "/submissions"
pub(crate) async fn api(b: &Arc<Bridge>, path: &str, req: Req) -> Result<Resp, Fail> {
    ensure_access(b).await?;
    let caps = caps(b).await?;
    let route = || routes(b, &caps, path, format!("{}{path}", api_base(&caps)));
    let mut res = send_routed(b, &route(), &req, true).await?;
    if res.status == 401 && res.json().pointer("/error/code").and_then(Value::as_str) == Some("token_expired") {
        b.st().access_token = None;
        refresh_access(b).await?;
        res = send_routed(b, &route(), &req, true).await?;
    }
    Ok(res)
}

// no token and no proof, for a report from someone who isn't connected
pub(crate) async fn api_open(b: &Arc<Bridge>, path: &str, req: Req) -> Result<Resp, Fail> {
    let caps = caps(b).await?;
    let mut last = None;
    for url in routes(b, &caps, path, format!("{}{path}", api_base(&caps))) {
        let mut rb = b.client().request(req.method.clone(), &url).timeout(req.timeout);
        for (k, v) in &req.headers {
            rb = rb.header(*k, v);
        }
        if let Some(body) = &req.body {
            rb = rb.body(body.clone());
        }
        match rb.send().await {
            Ok(res) => {
                let (status, headers) = (res.status().as_u16(), res.headers().clone());
                let body = res.bytes().await.map_err(|e| Fail::Other(net_message(&e)))?;
                return Ok(Resp { status, headers, body });
            }
            Err(e) => last = Some(Fail::Other(net_message(&e))),
        }
    }
    Err(last.unwrap_or_else(|| Fail::Other("No route to Mania Tracker.".into())))
}

pub(crate) const GONE: &str = "Mania Tracker doesn't accept this connection anymore. It was removed on the site, or it expired. Connect again.";

pub(crate) fn set_connection(b: &Arc<Bridge>, status: &str, message: Option<String>) {
    {
        let mut s = b.st();
        if s.connection.status != status && ["revoked", "error", "not_in_beta", "disabled"].contains(&status) {
            log!("connection {status}: {}", message.as_deref().unwrap_or(""));
        }
        s.connection.status = status.into();
        s.connection.message = message;
        if status != "connecting" {
            s.connection.authorize_url = None;
        }
    }
    b.broadcast();
}

pub(crate) fn connection_summary(env: &Env, s: &State) -> Value {
    let c = s.config.connection.as_ref();
    let user_id = c.map(|c| c.user_id.clone()).unwrap_or(Value::Null);
    let mine = !user_id.is_null() && s.profile.user_id == user_id;
    let environment = c
        .map(|c| c.environment.clone())
        .filter(|v| !v.is_null())
        .or_else(|| s.caps.as_ref().and_then(|x| x.get("environment").cloned()))
        .unwrap_or(Value::Null);
    json!({
        "status": s.connection.status,
        "message": s.connection.message,
        "authorizeUrl": s.connection.authorize_url,
        "origin": env.origin,
        "username": c.map(|c| c.username.clone()).unwrap_or(Value::Null),
        "userId": user_id,
        "installationId": c.map(|c| c.installation_id.clone()).unwrap_or(Value::Null),
        "environment": environment,
        "avatarUrl": if mine { s.profile.avatar_url.clone() } else { None },
        "accent": if mine { s.profile.accent.clone() } else { None },
    })
}

pub(crate) fn presence_supported(s: &State) -> bool {
    s.caps
        .as_ref()
        .and_then(|c| c.get("scopes_supported"))
        .and_then(Value::as_array)
        .is_some_and(|l| l.iter().any(|x| x.as_str() == Some(PRESENCE_SCOPE)))
}

// settled by whichever of callback, cancel or timeout comes first
pub(crate) struct Attempt {
    tx: Mutex<Option<oneshot::Sender<Result<String, String>>>>,
    cancelled: AtomicBool,
    stop: tokio::sync::Notify,
}

impl Attempt {
    fn new(tx: oneshot::Sender<Result<String, String>>) -> Self {
        Attempt { tx: Mutex::new(Some(tx)), cancelled: AtomicBool::new(false), stop: tokio::sync::Notify::new() }
    }

    fn settle(&self, r: Result<String, String>) {
        if let Some(tx) = self.tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(r);
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        // notify_one keeps the wakeup for a waiter that isn't waiting yet
        self.stop.notify_one();
        self.settle(Err("cancelled".into()));
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        if !self.is_cancelled() {
            self.stop.notified().await;
        }
    }
}

// matches encodeURIComponent, %20 not +
const COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

pub(crate) async fn start_connect(b: &Arc<Bridge>) -> Result<(), Fail> {
    if b.st().connect_attempt.is_some() {
        return Ok(());
    }
    let caps = load_capabilities(b).await?;
    if caps.get("enabled") != Some(&Value::Bool(true)) {
        set_connection(b, "disabled", Some("Score import is switched off on this server.".into()));
        return Ok(());
    }

    // keep the key across reconnects so the installation (and queued plays) stays the same
    let new_key = {
        let mut s = b.st();
        let missing = s.config.key.is_none();
        if missing {
            s.config.key = Some(create_key());
            s.signing_key = None;
        }
        missing
    };
    if new_key {
        b.save_config().await;
    }

    let verifier = random_b64(48);
    let challenge = b64(sha256(&verifier));
    let state = random_b64(32);

    let (tx, rx) = oneshot::channel();
    let attempt = Arc::new(Attempt::new(tx));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| Fail::Other(e.to_string()))?;
    let port = listener.local_addr().map_err(|e| Fail::Other(e.to_string()))?.port();
    // a site with the bridge's own sign-in page also takes the bridge's own callback path
    let (authorize_endpoint, callback_path) = match caps.get("bridge_authorization_endpoint").and_then(Value::as_str) {
        Some(e) => (e.to_string(), CALLBACK_PATH),
        None => (cap(&caps, "authorization_endpoint"), OLD_CALLBACK_PATH),
    };
    let redirect_uri = format!("http://127.0.0.1:{port}{callback_path}");
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    crate::server::serve_callback(b, listener, callback_path, state.clone(), attempt.clone(), stop_rx);

    let (scope, thumbprint) = {
        let s = b.st();
        // requested up front so turning sharing on later doesn't need another consent
        let mut scopes: Vec<&str> = SCOPES.to_vec();
        if presence_supported(&s) {
            scopes.push(PRESENCE_SCOPE);
        }
        (scopes.join(" "), s.config.key.as_ref().map(|k| k.thumbprint.clone()).unwrap_or_default())
    };
    let query = [
        ("client_id", CLIENT_ID.to_string()),
        ("response_type", "code".into()),
        ("redirect_uri", redirect_uri.clone()),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256".into()),
        ("scope", scope),
        ("dpop_jkt", thumbprint),
        ("app_name", APP_NAME.into()),
    ]
    .iter()
    .map(|(k, v)| format!("{k}={}", percent_encoding::utf8_percent_encode(v, COMPONENT)))
    .collect::<Vec<_>>()
    .join("&");
    let authorize = format!("{authorize_endpoint}?{query}");

    let before = {
        let mut s = b.st();
        s.connect_attempt = Some(attempt.clone());
        (s.connection.status.clone(), s.connection.message.clone())
    };
    set_connection(b, "connecting", None);
    b.st().connection.authorize_url = Some(authorize.clone());
    b.broadcast();
    let shown = url::Url::parse(&authorize).map(|u| format!("{}{}", u.origin().ascii_serialization(), u.path())).unwrap_or_default();
    log!("connect: opening {shown} (callback on port {port})");
    b.open_browser(&authorize);

    let b = b.clone();
    b.clone().spawn(async move {
        let result = match tokio::time::timeout(Duration::from_secs(600), rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("cancelled".into()),
            Err(_) => Err("timeout".into()),
        };
        let _ = stop_tx.send(());
        match result {
            Err(error) => {
                let message = match error.as_str() {
                    "access_denied" => Some("Nothing was connected.".into()),
                    "timeout" => Some("The browser did not come back within 10 minutes.".into()),
                    "cancelled" => before.1,
                    other => Some(format!("Authorization failed: {other}")),
                };
                log!("connect: {error}");
                b.st().connect_attempt = None;
                // disconnect() might have changed it already
                if b.st().connection.status == "connecting" {
                    set_connection(&b, &before.0, message);
                }
            }
            Ok(code) => {
                finish_exchange(&b, &caps, &code, &verifier, &redirect_uri, &attempt, before).await;
            }
        }
    });
    Ok(())
}

async fn finish_exchange(
    b: &Arc<Bridge>, caps: &Value, code: &str, verifier: &str, redirect_uri: &str,
    attempt: &Attempt, before: (String, Option<String>),
) {
    let result = exchange_code(b, caps, code, verifier, redirect_uri, attempt).await;
    // a cancel that got the attempt before this still wins
    let cancelled = {
        let mut s = b.st();
        s.connect_attempt = None;
        attempt.is_cancelled()
    };
    match result {
        Ok(true) if cancelled => {
            log!("connect: cancelled after sign-in, forgetting it");
            disconnect(b).await;
        }
        Ok(true) => {
            set_connection(b, "connected", None);
            queue::wake_queue(b, 0);
        }
        Ok(false) => {
            log!("connect: cancelled");
            if b.st().connection.status == "connecting" {
                set_connection(b, &before.0, before.1);
            }
        }
        Err(e) => {
            log!("connect: exchange failed: {e}");
            let status = if let Fail::Conn(s, _) = &e { s } else { "disconnected" };
            set_connection(b, status, Some(e.message().into()));
        }
    }
}

pub(crate) fn settle_callback(attempt: &Attempt, error: Option<String>, code: Option<String>) {
    attempt.settle(match error {
        Some(e) => Err(e),
        None => Ok(code.unwrap_or_default()),
    });
}

// false: cancelled before anything was kept, or disconnect already cleared it
async fn exchange_code(b: &Arc<Bridge>, caps: &Value, code: &str, verifier: &str, redirect_uri: &str, attempt: &Attempt) -> Result<bool, Fail> {
    let req = Req::json(
        reqwest::Method::POST,
        &json!({ "grant_type": "authorization_code", "code": code, "code_verifier": verifier, "client_id": CLIENT_ID, "redirect_uri": redirect_uri }),
    );
    let urls = routes(b, caps, "/oauth/token", cap(caps, "token_endpoint"));
    let res = tokio::select! {
        res = send_routed(b, &urls, &req, false) => res?,
        _ = attempt.cancelled() => return Ok(false),
    };
    let payload = res.json();
    if !res.ok() {
        if let Some(desc) = not_in_beta(&payload) {
            return Err(Fail::Conn("not_in_beta", desc));
        }
        let err = payload.get("error").and_then(Value::as_str).unwrap_or("unknown");
        return Err(Fail::Conn("disconnected", format!("Token exchange failed ({} {err}). Connect again.", res.status)));
    }
    let get = |k: &str| payload.get(k).cloned().unwrap_or(Value::Null);
    {
        let mut s = b.st();
        if attempt.is_cancelled() {
            return Ok(false);
        }
        s.access_token = payload.get("access_token").and_then(Value::as_str).map(String::from);
        s.access_expires_at = now_ms() + payload.get("expires_in").and_then(Value::as_i64).unwrap_or(300) * 1000;
        // boards hold the last account's own row
        s.boards = Default::default();
        s.config.connection = Some(Connection {
            issuer: caps.get("issuer").cloned().unwrap_or(Value::Null),
            environment: caps.get("environment").cloned().unwrap_or(Value::Null),
            refresh_token: payload.get("refresh_token").and_then(Value::as_str).map(String::from),
            scope: get("scope"),
            installation_id: get("installation_id"),
            user_id: get("user_id"),
            username: get("username"),
            connected_at: iso_now().into(),
        });
    }
    b.save_config().await;
    // token response doesn't always have the name
    if let Err(e) = load_identity(b).await {
        log!("me: {e}");
    }
    let (name, inst) = {
        let s = b.st();
        // disconnect can clear the connection while /me is in flight
        let Some(c) = s.config.connection.as_ref() else { return Ok(false) };
        (crate::types::text(&c.username), crate::types::text(&c.installation_id))
    };
    load_profile_later(b);
    log!("connected as {name} (installation {inst})");
    Ok(true)
}

// the import api has no avatar or accent, the profile does
fn load_profile_later(b: &Arc<Bridge>) {
    let b2 = b.clone();
    b.spawn(async move { load_profile(&b2).await });
}

async fn load_profile(b: &Arc<Bridge>) {
    let user_id = {
        let s = b.st();
        let Some(id) = s.config.connection.as_ref().map(|c| c.user_id.clone()).filter(|v| !v.is_null()) else {
            return;
        };
        if s.profile.user_id == id && s.profile.accent.is_some() {
            return;
        }
        id
    };
    let id = crate::types::text(&user_id);
    // a.ppy.sh/{id} is cached for a month, the profile's url changes with the avatar
    b.st().profile = crate::Profile { user_id, avatar_url: Some(format!("https://a.ppy.sh/{id}")), accent: None };
    let backend = match &b.env.live_backend_url {
        u if !u.is_empty() => Some(u.clone()),
        _ => caps(b).await.ok().as_ref().and_then(backend_origin),
    };
    if let Some(backend) = backend {
        let url = format!("{backend}/api/profiles/{id}/cached-snapshot");
        match b.client().get(&url).timeout(Duration::from_secs(10)).send().await {
            Ok(res) => {
                let user = if res.status().is_success() { res.json::<Value>().await.ok().and_then(|v| v.get("user").cloned()) } else { None };
                if let Some(user) = user {
                    let mut s = b.st();
                    if let Some(a) = user.get("avatar_url").and_then(Value::as_str).filter(|a| !a.is_empty()) {
                        s.profile.avatar_url = Some(a.into());
                    }
                    if let Some(c) = user.get("avatar_accent").and_then(Value::as_str) {
                        if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()) {
                            s.profile.accent = Some(c.into());
                        }
                    }
                }
            }
            Err(e) => log!("profile: {}", net_message(&e)),
        }
    }
    b.broadcast();
}

async fn load_identity(b: &Arc<Bridge>) -> Result<(), Fail> {
    let res = api(b, "/me", Req::get()).await?;
    let body = res.json();
    if !res.ok() {
        let code = body.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
        return Err(Fail::Other(format!("/me answered {} {code}", res.status)));
    }
    let changed = {
        let mut s = b.st();
        let Some(c) = s.config.connection.as_mut() else { return Ok(()) };
        let pick = |k: &str, old: &Value| body.get(k).filter(|v| !v.is_null()).cloned().unwrap_or(old.clone());
        let next = (pick("username", &c.username), pick("user_id", &c.user_id), pick("installation_id", &c.installation_id));
        let changed = c.username != next.0 || c.user_id != next.1 || c.installation_id != next.2;
        (c.username, c.user_id, c.installation_id) = next;
        changed
    };
    if changed {
        b.save_config().await;
    }
    Ok(())
}

pub(crate) async fn disconnect(b: &Arc<Bridge>) {
    let attempt = b.st().connect_attempt.clone();
    if let Some(a) = attempt {
        a.cancel();
    }
    let token = {
        let s = b.st();
        s.config.key.as_ref().and(s.config.connection.as_ref()).and_then(|c| c.refresh_token.clone())
    };
    if let Some(token) = token {
        let result = async {
            let caps = caps(b).await?;
            let req = Req::json(reqwest::Method::POST, &json!({ "token": token }));
            send_routed(b, &routes(b, &caps, "/oauth/revoke", cap(&caps, "revocation_endpoint")), &req, false).await
        }
        .await;
        match result {
            Ok(res) => log!("disconnect: revoke answered {}", res.status),
            Err(e) => log!("disconnect: revoke failed ({e}); forgetting the credential anyway"),
        }
    }
    {
        let mut s = b.st();
        s.config.connection = None;
        s.boards = Default::default();
        s.config.key = None;
        s.signing_key = None;
        s.access_token = None;
        s.access_expires_at = 0;
    }
    b.save_config().await;
    set_connection(b, "disconnected", None);
}

pub(crate) async fn check_connection(b: &Arc<Bridge>) {
    {
        let s = b.st();
        if s.config.connection.as_ref().and_then(|c| c.refresh_token.as_ref()).is_none() || s.connect_attempt.is_some() {
            return;
        }
    }
    let result: Result<(), Fail> = async {
        let caps = caps(b).await?;
        if caps.get("enabled") != Some(&Value::Bool(true)) {
            set_connection(b, "disabled", Some("Score import is switched off on this server.".into()));
            return Ok(());
        }
        // disconnected while caps was asked for
        let Some((issuer, environment)) = b.st().config.connection.as_ref().map(|c| (c.issuer.clone(), c.environment.clone())) else {
            return Ok(());
        };
        let want_issuer = caps.get("issuer").cloned().unwrap_or(Value::Null);
        if issuer != want_issuer || Some(&environment) != caps.get("environment").or(Some(&Value::Null)) {
            let msg = format!(
                "This connection belongs to {}, not {}. Connect again.",
                crate::types::text(&issuer),
                crate::types::text(&want_issuer)
            );
            set_connection(b, "revoked", Some(msg));
            return Ok(());
        }
        b.st().access_token = None;
        refresh_access(b).await?;
        load_identity(b).await?;
        load_profile_later(b);
        set_connection(b, "connected", None);
        queue::wake_queue(b, 0);
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {}
        Err(Fail::Conn(status, message)) => set_connection(b, status, Some(message)),
        Err(Fail::Other(message)) => set_connection(b, "offline", Some(message)),
    }
}

pub(crate) fn client_version() -> String {
    format!("mania-bridge/{VERSION}")
}

pub(crate) fn done_url(port: u16, ok: bool) -> String {
    format!("http://127.0.0.1:{port}{COUNTER_PATH}?done={}", if ok { "yes" } else { "no" })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn new_key() -> Key {
        create_key()
    }

    pub(crate) fn test_bridge(origin: &str) -> (Arc<Bridge>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("mania-auth-test-{}", uuid::Uuid::new_v4()));
        let mut env = Env::from_env();
        env.origin = origin.into();
        env.live_backend_url.clear();
        env.data_dir = dir.clone();
        env.config_file = dir.join("config.json");
        env.queue_file = dir.join("queue.json");
        env.chart_index_file = dir.join("chart-index.json");
        env.blob_dir = dir.join("blobs");
        let http = || reqwest::Client::builder().no_proxy();
        (bridge_with(env, http().build().unwrap(), http().http1_only().pool_max_idle_per_host(0).build().unwrap()), dir)
    }

    pub(crate) fn bridge_with(env: Env, http: reqwest::Client, http_fresh: reqwest::Client) -> Arc<Bridge> {
        Arc::new(Bridge {
            env,
            opts: Default::default(),
            st: Mutex::new(crate::tests::blank_state()),
            http,
            http_fresh,
            rt: tokio::runtime::Handle::current(),
            session_id: "test-session".into(),
            session_started_at: iso_now(),
            events: tokio::sync::broadcast::channel(64).0,
            broadcast_pending: AtomicBool::new(false),
            wake_gen: std::sync::atomic::AtomicU64::new(0),
            queue_write: tokio::sync::Mutex::new(()),
            config_write: tokio::sync::Mutex::new(()),
        })
    }

    #[tokio::test]
    async fn disconnect_during_identity_allows_reconnect() {
        use axum::{body::Body, response::Response, routing::{get, post}, Router};
        use tokio::sync::Notify;

        let identity_started = Arc::new(Notify::new());
        let identity_release = Arc::new(Notify::new());
        let reply = |value: Value| {
            Response::builder().header("content-type", "application/json").body(Body::from(value.to_string())).unwrap()
        };
        let app = Router::new()
            .route("/token", post(move || async move {
                reply(json!({ "access_token": "test-access", "refresh_token": "test-refresh", "expires_in": 300 }))
            }))
            .route("/revoke", post(move || async move { reply(json!({})) }))
            .route("/api/me", get({
                let started = identity_started.clone();
                let release = identity_release.clone();
                move || {
                    let started = started.clone();
                    let release = release.clone();
                    async move {
                        started.notify_one();
                        release.notified().await;
                        reply(json!({ "username": "test-player", "user_id": 1, "installation_id": "test-installation" }))
                    }
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let caps = json!({ "issuer": origin, "api_prefix": "/api", "token_endpoint": format!("{origin}/token"), "revocation_endpoint": format!("{origin}/revoke") });
        // No background workers or browser: exercise the same exchange completion
        // task as start_connect, with a server that lets us hold /me in flight.
        let (b, dir) = test_bridge(&origin);
        b.st().caps = Some(caps.clone());

        // First disconnect while /me is pending, then complete another sign-in.
        for disconnect_now in [true, false] {
            let (tx, _rx) = oneshot::channel();
            let attempt = Arc::new(Attempt::new(tx));
            {
                let mut s = b.st();
                assert!(s.connect_attempt.is_none(), "the previous attempt must be cleared");
                s.config.key = Some(create_key());
                s.signing_key = None;
                s.connection.status = "connecting".into();
                s.connect_attempt = Some(attempt.clone());
            }
            let task = {
                let b = b.clone();
                let caps = caps.clone();
                tokio::spawn(async move {
                    finish_exchange(&b, &caps, "test-code", "test-verifier", "http://127.0.0.1/callback", &attempt, ("disconnected".into(), None)).await;
                })
            };
            tokio::time::timeout(Duration::from_secs(5), identity_started.notified()).await.unwrap();
            if disconnect_now {
                tokio::time::timeout(Duration::from_secs(5), disconnect(&b)).await.unwrap();
            }
            identity_release.notify_one();
            tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().expect("sign-in must not panic after disconnect");
            {
                let s = b.st();
                assert!(s.connect_attempt.is_none());
                assert_eq!(s.connection.status, if disconnect_now { "disconnected" } else { "connected" });
                assert_eq!(s.config.connection.is_some(), !disconnect_now);
                assert_eq!(s.access_token.is_some(), !disconnect_now);
            }
            let saved = crate::read_json_file(&b.env.config_file).await.unwrap();
            assert_eq!(saved["connection"].is_null(), disconnect_now);
        }
        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn api_prefers_the_direct_host_and_falls_back_to_the_site() {
        use axum::{http::HeaderMap, routing::get, Router};

        async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            (origin, tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }))
        }
        // answers with who served it and the htu the proof was signed for
        let me = |via: &'static str| {
            get(move |h: HeaderMap| async move {
                let proof = h.get("dpop").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                let claims: Value = proof.split('.').nth(1).and_then(|p| B64.decode(p).ok()).and_then(|p| serde_json::from_slice(&p).ok()).unwrap_or(Value::Null);
                let body = json!({ "via": via, "htu": claims["htu"] }).to_string();
                axum::response::Response::builder().header("content-type", "application/json").body(axum::body::Body::from(body)).unwrap()
            })
        };
        let (site, site_server) = serve(Router::new().route("/api/me", me("site"))).await;
        let (direct, direct_server) = serve(Router::new().route("/v1/me", me("direct"))).await;
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };

        let (b, dir) = test_bridge(&site);
        {
            let mut s = b.st();
            s.config.key = Some(create_key());
            s.access_token = Some("test-access".into());
            s.access_expires_at = now_ms() + 3_600_000;
        }
        let call = |api_base_url: Value| {
            let b = b.clone();
            let site = site.clone();
            async move {
                b.st().caps = Some(json!({ "issuer": site, "api_prefix": "/api", "api_base_url": api_base_url }));
                api(&b, "/me", Req::get()).await.ok().unwrap().json()
            }
        };

        assert_eq!(call(Value::Null).await, json!({ "via": "site", "htu": format!("{site}/api/me") }));
        assert_eq!(call(json!(format!("{direct}/v1/"))).await, json!({ "via": "direct", "htu": format!("{direct}/v1/me") }));

        assert_eq!(call(json!(format!("{closed}/v1"))).await, json!({ "via": "site", "htu": format!("{site}/api/me") }));
        assert!(b.st().direct_down_until > now_ms());
        // stays on the site for a while, even once the direct host is back
        assert_eq!(call(json!(format!("{direct}/v1"))).await["via"], json!("site"));
        b.st().direct_down_until = 0;
        assert_eq!(call(json!(format!("{direct}/v1"))).await["via"], json!("direct"));

        for bad in [json!("ftp://x/v1"), json!("not a url"), json!("https://x/v1?a=b"), json!(5)] {
            assert_eq!(direct_base(&json!({ "api_base_url": bad })), None);
        }
        let origin = |u: &str| backend_origin(&json!({ "api_base_url": u }));
        assert_eq!(origin("https://api.mania-tracker.com/api/integrations/companella/v1"), Some("https://api.mania-tracker.com".into()));
        assert_eq!(origin("http://localhost:7227/api/integrations/companella/v1/"), Some("http://localhost:7227".into()));
        site_server.abort();
        direct_server.abort();
        std::fs::remove_dir_all(dir).ok();
    }

    // node's crypto checks this one: KEY_PEM from node in, a proof out
    #[test]
    fn proof_from_node_key() {
        let Ok(pem) = std::env::var("KEY_PEM") else { return };
        let mut s = crate::tests::blank_state();
        let secret = SigningKey::from_pkcs8_pem(&pem).unwrap();
        let point = secret.verifying_key().to_encoded_point(false);
        let jwk = Jwk { kty: "EC".into(), crv: "P-256".into(), x: b64(point.x().unwrap()), y: b64(point.y().unwrap()) };
        println!("THUMB {}", jwk_thumbprint(&jwk));
        s.config.key = Some(Key { private_key_pem: pem, thumbprint: jwk_thumbprint(&jwk), jwk, created_at: iso_now() });
        let proof = dpop_proof(&mut s, "post", "http://localhost:3000/api/x?y=1", Some("tok"), Some("n1")).ok().unwrap();
        println!("PROOF {proof}");
        let fresh = create_key();
        println!("FRESH {}", fresh.private_key_pem.replace('\n', "|"));
    }
}
