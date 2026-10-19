mod auth;
mod autostart;
mod get_tosu;
mod leaderboard;
mod osr;
mod presence;
mod queue;
mod replays;
mod server;
mod tosu;
mod types;
mod update;

pub use autostart::Autostart;
pub use update::{Progress, Updater};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast;

use types::{Config, Queue};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const APP_NAME: &str = "Hashi";
const CLIENT_ID: &str = "mania-bridge";
const CALLBACK_PATH: &str = "/mania-bridge/callback";
// sites from before /bridge/authorize only take this one
const OLD_CALLBACK_PATH: &str = "/companella/callback";
const PRESENCE_SCOPE: &str = "companella:presence:write";
const SCOPES: [&str; 4] = [
    "companella:scores:submit",
    "companella:submissions:read",
    "companella:charts:upload",
    "companella:installation:read",
];
// dev builds talk to a local mania-hub, TRACKER_ORIGIN picks any other
const DEFAULT_ORIGIN: &str = if cfg!(debug_assertions) { "http://localhost:3000" } else { "https://mania-tracker.com" };
pub const COUNTER_PATH: &str = "/mania-bridge/";
pub const DEFAULT_PORT: u16 = 24071;
pub const DEFAULT_HUE: u16 = 333;
pub const DEFAULT_SAT: u8 = 100;

// stable sometimes writes the .osr minutes after the results screen
const REPLAY_WAIT_MS: i64 = 30 * 60_000;
// filename ticks ~= end of play, imported replays have old ticks
const REPLAY_TICK_TOLERANCE_MS: i64 = 3 * 60_000;

// AT RX AP CN TP
const AUTOMATED_MODS: i64 = 2048 | 128 | 8192 | 4194304 | 8388608;

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {
        $crate::write_log(format!($($t)*))
    };
}

// the app has no console on windows, so it keeps a log file next to the config
static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

#[doc(hidden)]
pub fn write_log(line: String) {
    use std::io::Write;
    let line = format!("[{}] {line}", chrono::Utc::now().format("%H:%M:%S"));
    println!("{line}");
    if let Some(f) = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        let _ = writeln!(f, "{line}");
    }
}

fn open_log_file(dir: &Path) {
    let file = dir.join("bridge.log");
    let big = std::fs::metadata(&file).is_ok_and(|m| m.len() > 1 << 20);
    let f = std::fs::OpenOptions::new().create(true).append(!big).write(true).truncate(big).open(&file);
    if let Ok(f) = f {
        *LOG_FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn iso_now() -> String {
    iso_from_ms(now_ms())
}

pub fn iso_from_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

// 12/24h: linux has it in the time locale, windows in its region settings
fn clock() -> Value {
    static CLOCK: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    CLOCK
        .get_or_init(|| {
            #[cfg(windows)]
            {
                let key = windows_registry::CURRENT_USER.open(r"Control Panel\International");
                let get = |name: &str| key.as_ref().ok().and_then(|k| k.get_string(name).ok());
                json!({ "locale": get("LocaleName"), "hour12": get("sShortTime").map(|f| !f.contains('H')) })
            }
            #[cfg(not(windows))]
            {
                // es_CR.UTF-8@euro is es-CR. C and POSIX say nothing
                let locale = ["LC_ALL", "LC_TIME", "LANG"]
                    .iter()
                    .find_map(|v| var(v))
                    .map(|l| l.split(['.', '@']).next().unwrap_or("").replace('_', "-"))
                    .filter(|l| !l.is_empty() && l != "C" && l != "POSIX");
                json!({ "locale": locale, "hour12": null })
            }
        })
        .clone()
}

fn home() -> PathBuf {
    let v = if cfg!(windows) { var("USERPROFILE") } else { var("HOME") };
    v.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

pub struct Env {
    pub origin: String,
    pub tosu_host: String,
    // only used for the avatar accent (dev backend is on 7227)
    pub live_backend_url: String,
    pub port: u16,
    pub osu_dir: PathBuf,
    pub replay_dir: PathBuf,
    pub songs_dir: PathBuf,
    // lazer keeps everything in files/<a>/<ab>/<sha256>, tosu tells us the real folder once it sees lazer
    pub lazer_dir: PathBuf,
    pub data_dir: PathBuf,
    // where it lived before Hashi, newest first, moved over on first run
    pub old_data_dirs: Vec<PathBuf>,
    pub config_file: PathBuf,
    pub queue_file: PathBuf,
    pub chart_index_file: PathBuf,
    pub blob_dir: PathBuf,
    pub counter_origins: HashSet<String>,
    // only a local tosu can be started, TOSU_AUTOSTART=0 takes the setting away
    pub tosu_can_start: bool,
    pub tosu_path: Option<PathBuf>,
    // asks mania tracker now and then whether there's a newer release, UPDATE_CHECK=0 doesn't
    pub update_check: bool,
}

impl Env {
    pub fn from_env() -> Self {
        let origin = var("TRACKER_ORIGIN").unwrap_or(DEFAULT_ORIGIN.into()).trim_end_matches('/').to_string();
        let tosu_host = var("TOSU_URL").unwrap_or("127.0.0.1:24050".into());
        let local = regex_local(&origin);
        let live_backend_url = var("LIVE_BACKEND_URL")
            .unwrap_or_else(|| if local { "http://127.0.0.1:7227".into() } else { String::new() })
            .trim_end_matches('/')
            .to_string();
        let port: u16 = var("BRIDGE_PORT").and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT);
        let home = home();
        let osu_dir = var("OSU_DIR").map(PathBuf::from).unwrap_or_else(|| {
            if cfg!(windows) {
                var("LOCALAPPDATA").map(PathBuf::from).unwrap_or(home.join("AppData").join("Local")).join("osu!")
            } else {
                // osu-winello asks where to install and writes it down
                let data = var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or(home.join(".local").join("share"));
                std::fs::read_to_string(data.join("osuconfig").join("osupath"))
                    .ok()
                    .map(|p| PathBuf::from(p.trim()))
                    .filter(|p| p.is_dir())
                    .unwrap_or(home.join(".local/share/osu-wine/osu!"))
            }
        });
        let lazer_dir = var("LAZER_DIR").map(PathBuf::from).unwrap_or_else(|| {
            if cfg!(windows) {
                var("APPDATA").map(PathBuf::from).unwrap_or(home.join("AppData").join("Roaming")).join("osu")
            } else {
                var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or(home.join(".local").join("share")).join("osu")
            }
        });
        let config_root = if cfg!(windows) {
            var("APPDATA").map(PathBuf::from).unwrap_or(home.join("AppData").join("Roaming"))
        } else {
            var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or(home.join(".config"))
        };
        let data_dir = var("BRIDGE_DATA_DIR").map(PathBuf::from).unwrap_or(config_root.join(if cfg!(windows) { "Hashi" } else { "hashi" }));
        let old_data_dirs = match var("BRIDGE_DATA_DIR") {
            Some(_) => vec![],
            None => vec![config_root.join("mania-bridge"), config_root.join("mania-bridge-test")],
        };
        let counter_origins = HashSet::from([
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ]);
        let tosu_local = tosu_host.starts_with("127.0.0.1") || tosu_host.starts_with("localhost");
        Env {
            replay_dir: osu_dir.join("Data").join("r"),
            songs_dir: var("OSU_SONGS_DIR").map(PathBuf::from).unwrap_or(osu_dir.join("Songs")),
            osu_dir,
            lazer_dir,
            config_file: PathBuf::new(),
            queue_file: PathBuf::new(),
            chart_index_file: PathBuf::new(),
            blob_dir: PathBuf::new(),
            data_dir: PathBuf::new(),
            old_data_dirs,
            origin,
            tosu_host,
            live_backend_url,
            port,
            counter_origins,
            tosu_can_start: var("TOSU_AUTOSTART").as_deref() != Some("0") && tosu_local,
            tosu_path: var("TOSU_PATH").map(PathBuf::from),
            update_check: var("UPDATE_CHECK").as_deref() != Some("0"),
        }
        .with_data_dir(data_dir)
    }

    fn with_data_dir(mut self, dir: PathBuf) -> Self {
        self.config_file = dir.join("config.json");
        self.queue_file = dir.join("queue.json");
        self.chart_index_file = dir.join("chart-index.json");
        self.blob_dir = dir.join("blobs");
        self.data_dir = dir;
        self
    }

    pub fn counter_url(&self) -> String {
        format!("http://127.0.0.1:{}{COUNTER_PATH}", self.port)
    }
}

fn regex_local(origin: &str) -> bool {
    let rest = match origin.strip_prefix("http://") {
        Some(r) => r,
        None => return false,
    };
    let host = rest.split(':').next().unwrap_or("");
    let port_ok = rest.split_once(':').map_or(true, |(_, p)| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    (host == "localhost" || host == "127.0.0.1") && port_ok && !rest.contains('/')
}

// what the window wants from the helper: hide | minimize | close | drag | on-top on|off
pub type WindowHandler = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
pub struct Options {
    // the counter shows "hide to tray" when there is one
    pub tray: bool,
    // the counter draws its own title bar (windows, no system frame)
    pub titlebar: bool,
    // None: the helper on its own, there's no window for those buttons
    pub on_window: Option<WindowHandler>,
    // also write the log to bridge.log in the data folder
    pub log_file: bool,
    // the app installs updates itself, None: the counter links to the download page
    pub updater: Option<Arc<dyn Updater>>,
    // the app can open when you log in, None: no such setting
    pub autostart: Option<Arc<dyn Autostart>>,
}

// disconnected | connecting | connected | not_in_beta | revoked | error | offline | disabled
pub(crate) struct ConnState {
    pub status: String,
    pub message: Option<String>,
    pub authorize_url: Option<String>,
}

#[derive(Default, Clone)]
pub(crate) struct Profile {
    pub user_id: Value,
    pub avatar_url: Option<String>,
    pub accent: Option<String>,
}

pub(crate) struct State {
    pub config: Config,
    pub queue: Queue,
    pub caps: Option<Value>,
    pub dpop_nonce: Option<String>,
    pub direct_down_until: i64,
    // an upload stalled this session: some networks stop a connection after its first 16 KB
    pub stalled: bool,
    pub access_token: Option<String>,
    pub access_expires_at: i64,
    pub signing_key: Option<p256::ecdsa::SigningKey>,
    pub connection: ConnState,
    pub connect_attempt: Option<Arc<auth::Attempt>>,
    pub profile: Profile,
    pub tosu: tosu::TosuState,
    pub replays: replays::Replays,
    pub chart_index: Option<HashMap<String, replays::ChartEntry>>,
    pub pump: queue::PumpState,
    pub presence: presence::PresenceState,
    pub boards: leaderboard::Boards,
    pub update: update::UpdateState,
    pub get_tosu: get_tosu::GetState,
    pub autostart: Option<bool>,
}

impl State {
    pub(crate) fn sends_no_plays(&self) -> bool {
        self.config.settings.as_ref().is_some_and(|x| x.presence_only)
    }
}

pub struct Bridge {
    pub env: Env,
    pub(crate) opts: Options,
    st: Mutex<State>,
    http: reqwest::Client,
    // nothing kept open, every request dials again
    http_fresh: reqwest::Client,
    rt: tokio::runtime::Handle,
    session_id: String,
    session_started_at: String,
    events: broadcast::Sender<Arc<str>>,
    broadcast_pending: AtomicBool,
    wake_gen: AtomicU64,
    queue_write: tokio::sync::Mutex<()>,
    config_write: tokio::sync::Mutex<()>,
}

impl Bridge {
    pub(crate) fn client(&self) -> &reqwest::Client {
        if self.st().stalled {
            &self.http_fresh
        } else {
            &self.http
        }
    }

    pub(crate) fn st(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(&self, f: F) {
        self.rt.spawn(f);
    }

    pub(crate) fn broadcast(self: &Arc<Self>) {
        if self.broadcast_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let b = self.clone();
        self.spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            b.broadcast_pending.store(false, Ordering::SeqCst);
            let data = format!("data: {}\n\n", b.snapshot());
            let _ = b.events.send(data.into());
        });
    }

    pub(crate) fn snapshot(&self) -> Value {
        let theme = self.theme();
        let autostart = autostart::public(self);
        let s = self.st();
        let profile = s.config.connection.as_ref().map(|c| crate::types::text(&c.username)).filter(|u| !u.is_empty() && u != "null");
        let items: Vec<&types::Item> = {
            let mut v: Vec<&types::Item> = s
                .queue
                .items
                .iter()
                .filter(|i| i.session_id == self.session_id || ["waiting_replay", "send", "poll", "held"].contains(&i.phase.as_str()))
                .collect();
            v.sort_by(|a, b| b.captured_at.cmp(&a.captured_at));
            v
        };
        json!({
            "version": VERSION,
            "sessionStartedAt": self.session_started_at,
            "connection": auth::connection_summary(&self.env, &s),
            "tray": self.opts.tray,
            "titlebar": self.opts.titlebar,
            "clock": clock(),
            "tosu": s.tosu.public(),
            "presence": {
                "enabled": s.config.settings.as_ref().is_some_and(|x| x.presence),
                "status": s.presence.status,
                "message": s.presence.message,
            },
            "theme": { "hue": theme.0, "sat": theme.1 },
            "update": s.update.public(update::can_install(self)),
            "getTosu": get_tosu::public(self, &s),
            "autostart": autostart,
            "tosuAutostart": self.env.tosu_can_start.then(|| s.config.settings.as_ref().is_some_and(|x| x.tosu_autostart)),
            "startInTray": self.opts.tray.then(|| s.config.settings.as_ref().is_some_and(|x| x.start_in_tray)),
            "onTop": self.opts.on_window.is_some().then(|| s.config.settings.as_ref().is_some_and(|x| x.on_top)),
            "sendPlays": !s.sends_no_plays(),
            "tosuDashboard": self.env.tosu_can_start.then_some(s.tosu.dashboard).flatten(),
            "showDans": !s.config.settings.as_ref().is_some_and(|x| x.hide_dans),
            "unreadNotes": s.config.settings.as_ref().and_then(|x| x.notes_read.as_deref()) != Some(VERSION),
            "plays": items.into_iter().map(|i| queue::public_item(i, &self.env.origin, profile.as_deref())).collect::<Vec<_>>(),
        })
    }

    // where tosu says stable is, it can be installed anywhere
    pub(crate) fn replay_dir(&self) -> PathBuf {
        self.st().tosu.stable_dirs.as_ref().map(|(game, _)| game.join("Data").join("r")).unwrap_or_else(|| self.env.replay_dir.clone())
    }

    pub(crate) fn songs_dir(&self) -> PathBuf {
        self.st().tosu.stable_dirs.as_ref().map(|(_, songs)| songs.clone()).unwrap_or_else(|| self.env.songs_dir.clone())
    }

    pub fn theme(&self) -> (u16, u8) {
        let s = self.st();
        let set = s.config.settings.as_ref();
        (
            set.and_then(|x| x.theme_hue).filter(|h| *h < 360).unwrap_or(DEFAULT_HUE),
            set.and_then(|x| x.theme_sat).filter(|v| *v <= 100).unwrap_or(DEFAULT_SAT),
        )
    }

    pub(crate) async fn set_theme(self: &Arc<Self>, hue: u16, sat: u8) -> bool {
        {
            let mut s = self.st();
            let set = s.config.settings.get_or_insert_with(Default::default);
            set.theme_hue = (hue != DEFAULT_HUE).then_some(hue);
            set.theme_sat = (sat != DEFAULT_SAT).then_some(sat);
        }
        let saved = self.save_config().await;
        self.broadcast();
        saved
    }

    pub(crate) async fn set_send_plays(self: &Arc<Self>, on: bool) -> bool {
        self.st().config.settings.get_or_insert_with(Default::default).presence_only = !on;
        let saved = self.save_config().await;
        log!("sending plays {}", if on { "on" } else { "off" });
        self.broadcast();
        if on {
            queue::wake_queue(self, 0);
        }
        saved
    }

    pub(crate) async fn set_show_dans(self: &Arc<Self>, on: bool) -> bool {
        self.st().config.settings.get_or_insert_with(Default::default).hide_dans = !on;
        let saved = self.save_config().await;
        self.broadcast();
        saved
    }

    pub(crate) async fn read_notes(self: &Arc<Self>) -> bool {
        self.st().config.settings.get_or_insert_with(Default::default).notes_read = Some(VERSION.into());
        let saved = self.save_config().await;
        self.broadcast();
        saved
    }

    pub fn starts_in_tray(&self) -> bool {
        self.opts.tray && self.st().config.settings.as_ref().is_some_and(|x| x.start_in_tray)
    }

    pub(crate) async fn set_start_in_tray(self: &Arc<Self>, on: bool) -> bool {
        self.st().config.settings.get_or_insert_with(Default::default).start_in_tray = on;
        let saved = self.save_config().await;
        log!("starting in the tray {}", if on { "on" } else { "off" });
        self.broadcast();
        saved
    }

    pub fn on_top(&self) -> bool {
        self.st().config.settings.as_ref().is_some_and(|x| x.on_top)
    }

    pub(crate) async fn set_on_top(self: &Arc<Self>, on: bool) -> bool {
        self.st().config.settings.get_or_insert_with(Default::default).on_top = on;
        self.window_action(if on { "on-top on" } else { "on-top off" });
        let saved = self.save_config().await;
        self.broadcast();
        saved
    }

    // false if it couldn't be written (logged)
    pub(crate) async fn save_config(&self) -> bool {
        let _turn = self.config_write.lock().await;
        let data = serde_json::to_value(&self.st().config).unwrap_or(Value::Null);
        match write_json_file(&self.env.config_file, &data).await {
            Ok(()) => true,
            Err(e) => {
                log!("config save failed: {e}");
                false
            }
        }
    }

    pub(crate) async fn save_queue(&self) {
        let _turn = self.queue_write.lock().await;
        let data = self.st().queue.to_value();
        if let Err(e) = write_json_file(&self.env.queue_file, &data).await {
            log!("queue save failed: {e}");
        }
    }

    pub(crate) fn save_queue_later(self: &Arc<Self>) {
        let b = self.clone();
        self.spawn(async move { b.save_queue().await });
    }

    pub(crate) fn open_browser(&self, url: &str) {
        open_url(url);
    }

    pub(crate) fn window_action(&self, name: &str) {
        if let Some(handler) = &self.opts.on_window {
            handler(name);
        }
    }

    pub async fn shutdown(self: &Arc<Self>) {
        tosu::stop_tosu(self).await;
        if self.st().presence.sent.is_some() {
            let _ = tokio::time::timeout(Duration::from_millis(1500), presence::clear_presence(self)).await;
        }
        let _turn = self.queue_write.lock().await;
    }
}

// cap for files read whole from folders anything can write to
pub(crate) const MAX_REPLAY_BYTES: u64 = 64 << 20;
pub(crate) const MAX_CHART_BYTES: u64 = 16 << 20;
pub(crate) const MAX_IMAGE_BYTES: u64 = 32 << 20;

fn too_big(file: &Path, max: u64) -> std::io::Error {
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    std::io::Error::other(format!("{name} is bigger than {} MB.", max >> 20))
}

pub(crate) async fn read_capped(file: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let f = tokio::fs::File::open(file).await?;
    let len = f.metadata().await?.len();
    if len > max {
        return Err(too_big(file, max));
    }
    let mut bytes = Vec::with_capacity(len as usize);
    f.take(max + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > max {
        return Err(too_big(file, max));
    }
    Ok(bytes)
}

pub(crate) fn read_capped_sync(file: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(file)?;
    let len = f.metadata()?.len();
    if len > max {
        return Err(too_big(file, max));
    }
    let mut bytes = Vec::with_capacity(len as usize);
    f.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(too_big(file, max));
    }
    Ok(bytes)
}

pub fn open_url(url: &str) {
    if let Err(e) = open::that_detached(url) {
        log!("could not open the browser: {e}");
    }
}

pub(crate) async fn read_json_file(file: &Path) -> Option<Value> {
    let text = tokio::fs::read_to_string(file).await.ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) async fn write_json_file(file: &Path, data: &Value) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        create_private_dir(dir).await?;
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = file.with_extension(format!("json.{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let text = serde_json::to_string_pretty(data).map_err(std::io::Error::other)?;
    let result = match write_private(&tmp, text.as_bytes()).await {
        Ok(()) => tokio::fs::rename(&tmp, file).await,
        Err(e) => Err(e),
    };
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

pub(crate) async fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = tokio::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    b.mode(0o700);
    b.create(dir).await
}

pub(crate) async fn write_private(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut o = tokio::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    o.mode(0o600);
    let mut f = o.open(file).await?;
    f.write_all(bytes).await?;
    // on disk before it's renamed over the old one, or a crash can leave an empty file
    f.sync_all().await
}

// a file that's there but unreadable is set aside, not written over with an empty one
async fn keep_broken(file: &Path) {
    if tokio::fs::metadata(file).await.is_ok_and(|m| m.len() > 0) {
        let to = file.with_extension(format!("broken-{}.json", now_ms()));
        log!("{} can't be read, kept as {}", file.display(), to.display());
        let _ = tokio::fs::rename(file, &to).await;
    }
}

pub async fn start(opts: Options) -> Result<Arc<Bridge>, String> {
    let mut env = Env::from_env();

    let mut not_moved = None;
    let old = env.old_data_dirs.iter().find(|d| d.exists()).cloned();
    if let Some(old) = old.filter(|_| !env.data_dir.exists()) {
        if let Err(e) = tokio::fs::rename(&old, &env.data_dir).await {
            // windows won't move it while something in it is open, e.g. a tosu it downloaded. next start tries again
            not_moved = Some(format!("could not move {} to {}: {e}", old.display(), env.data_dir.display()));
            env = env.with_data_dir(old);
        }
    }
    create_private_dir(&env.data_dir).await.map_err(|e| format!("cannot create {}: {e}", env.data_dir.display()))?;
    if opts.log_file {
        open_log_file(&env.data_dir);
    }
    if let Some(why) = not_moved {
        log!("{why}");
    }
    let config: Config = match read_json_file(&env.config_file).await.map(serde_json::from_value) {
        Some(Ok(c)) => c,
        Some(Err(e)) => {
            log!("config.json can't be read ({e}), starting over");
            keep_broken(&env.config_file).await;
            Config::default()
        }
        None => {
            keep_broken(&env.config_file).await;
            // a first start, there's nothing new to it yet
            Config { settings: Some(types::Settings { notes_read: Some(VERSION.into()), ..Default::default() }), ..Default::default() }
        }
    };
    let queue = read_json_file(&env.queue_file).await;
    if queue.is_none() {
        keep_broken(&env.queue_file).await;
    }
    let mut queue = queue.map(Queue::from_value).unwrap_or_default();

    // can't match these after a restart
    for item in &mut queue.items {
        if item.phase == "waiting_replay" {
            item.phase = "failed".into();
            item.error = types::ItemError::new("no_replay", "The helper stopped before the replay was saved.");
        }
        if item.phase == "send" || item.phase == "poll" {
            item.next_attempt_at = 0;
        }
    }

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("mania-bridge/{VERSION}"))
        .build()
        .map_err(|e| e.to_string())?;
    let http_fresh = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(format!("mania-bridge/{VERSION}"))
        .http1_only()
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|e| e.to_string())?;

    // after an update the old copy can still be letting go of the port
    let mut tries = 0;
    let listener = loop {
        match tokio::net::TcpListener::bind(("127.0.0.1", env.port)).await {
            Ok(l) => break l,
            Err(e) if tries < 20 && e.kind() == std::io::ErrorKind::AddrInUse => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(e) => return Err(format!("cannot listen on 127.0.0.1:{}: {e}", env.port)),
        }
    };

    let lazer_files = env.lazer_dir.join("files");
    let bridge = Arc::new(Bridge {
        opts,
        st: Mutex::new(State {
            config,
            queue,
            caps: None,
            dpop_nonce: None,
            direct_down_until: 0,
            stalled: false,
            access_token: None,
            access_expires_at: 0,
            signing_key: None,
            connection: ConnState { status: "disconnected".into(), message: None, authorize_url: None },
            connect_attempt: None,
            profile: Profile::default(),
            tosu: tosu::TosuState::new(lazer_files),
            replays: replays::Replays::default(),
            chart_index: None,
            pump: queue::PumpState::default(),
            presence: presence::PresenceState::default(),
            boards: Default::default(),
            update: update::UpdateState::default(),
            get_tosu: get_tosu::GetState::default(),
            autostart: None,
        }),
        http,
        http_fresh,
        rt: tokio::runtime::Handle::current(),
        session_id: uuid::Uuid::new_v4().to_string(),
        session_started_at: iso_now(),
        events: broadcast::channel(64).0,
        broadcast_pending: AtomicBool::new(false),
        wake_gen: AtomicU64::new(0),
        queue_write: tokio::sync::Mutex::new(()),
        config_write: tokio::sync::Mutex::new(()),
        env,
    });

    queue::prune_queue(&bridge).await;
    bridge.save_queue().await;

    log!("mania-bridge {VERSION}");
    log!(
        "server {}, osu! at {}, data in {}",
        bridge.env.origin,
        bridge.env.osu_dir.display(),
        bridge.env.data_dir.display()
    );
    server::serve(&bridge, listener);
    replays::watch_replay_dir(&bridge);
    tosu::connect_tosu(&bridge);
    tosu::check_tosu_launch(&bridge);
    tosu::watch_osu_frames(&bridge);
    server::watch_counter(&bridge);
    update::watch_updates(&bridge);
    autostart::load(&bridge);

    let b = bridge.clone();
    bridge.spawn(async move {
        let has_token = b.st().config.connection.as_ref().is_some_and(|c| c.refresh_token.is_some());
        if has_token {
            auth::set_connection(&b, "offline", Some("Checking the connection.".into()));
            auth::check_connection(&b).await;
        } else if let Err(e) = auth::load_capabilities(&b).await {
            log!("capabilities: {e}");
        }

        let mut minute = tokio::time::interval(Duration::from_secs(60));
        let mut five = tokio::time::interval(Duration::from_secs(5));
        let mut tick = tokio::time::interval(Duration::from_millis(presence::PRESENCE_TICK_MS));
        // not every tick missed while the computer slept, all at once
        for t in [&mut minute, &mut five, &mut tick] {
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        }
        minute.tick().await;
        let mut minutes = 0u32;
        loop {
            tokio::select! {
                _ = minute.tick() => {
                    minutes += 1;
                    // it can stay up for weeks in the tray
                    if minutes % (24 * 60) == 0 {
                        let b = b.clone();
                        b.clone().spawn(async move {
                            if queue::prune_queue(&b).await {
                                b.save_queue().await;
                                b.broadcast();
                            }
                        });
                    }
                    // picks up a new api_base_url without a restart, keeps the old ones if the site is down
                    if minutes % 60 == 0 {
                        let b = b.clone();
                        b.clone().spawn(async move {
                            if let Err(e) = auth::load_capabilities(&b).await {
                                log!("capabilities: {e}");
                            }
                        });
                    }
                    let status = b.st().connection.status.clone();
                    if status == "offline" || status == "not_in_beta" {
                        let b = b.clone();
                        b.clone().spawn(async move { auth::check_connection(&b).await });
                    } else if status == "connected" {
                        queue::wake_queue(&b, 0);
                    }
                    tosu::recheck_found(&b);
                    tosu::refresh_dashboard(&b);
                }
                _ = five.tick() => {
                    let b = b.clone();
                    b.clone().spawn(async move { replays::match_replays(&b).await });
                }
                _ = tick.tick() => {
                    let b = b.clone();
                    b.clone().spawn(async move { presence::presence_tick(&b).await });
                }
            }
        }
    });

    Ok(bridge)
}

#[cfg(test)]
pub(crate) mod tests {
    #[tokio::test]
    async fn capped_reads() {
        let dir = std::env::temp_dir().join(format!("mania-bridge-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.osr");
        std::fs::write(&file, vec![7u8; 100]).unwrap();
        assert_eq!(crate::read_capped(&file, 100).await.unwrap().len(), 100);
        assert!(crate::read_capped(&file, 99).await.is_err());
        assert_eq!(crate::read_capped_sync(&file, 100).unwrap().len(), 100);
        assert!(crate::read_capped_sync(&file, 99).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn json_writes_at_once() {
        let dir = std::env::temp_dir().join(format!("mania-bridge-test-{}", uuid::Uuid::new_v4()));
        let file = dir.join("config.json");
        let writes: Vec<_> = (0..50)
            .map(|i| {
                let file = file.clone();
                tokio::spawn(async move { crate::write_json_file(&file, &serde_json::json!({ "n": i })).await })
            })
            .collect();
        for w in writes {
            w.await.unwrap().unwrap();
        }
        assert!(crate::read_json_file(&file).await.is_some());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    pub fn blank_state() -> crate::State {
        crate::State {
            config: Default::default(),
            queue: Default::default(),
            caps: None,
            dpop_nonce: None,
            direct_down_until: 0,
            stalled: false,
            access_token: None,
            access_expires_at: 0,
            signing_key: None,
            connection: crate::ConnState { status: "disconnected".into(), message: None, authorize_url: None },
            connect_attempt: None,
            profile: Default::default(),
            tosu: crate::tosu::TosuState::new(Default::default()),
            replays: Default::default(),
            chart_index: None,
            pump: Default::default(),
            presence: Default::default(),
            boards: Default::default(),
            update: Default::default(),
            get_tosu: Default::default(),
            autostart: None,
        }
    }
}
