use std::path::Path;

use crate::beatmap::Note;
use crate::judge::{self, Segment};

pub struct Sprite {
    pub width: u32,
    pub height: u32,
    // premultiplied
    pub rgba: Vec<u8>,
}

// the scene as the skin page gets it (scene.rs's json) and its sprites by index
pub fn scene(dir: &Path, keys: usize, at: &str, edits: &str, aspect: Option<f64>) -> Result<(serde_json::Value, Vec<Sprite>), String> {
    let (json, sprites) = crate::skin_render::player_scene(dir, keys, at, edits, aspect)?;
    Ok((json, sprites.into_iter().map(|(width, height, rgba)| Sprite { width, height, rgba }).collect()))
}

pub struct Song {
    pub rate: u32,
    pub channels: usize,
    // interleaved
    pub samples: Vec<i16>,
}

// the whole song, decoded as osu! plays it (gapless)
pub fn song(file: &Path) -> Result<Song, String> {
    let pcm = crate::practice::decode(file, 0.0, f64::INFINITY)?;
    let samples = pcm.samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    Ok(Song { rate: pcm.rate, channels: pcm.channels, samples })
}

// [column, time, end for a hold], in the map's own ms and the order /skin/chart gives them
pub type ChartNote = (usize, f64, Option<f64>);
// [column, down, up or None while it's still down], in map ms
pub type Press = (usize, f64, Option<f64>);

pub struct Judged {
    pub acc: f64,
    // 1 PERFECT to 6 MISS
    pub counts: [u32; 7],
    pub combo: u32,
    pub max_combo: u32,
    // the judgements of the last half second: note, judgement, map ms
    pub recent: Vec<(usize, u8, f64)>,
    // notes near now whose head was hit, so a tap goes and a held hold stays on the line
    pub hit: Vec<usize>,
    // notes near now judged all the way by now, so a hold let go of in time goes then, before its end
    pub done: Vec<usize>,
    // each head hit by now as (its time, how late it was hit), early negative
    pub heads: Vec<(f64, f64)>,
}

pub const MISS: u8 = judge::MISS;

// how early a press still counts for a note, the miss window
pub fn reach(od: f64, lazer: bool, mods: &[String]) -> f64 {
    judge::windows(od, &judge::ruleset(lazer, mods, Some(1.0))).miss
}

// `mods` the ones that change judging, HR, EZ, CL or V2; the song plays at its own speed here
#[allow(clippy::too_many_arguments)]
pub fn judge(chart: &[ChartNote], keys: usize, od: f64, lazer: bool, mods: &[String], from: f64, now: f64, presses: &[Press]) -> Judged {
    let rules = judge::ruleset(lazer, mods, Some(1.0));
    let w = judge::windows(od, &rules);
    let picked: Vec<usize> = (0..chart.len()).filter(|&i| chart[i].1 >= from).collect();
    let notes: Vec<Note> = picked
        .iter()
        .map(|&i| {
            let (column, time, end) = chart[i];
            Note { column, time, end_time: end.unwrap_or(time), is_hold: end.is_some() }
        })
        .collect();
    // stable's clock is whole milliseconds
    let at = |t: f64| if lazer { t } else { (t + 0.5).floor() };
    // a key still down is held far past anything judged by now
    let held = now + 60_000.0;
    let mut segments: Vec<Vec<Segment>> = vec![vec![]; keys];
    for &(column, down, up) in presses {
        let Some(column_segments) = segments.get_mut(column) else { continue };
        let (start, end) = (at(down), up.map_or(held, at).max(at(down)));
        column_segments.push(Segment { start, end, end_previous: Some(end), samples: vec![start], start_previous: None });
    }
    for s in &mut segments {
        s.sort_by(|a, b| a.start.total_cmp(&b.start));
    }
    let sim = judge::simulate(&notes, &segments, keys, &w, rules.mode, 1.0, false);

    let mut counts = [0u32; 7];
    let mut recent = vec![];
    for e in sim.events.iter().filter(|e| e.judgment >= 1 && e.time <= now) {
        counts[e.judgment as usize] += 1;
        if e.time >= now - 500.0 {
            recent.push((picked[e.note], e.judgment, e.time));
        }
    }
    let combo_events = if lazer { judged_combo(&sim.events) } else { stable_combo(&notes, &sim, &segments, &w) };
    let (mut combo, mut max_combo) = (0u32, 0u32);
    for &(_, hit) in combo_events.iter().filter(|(t, _)| *t <= now) {
        combo = if hit { combo + 1 } else { 0 };
        max_combo = max_combo.max(combo);
    }
    let hit = notes
        .iter()
        .zip(&sim.states)
        .enumerate()
        .filter(|(_, (n, s))| s.pressed && (1..MISS).contains(&s.head_judgment) && n.time - w.miss <= now && n.end_time >= now - 2000.0)
        .map(|(i, _)| picked[i])
        .collect();
    let mut last = vec![f64::NEG_INFINITY; notes.len()];
    for e in &sim.events {
        last[e.note] = last[e.note].max(e.time);
    }
    let done = notes
        .iter()
        .enumerate()
        .filter(|&(i, n)| last[i] <= now && n.end_time >= now - 2000.0 && n.time - w.miss <= now)
        .map(|(i, _)| picked[i])
        .collect();
    let heads = notes
        .iter()
        .zip(&sim.states)
        .filter(|(n, s)| s.pressed && (1..MISS).contains(&s.head_judgment) && n.time <= now)
        .map(|(n, s)| (n.time, s.head_offset))
        .collect();
    Judged { acc: judge::accuracy(&counts, rules.mode), counts, combo, max_combo, recent, hit, done, heads }
}

// combo goes up by one, or breaks, at each (map ms, hit)
type ComboEvents = Vec<(f64, bool)>;

// lazer: each judgement, a hold's head and tail apart
fn judged_combo(events: &[judge::Event]) -> ComboEvents {
    let mut out: ComboEvents = events.iter().filter(|e| e.judgment >= 1).map(|e| (e.time, e.judgment != MISS)).collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    out
}

// stable, as mania-hub's replay player counts it: a hold gives one for its head and one for every
// 100 ms of the map it's held after that, letting go before its tail's window breaks combo, and a
// missed hold breaks it at its judgement; the hold's own judgement adds nothing more
fn stable_combo(notes: &[Note], sim: &judge::Simulation, segments: &[Vec<Segment>], w: &judge::Windows) -> ComboEvents {
    let mut out: ComboEvents = vec![];
    let mut judged: Vec<Vec<&judge::Event>> = vec![vec![]; notes.len()];
    for e in sim.events.iter().filter(|e| e.judgment >= 1) {
        judged[e.note].push(e);
    }
    let tail_early = w.meh.floor();
    for (i, n) in notes.iter().enumerate() {
        let state = &sim.states[i];
        if !(n.is_hold && n.end_time > n.time) {
            out.extend(judged[i].iter().map(|e| (e.time, e.judgment != MISS)));
            continue;
        }
        let head_time = n.time + state.head_offset;
        let head_hit = state.pressed && (1..MISS).contains(&state.head_judgment);
        let last = judged[i].iter().max_by(|a, b| a.time.total_cmp(&b.time));
        if !head_hit {
            if let Some(e) = last {
                out.push((e.time, false));
            }
            continue;
        }
        out.push((head_time, true));
        let column = segments.get(n.column).map_or(&[][..], Vec::as_slice);
        // let go after the head and before the tail's window: the body breaks there
        for seg in column.iter().filter(|s| s.end > head_time && s.end < n.end_time - tail_early) {
            out.push((seg.end, false));
        }
        let tail_break = last.filter(|e| e.judgment == MISS).map(|e| e.time);
        out.extend(tail_break.map(|t| (t, false)));
        let mut t = n.time + 100.0;
        while t <= n.end_time + 1e-6 {
            if tail_break.is_some_and(|b| t >= b - 1e-6) {
                break;
            }
            // down at that moment, its release not: mania-hub's segmentCoversTime
            if t >= head_time - 1e-6 && column.iter().any(|s| s.start <= t + 1e-6 && t < s.end - 1e-6) {
                out.push((t, true));
            }
            t += 100.0;
        }
    }
    // a break before a hit at the same moment, as mania-hub orders them
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_holds_add_combo_while_held() {
        // a 1 s hold: its head, then one every 100 ms held up to its end, its judgement adds none
        let chart = [(0, 1000.0, Some(2000.0))];
        let j = judge(&chart, 4, 8.0, false, &[], 0.0, 2500.0, &[(0, 1000.0, Some(2030.0))]);
        assert_eq!(j.combo, 11);
        // halfway through it's the head and five ticks
        let j = judge(&chart, 4, 8.0, false, &[], 0.0, 1550.0, &[(0, 1000.0, None)]);
        assert_eq!(j.combo, 6);
        // let go at 1.4 s: the body breaks there, ticks after it while held again count anew
        let j = judge(&chart, 4, 8.0, false, &[], 0.0, 2500.0, &[(0, 1000.0, Some(1400.0)), (0, 1650.0, Some(2030.0))]);
        assert_eq!((j.max_combo, j.combo), (4, 4));
        // lazer: one for the head, one for the tail
        let j = judge(&chart, 4, 8.0, true, &[], 0.0, 2500.0, &[(0, 1000.0, Some(2030.0))]);
        assert_eq!(j.combo, 2);
    }

    #[test]
    fn judges_only_up_to_now() {
        let chart = [(0, 1000.0, None), (1, 2000.0, None)];
        let j = judge(&chart, 4, 8.0, false, &[], 0.0, 1100.0, &[(0, 1005.0, Some(1050.0))]);
        assert_eq!(j.counts[1], 1);
        assert_eq!(j.hit, [0]);
        // the second note's miss comes once its window has gone by
        let j = judge(&chart, 4, 8.0, false, &[], 0.0, 2300.0, &[(0, 1005.0, Some(1050.0))]);
        assert_eq!(j.counts[6], 1);
        assert_eq!((j.combo, j.max_combo), (0, 1));
    }

    #[test]
    fn a_held_hold_waits_for_its_release() {
        let chart = [(2, 1000.0, Some(2000.0))];
        // lazer: the head is judged, the tail only once let go
        let j = judge(&chart, 4, 8.0, true, &[], 0.0, 1500.0, &[(2, 1002.0, None)]);
        assert_eq!(j.counts.iter().sum::<u32>(), 1);
        assert_eq!(j.hit, [0]);
        let j = judge(&chart, 4, 8.0, true, &[], 0.0, 2100.0, &[(2, 1002.0, Some(1995.0))]);
        assert_eq!(j.counts.iter().sum::<u32>(), 2);
        assert_eq!(j.acc, 100.0);
        // let go a little early, inside the tail's window, it's done there and then
        let j = judge(&chart, 4, 8.0, true, &[], 0.0, 1990.0, &[(2, 1002.0, Some(1980.0))]);
        assert_eq!(j.done, [0]);
        let j = judge(&chart, 4, 8.0, true, &[], 0.0, 1500.0, &[(2, 1002.0, None)]);
        assert!(j.done.is_empty());
        // stable: one judgement for the whole hold, the head sticks to the line meanwhile
        let j = judge(&chart, 4, 8.0, false, &[], 0.0, 1500.0, &[(2, 1002.0, None)]);
        assert_eq!(j.counts.iter().sum::<u32>(), 0);
        assert_eq!(j.hit, [0]);
        let j = judge(&chart, 4, 8.0, false, &[], 0.0, 2100.0, &[(2, 1002.0, Some(1995.0))]);
        assert_eq!(j.counts[1], 1);
    }

    #[test]
    fn notes_before_the_seek_are_left_out() {
        let chart = [(0, 1000.0, None), (0, 3000.0, None)];
        let j = judge(&chart, 4, 8.0, false, &[], 2500.0, 4000.0, &[(0, 3001.0, Some(3040.0))]);
        assert_eq!(j.counts.iter().sum::<u32>(), 1);
        assert_eq!(j.acc, 100.0);
        assert_eq!(j.hit, [1]);
    }
}
