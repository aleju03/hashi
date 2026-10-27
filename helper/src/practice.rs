use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::analysis::judge_play;
use crate::types::Item;
use crate::{log, Bridge};

// also tells a practice map apart if practice.json is gone
pub(crate) const MARK: &str = "Hashi practice";
const LIST_FILE: &str = "practice.json";
const AUDIO_FILE: &str = "audio.wav";
const START_SILENCE_MS: f64 = 1000.0;
const GAP_MS: f64 = 1000.0;
// each repeat's music comes in over this, so it doesn't pop in
const FADE_IN_MS: f64 = 300.0;
// the song keeps going past the stretch's last note, at full volume for a moment, then fades to silence
const TAIL_MS: f64 = 2000.0;
const HOLD_MS: f64 = 250.0;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Practice {
    pub md5: String,
    pub created_at: String,
    pub from_play: String,
    pub title: String,
    pub version: String,
    // song seconds as played, like the analysis' practice tip
    pub from: f64,
    pub to: f64,
    pub repeats: Vec<Repeat>,
    // accuracy to reach on a full-speed repeat: what the rest of the map got in the play
    pub goal: Option<f64>,
    pub file: String,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub(crate) struct Repeat {
    pub start: f64,
    pub end: f64,
    pub speed: f64,
}

pub(crate) struct Options {
    pub from: f64,
    pub to: f64,
    pub repeats: usize,
    pub slowest: f64,
    pub lead: f64,
    pub goal: Option<f64>,
}

impl Options {
    pub fn from_query(q: &str) -> Option<Self> {
        let get = |k: &str| url::form_urlencoded::parse(q.as_bytes()).find(|(n, _)| n == k).and_then(|(_, v)| v.parse::<f64>().ok()).filter(|v| v.is_finite());
        let o = Options {
            from: get("from")?,
            to: get("to")?,
            repeats: get("repeats").unwrap_or(4.0).round() as usize,
            slowest: get("slowest").unwrap_or(0.85),
            lead: get("lead").unwrap_or(1.0),
            goal: get("goal").filter(|g| (0.0..=100.0).contains(g)),
        };
        let ok = o.from >= 0.0 && o.to - o.from >= 1.0 && o.to - o.from <= 60.0 && (1..=8).contains(&o.repeats) && (0.5..=1.0).contains(&o.slowest) && (0.0..=5.0).contains(&o.lead);
        ok.then_some(o)
    }
}

pub(crate) fn list_sync(b: &Bridge) -> Vec<Practice> {
    std::fs::read(b.env.data_dir.join(LIST_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

async fn update_list(b: &Bridge, f: impl FnOnce(&mut Vec<Practice>)) -> Result<(), String> {
    static WRITING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _writing = WRITING.lock().await;
    let mut list = list_sync(b);
    f(&mut list);
    crate::write_json_file(&b.env.data_dir.join(LIST_FILE), &json!(list)).await.map_err(|e| e.to_string())
}

pub(crate) async fn set_goal(b: &Bridge, md5: &str, goal: f64) {
    let saved = update_list(b, |list| {
        if let Some(p) = list.iter_mut().find(|p| p.md5 == md5) {
            p.goal = Some(goal);
        }
    });
    if let Err(e) = saved.await {
        log!("practice map: couldn't save the new goal ({e})");
    }
}

pub(crate) fn find(b: &Bridge, md5: &str) -> Option<Practice> {
    list_sync(b).into_iter().find(|p| p.md5 == md5)
}

// the version also catches a copy made on another computer
pub(crate) fn is_practice(b: &Bridge, md5: &str, version: &str) -> bool {
    version.starts_with(MARK) || find(b, md5).is_some()
}

pub(crate) async fn make(b: &Arc<Bridge>, id: &str, o: Options) -> (u16, Value) {
    match build(b, id, o).await {
        Ok(v) => (200, v),
        Err(e) => {
            log!("practice map: {e}");
            (422, json!({ "error": e }))
        }
    }
}

async fn build(b: &Arc<Bridge>, id: &str, o: Options) -> Result<Value, String> {
    let item = b.st().queue.items.iter().find(|i| i.id == id).cloned().ok_or("That play isn't here anymore.")?;
    let (Some(replay), Some(chart)) = (&item.replay, &item.chart) else { return Err("This play has no replay to read.".into()) };
    let replay = crate::replays::read_blob(b, replay).await?;
    let chart_bytes = crate::replays::read_blob(b, chart).await?;
    let audio = audio_file(b, &item, &chart_bytes).await?;
    let background = crate::replays::cover_file(b, &item).await;
    let lazer = item.play.is_lazer();
    let out_dir = b.env.data_dir.join("practice");
    crate::create_private_dir(&out_dir).await.map_err(|e| e.to_string())?;
    let item2 = item.clone();
    let made = tokio::task::spawn_blocking(move || render(&item2, &replay, &chart_bytes, lazer, &audio, background.as_deref(), &o, &out_dir))
        .await
        .map_err(|_| "Couldn't make the practice map.".to_string())??;
    update_list(b, |list| {
        list.retain(|p| p.md5 != made.md5);
        list.push(made.clone());
    })
    .await?;
    log!("practice map {} made from {}", made.version, crate::types::text(&item.play.title));
    if let Some(goal) = made.goal {
        crate::goals::record(b, crate::goals::Seen { kind: "practice", md5: &made.md5, title: &made.title, version: &made.version, target: goal, play: None, opens: true }).await;
    }
    open_in_osu(b, &item, Path::new(&made.file));
    Ok(json!({ "version": made.version, "file": made.file, "md5": made.md5 }))
}

// lazer keeps the song under its hash, which only tosu knows: from the play, or the map selected now
async fn audio_file(b: &Arc<Bridge>, item: &Item, chart: &[u8]) -> Result<PathBuf, String> {
    if let Some(f) = item.play.audio_file.as_ref().map(PathBuf::from).filter(|f| f.is_file()) {
        return Ok(f);
    }
    if !item.play.is_lazer() {
        if let (Some(source), Some(name)) = (item.chart.as_ref().and_then(|c| c.source.clone()), general_value(&String::from_utf8_lossy(chart), "AudioFilename")) {
            let f = Path::new(&source).parent().unwrap_or(Path::new("")).join(name.replace('\\', "/"));
            if f.is_file() {
                return Ok(f);
            }
        }
    }
    if let Some(f) = selected_audio(b, &item.play.beatmap_md5) {
        return Ok(f);
    }
    Err("Hashi can't find this song's audio. Select the map in osu! once, then try again.".into())
}

fn selected_audio(b: &Bridge, md5: &str) -> Option<PathBuf> {
    let (rel, folder, lazer_dir) = {
        let s = b.st();
        let frame = s.tosu.last_frame.as_ref()?;
        let checksum = frame.pointer("/beatmap/checksum").and_then(Value::as_str)?.to_lowercase();
        if checksum != md5 {
            return None;
        }
        let text = |p: &str| frame.pointer(p).and_then(Value::as_str).filter(|x| !x.is_empty()).map(String::from);
        (text("/files/audio")?, text("/folders/beatmap"), (s.tosu.client.as_deref() == Some("lazer")).then(|| s.tosu.lazer_files_dir.clone()))
    };
    let f = match lazer_dir {
        Some(dir) => dir.join(rel),
        None => b.songs_dir().join(folder?.replace('\\', "/")).join(rel),
    };
    f.is_file().then_some(f)
}

pub(crate) fn general_value(text: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines().map(|l| l.trim_end_matches('\r')) {
        if line.starts_with('[') {
            inside = line == "[General]" || line == "[Metadata]";
            continue;
        }
        if inside {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn render(item: &Item, replay: &[u8], chart: &[u8], lazer: bool, audio: &Path, background: Option<&Path>, o: &Options, out_dir: &Path) -> Result<Practice, String> {
    let j = judge_play(replay, chart, lazer)?;
    let text = String::from_utf8_lossy(chart).into_owned();
    let rate = j.rate;
    // song seconds as played to the map's own milliseconds
    let map_ms = |s: f64| s * rate * 1000.0;
    let (a, b) = (map_ms(o.from), map_ms(o.to));
    let notes: Vec<_> = j.notes.iter().filter(|n| n.time >= a && n.time < b).copied().collect();
    if notes.len() < 4 {
        return Err("There are hardly any notes in that stretch.".into());
    }
    let first = notes.iter().map(|n| n.time).fold(f64::MAX, f64::min);
    let last = notes.iter().map(|n| n.end_time.max(n.time)).fold(f64::MIN, f64::max);
    let seg_a = (first - o.lead * 1000.0 * rate).max(0.0);
    let seg_b = last + TAIL_MS * rate;

    let pcm = decode(audio, seg_a, seg_b)?;
    let speeds: Vec<f64> = (0..o.repeats)
        .map(|k| if o.repeats == 1 { 1.0 } else { o.slowest + (1.0 - o.slowest) * k as f64 / (o.repeats - 1) as f64 })
        .collect();

    // the practice map plays without mods, so the play's rate goes into the audio
    let mut out = Mix { rate: pcm.rate, channels: pcm.channels, parts: vec![], frames: 0 };
    out.silence(START_SILENCE_MS);
    let mut placed = vec![];
    for &s in &speeds {
        let f = s * rate;
        let at = out.ms();
        // where the fade out starts, in the cut-out audio's own milliseconds
        out.stretched(&pcm, f, last + HOLD_MS * rate - seg_a);
        placed.push((at, f, s));
        out.silence(GAP_MS);
    }

    let timing = timing_points(&text);
    let base_in = most_common_beat(&timing, j.notes.last().map_or(0.0, |n| n.end_time.max(n.time)));
    // red lines per repeat first: the scroll speed leans on the beat length that lasts longest
    let mut reds: Vec<(f64, f64, i64)> = vec![];
    let mut greens: Vec<(f64, f64, f64)> = vec![];
    let mut objects = vec![];
    for &(at, f, _) in &placed {
        let t = |x: f64| at + (x - seg_a) / f;
        let mut red = timing.iter().filter(|p| p.red && p.time <= seg_a).last().or(timing.iter().find(|p| p.red)).cloned().ok_or("The chart has no timing.")?;
        let mut sv = timing.iter().filter(|p| !p.red && p.time <= seg_a && p.time >= red.time).last().map_or(1.0, |p| p.sv());
        reds.push((t(seg_a), red.beat / f, red.meter));
        greens.push((t(seg_a), sv * base_in / red.beat, f));
        for p in timing.iter().filter(|p| p.time > seg_a && p.time < seg_b) {
            if p.red {
                red = p.clone();
                sv = 1.0;
                reds.push((t(p.time), red.beat / f, red.meter));
            } else {
                sv = p.sv();
            }
            greens.push((t(p.time), sv * base_in / red.beat, f));
        }
        for n in &notes {
            let x = ((n.column as f64 + 0.5) * 512.0 / j.chart.keys as f64).floor() as i64;
            let start = t(n.time).round() as i64;
            objects.push(if n.is_hold {
                format!("{x},192,{start},128,0,{}:0:0:0:0:", t(n.end_time).round() as i64)
            } else {
                format!("{x},192,{start},1,0,0:0:0:0:")
            });
        }
    }
    let end_out = out.ms();
    let base_out = {
        let mut spans: Vec<(f64, f64)> = vec![];
        for (k, r) in reds.iter().enumerate() {
            let until = reds.get(k + 1).map_or(end_out, |n| n.0);
            let key = (r.1 * 1000.0).round() / 1000.0;
            match spans.iter_mut().find(|s| (s.0 - key).abs() < 1e-9) {
                Some(s) => s.1 += until - r.0,
                None => spans.push((key, until - r.0)),
            }
        }
        spans.iter().max_by(|x, y| x.1.total_cmp(&y.1)).map_or(1.0, |s| s.0)
    };
    // the same scroll speed at every repeat, as in the play: velocity is sv times base over the beat length
    let mut points: Vec<String> = vec![];
    let mut all: Vec<(f64, String)> = reds.iter().map(|(t, beat, meter)| (*t, format!("{},{beat},{meter},1,0,40,1,0", t.round() as i64))).collect();
    for (t, velocity, _) in &greens {
        // velocity is relative to the play's base beat length, the practice map has its own
        let beat_here = reds.iter().filter(|r| r.0 <= *t + 0.5).last().map_or(base_out, |r| r.1);
        let sv = (velocity * beat_here / base_out).clamp(0.1, 10.0);
        all.push((*t + 0.001, format!("{},{},4,1,0,40,0,0", t.round() as i64, -100.0 / sv)));
    }
    all.sort_by(|x, y| x.0.total_cmp(&y.0));
    points.extend(all.into_iter().map(|(_, l)| l));

    let meta = |k: &str| general_value(&text, k).unwrap_or_default();
    let clock = |s: f64| format!("{}:{:02}", s as u64 / 60, s as u64 % 60);
    let version = format!(
        "{MARK} {} {}-{} x{}{}",
        crate::types::text(&item.play.version),
        clock(o.from),
        clock(o.to),
        o.repeats,
        if o.repeats > 1 { format!(" {:.0}-100%", o.slowest * 100.0) } else { String::new() }
    );
    let bg_name = background.and_then(|p| p.extension()).map(|e| format!("bg.{}", e.to_string_lossy().to_lowercase()));
    let osu = format!(
        "osu file format v14\n\n[General]\nAudioFilename: {AUDIO_FILE}\nAudioLeadIn: 0\nPreviewTime: -1\nCountdown: 0\nSampleSet: Soft\nStackLeniency: 0.7\nMode: 3\nLetterboxInBreaks: 0\nSpecialStyle: 0\nWidescreenStoryboard: 0\n\n\
         [Metadata]\nTitle:{}\nTitleUnicode:{}\nArtist:{}\nArtistUnicode:{}\nCreator:{}\nVersion:{version}\nSource:\nTags:hashi-practice hashi-{}\nBeatmapID:0\nBeatmapSetID:-1\n\n\
         [Difficulty]\nHPDrainRate:0\nCircleSize:{}\nOverallDifficulty:{}\nApproachRate:5\nSliderMultiplier:1.4\nSliderTickRate:1\n\n\
         [Events]\n//Background and Video events\n{}//Break Periods\n//Storyboard Layer 0 (Background)\n\n\
         [TimingPoints]\n{}\n\n\n[HitObjects]\n{}\n",
        meta("Title"),
        or(meta("TitleUnicode"), meta("Title")),
        meta("Artist"),
        or(meta("ArtistUnicode"), meta("Artist")),
        meta("Creator"),
        crate::now_ms(),
        j.chart.keys,
        j.chart.od,
        bg_name.as_ref().map_or(String::new(), |n| format!("0,0,\"{n}\",0,0\n")),
        points.join("\n"),
        objects.join("\n"),
    );
    let md5 = crate::replays::md5_hex(osu.as_bytes());
    let repeats: Vec<Repeat> = placed
        .iter()
        .map(|&(at, f, speed)| Repeat { start: at + (first - seg_a) / f - 1.0, end: at + (last - seg_a) / f + 1.0, speed })
        .collect();

    let name = sanitize(&format!("{} - {} ({}) [{}]", meta("Artist"), meta("Title"), meta("Creator"), version));
    let file = out_dir.join(format!("{}.osz", sanitize(&format!("{} - {} {}", meta("Artist"), meta("Title"), &md5[..8]))));
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&file).map_err(|e| e.to_string())?);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default();
    zip.start_file(format!("{name}.osu"), deflated).map_err(|e| e.to_string())?;
    zip.write_all(osu.as_bytes()).map_err(|e| e.to_string())?;
    zip.start_file(AUDIO_FILE, deflated).map_err(|e| e.to_string())?;
    out.write_wav(&pcm, &mut zip).map_err(|e| e.to_string())?;
    if let (Some(bg), Some(n)) = (background, &bg_name) {
        if let Ok(bytes) = std::fs::read(bg) {
            zip.start_file(n, stored).map_err(|e| e.to_string())?;
            zip.write_all(&bytes).map_err(|e| e.to_string())?;
        }
    }
    zip.finish().map_err(|e| e.to_string())?;

    Ok(Practice {
        md5,
        created_at: crate::iso_now(),
        from_play: item.id.clone(),
        title: crate::types::text(&item.play.title),
        version,
        from: o.from,
        to: o.to,
        repeats,
        goal: o.goal,
        file: file.to_string_lossy().into_owned(),
    })
}

fn or(a: String, b: String) -> String {
    if a.is_empty() { b } else { a }
}

fn sanitize(s: &str) -> String {
    let clean: String = s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    clean.trim().trim_end_matches('.').chars().take(150).collect()
}

#[derive(Clone)]
struct Point {
    time: f64,
    beat: f64,
    meter: i64,
    red: bool,
}

impl Point {
    // a green line's beat length is -100 over its multiplier
    fn sv(&self) -> f64 {
        if self.beat < 0.0 { (-100.0 / self.beat).clamp(0.1, 10.0) } else { 1.0 }
    }
}

fn timing_points(text: &str) -> Vec<Point> {
    let mut inside = false;
    let mut out = vec![];
    for line in text.lines().map(|l| l.trim_end_matches('\r')) {
        if line.starts_with('[') {
            inside = line == "[TimingPoints]";
            continue;
        }
        if !inside || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        let (Some(Ok(time)), Some(Ok(beat))) = (f.first().map(|x| x.parse::<f64>()), f.get(1).map(|x| x.parse::<f64>())) else { continue };
        let meter = f.get(2).and_then(|x| x.parse().ok()).unwrap_or(4);
        let red = f.get(6).map_or(beat > 0.0, |x| *x == "1") && beat > 0.0;
        out.push(Point { time, beat, meter, red });
    }
    out.sort_by(|a, b| a.time.total_cmp(&b.time).then(b.red.cmp(&a.red)));
    out
}

// the beat length that lasts longest, what osu! scales scroll speed against
fn most_common_beat(points: &[Point], end: f64) -> f64 {
    let reds: Vec<&Point> = points.iter().filter(|p| p.red).collect();
    let mut spans: Vec<(f64, f64)> = vec![];
    for (k, r) in reds.iter().enumerate() {
        let until = reds.get(k + 1).map_or(end, |n| n.time).max(r.time);
        let key = (r.beat * 1000.0).round() / 1000.0;
        match spans.iter_mut().find(|s| (s.0 - key).abs() < 1e-9) {
            Some(s) => s.1 += until - r.time,
            None => spans.push((key, until - r.time)),
        }
    }
    spans.iter().max_by(|x, y| x.1.total_cmp(&y.1)).map_or(500.0, |s| s.0)
}

fn open_in_osu(b: &Bridge, item: &Item, file: &Path) {
    let run = |exe: &Path, args: &[&std::ffi::OsStr]| std::process::Command::new(exe).args(args).arg(file).spawn().is_ok();
    let started = if cfg!(windows) {
        let exe = if item.play.is_lazer() {
            std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("osulazer").join("current").join("osu!.exe"))
        } else {
            b.st().tosu.stable_dirs.as_ref().map(|(game, _)| game.join("osu!.exe"))
        };
        exe.filter(|e| e.is_file()).is_some_and(|e| run(&e, &[]))
    } else if item.play.is_lazer() {
        // a second lazer hands the file to the one running
        running_lazer().is_some_and(|e| run(&e, &[]))
    } else {
        osu_wine().is_some_and(|e| run(&e, &["--osuhandler".as_ref()]))
    };
    // xdg-open without a desktop goes by the bytes, an .osz is a zip there and opens in the file manager.
    // gio goes by the extension, which osu!'s handlers register
    let started = started || (cfg!(target_os = "linux") && run(Path::new("gio"), &["open".as_ref()]));
    if !started {
        if let Err(e) = open::that_detached(file) {
            log!("practice map: couldn't open {}: {e}", file.display());
        }
    }
}

// osu-winello's launcher, whose --osuhandler gives a file to the stable running in wine
fn osu_wine() -> Option<PathBuf> {
    let local = crate::home().join(".local").join("bin").join("osu-wine");
    if local.is_file() {
        return Some(local);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join("osu-wine")).find(|p| p.is_file())
}

// the AppImage the running lazer came from, or the program itself
fn running_lazer() -> Option<PathBuf> {
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let dir = entry.path();
        if !std::fs::read_to_string(dir.join("comm")).is_ok_and(|c| c.trim() == "osu!") {
            continue;
        }
        let environ = std::fs::read(dir.join("environ")).unwrap_or_default();
        let appimage = environ.split(|c| *c == 0).find_map(|v| v.strip_prefix(b"APPIMAGE=")).map(|v| PathBuf::from(String::from_utf8_lossy(v).into_owned()));
        if let Some(p) = appimage.filter(|p| p.is_file()) {
            return Some(p);
        }
        if let Ok(exe) = std::fs::read_link(dir.join("exe")) {
            return Some(exe);
        }
    }
    None
}

struct Pcm {
    rate: u32,
    channels: usize,
    // interleaved, -1 to 1
    samples: Vec<f32>,
}

impl Pcm {
    fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
}

enum Part {
    Silence(usize),
    Stretched { frames: usize, factor: f64, fade_from: f64 },
}

// written straight into the archive, at the longest settings it's hundreds of MB
struct Mix {
    rate: u32,
    channels: usize,
    parts: Vec<Part>,
    frames: usize,
}

impl Mix {
    fn ms(&self) -> f64 {
        self.frames as f64 * 1000.0 / self.rate as f64
    }

    fn silence(&mut self, ms: f64) {
        let n = (ms / 1000.0 * self.rate as f64).round() as usize;
        self.parts.push(Part::Silence(n));
        self.frames += n;
    }

    // resampled, so the pitch moves with the speed. fade_from is ms into src
    fn stretched(&mut self, src: &Pcm, factor: f64, fade_from: f64) {
        let n = ((src.frames() as f64) / factor).floor() as usize;
        self.parts.push(Part::Stretched { frames: n, factor, fade_from });
        self.frames += n;
    }

    // 16-bit PCM, which every osu! plays
    fn write_wav(&self, src: &Pcm, w: &mut impl std::io::Write) -> std::io::Result<()> {
        let ch = self.channels;
        let data_len = (self.frames * ch * 2) as u32;
        let mut head = Vec::with_capacity(44);
        head.extend_from_slice(b"RIFF");
        head.extend_from_slice(&(36 + data_len).to_le_bytes());
        head.extend_from_slice(b"WAVEfmt ");
        head.extend_from_slice(&16u32.to_le_bytes());
        head.extend_from_slice(&1u16.to_le_bytes());
        head.extend_from_slice(&(ch as u16).to_le_bytes());
        head.extend_from_slice(&self.rate.to_le_bytes());
        head.extend_from_slice(&(self.rate * ch as u32 * 2).to_le_bytes());
        head.extend_from_slice(&(ch as u16 * 2).to_le_bytes());
        head.extend_from_slice(&16u16.to_le_bytes());
        head.extend_from_slice(b"data");
        head.extend_from_slice(&data_len.to_le_bytes());
        w.write_all(&head)?;

        let mut buf: Vec<u8> = Vec::with_capacity(1 << 16);
        let mut put = |w: &mut dyn std::io::Write, s: f32| -> std::io::Result<()> {
            buf.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            if buf.len() >= 1 << 16 {
                w.write_all(&buf)?;
                buf.clear();
            }
            Ok(())
        };
        let per_ms = self.rate as f64 / 1000.0;
        let fade_in = ((FADE_IN_MS * per_ms) as usize).max(1);
        for part in &self.parts {
            match *part {
                Part::Silence(n) => {
                    for _ in 0..n * ch {
                        put(w, 0.0)?;
                    }
                }
                Part::Stretched { frames: n, factor, fade_from } => {
                    let out_from = ((fade_from * per_ms) / factor).min(n as f64);
                    for i in 0..n {
                        let pos = i as f64 * factor;
                        let k = pos.floor() as usize;
                        let frac = (pos - k as f64) as f32;
                        let rise = (i as f64 / fade_in as f64).min(1.0);
                        let fall = if (i as f64) < out_from { 1.0 } else { 0.5 * (1.0 + (std::f64::consts::PI * (i as f64 - out_from) / (n as f64 - out_from).max(1.0)).cos()) };
                        let gain = (rise * fall) as f32;
                        for c in 0..ch {
                            let a = src.samples.get(k * ch + c).copied().unwrap_or(0.0);
                            let b = src.samples.get((k + 1) * ch + c).copied().unwrap_or(a);
                            put(w, (a + (b - a) * frac) * gain)?;
                        }
                    }
                }
            }
        }
        w.write_all(&buf)
    }
}

// from and to are map milliseconds
fn decode(file: &Path, from_ms: f64, to_ms: f64) -> Result<Pcm, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let bad = |e: Error| format!("Couldn't read the song's audio ({e}).");
    let src = std::fs::File::open(file).map_err(|e| format!("Couldn't open the song's audio ({e})."))?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());
    let mut hint = Hint::new();
    // lazer's copies have no extension, the probe looks at the bytes
    if let Some(ext) = file.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    // gapless: no encoder delay at the start, as osu!'s audio library plays it
    let opts = FormatOptions { enable_gapless: true, ..Default::default() };
    let probed = symphonia::default::get_probe().format(&hint, mss, &opts, &MetadataOptions::default()).map_err(bad)?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("The song's audio has no sound in it.")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(bad)?;
    let time_base = track.codec_params.time_base;
    let mut rate = track.codec_params.sample_rate.unwrap_or(44100);
    let mut channels = track.codec_params.channels.map_or(2, |c| c.count());
    let mut samples: Vec<f32> = vec![];
    let mut seen_frames: u64 = 0;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(bad(e)),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => Some(buf),
            Err(Error::DecodeError(_)) => None,
            Err(e) => return Err(bad(e)),
        };
        if let Some(buf) = &buf {
            rate = buf.spec().rate;
            channels = buf.spec().channels.count();
        }
        // a packet that won't decode still takes its time in the song, as silence
        let frames = match &buf {
            Some(buf) => buf.frames() as u64,
            None => time_base.map_or(packet.dur, |tb| {
                let t = tb.calc_time(packet.dur);
                ((t.seconds as f64 + t.frac) * rate as f64).round() as u64
            }),
        };
        let start_ms = seen_frames as f64 * 1000.0 / rate as f64;
        let end_ms = (seen_frames + frames) as f64 * 1000.0 / rate as f64;
        seen_frames += frames;
        if end_ms < from_ms {
            continue;
        }
        if start_ms > to_ms {
            break;
        }
        let decoded = buf.map(|buf| {
            let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, *buf.spec());
            sb.copy_interleaved_ref(buf);
            sb
        });
        let len = decoded.as_ref().map_or(frames as usize * channels, |sb| sb.samples().len());
        let skip = if start_ms < from_ms { (((from_ms - start_ms) / 1000.0) * rate as f64) as usize * channels } else { 0 };
        let keep = if end_ms > to_ms { ((((to_ms - start_ms) / 1000.0) * rate as f64) as usize * channels).min(len) } else { len };
        if skip < keep {
            match &decoded {
                Some(sb) => samples.extend_from_slice(&sb.samples()[skip..keep]),
                None => samples.resize(samples.len() + keep - skip, 0.0),
            }
        }
    }
    if samples.is_empty() {
        return Err("The song's audio ends before that stretch.".into());
    }
    if channels > 2 {
        samples = samples.chunks(channels).flat_map(|f| [f[0], f[1]]).collect();
        channels = 2;
    }
    Ok(Pcm { rate, channels, samples })
}

pub(crate) fn breakdown(p: &Practice, j: &crate::analysis::Judged) -> Value {
    let mode = j.mode;
    let rows: Vec<Value> = p
        .repeats
        .iter()
        .enumerate()
        .map(|(k, r)| {
            let mut units = [0u32; 7];
            let mut notes = 0;
            for (n, js) in j.notes.iter().zip(&j.judgments) {
                if n.time >= r.start && n.time <= r.end {
                    notes += 1;
                    for &u in js {
                        units[u as usize] += 1;
                    }
                }
            }
            let acc = (units[1..].iter().sum::<u32>() > 0).then(|| crate::judge::accuracy(&units, mode));
            json!({
                "repeat": k + 1,
                "speed": (r.speed * 100.0).round() / 100.0,
                "notes": notes,
                "acc": acc.map(|a| (a * 100.0).round() / 100.0),
                "misses": units[6],
                "met": match (p.goal, acc) { (Some(g), Some(a)) => r.speed >= 0.999 && a >= g, _ => false },
            })
        })
        .collect();
    json!({
        "from": p.from,
        "to": p.to,
        "goal": p.goal,
        "fromPlay": p.from_play,
        "repeats": rows,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    // makes a practice map from a real play in a data folder, without opening osu!:
    // HASHI_DATA=%APPDATA%\Hashi HASHI_PLAY=<id> cargo test -p mania-bridge-helper practice_from_real_play -- --ignored --nocapture
    #[test]
    #[ignore]
    fn practice_from_real_play() {
        let (Some(data), Some(id)) = (std::env::var_os("HASHI_DATA"), std::env::var("HASHI_PLAY").ok()) else { return };
        let data = std::path::PathBuf::from(data);
        let queue: serde_json::Value = serde_json::from_slice(&std::fs::read(data.join("queue.json")).unwrap()).unwrap();
        let raw = queue["items"].as_array().unwrap().iter().find(|i| i["id"] == id.as_str()).unwrap().clone();
        let item: crate::types::Item = serde_json::from_value(raw).unwrap();
        let blob = |b: &crate::types::Blob| std::fs::read(data.join("blobs").join(&b.sha256)).unwrap();
        let (replay, chart) = (blob(item.replay.as_ref().unwrap()), blob(item.chart.as_ref().unwrap()));
        let source = std::path::PathBuf::from(item.chart.as_ref().unwrap().source.clone().unwrap());
        let audio = source.parent().unwrap().join(super::general_value(&String::from_utf8_lossy(&chart), "AudioFilename").unwrap());
        let out = std::env::temp_dir().join("hashi-practice-test");
        std::fs::create_dir_all(&out).unwrap();
        let from: f64 = std::env::var("HASHI_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(30.0);
        let o = super::Options { from, to: from + 8.0, repeats: 4, slowest: 0.85, lead: 2.0, goal: Some(97.0) };
        let started = std::time::Instant::now();
        let p = super::render(&item, &replay, &chart, item.play.is_lazer(), &audio, None, &o, &out).unwrap();
        println!("made {} in {:?}: {}", p.version, started.elapsed(), p.file);

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&p.file).unwrap()).unwrap();
        let names: Vec<String> = zip.file_names().map(String::from).collect();
        println!("osz: {names:?}");
        let osu_name = names.iter().find(|n| n.ends_with(".osu")).unwrap().clone();
        let mut osu = String::new();
        zip.by_name(&osu_name).unwrap().read_to_string(&mut osu).unwrap();
        let mut wav = vec![];
        zip.by_name(super::AUDIO_FILE).unwrap().read_to_end(&mut wav).unwrap();
        let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap()) as f64;
        let ch = u16::from_le_bytes(wav[22..24].try_into().unwrap()) as f64;
        let wav_ms = (wav.len() - 44) as f64 / 2.0 / ch / rate * 1000.0;

        let parsed = crate::beatmap::parse_chart(&osu).unwrap();
        assert_eq!(crate::replays::md5_hex(osu.as_bytes()), p.md5);
        let (a, b) = (o.from * 1000.0, o.to * 1000.0);
        let orig = crate::analysis::judge_play(&replay, &chart, item.play.is_lazer()).unwrap();
        let in_stretch = orig.notes.iter().filter(|n| n.time >= a * orig.rate && n.time < b * orig.rate).count();
        println!("{} notes in the stretch, {} in the practice map, {} repeats", in_stretch, parsed.notes.len(), p.repeats.len());
        assert_eq!(parsed.notes.len(), in_stretch * 4);
        for (k, r) in p.repeats.iter().enumerate() {
            let inside = parsed.notes.iter().filter(|n| n.time >= r.start && n.time <= r.end).count();
            println!("repeat {} at {:.0}% speed: {:.0}-{:.0} ms, {inside} notes", k + 1, r.speed * 100.0, r.start, r.end);
            assert_eq!(inside, in_stretch);
        }
        // the gap between two notes stretches by the speed: same notes, so same ratio
        let first_two = |r: &super::Repeat| {
            let v: Vec<f64> = parsed.notes.iter().filter(|n| n.time >= r.start && n.time <= r.end).map(|n| n.time).collect();
            v[v.len() - 1] - v[0]
        };
        let (slow, full) = (first_two(&p.repeats[0]), first_two(&p.repeats[3]));
        println!("stretch length: {:.0} ms at 85%, {:.0} ms at 100% (ratio {:.3})", slow, full, full / slow);
        assert!((full / slow - 0.85).abs() < 0.01);
        let last_end = p.repeats.last().unwrap().end;
        println!("audio {:.0} ms, last note by {:.0} ms", wav_ms, last_end);
        assert!(wav_ms > last_end);
        let reds = osu.lines().filter(|l| l.ends_with(",1,0")).count();
        println!("timing lines: {} ({} red)", osu.lines().skip_while(|l| *l != "[TimingPoints]").skip(1).take_while(|l| !l.is_empty()).count(), reds);
    }
}
