use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::beatmap::parse_int;
use crate::cut::{number, Source};
use crate::practice::{decode, general_value, sanitize, wav_header, Pcm};
use crate::{log, Bridge};

const AUDIO_FILE: &str = "audio.wav";
pub(crate) const SLOWEST: f64 = 0.5;
pub(crate) const FASTEST: f64 = 2.0;
pub(crate) const RANGE_ERROR: &str = "Pick a rate from 0.5x to 2x.";

// to 0.01x, None outside what the stretch handles
pub(crate) fn checked(speed: f64) -> Option<f64> {
    let s = (speed * 100.0).round() / 100.0;
    (SLOWEST..=FASTEST).contains(&s).then_some(s)
}

// " 1.2x", nothing at 1x
pub(crate) fn suffix(speed: f64) -> String {
    if speed == 1.0 { String::new() } else { format!(" {}x", number(speed)) }
}

pub(crate) async fn map(b: &Arc<Bridge>) -> (u16, Value) {
    if b.st().tosu.client.as_deref() == Some("lazer") {
        return (422, json!({ "error": "Use Rate Adjust in lazer." }));
    }
    let s = match crate::cut::load_selected(b).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let meta = |k: &str| general_value(&s.text, k);
    let length = s.chart.notes.iter().map(|n| n.end_time.max(n.time)).fold(0.0, f64::max);
    let beat = crate::practice::longest_beat(s.chart.timing_points.iter().map(|p| (p.time, p.beat_length)), length);
    (
        200,
        json!({
            "md5": s.md5,
            "keys": s.chart.keys,
            "artist": meta("Artist"),
            "title": meta("Title"),
            "version": meta("Version"),
            "notes": s.chart.notes.len(),
            "length": length,
            "bpm": (60000.0 / beat * 100.0).round() / 100.0,
        }),
    )
}

pub(crate) async fn make(b: &Arc<Bridge>, md5: &str, rate: f64, keep_pitch: bool) -> (u16, Value) {
    let Some(src) = crate::cut::source(md5) else { return (404, json!({ "error": "This map isn't open here anymore. Open it again." })) };
    let Some(rate) = checked(rate).filter(|r| *r != 1.0) else { return (400, json!({ "error": "Pick a rate from 0.5x to 2x, other than 1x." })) };
    let bytes = match crate::read_capped(&src.chart, crate::MAX_CHART_BYTES).await {
        Ok(b) if crate::replays::md5_hex(&b) == src.md5 => b,
        _ => return (409, json!({ "error": "The map changed since it was opened here. Open it again." })),
    };
    let out_dir = b.env.data_dir.join("rates");
    if let Err(e) = crate::create_private_dir(&out_dir).await {
        return (500, json!({ "error": e.to_string() }));
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let src2 = src.clone();
    let made = tokio::task::spawn_blocking(move || render(&text, &src2, rate, keep_pitch, &out_dir)).await.unwrap_or_else(|_| Err("Couldn't make the map.".into()));
    match made {
        Ok((version, file, md5)) => {
            log!("rate {version} made");
            crate::practice::open_in_osu(b, src.lazer, Path::new(&file));
            (200, json!({ "version": version, "file": file, "md5": md5 }))
        }
        Err(e) => {
            log!("rate: {e}");
            (422, json!({ "error": e }))
        }
    }
}

fn render(text: &str, src: &Source, rate: f64, keep_pitch: bool, out_dir: &Path) -> Result<(String, String, String), String> {
    let meta = |k: &str| general_value(text, k).unwrap_or_default();
    let version = format!("{}{}", meta("Version"), suffix(rate)).trim_start().to_string();
    let bg = crate::cut::background(src);
    let osu = rewrite(text, rate, &version, bg.as_ref().map(|(n, _)| n.as_str()));
    let pcm = decode(&src.audio, 0.0, f64::INFINITY)?;

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
    write_audio(&pcm, rate, keep_pitch, &mut zip).map_err(|e| bad(&e))?;
    drop(pcm);
    if let Some((n, bytes)) = &bg {
        zip.start_file(n, stored).map_err(|e| bad(&e))?;
        zip.write_all(bytes).map_err(|e| bad(&e))?;
    }
    let mut named: Vec<String> = crate::cut::objects(&osu).iter().filter_map(|o| o.sample_file()).collect();
    named.extend(crate::cut::section(&osu, "Events").into_iter().filter_map(event_sample));
    crate::cut::add_samples(&mut zip, src, &named, AUDIO_FILE).map_err(|e| bad(&e))?;
    zip.finish().map_err(|e| bad(&e))?;
    Ok((version, file.to_string_lossy().into_owned(), md5))
}

fn event_sample(line: &str) -> Option<String> {
    let f: Vec<&str> = line.split(',').collect();
    (["5", "Sample"].contains(&f[0]) && f.len() >= 4).then(|| f[3].trim().trim_matches('"').to_lowercase())
}

// every time in the map divided by the rate, the rest kept as it was
fn rewrite(text: &str, rate: f64, version: &str, bg: Option<&str>) -> String {
    let at = |t: f64| t / rate;
    let whole = |s: &str| {
        let t = parse_int(s);
        t.is_finite().then(|| format!("{}", at(t).round() as i64))
    };
    let mut out = String::with_capacity(text.len());
    let mut section = String::new();
    for line in text.lines().map(|l| l.trim_end_matches('\r')) {
        let head = line.trim();
        if head.starts_with('[') && head.ends_with(']') {
            section = head[1..head.len() - 1].to_string();
            match section.as_str() {
                // bookmarks would be in the wrong places
                "Editor" => {}
                "Events" => {
                    out.push_str("[Events]\n");
                    out.push_str(&events(text, rate, bg));
                }
                _ => {
                    out.push_str(head);
                    out.push('\n');
                }
            }
            continue;
        }
        let kept: Option<String> = match section.as_str() {
            "Editor" | "Events" => None,
            "General" => match line.split_once(':').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("AudioFilename", _)) => Some(format!("AudioFilename: {AUDIO_FILE}")),
                Some((k @ ("AudioLeadIn" | "PreviewTime"), v)) => match whole(v) {
                    Some(t) if parse_int(v) > 0.0 => Some(format!("{k}: {t}")),
                    _ => Some(line.to_string()),
                },
                Some(("AudioHash" | "EditorBookmarks", _)) => None,
                _ => Some(line.to_string()),
            },
            "Metadata" => match line.split_once(':').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("Version", _)) => Some(format!("Version:{version}")),
                Some(("BeatmapID", _)) => Some("BeatmapID:0".into()),
                Some(("BeatmapSetID", _)) => Some("BeatmapSetID:-1".into()),
                Some(("Tags", v)) => Some(if v.is_empty() { "Tags:hashi-rate".into() } else { format!("Tags:{v} hashi-rate") }),
                _ => Some(line.to_string()),
            },
            "TimingPoints" => {
                let mut f: Vec<String> = line.split(',').map(|x| x.trim().to_string()).collect();
                match (f.first().and_then(|x| x.parse::<f64>().ok()), f.get(1).and_then(|x| x.parse::<f64>().ok())) {
                    (Some(time), Some(beat)) => {
                        f[0] = number(at(time));
                        // greens are multipliers of the red before them, which already moved
                        if f.get(6).map_or(beat > 0.0, |x| x == "1") && beat > 0.0 {
                            f[1] = format!("{}", beat / rate);
                        }
                        Some(f.join(","))
                    }
                    _ => Some(line.to_string()),
                }
            }
            "HitObjects" if !head.is_empty() => {
                let mut f: Vec<String> = line.split(',').map(|x| x.trim().to_string()).collect();
                match (f.len() >= 5).then(|| whole(&f[2])).flatten() {
                    Some(t) => {
                        f[2] = t;
                        if parse_int(&f[3]) as i64 & 128 != 0 && f.len() >= 6 {
                            let (end, tail) = f[5].split_once(':').map_or((f[5].clone(), String::new()), |(e, r)| (e.to_string(), format!(":{r}")));
                            if let Some(e) = whole(&end).filter(|_| parse_int(&end) != 0.0) {
                                f[5] = format!("{e}{tail}");
                            }
                        }
                        Some(f.join(","))
                    }
                    None => None,
                }
            }
            _ => Some(line.to_string()),
        };
        if let Some(l) = kept {
            out.push_str(&l);
            out.push('\n');
        }
    }
    out
}

// the background, breaks and storyboard sounds. A video or storyboard can't be sped up
fn events(text: &str, rate: f64, bg: Option<&str>) -> String {
    let mut breaks = vec![];
    let mut sounds = vec![];
    for line in crate::cut::section(text, "Events") {
        let mut f: Vec<String> = line.split(',').map(|x| x.trim().to_string()).collect();
        // whole, stable reads these as integers and drops the map's notes at a fraction
        let at = |s: &str| s.parse::<f64>().ok().map(|t| format!("{}", (t / rate).round() as i64));
        match f[0].as_str() {
            "2" | "Break" if f.len() >= 3 => {
                if let (Some(a), Some(b)) = (at(&f[1]), at(&f[2])) {
                    breaks.push(format!("2,{a},{b}"));
                }
            }
            "5" | "Sample" if f.len() >= 4 && !line.starts_with([' ', '_']) => {
                if let Some(t) = at(&f[1]) {
                    f[1] = t;
                    sounds.push(f.join(","));
                }
            }
            _ => {}
        }
    }
    let lines = |v: &[String]| v.iter().map(|l| format!("{l}\n")).collect::<String>();
    format!(
        "//Background and Video events\n{}//Break Periods\n{}//Storyboard Layer 0 (Background)\n//Storyboard Layer 1 (Fail)\n//Storyboard Layer 2 (Pass)\n//Storyboard Layer 3 (Foreground)\n//Storyboard Sound Samples\n{}\n",
        bg.map_or(String::new(), |n| format!("0,0,\"{n}\",0,0\n")),
        lines(&breaks),
        lines(&sounds),
    )
}

fn write_audio(pcm: &Pcm, rate: f64, keep_pitch: bool, w: &mut impl std::io::Write) -> std::io::Result<()> {
    let frames = (pcm.frames() as f64 / rate).round() as usize;
    w.write_all(&wav_header(pcm.rate, pcm.channels, frames))?;
    let mut sink = Sink { w, left: frames * pcm.channels, buf: Vec::with_capacity(1 << 16) };
    produce(pcm, rate, keep_pitch, |s| sink.put(s))?;
    sink.finish()
}

fn produce(pcm: &Pcm, rate: f64, keep_pitch: bool, out: impl FnMut(&[f32]) -> std::io::Result<bool>) -> std::io::Result<()> {
    if keep_pitch {
        stretch(pcm, rate, out)
    } else {
        resample(pcm, rate, out)
    }
}

// the whole of it in memory, for a cut or the cut page's preview
pub(crate) fn changed(pcm: &Pcm, rate: f64, keep_pitch: bool) -> Pcm {
    let total = (pcm.frames() as f64 / rate).round() as usize * pcm.channels;
    let mut samples: Vec<f32> = Vec::with_capacity(total);
    let _ = produce(pcm, rate, keep_pitch, |s| {
        let n = s.len().min(total - samples.len());
        samples.extend_from_slice(&s[..n]);
        Ok(samples.len() < total)
    });
    samples.resize(total, 0.0);
    Pcm { rate: pcm.rate, channels: pcm.channels, samples }
}

// takes exactly the header's length, padding with silence
struct Sink<'a, W: std::io::Write> {
    w: &'a mut W,
    left: usize,
    buf: Vec<u8>,
}

impl<W: std::io::Write> Sink<'_, W> {
    fn put(&mut self, s: &[f32]) -> std::io::Result<bool> {
        let n = s.len().min(self.left);
        for &x in &s[..n] {
            self.buf.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        self.left -= n;
        if self.buf.len() >= 1 << 16 {
            self.w.write_all(&self.buf)?;
            self.buf.clear();
        }
        Ok(self.left > 0)
    }

    fn finish(mut self) -> std::io::Result<()> {
        while self.left > 0 {
            let n = self.left.min(4096);
            self.put(&[0.0; 4096][..n])?;
        }
        self.w.write_all(&self.buf)
    }
}

// WSOLA, what SoundTouch does for osu!'s DT and HT: pieces of the song at its own speed, each placed where it
// lines up best with the one before, overlapping and crossfaded. Lengths are SoundTouch's automatic ones
fn stretch(src: &Pcm, tempo: f64, mut out: impl FnMut(&[f32]) -> std::io::Result<bool>) -> std::io::Result<()> {
    let ch = src.channels.max(1);
    let n = src.frames() as isize;
    let ms = |m: f64| ((m * src.rate as f64 / 1000.0).round() as usize).max(1);
    let k = ((tempo - 0.5) / 1.5).clamp(0.0, 1.0);
    let overlap = ms(8.0);
    let hop = ms(90.0 - 50.0 * k).max(2 * overlap) - overlap;
    let seek = ms(20.0 - 5.0 * k) as isize;
    let mono: Vec<f32> = src.samples.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    let mono_at = |i: isize| if (0..n).contains(&i) { mono[i as usize] } else { 0.0 };
    let at = |i: isize, c: usize| if (0..n).contains(&i) { src.samples[i as usize * ch + c] } else { 0.0 };
    let weight: Vec<f32> = (0..overlap).map(|i| (i * (overlap - i)) as f32).collect();
    let mut tail = vec![0.0f32; overlap * ch];
    let mut reference = vec![0.0f32; overlap];
    let mut block = vec![0.0f32; hop * ch];
    let score = |reference: &[f32], from: isize| {
        let (mut dot, mut energy) = (0.0f32, 1e-9f32);
        for (i, r) in reference.iter().enumerate() {
            let x = mono_at(from + i as isize);
            dot += r * x;
            energy += x * x;
        }
        dot / energy.sqrt()
    };
    for step in 0.. {
        // centred so the piece's middle sits where the song would be, as early as late on average
        let nominal = (step as f64 * hop as f64 * tempo + (tempo - 1.0) * hop as f64 / 2.0).round() as isize;
        let mut best = (score(&reference, nominal), nominal);
        let look = |best: &mut (f32, isize), from: isize| {
            let s = score(&reference, from);
            if s > best.0 {
                *best = (s, from);
            }
        };
        for o in (-seek / 2..=seek / 2).step_by(4) {
            look(&mut best, nominal + o);
        }
        let coarse = best.1;
        for o in -3..=3 {
            look(&mut best, coarse + o);
        }
        let start = best.1;
        for j in 0..overlap {
            let w = (j as f32 + 0.5) / overlap as f32;
            for c in 0..ch {
                block[j * ch + c] = tail[j * ch + c] * (1.0 - w) + at(start + j as isize, c) * w;
            }
        }
        for j in overlap..hop {
            for c in 0..ch {
                block[j * ch + c] = at(start + j as isize, c);
            }
        }
        for j in 0..overlap {
            let i = start + (hop + j) as isize;
            for c in 0..ch {
                tail[j * ch + c] = at(i, c);
            }
            reference[j] = mono_at(i) * weight[j];
        }
        if !out(&block)? {
            break;
        }
    }
    Ok(())
}

// played faster or slower like a record, through a windowed sinc so a speed-up doesn't alias
fn resample(src: &Pcm, factor: f64, mut out: impl FnMut(&[f32]) -> std::io::Result<bool>) -> std::io::Result<()> {
    const HALF: isize = 16;
    const PHASES: usize = 256;
    let ch = src.channels.max(1);
    let n = src.frames() as isize;
    let cutoff = 0.95 * (1.0 / factor).min(1.0);
    let table: Vec<Vec<f32>> = (0..=PHASES)
        .map(|p| {
            let frac = p as f64 / PHASES as f64;
            let row: Vec<f64> = (1 - HALF..=HALF)
                .map(|t| {
                    let x = t as f64 - frac;
                    let sinc = if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * cutoff * x) };
                    let u = x / HALF as f64;
                    let window = if u.abs() >= 1.0 { 0.0 } else { 0.42 + 0.5 * (std::f64::consts::PI * u).cos() + 0.08 * (2.0 * std::f64::consts::PI * u).cos() };
                    sinc * window
                })
                .collect();
            let sum: f64 = row.iter().sum();
            row.iter().map(|w| (w / sum) as f32).collect()
        })
        .collect();
    let mut block = vec![0.0f32; 4096 * ch];
    let mut i = 0usize;
    loop {
        for frame in block.chunks_mut(ch) {
            let pos = i as f64 * factor;
            let k = pos.floor() as isize;
            let row = &table[((pos - k as f64) * PHASES as f64).round() as usize];
            for (c, s) in frame.iter_mut().enumerate() {
                *s = 0.0;
                for (t, w) in row.iter().enumerate() {
                    let at = k + 1 - HALF + t as isize;
                    if (0..n).contains(&at) {
                        *s += w * src.samples[at as usize * ch + c];
                    }
                }
            }
            i += 1;
        }
        if !out(&block)? {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = "osu file format v14\n\n[General]\nAudioFilename: song.mp3\nAudioLeadIn: 0\nPreviewTime: 30000\nMode: 3\n\n[Editor]\nBookmarks: 1000,2000\n\n\
        [Metadata]\nTitle:Song\nArtist:Someone\nVersion:Insane\nTags:fast\nBeatmapID:123\nBeatmapSetID:45\n\n[Difficulty]\nOverallDifficulty:8\n\n\
        [Events]\n//Background and Video events\n0,0,\"bg.jpg\",0,0\nVideo,0,\"v.mp4\"\n//Break Periods\n2,6001,9000\nSprite,Background,Centre,\"x.png\",320,240\n F,0,0,1000,1\nSample,12000,0,\"kick.wav\",70\n\n\
        [TimingPoints]\n600,500,4,1,0,60,1,0\n3000,-50,4,1,0,60,0,0\n\n[HitObjects]\n64,192,600,1,0,0:0:0:0:\n192,192,1200,128,0,1800:0:0:0:0:ln.wav\n";

    #[test]
    fn moves_every_time_and_only_red_beats() {
        let osu = rewrite(MAP, 1.5, "Insane 1.5x", Some("bg.png"));
        assert!(osu.contains("AudioFilename: audio.wav\n"));
        assert!(osu.contains("PreviewTime: 20000\n"));
        assert!(!osu.contains("Bookmarks"));
        assert!(osu.contains("Version:Insane 1.5x\nTags:fast hashi-rate\nBeatmapID:0\nBeatmapSetID:-1\n"));
        assert!(osu.contains("0,0,\"bg.png\",0,0\n//Break Periods\n2,4001,6000\n"));
        assert!(!osu.contains("v.mp4") && !osu.contains("x.png") && !osu.contains(" F,"));
        assert!(osu.contains("Sample,8000,0,\"kick.wav\",70\n"));
        assert!(osu.contains("400,333.3333333333333,4,1,0,60,1,0\n2000,-50,4,1,0,60,0,0\n"));
        assert!(osu.contains("64,192,400,1,0,0:0:0:0:\n192,192,800,128,0,1200:0:0:0:0:ln.wav\n"));
        let chart = crate::beatmap::parse_chart(&osu).unwrap();
        assert_eq!(chart.notes.len(), 2);
        assert_eq!((chart.notes[1].time, chart.notes[1].end_time), (800.0, 1200.0));
    }

    #[test]
    fn names_the_mod() {
        assert_eq!(suffix(1.2), " 1.2x");
        assert_eq!(suffix(1.0), "");
    }

    fn clicks(rate: u32, at_ms: &[f64], len_ms: f64) -> Pcm {
        let frames = (len_ms / 1000.0 * rate as f64) as usize;
        let mut samples = vec![0.0f32; frames * 2];
        // a decaying tone, like a drum hit
        for &t in at_ms {
            let start = (t / 1000.0 * rate as f64) as usize;
            let len = rate as usize / 12;
            for i in 0..len {
                let v = (i as f32 * 0.07).sin() * 0.8 * (-(i as f32) / (len as f32 / 5.0)).exp();
                samples[(start + i) * 2] = v;
                samples[(start + i) * 2 + 1] = v;
            }
        }
        Pcm { rate, channels: 2, samples }
    }

    fn onsets(wav: &[u8], channels: usize, rate: u32) -> Vec<f64> {
        let s: Vec<f32> = wav[44..].chunks(2 * channels).map(|f| i16::from_le_bytes([f[0], f[1]]) as f32 / 32767.0).collect();
        let mut found = vec![];
        let mut quiet_until = 0;
        for (i, x) in s.iter().enumerate() {
            if i >= quiet_until && x.abs() > 0.2 {
                found.push(i as f64 * 1000.0 / rate as f64);
                quiet_until = i + rate as usize / 5;
            }
        }
        found
    }

    #[test]
    fn the_song_stays_on_the_notes() {
        let times: Vec<f64> = (0..40).map(|k| 500.0 + k as f64 * 437.0).collect();
        let pcm = clicks(44100, &times, 18500.0);
        for &(rate, keep) in &[(1.5, true), (1.2, true), (0.75, true), (1.5, false), (0.75, false)] {
            let mut wav = vec![];
            write_audio(&pcm, rate, keep, &mut wav).unwrap();
            let frames = (wav.len() - 44) / 4;
            assert_eq!(frames, (pcm.frames() as f64 / rate).round() as usize);
            let heard = onsets(&wav, 2, 44100);
            assert_eq!(heard.len(), times.len(), "{rate} {keep}");
            let off: Vec<f64> = heard.iter().zip(&times).map(|(h, t)| h - t / rate).collect();
            let worst = off.iter().fold(0.0f64, |m, x| m.max(x.abs()));
            println!("{rate}x keep pitch {keep}: worst {worst:.1} ms, mean {:.1} ms", off.iter().sum::<f64>() / off.len() as f64);
            // a piece of the stretch can land up to half a seek early or late
            assert!(worst < 15.0, "{rate}x keep pitch {keep}: off by {worst:.1} ms");
        }
    }

    // makes a rate of a map on disk without opening osu!:
    // HASHI_RATE=<.osu> HASHI_SPEED=1.3 HASHI_PITCH=keep cargo test -p mania-bridge-helper rate_real_map -- --ignored --nocapture
    #[test]
    #[ignore]
    fn rate_real_map() {
        let Some(osu) = std::env::var_os("HASHI_RATE").map(std::path::PathBuf::from) else { return };
        let rate: f64 = std::env::var("HASHI_SPEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1.3);
        let keep = std::env::var("HASHI_PITCH").map_or(true, |v| v == "keep");
        let text = std::fs::read_to_string(&osu).unwrap();
        let audio = osu.parent().unwrap().join(general_value(&text, "AudioFilename").unwrap());
        let src = Source { md5: String::new(), chart: osu.clone(), audio, background: None, lazer: false };
        let out = std::env::temp_dir().join("hashi-rate-test");
        std::fs::create_dir_all(&out).unwrap();
        let started = std::time::Instant::now();
        let (version, file, _) = render(&text, &src, rate, keep, &out).unwrap();
        println!("made {version} in {:?}: {file}", started.elapsed());
    }
}
