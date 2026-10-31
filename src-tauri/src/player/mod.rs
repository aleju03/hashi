mod chart;
mod draw;
mod gpu;
pub mod hush;
mod link;
mod panel;
mod settings;
mod song;
mod text;

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mania_bridge::player::{ChartNote, Judged, Press, MISS};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use chart::{Made, MapNotes};
use draw::{Hold, Overlay, Pen, Pop, SeekBar, Stage, Tap};
use gpu::{Frame, Gpu};
use link::{Live, Map, News, Out};
use panel::{Action, Binding, Hit, Look, Painter};
use settings::{Fps, Game};

pub const MODE: &str = "--skin-play";

// a map starts this long before its first note, so the notes scroll in
const LEAD_MS: f64 = 1500.0;
// how long a column stays down after an autoplayed tap reaches the line
const TAP_PRESS_MS: f64 = 90.0;
const LIGHTING_MS: f64 = 120.0;
// a press this close to a hold's head holds it on the line before the judge has answered
const PRESS_NEAR_MS: f64 = 150.0;
// the seek bar and the pointer stay this long after the mouse stops
const STILL: Duration = Duration::from_millis(1500);
const FADE_MS: f64 = 200.0;
// judged again this often even without presses, so unpressed notes turn into misses
const JUDGE_EVERY: Duration = Duration::from_millis(30);

struct Args {
    port: u16,
    // None: the whole screen
    size: Option<(u32, u32)>,
    // the main window's middle, in logical pixels
    near: Option<(f64, f64)>,
    // draw one frame into this png instead, without a window
    shot: Option<String>,
    keys: usize,
    // Hashi's colour, for the seek bar
    accent: [f64; 3],
}

fn parse(args: &[String]) -> Args {
    let value = |name: &str| args.iter().find_map(|a| a.strip_prefix(&format!("--{name}=")).map(str::to_string));
    fn pair<T: std::str::FromStr>(v: Option<String>, sep: char) -> Option<(T, T)> {
        v.and_then(|v| v.split_once(sep).and_then(|(a, b)| Some((a.trim().parse().ok()?, b.trim().parse().ok()?))))
    }
    Args {
        port: value("bridge").and_then(|p| p.parse().ok()).unwrap_or(mania_bridge::DEFAULT_PORT),
        size: pair(value("size"), 'x'),
        near: pair(value("near"), ','),
        shot: value("shot"),
        keys: value("keys").and_then(|k| k.parse().ok()).unwrap_or(4),
        accent: pair::<f64>(value("theme"), ',').map_or([1.0, 0.4, 0.67], |(h, s)| hsl(h, s / 100.0, 0.7)),
    }
}

// the page's --osu-pink: hsl(hue, 100% * sat, 70%)
fn hsl(h: f64, s: f64, l: f64) -> [f64; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}

// native rather than a webview, which is held to 60 fps on Linux and needs GStreamer plugins for sound
pub fn run(args: &[String]) {
    let args = parse(args);
    if let Some(out) = args.shot.clone() {
        std::process::exit(match shot(&args, &out) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        });
    }
    let event_loop = match EventLoop::<News>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => return eprintln!("no window: {e}"),
    };
    let proxy = event_loop.create_proxy();
    let mut app = App::new(args, proxy);
    let _ = event_loop.run_app(&mut app);
    app.hush.stop();
    hush::restore();
}

enum Notes {
    None,
    Map(MapNotes),
    Made(Made),
}

#[derive(Default)]
struct Run {
    id: u64,
    presses: Vec<Press>,
    open: Vec<Option<usize>>,
    gone: HashSet<usize>,
    // judged all the way, so drawn no more
    done: HashSet<usize>,
    // a hold let go of stays on the line until the judge has caught up, or its head would drop under
    // the line meanwhile
    released: Vec<f64>,
    judged_to: f64,
    seen: HashSet<(usize, u8, u64)>,
    lit: Vec<f64>,
    pop: Option<(u8, f64)>,
    combo: u32,
    combo_at: f64,
    acc: f64,
    // the heads hit as (time, how late), and from when they count towards the offset
    heads: Vec<(f64, f64)>,
    late_from: f64,
    dirty: bool,
    sent: Option<Instant>,
}

struct Job {
    id: u64,
    chart: Arc<Vec<ChartNote>>,
    keys: usize,
    od: f64,
    lazer: bool,
    mods: Vec<String>,
    from: f64,
    now: f64,
    presses: Vec<Press>,
}

fn judge_worker() -> (Sender<Job>, Receiver<(u64, f64, Judged)>) {
    let (jobs, rx) = std::sync::mpsc::channel::<Job>();
    let (tx, results) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(mut job) = rx.recv() {
            // only the newest is worth judging
            while let Ok(newer) = rx.try_recv() {
                job = newer;
            }
            let j = mania_bridge::player::judge(&job.chart, job.keys, job.od, job.lazer, &job.mods, job.from, job.now, &job.presses);
            if tx.send((job.id, job.now, j)).is_err() {
                return;
            }
        }
    });
    (jobs, results)
}

struct App {
    args: Args,
    proxy: Option<EventLoopProxy<News>>,
    link: Option<Sender<Out>>,
    shared: link::Shared,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    text: text::Text,
    stage: Option<Stage>,
    live: Option<Live>,
    map: Option<Arc<Map>>,
    notes: Notes,
    // map ms for a map, the preview's own for made-up notes, as of clock_at
    clock: f64,
    clock_at: Instant,
    playing: bool,
    // where the map was seeked to: notes before it aren't played
    from: f64,
    run: Run,
    jobs: Sender<Job>,
    // which run, judged up to when, and what came of it
    results: Receiver<(u64, f64, Judged)>,
    judging: bool,
    song: Option<song::Player>,
    song_ready: bool,
    song_failed: bool,
    song_reopened: bool,
    // in screen pixels
    mouse: Option<(f64, f64)>,
    moved: Instant,
    seeking: bool,
    // a seek sent to the page, which comes back as its own
    sent_seek: Option<f64>,
    pointer: bool,
    // screen pixels a scene unit, and where the scene starts
    view: (f64, f64, f64),
    auto: Option<(String, bool, Vec<(f64, f64, bool)>)>,
    game: Game,
    panel: bool,
    binding: Option<Binding>,
    hits: Vec<Hit>,
    // dragging the volume slider
    turning: bool,
    hush: hush::Hush,
    vsync: bool,
    next_frame: Instant,
    frames: u32,
    fps_from: Instant,
    fps: u32,
    scratch: Vec<chart::Note>,
}

impl App {
    fn new(args: Args, proxy: EventLoopProxy<News>) -> App {
        let (jobs, results) = judge_worker();
        let game = Game::load();
        App {
            args,
            proxy: Some(proxy),
            link: None,
            shared: Arc::new(Mutex::new(None)),
            window: None,
            gpu: None,
            text: text::Text::new(),
            stage: None,
            live: None,
            map: None,
            notes: Notes::None,
            clock: 0.0,
            clock_at: Instant::now(),
            playing: true,
            from: 0.0,
            run: Run::default(),
            jobs,
            results,
            judging: false,
            song: None,
            song_ready: false,
            song_failed: false,
            song_reopened: false,
            mouse: None,
            moved: Instant::now(),
            seeking: false,
            sent_seek: None,
            pointer: true,
            view: (1.0, 0.0, 0.0),
            auto: None,
            vsync: game.fps == Fps::Vsync,
            game,
            panel: false,
            binding: None,
            hits: vec![],
            turning: false,
            hush: hush::Hush::start(),
            next_frame: Instant::now(),
            frames: 0,
            fps_from: Instant::now(),
            fps: 0,
            scratch: vec![],
        }
    }

    fn keys(&self) -> usize {
        self.stage.as_ref().map_or(self.args.keys, Stage::keys)
    }

    fn lazer(&self) -> bool {
        self.game.lazer.unwrap_or(self.map.as_ref().is_some_and(|m| m.lazer))
    }

    fn by_hand(&self) -> bool {
        self.map.is_some() && !self.game.auto
    }

    fn scroll(&self) -> f64 {
        let hit = self.stage.as_ref().map_or(402.0, |s| s.hit_y / s.height * 480.0);
        settings::speed_ms(self.speed(), hit)
    }

    fn speed(&self) -> f64 {
        self.game.speed.unwrap_or_else(|| self.live.as_ref().map_or(24.0, |l| l.speed))
    }

    // average lateness of the hits since the offset was last set from them
    fn late(&self) -> Option<f64> {
        let heads: Vec<f64> = self.run.heads.iter().filter(|h| h.0 >= self.run.late_from).map(|h| h.1).collect();
        (self.by_hand() && heads.len() >= 10).then(|| heads.iter().sum::<f64>() / heads.len() as f64)
    }

    // "OD 8", with the mods that change it
    fn od(&self) -> Option<String> {
        let m = self.map.as_ref()?;
        let mut s = format!("OD {}", (m.od * 10.0).round() / 10.0);
        if !m.mods.is_empty() {
            s += &format!(" with {}", m.mods.join(""));
        }
        Some(s)
    }

    fn time_at(&self, t: Instant) -> f64 {
        if !self.playing {
            return self.clock;
        }
        match t.checked_duration_since(self.clock_at) {
            Some(d) => self.clock + d.as_secs_f64() * 1000.0,
            None => self.clock - (self.clock_at - t).as_secs_f64() * 1000.0,
        }
    }

    fn binds(&self) -> Vec<String> {
        self.game.keys_for(self.keys())
    }

    fn set_game(&mut self, change: impl FnOnce(&mut Game)) {
        let was = self.game.clone();
        change(&mut self.game);
        if self.game == was {
            return;
        }
        self.game.save();
        if (was.auto, was.lazer) != (self.game.auto, self.game.lazer) && self.map.is_some() {
            let now = self.time_at(Instant::now());
            self.seek(now);
        }
        let vsync = self.game.fps == Fps::Vsync;
        if vsync != self.vsync {
            self.vsync = vsync;
            if let Some(g) = &mut self.gpu {
                g.set_vsync(vsync);
            }
        }
    }

    // a click on the panel or its gear; false when it wasn't on them
    fn panel_click(&mut self, at: (f64, f64)) -> bool {
        let Some(action) = self.hits.iter().rev().find(|h| h.holds(at)).map(|h| h.action) else {
            if self.panel {
                self.panel = false;
                self.binding = None;
                return true;
            }
            return false;
        };
        let lazer = self.lazer();
        match action {
            Action::Gear => {
                self.panel = !self.panel;
                self.binding = None;
            }
            Action::Auto(auto) => self.set_game(|g| g.auto = auto),
            // the client's own way is saved as no choice, so it follows the client
            Action::Lazer(l) if l == lazer => {}
            Action::Lazer(l) => {
                let client = self.map.as_ref().is_some_and(|m| m.lazer);
                self.set_game(|g| g.lazer = (l != client).then_some(l));
            }
            Action::Bind => self.binding = Some(Binding { keys: self.keys(), codes: vec![] }),
            Action::DefaultKeys => {
                let keys = self.keys().to_string();
                self.set_game(|g| {
                    g.binds.remove(&keys);
                });
            }
            Action::Volume => {
                self.turning = true;
                self.turn(at.0);
            }
            Action::Offset(by) => self.set_game(|g| g.offset += by),
            Action::UseOffset => {
                if let Some(late) = self.late() {
                    self.set_game(|g| g.offset = (g.offset + late).round());
                    self.run.late_from = self.time_at(Instant::now());
                }
            }
            Action::Speed(s) => self.set_game(|g| g.speed = Some(s)),
            Action::MuteOsu(m) => self.set_game(|g| g.mute_osu = m),
            Action::Fps(f) => self.set_game(|g| g.fps = f),
            Action::Panel => {}
        }
        true
    }

    fn turn(&mut self, x: f64) {
        if let Some(v) = panel::volume_at(&self.hits, x) {
            self.game.volume = v;
        }
    }

    fn bind(&mut self, code: KeyCode) {
        let Some(b) = &mut self.binding else { return };
        if code == KeyCode::Escape {
            self.binding = None;
            return;
        }
        let name = format!("{code:?}");
        if !b.codes.contains(&name) {
            b.codes.push(name);
        }
        if b.codes.len() == b.keys {
            let (keys, codes) = (b.keys.to_string(), std::mem::take(&mut b.codes));
            self.binding = None;
            self.set_game(|g| {
                g.binds.insert(keys, codes.into());
            });
        }
    }

    fn seek(&mut self, to: f64) {
        self.clock = to;
        self.clock_at = Instant::now();
        self.from = to;
        let id = self.run.id + 1;
        self.run = Run { id, acc: 100.0, combo_at: f64::NEG_INFINITY, ..Run::default() };
    }

    fn on_live(&mut self, live: Live) {
        let was = self.live.take();
        let fresh = was.as_ref().is_none_or(|w| w.source != live.source || w.play != live.play || w.keys != live.keys);
        if !fresh && self.map.is_some() && live.from != was.as_ref().and_then(|w| w.from) {
            if let Some(from) = live.from.filter(|f| self.sent_seek.is_none_or(|s| (s - f).abs() > 0.5)) {
                self.seek(from);
            }
        }
        // only on a change, so a click here pauses at once and stays paused with the skin page closed
        if was.as_ref().is_none_or(|w| w.playing != live.playing) {
            self.set_playing(live.playing);
        }
        self.live = Some(live);
    }

    fn set_playing(&mut self, playing: bool) {
        if playing == self.playing {
            return;
        }
        self.clock = self.time_at(Instant::now());
        self.clock_at = Instant::now();
        self.playing = playing;
    }

    fn on_map(&mut self, map: Option<Arc<Map>>) {
        self.auto = None;
        match &map {
            Some(m) => {
                let notes = MapNotes::new(&m.notes);
                let from = self.live.as_ref().and_then(|l| l.from).unwrap_or(notes.first);
                self.notes = Notes::Map(notes);
                self.seek(from - LEAD_MS);
                self.from = from;
            }
            None => {
                let lns = self.live.as_ref().is_some_and(|l| l.source == "lns");
                let keys = self.live.as_ref().map_or(self.args.keys, |l| l.keys);
                self.notes = Notes::Made(Made::new(keys, lns, 600.0));
                self.seek(0.0);
            }
        }
        self.map = map;
    }

    fn key(&mut self, code: KeyCode, down: bool, repeat: bool) {
        if !self.by_hand() {
            return;
        }
        let name = format!("{code:?}");
        let Some(c) = self.binds().iter().position(|b| *b == name) else { return };
        let t = self.time_at(Instant::now());
        let run = &mut self.run;
        if run.open.len() <= c {
            run.open.resize(c + 1, None);
        }
        if down {
            if repeat || !self.playing || run.open[c].is_some() {
                return;
            }
            run.presses.push((c, t, None));
            run.open[c] = Some(run.presses.len() - 1);
        } else {
            let Some(k) = run.open[c].take() else { return };
            run.presses[k].2 = Some(run.presses[k].1.max(t));
            if run.released.len() <= c {
                run.released.resize(c + 1, f64::NEG_INFINITY);
            }
            run.released[c] = t;
        }
        run.dirty = true;
        self.judge();
    }

    fn let_go(&mut self) {
        let t = self.time_at(Instant::now());
        let run = &mut self.run;
        for c in 0..run.open.len() {
            let Some(k) = run.open[c].take() else { continue };
            run.presses[k].2 = Some(run.presses[k].1.max(t));
            if run.released.len() <= c {
                run.released.resize(c + 1, f64::NEG_INFINITY);
            }
            run.released[c] = t;
            run.dirty = true;
        }
    }

    fn judge(&mut self) {
        while let Ok((id, to, j)) = self.results.try_recv() {
            self.judging = false;
            if id == self.run.id {
                self.run.judged_to = to;
                self.judged(j);
            }
        }
        let Some(map) = self.map.clone().filter(|_| self.by_hand()) else { return };
        // judging starts at the first press, from the earliest note it can be for
        let Some(first) = self.run.presses.first().map(|p| p.1) else { return };
        let from = self.from.max(first - mania_bridge::player::reach(map.od, self.lazer(), &map.mods));
        let due = self.run.dirty || (self.playing && self.run.sent.is_none_or(|s| s.elapsed() >= JUDGE_EVERY));
        if self.judging || !due {
            return;
        }
        let job = Job {
            id: self.run.id,
            chart: map.notes.clone(),
            keys: map.keys,
            od: map.od,
            lazer: self.lazer(),
            mods: map.mods.clone(),
            from,
            now: self.time_at(Instant::now()),
            presses: self.run.presses.clone(),
        };
        if self.jobs.send(job).is_ok() {
            self.judging = true;
            self.run.dirty = false;
            self.run.sent = Some(Instant::now());
        }
    }

    fn judged(&mut self, j: Judged) {
        let notes = self.map.as_ref().map(|m| m.notes.clone()).unwrap_or_default();
        let run = &mut self.run;
        run.gone.extend(j.hit);
        run.done.extend(j.done);
        let mut latest: Option<(u8, f64)> = None;
        for (i, judgement, t) in j.recent {
            if !run.seen.insert((i, judgement, t.to_bits())) {
                continue;
            }
            if latest.is_none_or(|(_, lt)| t >= lt) {
                latest = Some((judgement, t));
            }
            if let (true, Some(&(c, ..))) = (judgement != MISS, notes.get(i)) {
                if run.lit.len() <= c {
                    run.lit.resize(c + 1, f64::NEG_INFINITY);
                }
                run.lit[c] = run.lit[c].max(t);
            }
        }
        if let Some(l) = latest {
            if run.pop.is_none_or(|(_, t)| l.1 >= t) {
                run.pop = Some(l);
            }
            if j.combo > run.combo {
                run.combo_at = l.1;
            }
        }
        run.combo = j.combo;
        run.acc = j.acc;
        run.heads = j.heads;
    }

    // lazer judges a hold's head and tail, stable the whole hold once at its end. Combo goes up with
    // each judgement on lazer; on stable with a hold's head and every 100 ms of it held after that,
    // as mania-hub's replay player counts it
    fn auto_overlay(&mut self, now: f64) -> Overlay {
        let lazer = self.lazer();
        let Some(map) = &self.map else { return Overlay { now, pop: None, combo: 0, combo_at: 0.0, acc: None } };
        if self.auto.as_ref().is_none_or(|(id, l, _)| *id != map.id || *l != lazer) {
            // (time, the note's head, a judgement shows) for each step of the combo
            let mut times: Vec<(f64, f64, bool)> = map.notes.iter().flat_map(|&(_, t, e)| match e {
                None => vec![(t, t, true)],
                Some(e) if lazer => vec![(t, t, true), (e, t, true)],
                Some(e) => {
                    let ticks = ((e - t) / 100.0 + 1e-6).floor().max(0.0) as usize;
                    let mut v = vec![(t, t, false)];
                    v.extend((1..=ticks).map(|k| (t + k as f64 * 100.0, t, false)));
                    v.push((e, t, true));
                    v
                }
            }).collect();
            times.sort_by(|a, b| a.0.total_cmp(&b.0));
            self.auto = Some((map.id.clone(), lazer, times));
        }
        let times = &self.auto.as_ref().unwrap().2;
        let (mut combo, mut bumped, mut pop) = (0, None, None);
        for &(t, head, judged) in times {
            if t > now {
                break;
            }
            if head < self.from {
                continue;
            }
            // a stable hold's end is its judgement, which adds no combo of its own
            if lazer || !judged || t <= head {
                combo += 1;
                bumped = Some(t);
            }
            if judged {
                pop = Some(t);
            }
        }
        Overlay { now, pop: pop.map(|t| Pop { j: 1, t }), combo, combo_at: bumped.unwrap_or(f64::NEG_INFINITY), acc: Some(100.0) }
    }

    fn frame(&mut self) {
        let now_i = Instant::now();
        if self.playing {
            self.clock += (now_i - self.clock_at).as_secs_f64() * 1000.0;
        }
        self.clock_at = now_i;
        if let Notes::Map(m) = &self.notes {
            if self.clock > m.end + 1000.0 {
                let to = m.first - LEAD_MS;
                self.seek(to);
            }
        }
        self.judge();
        if self.song.as_ref().is_some_and(song::Player::broken) {
            self.reopen_song();
        }
        let now = self.clock;
        let (offset, volume) = (self.game.offset, self.game.volume);
        if let Some(p) = &self.song {
            let on = self.playing && self.song_ready && self.map.is_some() && !self.seeking;
            p.follow(on.then_some(now + offset), volume);
        }
        self.hush.set(self.game.mute_osu && self.playing && self.song_ready && self.map.is_some() && volume > 0.0);
        *self.shared.lock().unwrap_or_else(|e| e.into_inner()) = self.map.is_some().then_some(now);

        let Some(gpu) = &mut self.gpu else { return };
        // black until the skin is laid out: a window that hasn't shown a frame isn't on screen yet
        let Some(stage) = &self.stage else {
            if !gpu.present(&Frame::default()) {
                gpu.reconfigure();
            }
            return;
        };
        let (pw, ph) = gpu.size();
        let k = (f64::from(pw) / stage.width).min(f64::from(ph) / stage.height);
        let (ox, oy) = ((f64::from(pw) - stage.width * k) / 2.0, (f64::from(ph) - stage.height * k) / 2.0);
        self.view = (k, ox, oy);
        let speed = stage.hit_y / self.scroll();
        let ahead = now + stage.height * 1.5 / speed;
        let keys = stage.keys();
        let by_hand = self.by_hand();

        let mut visible = std::mem::take(&mut self.scratch);
        visible.clear();
        let past = if by_hand { (stage.height - stage.hit_y + 200.0) / speed } else { TAP_PRESS_MS };
        match &mut self.notes {
            Notes::Map(m) => m.visible(self.from, now - past, ahead, &mut visible),
            Notes::Made(m) => m.visible(now - past, ahead, &mut visible),
            Notes::None => {}
        }
        let mut pressed = vec![false; keys];
        let mut lit = vec![false; keys];
        let (mut holds, mut taps) = (vec![], vec![]);
        if by_hand {
            (holds, taps, pressed, lit) = by_hand_notes(&visible, &self.run, now, stage.hit_y, speed, keys);
        } else {
            for n in &visible {
                match n.e {
                    Some(e) if e >= now => {
                        if n.t <= now {
                            if let Some(p) = pressed.get_mut(n.c) {
                                *p = true;
                            }
                        }
                        holds.push(Hold { c: n.c, head: stage.hit_y - (n.t - now).max(0.0) * speed, tail: stage.hit_y - (e - now) * speed, t: n.t - now });
                    }
                    Some(_) => {}
                    None if n.t <= now => {
                        if let Some(p) = pressed.get_mut(n.c) {
                            *p = true;
                        }
                    }
                    None => taps.push(Tap { c: n.c, y: stage.hit_y - (n.t - now) * speed, t: n.t - now }),
                }
            }
            lit.clone_from(&pressed);
        }
        taps.sort_by(|a, b| a.y.total_cmp(&b.y));
        self.scratch = visible;

        let overlay = if self.map.is_none() {
            None
        } else if by_hand {
            let r = &self.run;
            Some(Overlay { now, pop: r.pop.map(|(j, t)| Pop { j, t }), combo: r.combo, combo_at: r.combo_at, acc: Some(r.acc) })
        } else {
            Some(self.auto_overlay(now))
        };
        let bar = self.seek_bar();
        let (od, speed, late) = (self.od(), self.speed(), self.late());
        let lazer = self.lazer();
        let keys = self.keys();
        let shown = self.chrome();
        self.text.next_frame();
        let (Some(gpu), Some(stage)) = (&mut self.gpu, &self.stage) else { return };
        let mut frame = Frame::default();
        {
            let mut pen = Pen { frame: &mut frame, stage, k, ox, oy };
            pen.stage(&holds, &taps, &pressed, &lit, overlay.is_some());
            if let Some(o) = &overlay {
                pen.overlay(gpu, &mut self.text, o);
            }
            pen.fps(gpu, &mut self.text, self.fps);
            if self.map.as_ref().is_some_and(|m| m.song.is_some()) {
                if self.song_failed {
                    pen.note(gpu, &mut self.text, "Couldn't play the song.");
                } else if !self.song_ready {
                    pen.note(gpu, &mut self.text, "Loading the song…");
                }
            }
            if let Some(bar) = &bar {
                pen.seek_bar(gpu, &mut self.text, bar, self.args.accent);
            }
        }
        let look = Look { game: &self.game, keys, lazer, od, speed, late, binding: self.binding.as_ref(), open: self.panel, alpha: shown, accent: self.args.accent };
        let mut painter = Painter { frame: &mut frame, gpu, text: &mut self.text, k, ox, oy, hits: vec![] };
        painter.draw(&look, stage.width, stage.height);
        self.hits = painter.hits;
        let Some(gpu) = &mut self.gpu else { return };
        if !gpu.present(&frame) {
            gpu.reconfigure();
        }
        self.frames += 1;
        let since = self.fps_from.elapsed();
        if since >= Duration::from_millis(500) {
            self.fps = (f64::from(self.frames) / since.as_secs_f64()).round() as u32;
            self.frames = 0;
            self.fps_from = Instant::now();
        }
    }

    fn chrome(&self) -> f64 {
        let rest = self.moved.elapsed().saturating_sub(STILL).as_secs_f64() * 1000.0;
        if self.seeking || self.panel || self.turning { 1.0 } else { (1.0 - rest / FADE_MS).clamp(0.0, 1.0) }
    }

    fn seek_bar(&self) -> Option<SeekBar> {
        let (Notes::Map(m), Some(stage)) = (&self.notes, &self.stage) else { return None };
        let mut bar = SeekBar::at(stage.width, stage.height);
        let over = self.mouse.is_some_and(|p| bar.holds(self.scene_point(p)));
        bar.alpha = if over { 1.0 } else { self.chrome() };
        if bar.alpha <= 0.0 {
            return None;
        }
        let at = self.clock.clamp(m.first, m.end);
        bar.done = (at - m.first) / (m.end - m.first).max(1.0);
        let clock = |ms: f64| format!("{}:{:02}", (ms / 60000.0).floor() as i64, ((ms / 1000.0).floor() as i64).rem_euclid(60));
        bar.time = format!("{} / {}", clock(at), clock(m.end));
        bar.held = self.seeking;
        Some(bar)
    }

    fn scene_point(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (k, ox, oy) = self.view;
        ((x - ox) / k, (y - oy) / k)
    }

    fn seek_to(&mut self, (x, _): (f64, f64)) {
        let Notes::Map(m) = &self.notes else { return };
        let Some(stage) = &self.stage else { return };
        let t = m.first + SeekBar::at(stage.width, stage.height).along(x) * (m.end - m.first);
        self.seek((t / 10.0).round() * 10.0);
    }

    // the sound output died: tried once more on whatever is the default now, else osu! is let be heard
    fn reopen_song(&mut self) {
        let song = self.song.take().and_then(|p| p.song());
        let again = if self.song_reopened { Err("The sound output stopped again.".to_string()) } else { song::Player::open() };
        self.song_reopened = true;
        match again {
            Ok(p) => {
                p.set_song(song);
                self.song = Some(p);
            }
            Err(e) => {
                eprintln!("sound: {e}");
                self.song_ready = false;
                self.song_failed = true;
            }
        }
    }

    // the song decoded for the map on now, not one before it
    fn wants_song(&self, file: &std::path::Path) -> bool {
        self.map.as_ref().and_then(|m| m.song.as_deref()) == Some(file)
    }

    fn interval(&self) -> Option<Duration> {
        let fps = self.game.fps;
        let refresh = self.window.as_ref().and_then(|w| w.current_monitor()).and_then(|m| m.refresh_rate_millihertz()).map_or(60.0, |mhz| f64::from(mhz) / 1000.0);
        match fps {
            Fps::Vsync => None,
            Fps::Times(n) => Some(Duration::from_secs_f64(1.0 / (refresh * f64::from(n)).max(1.0))),
            // lazer's "basically unlimited"
            Fps::Unlimited => Some(Duration::from_millis(1)),
        }
    }
}

// how much Hyprland scales a screen by, which winit doesn't know of a window not yet on it
fn hyprland_scale(name: &str) -> Option<f64> {
    if !cfg!(target_os = "linux") || std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return None;
    }
    let out = std::process::Command::new("hyprctl").args(["monitors", "-j"]).stderr(std::process::Stdio::null()).output().ok()?;
    let monitors: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let m = monitors.as_array()?.iter().find(|m| m.get("name").and_then(serde_json::Value::as_str) == Some(name))?;
    m.get("scale")?.as_f64().filter(|s| *s > 0.0)
}

fn icon(png: &[u8]) -> Option<winit::window::Icon> {
    let (data, width, height) = crate::tray::rgba(png)?;
    winit::window::Icon::from_rgba(data, width, height).ok()
}

// a size that fits on no screen goes fullscreen, the closest there is to the game
fn place(el: &ActiveEventLoop, args: &Args) -> (winit::window::WindowAttributes, f64) {
    let monitors: Vec<_> = el.available_monitors().collect();
    let contains = |m: &winit::monitor::MonitorHandle, (x, y): (f64, f64)| {
        let (p, s, scale) = (m.position(), m.size(), m.scale_factor());
        let (x, y) = (x * scale, y * scale);
        x >= f64::from(p.x) && y >= f64::from(p.y) && x < f64::from(p.x) + f64::from(s.width) && y < f64::from(p.y) + f64::from(s.height)
    };
    let here = args.near.and_then(|n| monitors.iter().find(|m| contains(m, n)).cloned()).or_else(|| el.primary_monitor()).or_else(|| monitors.first().cloned());
    let fits = |m: &winit::monitor::MonitorHandle| args.size.is_some_and(|(w, h)| w < m.size().width && h < m.size().height);
    let fit_on = here.clone().filter(fits).or_else(|| monitors.iter().find(|m| fits(m)).cloned());
    let full = fit_on.is_none();
    let monitor = fit_on.or(here);
    let screen = monitor.as_ref().map_or(PhysicalSize::new(1920, 1080), |m| m.size());
    let (w, h) = args.size.unwrap_or((screen.width, screen.height));
    // Wayland makes a window before telling it its screen's scaling, so it's sized in the screen's units
    let size: winit::dpi::Size = match monitor.as_ref().and_then(|m| m.name()).and_then(|n| hyprland_scale(&n)) {
        Some(scale) => LogicalSize::new(f64::from(w) / scale, f64::from(h) / scale).into(),
        None => PhysicalSize::new(w, h).into(),
    };
    let mut attrs = Window::default_attributes()
        .with_title(format!("Hashi · your skin at {w}×{h}"))
        .with_inner_size(size)
        // fixed in size, so tiling window managers float it at that size
        .with_resizable(false)
        .with_window_icon(icon(include_bytes!("../../icons/32x32.png")));
    // the window icon is the title bar's, the taskbar wants a bigger one
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowAttributesExtWindows;
        attrs = attrs.with_taskbar_icon(icon(include_bytes!("../../icons/64x64.png")));
    }
    #[cfg(target_os = "linux")]
    {
        use winit::platform::wayland::WindowAttributesExtWayland;
        attrs = attrs.with_name("hashi", "hashi-skin");
    }
    match monitor {
        Some(m) if full => attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(Some(m)))),
        Some(m) => {
            let (at, s) = (m.position(), m.size());
            let centre = |start: i32, room: u32, taken: u32| start + (i64::from(room) - i64::from(taken)).max(0) as i32 / 2;
            attrs = attrs.with_position(PhysicalPosition::new(centre(at.x, s.width, w), centre(at.y, s.height, h)));
        }
        None => {}
    }
    (attrs, f64::from(w) / f64::from(h).max(1.0))
}

impl ApplicationHandler<News> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (attrs, aspect) = place(el, &self.args);
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("no window: {e}");
                return el.exit();
            }
        };
        match Gpu::new(Some(window.clone()), self.vsync) {
            Ok(g) => self.gpu = Some(g),
            Err(e) => {
                eprintln!("can't draw: {e}");
                return el.exit();
            }
        }
        if let Some(proxy) = self.proxy.take() {
            self.link = Some(link::start(self.args.port, aspect, proxy, self.shared.clone()));
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn user_event(&mut self, el: &ActiveEventLoop, news: News) {
        match news {
            News::Live(l) => self.on_live(l),
            News::Scene(s) => {
                if let Some(gpu) = &mut self.gpu {
                    let old = self.stage.take();
                    self.stage = Some(Stage::new(&s.json, &s.sprites, gpu, old));
                }
            }
            News::Map(m) => self.on_map(m),
            News::SongFailed(file) => self.song_failed |= self.wants_song(&file),
            News::Song(Some((file, _))) if !self.wants_song(&file) => {}
            News::Song(s) => {
                let s = s.map(|(_, s)| s);
                self.song_failed = false;
                if s.is_some() && self.song.is_none() {
                    match song::Player::open() {
                        Ok(p) => {
                            self.song = Some(p);
                            self.song_reopened = false;
                        }
                        Err(e) => {
                            eprintln!("sound: {e}");
                            self.song_failed = true;
                        }
                    }
                }
                // osu! is only muted for a song that really plays here
                self.song_ready = s.is_some() && self.song.is_some();
                if let Some(p) = &self.song {
                    p.set_song(s);
                }
            }
            News::Quit => el.exit(),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => self.frame(),
            // a window made before it knows its screen's scaling takes the size as unscaled, and keeps
            // that once it does: it's asked again in the screen's own pixels, osu!'s size there
            WindowEvent::ScaleFactorChanged { mut inner_size_writer, .. } => {
                if let (Some((w, h)), Some(window)) = (self.args.size, &self.window) {
                    if window.fullscreen().is_none() {
                        let _ = inner_size_writer.request_inner_size(PhysicalSize::new(w, h));
                    }
                }
            }
            WindowEvent::KeyboardInput { event: KeyEvent { physical_key: PhysicalKey::Code(code), state, repeat, .. }, .. } => {
                let down = state == ElementState::Pressed;
                if self.binding.is_some() {
                    if down && !repeat {
                        self.bind(code);
                    }
                    return;
                }
                if code == KeyCode::Escape && down {
                    if self.panel {
                        self.panel = false;
                        return;
                    }
                    return el.exit();
                }
                self.key(code, down, repeat);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let at = (position.x, position.y);
                self.mouse = Some(at);
                self.moved = Instant::now();
                if !self.pointer {
                    self.pointer = true;
                    if let Some(w) = &self.window {
                        w.set_cursor_visible(true);
                    }
                }
                let p = self.scene_point(at);
                if self.seeking {
                    self.seek_to(p);
                }
                if self.turning {
                    self.turn(p.0);
                }
            }
            WindowEvent::CursorLeft { .. } => self.mouse = None,
            // a click off the seek bar pauses, as it does on the skin page
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let at = self.mouse.map(|p| self.scene_point(p));
                if state == ElementState::Pressed && at.is_some_and(|p| self.panel_click(p)) {
                    return;
                }
                if state == ElementState::Released && self.turning {
                    self.turning = false;
                    self.game.save();
                    return;
                }
                let on_bar = at.filter(|&p| self.seek_bar().is_some_and(|b| b.holds(p)));
                match state {
                    ElementState::Pressed if on_bar.is_some() => {
                        self.seeking = true;
                        self.seek_to(on_bar.unwrap());
                    }
                    ElementState::Pressed => {
                        self.set_playing(!self.playing);
                        if let Some(l) = &self.link {
                            let _ = l.send(Out::Pause);
                        }
                    }
                    ElementState::Released if self.seeking => {
                        self.seeking = false;
                        self.sent_seek = Some(self.from);
                        if let Some(l) = &self.link {
                            let _ = l.send(Out::Seek(self.from));
                        }
                    }
                    ElementState::Released => {}
                }
            }
            // releases while unfocused never arrive
            WindowEvent::Focused(false) => self.let_go(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        let Some(window) = &self.window else { return };
        if self.pointer && self.playing && !self.seeking && !self.panel && self.moved.elapsed() > STILL {
            self.pointer = false;
            window.set_cursor_visible(false);
        }
        match self.interval() {
            // Fifo presenting paces it
            None => {
                window.request_redraw();
                el.set_control_flow(ControlFlow::Poll);
            }
            Some(every) => {
                let now = Instant::now();
                if now >= self.next_frame {
                    window.request_redraw();
                    self.next_frame += every;
                    if self.next_frame < now {
                        self.next_frame = now + every;
                    }
                }
                el.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
            }
        }
    }
}

// a tap goes once hit; a hold whose head was hit stays on the line while held, and goes once judged
// all the way
fn by_hand_notes(visible: &[chart::Note], run: &Run, now: f64, hit_y: f64, speed: f64, keys: usize) -> (Vec<Hold>, Vec<Tap>, Vec<bool>, Vec<bool>) {
    let (mut holds, mut taps) = (vec![], vec![]);
    let mut pressed = vec![false; keys];
    let mut lit = vec![false; keys];
    for c in 0..keys {
        pressed[c] = run.open.get(c).copied().flatten().is_some();
        lit[c] = now - run.lit.get(c).copied().unwrap_or(f64::NEG_INFINITY) < LIGHTING_MS;
    }
    for n in visible {
        let hit = n.i.is_some_and(|i| run.gone.contains(&i));
        let done = n.i.is_some_and(|i| run.done.contains(&i));
        match n.e {
            // judged can be before its end, when let go of in time
            Some(e) if hit && (done || e <= now) => {}
            Some(e) => {
                let waiting = run.released.get(n.c).is_some_and(|&r| r >= run.judged_to);
                // just pressed near its head, it's taken as hit until the judge says
                let down = run.open.get(n.c).copied().flatten().map(|k| run.presses[k].1);
                let pressing = down.is_some_and(|d| d >= run.judged_to && (d - n.t).abs() <= PRESS_NEAR_MS);
                let holding = (hit && (pressed.get(n.c).copied().unwrap_or(false) || waiting)) || pressing;
                if holding && n.t <= now {
                    if let Some(l) = lit.get_mut(n.c) {
                        *l = true;
                    }
                }
                let head = hit_y - if holding { (n.t - now).max(0.0) } else { n.t - now } * speed;
                holds.push(Hold { c: n.c, head, tail: hit_y - (e - now) * speed, t: n.t - now });
            }
            None if !hit => taps.push(Tap { c: n.c, y: hit_y - (n.t - now) * speed, t: n.t - now }),
            None => {}
        }
    }
    (holds, taps, pressed, lit)
}

fn shot(args: &Args, out: &str) -> Result<(), String> {
    let info = link::request(args.port, "GET", "/skin").ok_or("The helper isn't running.")?.1;
    let info: serde_json::Value = serde_json::from_slice(&info).map_err(|e| e.to_string())?;
    let dir = std::path::PathBuf::from(info["folder"].as_str().ok_or("No skin.")?);
    let (w, h) = args.size.unwrap_or((1280, 720));
    let (json, sprites) = mania_bridge::player::scene(&dir, args.keys, "shot", "", Some(f64::from(w) / f64::from(h)))?;
    let mut gpu = Gpu::new(None, false)?;
    let mut text = text::Text::new();
    let stage = Stage::new(&json, &sprites, &mut gpu, None);
    let k = f64::from(h) / stage.height;
    let (ftaps, fholds, down) = &stage.frozen;
    let taps: Vec<Tap> = ftaps.iter().map(|t| Tap { c: t.c, y: t.y, t: t.t }).collect();
    let holds: Vec<Hold> = fholds.iter().map(|t| Hold { c: t.c, head: t.head, tail: t.tail, t: t.t }).collect();
    let pressed: Vec<bool> = (0..stage.keys()).map(|c| c == *down).collect();
    let hud = std::env::var_os("SHOT_HUD").is_some();
    let mut frame = Frame::default();
    {
        let mut pen = Pen { frame: &mut frame, stage: &stage, k, ox: (f64::from(w) - stage.width * k) / 2.0, oy: 0.0 };
        pen.stage(&holds, &taps, &pressed, &pressed, hud);
        if hud {
            pen.overlay(&mut gpu, &mut text, &Overlay { now: 100.0, pop: Some(Pop { j: 6, t: 0.0 }), combo: 123, combo_at: 0.0, acc: Some(97.4567) });
            pen.fps(&mut gpu, &mut text, 240);
        }
    }
    if std::env::var_os("SHOT_PANEL").is_some() {
        let game = Game::load();
        let look = Look { game: &game, keys: args.keys, lazer: true, od: Some("OD 8 with HR".into()), speed: 24.0, late: Some(23.0), binding: None, open: true, alpha: 1.0, accent: args.accent };
        let mut painter = Painter { frame: &mut frame, gpu: &mut gpu, text: &mut text, k, ox: (f64::from(w) - stage.width * k) / 2.0, oy: 0.0, hits: vec![] };
        painter.draw(&look, stage.width, stage.height);
    }
    let rgba = gpu.image(&frame, w, h);
    let file = std::fs::File::create(out).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().and_then(|mut wr| wr.write_image_data(&rgba)).map_err(|e| e.to_string())
}
