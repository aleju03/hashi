use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::beatmap::parse_int;
use crate::practice::{decode, decode_each, general_value, or, sanitize, Chunk, Mix};
use crate::{log, Bridge};

// see practice::is_practice
pub(crate) const MARK: &str = "Hashi cut";
const AUDIO_FILE: &str = "hashi-cut.wav";
const START_SILENCE_MS: f64 = 1000.0;
const LEAD_MS: f64 = 1500.0;
const HOLD_MS: f64 = 500.0;
const FADE_MS: f64 = 1500.0;
const SAMPLES_MAX_BYTES: u64 = 64 << 20;
const SHORTEST_MS: f64 = 1000.0;

struct Source {
    md5: String,
    chart: PathBuf,
    audio: PathBuf,
    background: Option<PathBuf>,
    lazer: bool,
}

// by md5, so the map cut is the one the page shows even after another is selected
static SOURCES: Mutex<Vec<Arc<Source>>> = Mutex::new(Vec::new());

fn remember(s: Source) {
    let mut all = SOURCES.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|x| x.md5 != s.md5);
    if all.len() >= 8 {
        all.remove(0);
    }
    all.push(Arc::new(s));
}

fn source(md5: &str) -> Option<Arc<Source>> {
    SOURCES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|s| s.md5 == md5).cloned()
}

fn selected_background(b: &Bridge) -> Option<PathBuf> {
    let (rel, folder, lazer_dir) = {
        let s = b.st();
        let frame = s.tosu.last_frame.as_ref()?;
        let text = |p: &str| frame.pointer(p).and_then(Value::as_str).filter(|x| !x.is_empty()).map(String::from);
        (text("/files/background")?, text("/folders/beatmap"), (s.tosu.client.as_deref() == Some("lazer")).then(|| s.tosu.lazer_files_dir.clone()))
    };
    if [rel.as_str(), folder.as_deref().unwrap_or("")].iter().any(|p| p.split(['/', '\\']).any(|c| c == "..")) {
        return None;
    }
    let f = match lazer_dir {
        Some(dir) => dir.join(rel),
        None => b.songs_dir().join(folder?.replace('\\', "/")).join(rel.replace('\\', "/")),
    };
    f.is_file().then_some(f)
}

pub(crate) async fn map(b: &Arc<Bridge>) -> (u16, Value) {
    let Some(chart) = crate::skin::selected_chart(b) else { return (404, json!({ "error": "Select a map in osu! first." })) };
    let lazer = b.st().tosu.client.as_deref() == Some("lazer");
    let Ok(bytes) = crate::read_capped(&chart, crate::MAX_CHART_BYTES).await else { return (404, json!({ "error": "Couldn't read the map." })) };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let parsed = match crate::beatmap::parse_chart(&text) {
        Ok(c) if !c.notes.is_empty() => c,
        Ok(_) => return (422, json!({ "error": "This map has no notes." })),
        Err(_) => return (422, json!({ "error": "That isn't a mania map." })),
    };
    let md5 = crate::replays::md5_hex(&bytes);
    let Some(audio) = crate::practice::selected_audio(b, &md5) else { return (404, json!({ "error": "Couldn't find this map's song." })) };
    let background = selected_background(b);
    remember(Source { md5: md5.clone(), chart, audio, background, lazer });
    let meta = |k: &str| general_value(&text, k);
    let notes: Vec<Value> = parsed.notes.iter().map(|n| json!([n.column, n.time, if n.is_hold { json!(n.end_time) } else { Value::Null }])).collect();
    (
        200,
        json!({
            "md5": md5,
            "keys": parsed.keys,
            "od": parsed.od,
            "artist": meta("Artist"),
            "title": meta("Title"),
            "version": meta("Version"),
            "notes": notes,
        }),
    )
}

// [rate u32][channels u32][mono i16...], so the page doesn't need a decoder for the format
pub(crate) async fn song(md5: &str) -> Option<Vec<u8>> {
    let src = source(md5)?;
    tokio::task::spawn_blocking(move || {
        let mut out = vec![0u8; 8];
        let rate = decode_each(&src.audio, 0.0, f64::INFINITY, |chunk, ch| {
            let ch = ch.max(1);
            match chunk {
                Chunk::Samples(s) => {
                    for frame in s.chunks(ch) {
                        let m = frame.iter().sum::<f32>() / ch as f32;
                        out.extend_from_slice(&((m.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                    }
                }
                Chunk::Silence(n) => out.resize(out.len() + n / ch * 2, 0),
            }
        })
        .map_err(|e| log!("cut: {e}"))
        .ok()?;
        out[..4].copy_from_slice(&rate.to_le_bytes());
        out[4..8].copy_from_slice(&1u32.to_le_bytes());
        Some(out)
    })
    .await
    .ok()
    .flatten()
}

// from and to are map ms, notes whose heads fall between them are kept
pub(crate) async fn make(b: &Arc<Bridge>, md5: &str, from: f64, to: f64, name: Option<String>) -> (u16, Value) {
    let Some(src) = source(md5) else { return (404, json!({ "error": "This map isn't open here anymore. Open it again." })) };
    if !(from.is_finite() && to.is_finite() && from >= 0.0 && to - from >= SHORTEST_MS) {
        return (400, json!({ "error": "Pick a stretch at least a second long." }));
    }
    let bytes = match crate::read_capped(&src.chart, crate::MAX_CHART_BYTES).await {
        Ok(b) if crate::replays::md5_hex(&b) == src.md5 => b,
        _ => return (409, json!({ "error": "The map changed since it was opened here. Open it again." })),
    };
    let out_dir = b.env.data_dir.join("cuts");
    if let Err(e) = crate::create_private_dir(&out_dir).await {
        return (500, json!({ "error": e.to_string() }));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let src2 = src.clone();
    let name = name.map(|n| clean_name(&n)).filter(|n| !n.is_empty());
    let made = tokio::task::spawn_blocking(move || render(&text, &src2, from, to, name.as_deref(), &out_dir)).await.unwrap_or_else(|_| Err("Couldn't make the map.".into()));
    match made {
        Ok(m) => {
            log!("cut {} made, {} notes", m.version, m.notes);
            remember_cut(b, &m.md5).await;
            crate::practice::open_in_osu(b, src.lazer, Path::new(&m.file));
            (200, json!({ "version": m.version, "file": m.file, "notes": m.notes }))
        }
        Err(e) => {
            log!("cut: {e}");
            (422, json!({ "error": e }))
        }
    }
}

// plays of a cut stay in Hashi, a named one can't be told apart by its version
const LIST_FILE: &str = "cuts.json";

pub(crate) fn is_cut(b: &Bridge, md5: &str) -> bool {
    let list: Vec<String> = std::fs::read(b.env.data_dir.join(LIST_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    list.iter().any(|m| m == md5)
}

async fn remember_cut(b: &Bridge, md5: &str) {
    static WRITING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _writing = WRITING.lock().await;
    let file = b.env.data_dir.join(LIST_FILE);
    let mut list: Vec<String> = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    if list.iter().any(|m| m == md5) {
        return;
    }
    list.push(md5.to_string());
    if let Err(e) = crate::write_json_file(&file, &json!(list)).await {
        log!("cut: couldn't save the list of cuts ({e})");
    }
}

fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(80).collect()
}

struct Made {
    md5: String,
    version: String,
    file: String,
    notes: usize,
}

struct Object {
    time: f64,
    end: f64,
    hold: bool,
    fields: Vec<String>,
}

impl Object {
    // the hit sample's file name, last in the last field, which a hold's end time shares
    fn sample_file(&self) -> Option<String> {
        let last = self.fields.last()?;
        let rest = if self.hold { last.split_once(':')?.1 } else { last.as_str() };
        rest.splitn(5, ':').nth(4).map(|f| f.trim().to_lowercase()).filter(|f| !f.is_empty())
    }
}

fn section<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let head = format!("[{name}]");
    let mut inside = false;
    let mut out = vec![];
    for line in text.lines().map(|l| l.trim_end_matches('\r')) {
        if line.starts_with('[') {
            inside = line.trim() == head;
            continue;
        }
        if inside && !line.trim().is_empty() && !line.starts_with("//") {
            out.push(line);
        }
    }
    out
}

fn objects(text: &str) -> Vec<Object> {
    section(text, "HitObjects")
        .into_iter()
        .filter_map(|line| {
            // read as beatmap::parse_chart does, so the notes are the ones the page showed
            let fields: Vec<String> = line.split(',').map(|f| f.trim().to_string()).collect();
            if fields.len() < 5 {
                return None;
            }
            let time = parse_int(&fields[2]);
            if !time.is_finite() {
                return None;
            }
            let hold = parse_int(&fields[3]) as i64 & 128 != 0;
            let end = match fields.get(5).filter(|_| hold).map(|f| parse_int(f.split(':').next().unwrap_or(""))) {
                Some(e) if e.is_finite() && e != 0.0 => e,
                _ => time,
            };
            Some(Object { time, end, hold: hold && fields.len() >= 6, fields })
        })
        .collect()
}

#[derive(Clone)]
struct Point {
    time: f64,
    beat: f64,
    red: bool,
    fields: Vec<String>,
}

fn points(text: &str) -> Vec<Point> {
    let mut out: Vec<Point> = section(text, "TimingPoints")
        .into_iter()
        .filter_map(|line| {
            let fields: Vec<String> = line.split(',').map(|f| f.trim().to_string()).collect();
            let time = fields.first()?.parse::<f64>().ok()?;
            let beat = fields.get(1)?.parse::<f64>().ok()?;
            let red = fields.get(6).map_or(beat > 0.0, |x| x == "1") && beat > 0.0;
            Some(Point { time, beat, red, fields })
        })
        .collect();
    out.sort_by(|a, b| a.time.total_cmp(&b.time).then(b.red.cmp(&a.red)));
    out
}

fn most_common_beat(points: &[Point], end: f64) -> f64 {
    crate::practice::longest_beat(points.iter().filter(|p| p.red).map(|p| (p.time, p.beat)), end)
}

fn number(t: f64) -> String {
    let r = (t * 1000.0).round() / 1000.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}

// map_end and cut_end are where each one's last note ends, what osu! weighs beat lengths up to
fn timing(points: &[Point], seg_a: f64, seg_b: f64, map_end: f64, cut_end: f64, shift: impl Fn(f64) -> f64) -> Result<Vec<String>, String> {
    let mut placed: Vec<Point> = vec![];
    let first_red = points.iter().filter(|p| p.red && p.time <= seg_a).last().or(points.iter().find(|p| p.red)).ok_or("The map has no timing.")?;
    if first_red.time <= seg_a {
        // whole beats, to keep the beat phase. It can come out before 0, which osu! takes
        let at = shift(first_red.time + ((seg_a - first_red.time) / first_red.beat).floor() * first_red.beat);
        placed.push(Point { time: at, ..first_red.clone() });
        if let Some(g) = points.iter().filter(|p| !p.red && p.time <= seg_a && p.time >= first_red.time).last() {
            placed.push(Point { time: at, ..g.clone() });
        }
    }
    placed.extend(points.iter().filter(|p| p.time > seg_a && p.time < seg_b).map(|p| Point { time: shift(p.time), ..p.clone() }));

    // the cut can have a different most common beat, rescale the greens to keep note velocity
    let ratio = most_common_beat(points, map_end) / most_common_beat(&placed, cut_end);
    let same = (ratio - 1.0).abs() < 1e-6;
    let mut lines = vec![];
    for (k, p) in placed.iter().enumerate() {
        let mut fields = p.fields.clone();
        fields[0] = number(p.time);
        if !p.red && !same {
            fields[1] = number(p.beat / ratio);
        }
        lines.push(fields.join(","));
        let green_follows = placed.get(k + 1).is_some_and(|n| !n.red && (n.time - p.time).abs() < 0.5);
        if p.red && !same && !green_follows {
            let mut g = p.fields.clone();
            g.resize(8, "0".into());
            g[0] = number(p.time);
            g[1] = number(-100.0 / ratio);
            g[6] = "0".into();
            lines.push(g.join(","));
        }
    }
    Ok(lines)
}

fn render(text: &str, src: &Source, from: f64, to: f64, name: Option<&str>, out_dir: &Path) -> Result<Made, String> {
    let all = objects(text);
    let kept: Vec<&Object> = all.iter().filter(|o| o.time >= from && o.time <= to).collect();
    if kept.is_empty() {
        return Err("There are no notes in that stretch.".into());
    }
    let first = kept.iter().map(|o| o.time).fold(f64::MAX, f64::min);
    let last = kept.iter().map(|o| o.end).fold(f64::MIN, f64::max);
    let seg_a = from.min(first - LEAD_MS).max(0.0);
    let hold_until = to.max(last) + HOLD_MS;
    let seg_b = hold_until + FADE_MS;
    let pcm = decode(&src.audio, seg_a, seg_b)?;
    let mut mix = Mix::new(pcm.rate, pcm.channels);
    mix.silence(START_SILENCE_MS);
    mix.stretched(&pcm, 1.0, hold_until - seg_a);
    let shift = |t: f64| t - seg_a + START_SILENCE_MS;

    let hit_objects: Vec<String> = kept
        .iter()
        .map(|o| {
            let mut f = o.fields.clone();
            f[2] = format!("{}", shift(o.time).round() as i64);
            if o.hold {
                let tail = f[5].split_once(':').map_or(String::new(), |(_, r)| format!(":{r}"));
                f[5] = format!("{}{tail}", shift(o.end).round() as i64);
            }
            f.join(",")
        })
        .collect();
    let map_end = all.iter().map(|o| o.end).fold(0.0, f64::max);
    let timing_lines = timing(&points(text), seg_a, seg_b, map_end, shift(last), shift)?;

    let meta = |k: &str| general_value(text, k).unwrap_or_default();
    let clock = |ms: f64| {
        let s = (ms / 1000.0).max(0.0) as u64;
        format!("{}:{:02}", s / 60, s % 60)
    };
    let last_head = kept.iter().map(|o| o.time).fold(f64::MIN, f64::max);
    let version = name.map_or_else(|| format!("{MARK} {} {}-{}", meta("Version"), clock(first), clock(last_head)), String::from);
    let difficulty = section(text, "Difficulty").join("\n");
    // by its bytes, lazer's files have no extension
    let bg = src
        .background
        .as_ref()
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() <= crate::MAX_IMAGE_BYTES))
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| {
            let ext = match crate::replays::sniff_image(&bytes)? {
                "image/png" => "png",
                "image/bmp" => "bmp",
                _ => "jpg",
            };
            Some((format!("bg.{ext}"), bytes))
        });
    let bg_name = bg.as_ref().map(|(n, _)| n.clone());
    let osu = format!(
        "osu file format v14\n\n[General]\nAudioFilename: {AUDIO_FILE}\nAudioLeadIn: 0\nPreviewTime: -1\nCountdown: 0\nSampleSet: {}\nStackLeniency: 0.7\nMode: 3\nLetterboxInBreaks: 0\nSpecialStyle: {}\nWidescreenStoryboard: 0\n\n\
         [Metadata]\nTitle:{}\nTitleUnicode:{}\nArtist:{}\nArtistUnicode:{}\nCreator:{}\nVersion:{version}\nSource:{}\nTags:hashi-cut\nBeatmapID:0\nBeatmapSetID:-1\n\n\
         [Difficulty]\n{difficulty}\n\n\
         [Events]\n//Background and Video events\n{}//Break Periods\n//Storyboard Layer 0 (Background)\n\n\
         [TimingPoints]\n{}\n\n\n[HitObjects]\n{}\n",
        or(meta("SampleSet"), "Normal".into()),
        or(meta("SpecialStyle"), "0".into()),
        meta("Title"),
        or(meta("TitleUnicode"), meta("Title")),
        meta("Artist"),
        or(meta("ArtistUnicode"), meta("Artist")),
        meta("Creator"),
        meta("Source"),
        bg_name.as_ref().map_or(String::new(), |n| format!("0,0,\"{n}\",0,0\n")),
        timing_lines.join("\n"),
        hit_objects.join("\n"),
    );
    let md5 = crate::replays::md5_hex(osu.as_bytes());
    let name = sanitize(&format!("{} - {} ({}) [{}]", meta("Artist"), meta("Title"), meta("Creator"), version));
    let stem: String = sanitize(&format!("{} - {}", meta("Artist"), meta("Title"))).chars().take(141).collect();
    let file = out_dir.join(format!("{} {}.osz", stem.trim_end(), &md5[..8]));
    let bad = |e: &dyn std::fmt::Display| format!("Couldn't write the map ({e}).");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&file).map_err(|e| bad(&e))?);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default();
    zip.start_file(format!("{name}.osu"), deflated).map_err(|e| bad(&e))?;
    zip.write_all(osu.as_bytes()).map_err(|e| bad(&e))?;
    zip.start_file(AUDIO_FILE, deflated).map_err(|e| bad(&e))?;
    mix.write_wav(&pcm, &mut zip).map_err(|e| bad(&e))?;
    if let Some((n, bytes)) = &bg {
        zip.start_file(n, stored).map_err(|e| bad(&e))?;
        zip.write_all(bytes).map_err(|e| bad(&e))?;
    }
    // lazer stores set files by hash
    if !src.lazer {
        let named: Vec<String> = kept.iter().filter_map(|o| o.sample_file()).collect();
        let mut room = SAMPLES_MAX_BYTES;
        for (path, name) in samples(src.chart.parent().unwrap_or(Path::new(""))) {
            let lower = name.to_lowercase();
            if lower == AUDIO_FILE || !(is_hitsound(&lower) || named.contains(&lower)) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if bytes.len() as u64 > room {
                break;
            }
            room -= bytes.len() as u64;
            zip.start_file(&name, if lower.ends_with(".wav") { deflated } else { stored }).map_err(|e| bad(&e))?;
            zip.write_all(&bytes).map_err(|e| bad(&e))?;
        }
    }
    zip.finish().map_err(|e| bad(&e))?;
    Ok(Made { md5, version, file: file.to_string_lossy().into_owned(), notes: kept.len() })
}

// soft-hitclap2.wav and the like. Anything else has to be named by a kept note: dan sets keep their
// other songs in the same folder
fn is_hitsound(name: &str) -> bool {
    let Some((stem, _)) = name.rsplit_once('.') else { return false };
    let Some((set, rest)) = stem.split_once('-') else { return false };
    let kind = rest.trim_end_matches(|c: char| c.is_ascii_digit());
    ["normal", "soft", "drum"].contains(&set) && ["hitnormal", "hitwhistle", "hitfinish", "hitclap", "slidertick", "sliderslide", "sliderwhistle"].contains(&kind)
}

fn samples(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<(PathBuf, String)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ext = Path::new(&name).extension()?.to_string_lossy().to_lowercase();
            ["wav", "ogg", "mp3"].contains(&ext.as_str()).then(|| (e.path(), name))
        })
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = "osu file format v14\n\n[TimingPoints]\n0,500,4,1,0,60,1,0\n10000,-50,4,1,0,60,0,1\n20000,250,4,1,0,60,1,0\n\n[HitObjects]\n64,192,1000,1,0,0:0:0:0:\n64,192,21000,128,0,22000:0:0:0:0:\n";

    #[test]
    fn keeps_the_beat_and_the_scroll_speed() {
        let pts = points(MAP);
        // from 12.3 s: the red line moves to the last whole beat before it, the green stays in effect
        let shift = |t: f64| t - 12300.0 + START_SILENCE_MS;
        let lines = timing(&pts, 12300.0, 30000.0, 22000.0, shift(30000.0), shift).unwrap();
        assert_eq!(lines[0], "700,500,4,1,0,60,1,0");
        // the cut's longest beat is 250 against the whole map's 500, so every speed doubles back
        assert_eq!(lines[1], "700,-25,4,1,0,60,0,1");
        assert_eq!(lines[2], "8700,250,4,1,0,60,1,0");
        assert_eq!(lines[3], "8700,-50,4,1,0,60,0,0");
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn a_cut_of_one_tempo_is_left_as_it_was() {
        let pts = points(MAP);
        let lines = timing(&pts, 500.0, 9000.0, 22000.0, 9000.0, |t| t - 500.0 + START_SILENCE_MS).unwrap();
        assert_eq!(lines, ["1000,500,4,1,0,60,1,0"]);
    }

    // cuts a map on disk without opening osu!, from and to in seconds:
    // HASHI_CUT=<.osu> HASHI_FROM=60 HASHI_TO=90 cargo test -p mania-bridge-helper cut_real_map -- --ignored --nocapture
    #[test]
    #[ignore]
    fn cut_real_map() {
        use std::io::Read as _;
        let Some(osu) = std::env::var_os("HASHI_CUT").map(PathBuf::from) else { return };
        let secs = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(d) * 1000.0;
        let (from, to) = (secs("HASHI_FROM", 60.0), secs("HASHI_TO", 90.0));
        let text = std::fs::read_to_string(&osu).unwrap();
        let dir = osu.parent().unwrap();
        let audio = dir.join(general_value(&text, "AudioFilename").unwrap());
        let src = Source { md5: String::new(), chart: osu.clone(), audio, background: None, lazer: false };
        let out = std::env::temp_dir().join("hashi-cut-test");
        std::fs::create_dir_all(&out).unwrap();
        let made = render(&text, &src, from, to, std::env::var("HASHI_NAME").ok().as_deref(), &out).unwrap();
        println!("made {} with {} notes: {}", made.version, made.notes, made.file);

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&made.file).unwrap()).unwrap();
        let names: Vec<String> = zip.file_names().map(String::from).collect();
        println!("osz: {names:?}");
        let name = names.iter().find(|n| n.ends_with(".osu")).unwrap().clone();
        let mut cut = String::new();
        zip.by_name(&name).unwrap().read_to_string(&mut cut).unwrap();
        let before = crate::beatmap::parse_chart(&text).unwrap();
        let after = crate::beatmap::parse_chart(&cut).unwrap();
        let inside: Vec<_> = before.notes.iter().filter(|n| n.time >= from && n.time <= to).collect();
        assert_eq!(after.notes.len(), inside.len());
        let shift = after.notes[0].time - inside[0].time;
        for (a, b) in after.notes.iter().zip(&inside) {
            assert_eq!(a.column, b.column);
            assert!((a.time - b.time - shift).abs() <= 1.0);
            assert!(((a.end_time - a.time) - (b.end_time - b.time)).abs() <= 1.0);
        }
        println!("first note at {} ms in the cut, {} ms in the map", after.notes[0].time, inside[0].time);
        println!("{}", cut.lines().skip_while(|l| *l != "[TimingPoints]").take_while(|l| *l != "[HitObjects]").collect::<Vec<_>>().join("\n"));
    }

    #[test]
    fn hold_keysounds_and_zero_length_holds() {
        let o = objects("[HitObjects]\n64,192,21000,128,0,21000:0:0:0:0:ln_kick.wav\n64,192,500,1,0,0:0:0:50:K.wav\n64,192,600.7,1,0,0:0:0:0:\n");
        assert!(o[0].hold && o[0].end == o[0].time);
        assert_eq!(o[0].sample_file().as_deref(), Some("ln_kick.wav"));
        assert_eq!(o[1].sample_file().as_deref(), Some("k.wav"));
        // whole milliseconds, as the page's notes have them
        assert_eq!(o[2].time, 600.0);
    }

    #[test]
    fn takes_hitsounds_but_not_songs() {
        assert!(is_hitsound("soft-hitclap2.wav"));
        assert!(is_hitsound("normal-hitnormal.ogg"));
        assert!(!is_hitsound("audio.mp3"));
        assert!(!is_hitsound("ln_zenith.mp3"));
        assert!(!is_hitsound("soft-hitclapper.wav"));
    }

    #[test]
    fn hold_ends_move_with_their_heads() {
        let o = objects(MAP);
        assert_eq!((o[1].time, o[1].end), (21000.0, 22000.0));
        assert_eq!(o.len(), 2);
    }
}
