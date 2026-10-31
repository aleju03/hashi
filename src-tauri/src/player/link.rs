use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mania_bridge::player::{ChartNote, Song, Sprite};
use serde_json::{json, Value};
use winit::event_loop::EventLoopProxy;

#[derive(Clone, PartialEq)]
pub struct Live {
    pub keys: usize,
    pub at: String,
    pub edits: String,
    pub source: String,
    pub play: Option<String>,
    pub from: Option<f64>,
    // scroll speed, 1 to 40
    pub speed: f64,
    pub playing: bool,
    // the map selected in osu! and its mods
    pub selected: Option<String>,
}

impl Live {
    fn parse(v: &Value) -> Option<Live> {
        let keys = v.get("keys")?.as_u64()? as usize;
        let text = |k: &str| v.get(k).map(|x| x.as_str().map_or_else(|| x.to_string(), String::from)).unwrap_or_default();
        Some(Live {
            keys,
            at: text("at"),
            edits: text("edits"),
            source: text("source"),
            play: v.get("play").and_then(Value::as_str).map(String::from),
            from: v.get("from").and_then(Value::as_f64),
            speed: v.get("speed").and_then(Value::as_f64).unwrap_or(24.0),
            playing: v.get("playing").and_then(Value::as_bool).unwrap_or(true),
            selected: v.get("selected").and_then(Value::as_str).map(String::from),
        })
    }
}

pub struct Map {
    pub id: String,
    pub keys: usize,
    pub od: f64,
    // in the order the judge goes by
    pub notes: Arc<Vec<ChartNote>>,
    pub song: Option<PathBuf>,
    pub lazer: bool,
    // the ones that change judging
    pub mods: Vec<String>,
}

pub struct Scene {
    pub json: Value,
    pub sprites: Vec<Sprite>,
}

pub enum News {
    Live(Live),
    Scene(Box<Scene>),
    // None: generated notes
    Map(Option<Arc<Map>>),
    // None: no song, or another one coming. Tagged with the file, as one decoded for an earlier map can
    // still come in after the next map
    Song(Option<(PathBuf, Arc<Song>)>),
    SongFailed(PathBuf),
    // Hashi quit
    Quit,
}

pub enum Out {
    Pause,
    Seek(f64),
}

// one request on a fresh connection; the helper answers with a length, never in chunks
pub fn request(port: u16, method: &str, path: &str) -> Option<(u16, Vec<u8>)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut all = vec![];
    s.read_to_end(&mut all).ok()?;
    let split = all.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&all[..split]);
    let status = head.split(' ').nth(1)?.parse().ok()?;
    Some((status, all[split + 4..].to_vec()))
}

fn get_json(port: u16, path: &str) -> Option<Value> {
    let (status, body) = request(port, "GET", path)?;
    (status == 200).then(|| serde_json::from_slice(&body).ok()).flatten()
}

fn url_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// where this window is in the map, for the page's seek bar
pub type Shared = Arc<Mutex<Option<f64>>>;

pub fn start(port: u16, aspect: f64, proxy: EventLoopProxy<News>, shared: Shared) -> Sender<Out> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || follow(port, aspect, proxy, shared, rx));
    tx
}

fn status(port: u16, v: Value) {
    let _ = request(port, "POST", &format!("/skin/player?state={}", url_escape(&v.to_string())));
}

// one song decoded at a time, skipping to the newest asked for
fn decoder(proxy: EventLoopProxy<News>) -> Sender<PathBuf> {
    let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
    std::thread::spawn(move || {
        while let Ok(mut file) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                file = newer;
            }
            let news = match mania_bridge::player::song(&file) {
                Ok(song) => News::Song(Some((file, Arc::new(song)))),
                Err(e) => {
                    eprintln!("song: {e}");
                    News::SongFailed(file)
                }
            };
            if proxy.send_event(news).is_err() {
                return;
            }
        }
    });
    tx
}

fn follow(port: u16, aspect: f64, proxy: EventLoopProxy<News>, shared: Shared, rx: Receiver<Out>) {
    let decode = decoder(proxy.clone());
    let mut last: Option<Live> = None;
    let mut told = Instant::now() - Duration::from_secs(10);
    let mut song_for: Option<PathBuf> = None;
    let mut heard = Instant::now();
    loop {
        loop {
            match rx.try_recv() {
                Ok(Out::Pause) => status(port, json!({ "open": true, "pause": true })),
                Ok(Out::Seek(t)) => status(port, json!({ "open": true, "seek": t })),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    status(port, json!({ "open": false }));
                    return;
                }
            }
        }
        if told.elapsed() >= Duration::from_millis(500) {
            let at = *shared.lock().unwrap_or_else(|e| e.into_inner());
            status(port, json!({ "open": true, "at": at }));
            told = Instant::now();
        }
        let reply = get_json(port, "/skin/live");
        // no answer for a while: Hashi quit
        if reply.is_some() {
            heard = Instant::now();
        } else if heard.elapsed() > Duration::from_secs(5) {
            // the window's own exit stops its muting first; this is for one that's stuck
            if proxy.send_event(News::Quit).is_ok() {
                std::thread::sleep(Duration::from_secs(5));
            }
            super::hush::restore();
            std::process::exit(0);
        }
        let live = reply.as_ref().and_then(Live::parse);
        if let Some(live) = live.filter(|l| last.as_ref() != Some(l)) {
            let was = last.clone();
            let skin_changed = was.as_ref().is_none_or(|w| w.keys != live.keys || w.at != live.at || w.edits != live.edits);
            let notes_changed = was.as_ref().is_none_or(|w| {
                w.keys != live.keys || w.source != live.source || w.play != live.play || (live.source == "selected" && w.selected != live.selected)
            });
            // before the map and the scene, whose handling reads it
            if proxy.send_event(News::Live(live.clone())).is_err() {
                return;
            }
            if notes_changed {
                let map = map(port, &live).map(Arc::new);
                let song = map.as_ref().and_then(|m| m.song.clone());
                let new_song = song != song_for;
                if new_song {
                    song_for = song.clone();
                    let _ = proxy.send_event(News::Song(None));
                }
                // the map before its song, which is only taken when it's the map's
                let _ = proxy.send_event(News::Map(map));
                if let Some(file) = song.filter(|_| new_song) {
                    let _ = decode.send(file);
                }
            }
            if skin_changed {
                if let Some(scene) = scene(port, &live, aspect) {
                    let _ = proxy.send_event(News::Scene(Box::new(scene)));
                }
            }
            last = Some(live);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn scene(port: u16, live: &Live, aspect: f64) -> Option<Scene> {
    let info = get_json(port, "/skin")?;
    let dir = PathBuf::from(info.get("folder")?.as_str()?);
    match mania_bridge::player::scene(&dir, live.keys, &live.at, &live.edits, Some(aspect)) {
        Ok((json, sprites)) => Some(Scene { json, sprites }),
        Err(e) => {
            eprintln!("skin: {e}");
            None
        }
    }
}

fn map(port: u16, live: &Live) -> Option<Map> {
    let path = match (live.source.as_str(), &live.play) {
        ("selected", _) => "/skin/chart".to_string(),
        ("play", Some(id)) => format!("/skin/chart?play={}", url_escape(id)),
        _ => return None,
    };
    let v = get_json(port, &path)?;
    let keys = v.get("keys")?.as_u64()? as usize;
    if keys != live.keys {
        return None;
    }
    let notes = v
        .get("notes")?
        .as_array()?
        .iter()
        .filter_map(|n| Some((n.get(0)?.as_u64()? as usize, n.get(1)?.as_f64()?, n.get(2).and_then(Value::as_f64))))
        .collect::<Vec<_>>();
    (!notes.is_empty()).then(|| Map {
        id: v.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
        keys,
        od: v.get("od").and_then(Value::as_f64).unwrap_or(8.0),
        notes: Arc::new(notes),
        song: v.get("song").and_then(Value::as_str).map(PathBuf::from),
        lazer: v.get("client").and_then(Value::as_str) == Some("lazer"),
        mods: v.get("mods").and_then(Value::as_array).map(|m| m.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
    })
}
