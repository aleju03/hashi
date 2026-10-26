// which key press belongs to which note, and what each was judged. a port of mania tracker's replay
// judge (backend/src/replay-judge) with the options it runs with when it rates imports: stable reads
// its sampled frames with legacy rounding, lazer reads the times as written. keep it in step with that
// file, the comparison test in analysis.rs checks the two agree

use crate::beatmap::{Note, TimingPoint};

// 1 PERFECT, 2 GREAT, 3 GOOD, 4 OK, 5 MEH, 6 MISS, 0 nothing
pub type Judgment = u8;
pub const MISS: Judgment = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Stable,
    StableV2,
    Lazer,
}

#[derive(Clone, Copy)]
pub struct Ruleset {
    pub mode: Mode,
    pub difficulty: f64,
    pub speed: f64,
    pub classic: bool,
}

pub fn ruleset(lazer: bool, mods: &[String], speed: Option<f64>) -> Ruleset {
    let has = |m: &str| mods.iter().any(|x| x.eq_ignore_ascii_case(m));
    let v2 = has("SV2") || has("V2");
    let speed = speed.filter(|s| s.is_finite() && *s > 0.0).unwrap_or(if has("DT") || has("NC") {
        1.5
    } else if has("HT") || has("DC") {
        0.75
    } else {
        1.0
    });
    Ruleset {
        mode: if lazer { Mode::Lazer } else if v2 { Mode::StableV2 } else { Mode::Stable },
        difficulty: if has("HR") { 1.4 } else if has("EZ") { 1.0 / 1.4 } else { 1.0 },
        speed,
        classic: (!lazer || has("CL")) && !v2,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Windows {
    pub perfect: f64,
    pub great: f64,
    pub good: f64,
    pub ok: f64,
    pub meh: f64,
    pub miss: f64,
}

fn difficulty_range(d: f64, min: f64, mid: f64, max: f64) -> f64 {
    if d > 5.0 {
        mid + (max - mid) * ((d - 5.0) / 5.0)
    } else if d < 5.0 {
        mid + (mid - min) * ((d - 5.0) / 5.0)
    } else {
        mid
    }
}

pub fn windows(raw_od: f64, r: &Ruleset) -> Windows {
    let od = if raw_od.is_finite() { raw_od.clamp(0.0, 10.0) } else { 8.0 };
    let total = r.speed / r.difficulty;
    let w = |x: f64| (x * total).floor() + 0.5;
    if r.classic {
        let inv = 10.0 - od;
        return Windows {
            perfect: w(16.0),
            great: w(34.0 + 3.0 * inv),
            good: w(67.0 + 3.0 * inv),
            ok: w(97.0 + 3.0 * inv),
            meh: w(121.0 + 3.0 * inv),
            miss: w(158.0 + 3.0 * inv),
        };
    }
    Windows {
        perfect: w(difficulty_range(od, 22.4, 19.4, 13.9)),
        great: w(difficulty_range(od, 64.0, 49.0, 34.0)),
        good: w(difficulty_range(od, 97.0, 82.0, 67.0)),
        ok: w(difficulty_range(od, 127.0, 112.0, 97.0)),
        meh: w(difficulty_range(od, 151.0, 136.0, 121.0)),
        miss: w(difficulty_range(od, 188.0, 173.0, 158.0)),
    }
}

// javascript's Math.round, halves go up
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

#[derive(Clone, Copy)]
pub struct Frame {
    pub time: f64,
    pub keys: u32,
}

const KEY_MASK: u32 = (1 << 20) - 1;

// .osr rows (ms since the last row, x, y) as absolute key states, the way stable's ReplayWatcher reads them
pub fn decode_frames(rows: &[(f64, f64, f64)]) -> Vec<Frame> {
    struct F {
        time: f64,
        keys: u32,
        x: f64,
        y: f64,
    }
    let mut time = 0.0;
    let mut f: Vec<F> = rows
        .iter()
        .map(|&(delta, x, y)| {
            time += delta;
            F { time: js_round(time), keys: (js_round(x) as i64 as u32) & KEY_MASK, x, y }
        })
        .filter(|f| f.time.is_finite() && f.x.is_finite() && f.y.is_finite())
        .collect();
    if f.len() >= 2 && f[1].time < f[0].time {
        f[1].time = f[0].time;
        f[0].time = 0.0;
    }
    if f.len() >= 3 && f[0].time > f[2].time {
        f[0].time = f[2].time;
        f[1].time = f[2].time;
    }
    let dummy = |f: &F| js_round(f.x) == 256.0 && js_round(f.y) == -500.0;
    if f.len() >= 2 && dummy(&f[1]) {
        f.remove(1);
    }
    if !f.is_empty() && dummy(&f[0]) {
        f.remove(0);
    }
    let mut out: Vec<Frame> = vec![];
    let mut previous = f64::NEG_INFINITY;
    for x in f {
        if x.time < previous {
            continue;
        }
        out.push(Frame { time: x.time, keys: x.keys });
        previous = x.time;
    }
    out
}

// one stretch of a key held down, with the sample before each edge
#[derive(Clone)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub end_previous: Option<f64>,
    pub samples: Vec<f64>,
    pub start_previous: Option<f64>,
}

pub fn build_segments(frames: &[Frame], keys: usize, total: f64) -> Vec<Vec<Segment>> {
    let mut segments: Vec<Vec<Segment>> = vec![vec![]; keys];
    let mut active: Vec<Option<(Vec<f64>, f64, Option<f64>)>> = vec![None; keys];
    let mut previous: Vec<Option<f64>> = vec![None; keys];
    for frame in frames {
        for c in 0..keys {
            let pressed = frame.keys & (1 << c) != 0;
            match (&mut active[c], pressed) {
                (None, true) => active[c] = Some((vec![frame.time], frame.time, previous[c])),
                (Some(a), true) => a.0.push(frame.time),
                (Some(_), false) => {
                    let (samples, start, start_previous) = active[c].take().unwrap();
                    segments[c].push(Segment { start, end: frame.time, end_previous: samples.last().copied(), samples, start_previous });
                }
                (None, false) => {}
            }
            previous[c] = Some(frame.time);
        }
    }
    for c in 0..keys {
        if let Some((samples, start, start_previous)) = active[c].take() {
            segments[c].push(Segment { start, end: total, end_previous: samples.last().copied(), samples, start_previous });
        }
    }
    segments
}

// .NET's seeded System.Random, lazer's Random mod shuffles columns with it
struct CSharpRandom {
    seeds: [i32; 56],
    inext: usize,
    inextp: usize,
}

impl CSharpRandom {
    fn new(seed: i32) -> Self {
        let mut seeds = [0i32; 56];
        let subtraction = if seed == i32::MIN { i32::MAX } else { seed.abs() };
        let mut mj = 161803398i32.wrapping_sub(subtraction);
        seeds[55] = mj;
        let mut mk = 1i32;
        let mut ii = 0;
        for _ in 1..55 {
            ii += 21;
            if ii >= 55 {
                ii -= 55;
            }
            seeds[ii] = mk;
            mk = mj.wrapping_sub(mk);
            if mk < 0 {
                mk = mk.wrapping_add(i32::MAX);
            }
            mj = seeds[ii];
        }
        for _ in 1..5 {
            for i in 1..56 {
                let mut n = i + 30;
                if n >= 55 {
                    n -= 55;
                }
                seeds[i] = seeds[i].wrapping_sub(seeds[1 + n]);
                if seeds[i] < 0 {
                    seeds[i] = seeds[i].wrapping_add(i32::MAX);
                }
            }
        }
        CSharpRandom { seeds, inext: 0, inextp: 21 }
    }

    fn next(&mut self) -> i32 {
        let mut a = self.inext + 1;
        if a >= 56 {
            a = 1;
        }
        let mut b = self.inextp + 1;
        if b >= 56 {
            b = 1;
        }
        let mut v = self.seeds[a].wrapping_sub(self.seeds[b]);
        if v == i32::MAX {
            v -= 1;
        }
        if v < 0 {
            v = v.wrapping_add(i32::MAX);
        }
        self.seeds[a] = v;
        self.inext = a;
        self.inextp = b;
        v
    }
}

pub fn random_columns(keys: usize, seed: i32) -> Vec<usize> {
    let mut rng = CSharpRandom::new(seed);
    let mut order: Vec<(usize, i32)> = (0..keys).map(|c| (c, rng.next())).collect();
    order.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    order.into_iter().map(|(c, _)| c).collect()
}

fn beat_length_at(points: &[TimingPoint], time: f64) -> f64 {
    let Some(mut active) = points.first() else { return 1000.0 };
    for p in points {
        if p.time > time {
            break;
        }
        active = p;
    }
    active.beat_length
}

// every object start becomes a hold reaching (almost) to the next one in its column, the last is dropped
fn invert(notes: &[Note], points: &[TimingPoint]) -> Vec<Note> {
    let mut columns: Vec<(usize, Vec<Note>)> = vec![];
    for n in notes {
        match columns.iter_mut().find(|(c, _)| *c == n.column) {
            Some((_, list)) => list.push(*n),
            None => columns.push((n.column, vec![*n])),
        }
    }
    let mut out = vec![];
    for (_, mut list) in columns {
        list.sort_by(|a, b| a.time.total_cmp(&b.time));
        for i in 0..list.len().saturating_sub(1) {
            let next = list[i + 1].time;
            let full = next - list[i].time;
            let duration = (full / 2.0).max(full - beat_length_at(points, next) / 4.0);
            out.push(Note { end_time: list[i].time + duration, is_hold: true, ..list[i] });
        }
    }
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    out
}

pub struct Mod {
    pub acronym: String,
    pub seed: Option<f64>,
}

pub fn apply_mods(notes: &[Note], keys: usize, mods: &[Mod], points: &[TimingPoint]) -> Vec<Note> {
    let has = |m: &str| mods.iter().any(|x| x.acronym.eq_ignore_ascii_case(m));
    let mut out: Vec<Note> = if has("HO") {
        notes.iter().map(|n| if n.is_hold { Note { end_time: n.time, is_hold: false, ..*n } } else { *n }).collect()
    } else if has("IN") {
        invert(notes, points)
    } else {
        notes.to_vec()
    };
    for m in mods {
        if m.acronym.eq_ignore_ascii_case("RD") {
            let Some(seed) = m.seed.filter(|s| s.is_finite()) else { continue };
            let map = random_columns(keys, seed as i64 as i32);
            for n in &mut out {
                n.column = map.get(n.column).copied().unwrap_or(n.column);
            }
        } else if m.acronym.eq_ignore_ascii_case("MR") {
            for n in &mut out {
                n.column = keys - 1 - n.column;
            }
        }
    }
    out
}

fn judgment_for(offset: f64, w: &Windows) -> Judgment {
    let d = offset.abs();
    if d <= w.perfect {
        1
    } else if d <= w.great {
        2
    } else if d <= w.good {
        3
    } else if d <= w.ok {
        4
    } else if d <= w.meh {
        5
    } else if d <= w.miss {
        6
    } else {
        0
    }
}

fn cap_lazer_tail(j: Judgment, cap: bool) -> Judgment {
    if cap && j != 0 && j < 5 {
        5
    } else {
        j
    }
}

const LENIENCE: f64 = 1.5;

fn stable_v2_tail(offset: f64, w: &Windows) -> Judgment {
    let d = js_round(offset).abs();
    if d <= w.perfect.floor() * LENIENCE {
        1
    } else if d <= w.great.floor() * LENIENCE {
        2
    } else if d <= w.good.floor() * LENIENCE {
        3
    } else if offset >= w.ok.floor() * LENIENCE {
        6
    } else if d <= w.ok.floor() * LENIENCE {
        4
    } else if d <= w.meh.floor() * LENIENCE {
        5
    } else if d <= w.miss.floor() * LENIENCE {
        6
    } else {
        0
    }
}

fn stable_head(offset: f64, w: &Windows) -> Judgment {
    let offset = js_round(offset);
    // stable's late OK window is open: at OD5 111ms is an OK and 112ms already a miss
    if offset >= w.ok.floor() {
        return if offset <= w.miss { 6 } else { 0 };
    }
    judgment_for(offset, w)
}

const PRECISE_EDGE_INTERVAL: f64 = 4.0;
const COARSE_MEDIAN_TRANSITION: f64 = 6.0;
const CROSSED_LN_HEAD_MEDIAN_TRANSITION: f64 = 9.0;

// stable's frames are sampled key states, an edge happened somewhere between two samples
fn press_estimate(s: &Segment, coarse: bool) -> f64 {
    let min = s.start_previous.unwrap_or(s.start);
    let (lower, upper) = (min.min(s.start), min.max(s.start));
    if upper - lower > PRECISE_EDGE_INTERVAL && coarse {
        s.start
    } else {
        upper
    }
}

fn release_estimate(s: &Segment, coarse: bool) -> f64 {
    let min = s.end_previous.unwrap_or(s.end);
    let (lower, upper) = (min.min(s.end), min.max(s.end));
    if upper - lower > PRECISE_EDGE_INTERVAL && coarse {
        s.end
    } else {
        upper
    }
}

fn held_tail_timeout(s: &Segment, timeout: f64) -> f64 {
    s.samples.iter().copied().find(|&x| x >= timeout).unwrap_or(timeout)
}

fn press_range(s: &Segment) -> (f64, f64) {
    let min = s.start_previous.unwrap_or(s.start);
    (min.min(s.start), min.max(s.start))
}

fn past_head_deadline(range_min: f64, deadline: f64, grace: f64) -> bool {
    range_min > deadline + grace.max(0.0)
}

fn can_bridge_tail_edge_gap(s: &Segment, next: Option<&Segment>, tail_early: f64, edge_grace: f64) -> bool {
    let Some(next) = next else { return false };
    if tail_early - s.end > 0.0 {
        return false;
    }
    press_range(next).0 <= tail_early + edge_grace
}

fn transition_median(segments: &[Vec<Segment>]) -> f64 {
    let mut spans = vec![];
    for s in segments.iter().flatten() {
        if let Some(p) = s.start_previous {
            let span = (s.start - p).abs();
            if span > 0.0 {
                spans.push(span);
            }
        }
        if let Some(p) = s.end_previous {
            let span = (s.end - p).abs();
            if span > 0.0 {
                spans.push(span);
            }
        }
    }
    if spans.is_empty() {
        return 0.0;
    }
    spans.sort_by(f64::total_cmp);
    spans[spans.len() / 2]
}

// stable's ScoreV1 hold: one judgement from the head error and head + tail together
fn stable_combined(head: f64, tail: f64, cap: Option<Judgment>, w: &Windows) -> Judgment {
    let head = js_round(head).abs();
    let both = head + js_round(tail).abs();
    let (perfect, great, good, ok) = (w.perfect.floor(), w.great.floor(), w.good.floor(), w.ok.floor());
    let j = if head <= perfect * 1.2 && both <= perfect * 2.4 {
        1
    } else if head <= great * 1.1 && both <= great * 2.2 {
        2
    } else if head <= good && both <= good * 2.0 {
        3
    } else if head <= ok && both <= ok * 2.0 {
        4
    } else {
        5
    };
    cap.map_or(j, |c| j.max(c))
}

fn scoring_head_time(n: &Note, press: f64, w: &Windows) -> f64 {
    let meh = w.meh.floor();
    if press < n.time - meh {
        n.end_time - 1.0
    } else if press < n.time {
        n.time + (n.time - press)
    } else {
        press
    }
}

fn scoring_tail_time(n: &Note, tail: f64, held: bool) -> f64 {
    if held || tail <= n.end_time {
        tail
    } else {
        n.end_time - (tail - n.end_time)
    }
}

fn stable_ln_head_preview(offset: f64, w: &Windows) -> Judgment {
    let d = js_round(offset).abs();
    if d <= w.perfect.floor() {
        1
    } else if d <= w.great.floor() {
        2
    } else if d <= w.good.floor() {
        3
    } else if d <= w.ok.floor() {
        4
    } else if d <= w.miss.floor() {
        5
    } else {
        0
    }
}

fn judge_stable_ln(n: &Note, press: f64, tail: f64, held: bool, cap: Option<Judgment>, w: &Windows) -> Judgment {
    let head = scoring_head_time(n, press, w) - n.time;
    let tail = scoring_tail_time(n, tail, held) - n.end_time;
    if tail < -w.meh.floor() {
        return 6;
    }
    stable_combined(head, tail, cap, w)
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub note: usize,
    pub judgment: Judgment,
    pub time: f64,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct NoteState {
    // a press reached the head (also an early press the game counted as a miss)
    pub pressed: bool,
    pub head_judgment: Judgment,
    // press minus head time, in replay time
    pub head_offset: f64,
    // let go minus tail time for holds, None when the key was still down
    pub release: Option<f64>,
    // let go in the middle of the hold
    pub broke: bool,
    // how long a tap's key stayed down
    pub held: Option<f64>,
}

fn miss_state(n: &Note, index: usize, w: &Windows, events: &mut Vec<Event>, mode: Mode, actual: Option<(f64, f64)>) -> NoteState {
    let ln = n.is_hold && n.end_time > n.time;
    let passive = if mode == Mode::Lazer || ln {
        w.meh
    } else if mode == Mode::StableV2 {
        w.ok
    } else {
        w.miss
    };
    let head_time = actual.map_or(n.time + passive, |a| a.0);
    let head_offset = actual.map_or(passive, |a| a.1);
    let state = NoteState { pressed: actual.is_some(), head_judgment: 6, head_offset, release: None, broke: false, held: None };
    if !ln {
        let time = match (actual, mode) {
            (None, Mode::Lazer) => n.time + w.meh,
            (None, _) => n.time + w.ok,
            _ => head_time,
        };
        events.push(Event { note: index, judgment: 6, time });
        return state;
    }
    if mode != Mode::Stable {
        let tail_window = if mode == Mode::StableV2 { w.ok } else { w.meh };
        events.push(Event { note: index, judgment: 6, time: head_time });
        events.push(Event { note: index, judgment: 6, time: n.end_time + tail_window * LENIENCE });
        return state;
    }
    // stable misses a hold whose head timed out right then
    events.push(Event { note: index, judgment: 6, time: head_time });
    state
}

pub struct Simulation {
    pub events: Vec<Event>,
    pub states: Vec<NoteState>,
}

pub fn simulate(notes: &[Note], segments: &[Vec<Segment>], keys: usize, w: &Windows, mode: Mode, speed: f64, no_release: bool) -> Simulation {
    let mut states = vec![NoteState::default(); notes.len()];
    let mut events: Vec<Event> = vec![];
    let separate = mode != Mode::Stable;
    let v2 = mode == Mode::StableV2;
    let stable = mode == Mode::Stable;
    // stable judges on its coarse frame clock, lazer reads the times as written
    let legacy = mode != Mode::Lazer;
    let separate_window = if v2 { w.ok } else { w.meh };
    // stable gates input on its unscaled miss window
    let v2_input = (w.miss / speed).floor();
    let v2_tail_input = v2_input * LENIENCE;
    let no_release_tails = mode == Mode::Lazer && no_release;
    let median = if stable { transition_median(segments) } else { 0.0 };
    let coarse_press = stable && median > COARSE_MEDIAN_TRANSITION;
    let coarse_release = stable;
    let held_ok_timeout_as_miss = keys <= 4;
    let next_note_edge_grace: f64 = match (stable, median > 0.0) {
        (false, _) => 0.0,
        (true, true) => 15.0,
        (true, false) => 10.0,
    };

    let mut by_column: Vec<Vec<usize>> = vec![vec![]; keys];
    for (i, n) in notes.iter().enumerate() {
        if n.column < keys {
            by_column[n.column].push(i);
        }
    }

    for column in 0..keys {
        let col_notes = &by_column[column];
        let segs = &segments[column];
        let mut cursor = 0usize;

        for pos in 0..col_notes.len() {
            let index = col_notes[pos];
            let note = &notes[index];
            let previous = pos.checked_sub(1).map(|p| &notes[col_notes[p]]);
            let next = col_notes.get(pos + 1).map(|&i| &notes[i]);
            let next_next = col_notes.get(pos + 2).map(|&i| &notes[i]);
            let ln = note.is_hold && note.end_time > note.time;
            // a sampled tap press can straddle the next note's start, holds keep scrolling when held late
            let crossed_head_ok = !ln || v2;
            let next_edge_grace = if stable && crossed_head_ok && next.is_some() { next_note_edge_grace } else { 0.0 };

            while cursor < segs.len() && segs[cursor].start < note.time - w.miss {
                cursor += 1;
            }

            let mut matched: Option<usize> = None;
            let mut head_judgment: Judgment = 0;
            let mut head_time = 0.0;
            let head_deadline = note.time + (if ln { w.meh } else { w.ok }).floor() - f64::EPSILON;
            let head_edge_grace = match next {
                Some(nx) if stable && crossed_head_ok => next_edge_grace.max(head_deadline - nx.time),
                _ => next_edge_grace,
            };
            let head_window = if v2 { (if ln { w.meh } else { w.ok }).floor() } else { separate_window };
            let latest_head = if separate {
                (note.time + head_window).min(next.map_or(note.time + head_window, |nx| nx.time - f64::EPSILON))
            } else if ln {
                head_deadline
            } else {
                head_deadline.min(next.map_or(head_deadline, |nx| nx.time - f64::EPSILON))
            };

            for (s, seg) in segs.iter().enumerate().skip(cursor) {
                let range = if (!separate || v2) && legacy { Some(press_range(seg)) } else { None };
                let range_min = range.map_or(seg.start, |r| r.0);
                if past_head_deadline(range_min, latest_head, head_edge_grace) {
                    break;
                }
                // stable expires the object on its frame update before it reads input
                if v2 && range_min >= note.time + head_window {
                    break;
                }
                let press = if range.is_some() { press_estimate(seg, coarse_press) } else { seg.start };
                if !coarse_press && press > latest_head {
                    if !crossed_head_ok {
                        break;
                    }
                    if range.is_none_or(|r| past_head_deadline(r.0, latest_head, head_edge_grace)) {
                        break;
                    }
                }
                if v2 && next.is_some_and(|nx| press >= nx.time) {
                    break;
                }
                let offset = press - note.time;
                let previous_is_ln = previous.is_some_and(|p| p.is_hold && p.end_time > p.time);
                if stable && !previous_is_ln && previous.is_some_and(|p| press < p.time) {
                    continue;
                }
                let j = if v2 {
                    stable_head(offset, w)
                } else if separate {
                    judgment_for(offset, w)
                } else if ln {
                    stable_ln_head_preview(offset, w)
                } else {
                    stable_head(offset, w)
                };
                // an early press can be inside a band while the object isn't hittable yet
                if v2 && offset < 0.0 && (j == 6 || offset < -v2_input) {
                    continue;
                }
                if j != 0 {
                    // a press past the next note's head still hits a waiting hold head inside MEH
                    if stable && ln && next.is_some_and(|nx| press >= nx.time) && offset > w.meh {
                        break;
                    }
                    matched = Some(s);
                    head_judgment = j;
                    head_time = press;
                    break;
                }
            }

            // slowed down, stable still takes hold heads out to the unscaled window
            if matched.is_none() && stable && ln && speed < 1.0 {
                let deadline = note.time + (w.meh / speed).floor();
                for (s, seg) in segs.iter().enumerate().skip(cursor) {
                    if press_range(seg).0 > deadline {
                        break;
                    }
                    let press = press_estimate(seg, coarse_press);
                    if press > deadline {
                        break;
                    }
                    if press <= note.time + w.meh.floor() {
                        continue;
                    }
                    if next.is_some_and(|nx| press >= nx.time - w.miss.floor()) {
                        break;
                    }
                    matched = Some(s);
                    head_judgment = 5;
                    head_time = press;
                    break;
                }
            }

            let Some(matched) = matched else {
                // lazer: a press after the head timed out still holds the note and its release judges the tail
                if separate && ln {
                    let tail_deadline = note.end_time + separate_window * LENIENCE;
                    let mut regrab: Option<(Judgment, f64, f64)> = None;
                    for (s, seg) in segs.iter().enumerate().skip(cursor) {
                        if v2 && seg.start < note.time - w.meh {
                            continue;
                        }
                        if v2 && seg.start > note.end_time {
                            break;
                        }
                        if seg.start > note.end_time + w.meh {
                            break;
                        }
                        if !no_release_tails && seg.end > tail_deadline {
                            break;
                        }
                        // OrderedHitPolicy: only before the next alive object's start
                        let next_alive = col_notes[pos + 1..].iter().map(|&i| notes[i].time).find(|&t| seg.start <= t + w.meh);
                        if let Some(t) = next_alive {
                            if seg.start >= t {
                                continue;
                            }
                            if !v2 && t - seg.start <= w.meh && note.end_time < t {
                                break;
                            }
                        }
                        if no_release_tails && seg.end >= note.end_time {
                            regrab = Some((cap_lazer_tail(1, true), seg.end - note.end_time, seg.start.max(note.end_time)));
                            break;
                        }
                        if seg.end > tail_deadline {
                            break;
                        }
                        let release = (seg.end - note.end_time) / LENIENCE;
                        let raw = if v2 { stable_v2_tail(seg.end - note.end_time, w) } else { judgment_for(release, w) };
                        if raw == 0 || (v2 && raw == 6 && release < 0.0) {
                            continue;
                        }
                        if v2 {
                            cursor = s + 1;
                        }
                        regrab = Some((cap_lazer_tail(raw, true), seg.end - note.end_time, seg.end));
                        break;
                    }
                    if let Some((j, release, time)) = regrab {
                        let head_miss = if v2 { w.meh } else { separate_window };
                        events.push(Event { note: index, judgment: 6, time: note.time + head_miss });
                        events.push(Event { note: index, judgment: j, time });
                        states[index] = NoteState { pressed: false, head_judgment: 6, head_offset: head_miss, release: Some(release), broke: false, held: None };
                        continue;
                    }
                }

                // a key held across the hold's head, its press already spent, is used up when the head times out
                let mut timed_out_held: Option<usize> = None;
                if let (true, Some(nx)) = (stable && ln, next) {
                    let passive = note.time + w.meh;
                    for (s, seg) in segs.iter().enumerate().skip(cursor) {
                        let r = press_range(seg);
                        if r.0 > passive + PRECISE_EDGE_INTERVAL {
                            break;
                        }
                        if seg.end < passive {
                            continue;
                        }
                        let press = press_estimate(seg, coarse_press);
                        if r.0 < nx.time && press >= nx.time && press > note.end_time {
                            timed_out_held = Some(s);
                            break;
                        }
                    }
                }
                states[index] = miss_state(note, index, w, &mut events, mode, None);
                if let Some(s) = timed_out_held.filter(|&s| s >= cursor) {
                    cursor = s + 1;
                }
                continue;
            };

            let head_seg = &segs[matched];
            let head_offset = head_time - note.time;

            // stable: a tap pressed in the miss tier misses right there, and the press stays free
            if head_judgment == 6 && !separate && !ln {
                states[index] = miss_state(note, index, w, &mut events, mode, Some((head_time, head_offset)));
                continue;
            }

            if !ln {
                events.push(Event { note: index, judgment: head_judgment, time: head_time });
                states[index] = NoteState { pressed: true, head_judgment, head_offset, release: None, broke: false, held: Some(head_seg.end - head_time) };
                if matched >= cursor {
                    cursor = matched + 1;
                }
                continue;
            }

            if separate {
                events.push(Event { note: index, judgment: head_judgment, time: head_time });
                let mut body_break: Option<f64> = None;
                let mut scan = matched;
                let mut unconsumed_recovery = false;
                let mut released_tail_timeout: Option<f64> = None;
                let tail_deadline = note.end_time + separate_window * LENIENCE;
                let mut tail_judgment: Judgment = 6;
                let mut tail_time = tail_deadline;
                let mut release: Option<f64> = None;
                let head_missed = head_judgment == 6;

                while scan < segs.len() {
                    let seg = &segs[scan];
                    if v2 && body_break.is_some() && seg.start > note.end_time {
                        unconsumed_recovery = true;
                        tail_judgment = 6;
                        break;
                    }
                    if no_release_tails && seg.end >= note.end_time && seg.start <= tail_deadline {
                        tail_judgment = cap_lazer_tail(1, body_break.is_some() || head_missed);
                        tail_time = seg.start.max(note.end_time);
                        release = Some(seg.end - note.end_time);
                        break;
                    }
                    if seg.end > tail_deadline {
                        tail_judgment = 6;
                        tail_time = tail_deadline;
                        break;
                    }
                    let release_offset = (seg.end - note.end_time) / LENIENCE;
                    let raw = if v2 { stable_v2_tail(seg.end - note.end_time, w) } else { judgment_for(release_offset, w) };
                    if raw != 0 && !(v2 && seg.end < note.end_time - v2_tail_input) {
                        let update = seg.end + (seg.end - seg.end_previous.unwrap_or(seg.end));
                        let combo_break = body_break.is_some() || head_missed || (v2 && update < note.end_time - w.meh);
                        tail_judgment = cap_lazer_tail(raw, combo_break);
                        tail_time = seg.end;
                        release = Some(seg.end - note.end_time);
                        if v2 && combo_break && body_break.is_none() && !head_missed {
                            tail_time = seg.end.max(note.end_time - w.meh);
                        }
                        break;
                    }
                    if seg.end >= tail_deadline {
                        tail_judgment = 6;
                        tail_time = tail_deadline;
                        break;
                    }
                    if body_break.is_none() {
                        body_break = Some(seg.end);
                    }
                    if v2 && released_tail_timeout.is_none() {
                        // a let-go hold can get a MEH as its tail enters the early window
                        let early = seg.end.max(note.end_time - w.meh);
                        if early <= note.end_time && segs.get(scan + 1).is_none_or(|n| n.start > early) {
                            released_tail_timeout = Some(early);
                        }
                    }
                    scan += 1;
                    if scan >= segs.len() || segs[scan].start > tail_deadline {
                        tail_judgment = 6;
                        tail_time = tail_deadline;
                        break;
                    }
                }
                if let Some(t) = released_tail_timeout {
                    tail_judgment = 5;
                    tail_time = t;
                }
                events.push(Event { note: index, judgment: tail_judgment, time: tail_time });
                states[index] = NoteState { pressed: true, head_judgment, head_offset, release, broke: body_break.is_some(), held: None };
                if matched >= cursor {
                    let hold_end = if !unconsumed_recovery && scan < segs.len() && segs[scan].start <= tail_deadline { scan + 1 } else { scan };
                    cursor = if v2 { hold_end } else { matched + 1 };
                }
                continue;
            }

            // stable hold: one combined judgement from the last press and release on it. early heads and
            // late releases are reflected around the note's ends, and a held tail times out at MEH
            let meh = w.meh.floor();
            let tail_early = note.end_time - meh;
            let tail_timeout = note.end_time + meh;

            let mut body_break: Option<f64> = None;
            let mut body_breaks: Vec<f64> = vec![];
            let mut cap: Option<Judgment> = None;
            // a release past the tail's miss window kills the hold, a softer one keeps it judgeable
            let mut hard_drop = false;
            let mut release_time = head_seg.end;
            let mut scan = matched;
            let mut scoring_press = head_time;
            let mut previous_release: Option<f64> = None;
            let mut previous_bridged = false;
            let mut tail_source: Option<f64> = None;
            let mut tail_held = false;
            let mut last_scanned = matched;
            let mut tail_index: Option<usize> = None;
            let mut tail_release_delay = 0.0;
            let mut tail_press_min: Option<f64> = None;
            let mut break_miss: Option<(f64, f64)> = None;

            while scan < segs.len() {
                let seg = &segs[scan];
                let first = scan == matched;
                let range = press_range(seg);
                if !first
                    && !previous_bridged
                    && previous_release.is_some_and(|r| r < tail_early && tail_early - r <= next_note_edge_grace)
                    && range.0 > tail_early
                    && seg.end < note.end_time
                {
                    tail_source = previous_release;
                    tail_held = false;
                    break;
                }
                last_scanned = scan;
                let press = press_estimate(seg, coarse_press);
                if !first && !previous_bridged {
                    scoring_press = press;
                }
                release_time = release_time.max(seg.end);
                let seg_release = release_estimate(seg, coarse_release);
                let raw_delay = if coarse_release && seg_release > note.end_time { seg.end_previous.map_or(0.0, |p| (seg.end - p).max(0.0)) } else { 0.0 };
                let seg_delay = if keys > 4 { raw_delay.min(2.0) } else { 0.0 };

                if seg.end < tail_early {
                    let next_seg = segs.get(scan + 1);
                    let bridge = can_bridge_tail_edge_gap(seg, next_seg, tail_early, next_note_edge_grace);
                    let after_head = seg.end > note.time;
                    let pre_head = first && seg.start < note.time && seg.end <= note.time;
                    let next_range = next_seg.map(press_range);
                    let next_press = next_seg.map_or(f64::INFINITY, |n| press_estimate(n, coarse_press));
                    let recovery_at_head = next_press <= note.time + PRECISE_EDGE_INTERVAL;
                    let next_covers_tail = next_seg.is_some_and(|n| press_range(n).0 <= tail_early && n.end >= tail_early);
                    let short = note.end_time - note.time <= meh;
                    let pre_head_activated = pre_head && short && (recovery_at_head || scoring_press < note.time - meh);
                    let pre_head_misses = pre_head && (pre_head_activated || (!recovery_at_head && !next_covers_tail));
                    let next_next_gap = match (next, next_next) {
                        (Some(n), Some(nn)) => nn.time - n.end_time,
                        _ => f64::INFINITY,
                    };
                    let before_edge = tail_early - seg.end;
                    let late_edge_break_misses = after_head && before_edge > 1.0 && before_edge <= next_note_edge_grace && !next_covers_tail;
                    let pre_head_recovered = pre_head && !pre_head_misses && next_covers_tail;

                    if !bridge && (after_head || pre_head) {
                        if seg.end < note.end_time - w.miss.min(w.miss / speed).floor() {
                            hard_drop = true;
                        }
                        cap = Some(cap.map_or(3, |c| c.max(3)));
                        if after_head || pre_head_recovered {
                            body_breaks.push(seg.end);
                        }
                        if pre_head_misses || late_edge_break_misses {
                            let time = if pre_head_misses { note.time } else { seg.end };
                            break_miss = Some((seg.end - note.end_time, time));
                            body_break = Some(time);
                            if pre_head_activated && next_seg.is_some_and(|n| n.start < note.time) {
                                last_scanned = last_scanned.max(scan + 1);
                            }
                            // a quick re-press before the head goes to this hold when the chain leaves a gap
                            if pre_head_misses
                                && next_seg.is_some_and(|n| next_range.unwrap().0 < note.end_time && n.start < note.time)
                                && next_next_gap >= 100.0
                                && seg.end - note.end_time <= -200.0
                            {
                                last_scanned = last_scanned.max(scan + 1);
                            }
                            break;
                        }
                        if (after_head || pre_head_recovered) && body_break.is_none() {
                            body_break = Some(seg.end);
                        }
                    }
                    previous_release = Some(seg_release);
                    previous_bridged = bridge;
                    scan += 1;
                    continue;
                }

                tail_index = Some(scan);
                tail_press_min = Some(range.0);
                if seg.end <= tail_timeout {
                    tail_source = Some(seg_release);
                    tail_held = false;
                    tail_release_delay = seg_delay;
                } else {
                    tail_source = Some(held_tail_timeout(seg, tail_timeout));
                    tail_held = true;
                    tail_release_delay = 0.0;
                }
                break;
            }

            let drop_press_after_tail = body_break.is_some() && tail_index.is_some_and(|t| t != matched) && tail_press_min.is_some_and(|m| m > note.end_time);
            let broken_press_after_tail = drop_press_after_tail && hard_drop;
            let soft_drop_at_tail = drop_press_after_tail && !hard_drop;

            let combined: Judgment;
            let combined_time: f64;
            let release: Option<f64>;
            if let Some((offset, time)) = break_miss {
                combined = 6;
                combined_time = time;
                release = Some(offset);
            } else if soft_drop_at_tail {
                let drop = body_breaks.last().copied().or(previous_release).unwrap_or(tail_early);
                let head = scoring_head_time(note, head_time, w) - note.time;
                combined = stable_combined(head, drop - note.end_time, cap, w).max(5);
                combined_time = note.end_time;
                release = Some(drop - note.end_time);
            } else if broken_press_after_tail {
                combined = 6;
                combined_time = note.end_time + w.miss.floor();
                release = tail_source.map(|t| t - note.end_time);
            } else {
                match tail_source.or(previous_release) {
                    None => {
                        combined = 6;
                        combined_time = head_time.max(tail_early);
                        release = None;
                    }
                    Some(source) => {
                        let judged = judge_stable_ln(note, scoring_press, source, tail_held, cap, w);
                        let delayed = if !tail_held && tail_release_delay > 0.0 && (3..=4).contains(&judged) {
                            judge_stable_ln(note, scoring_press, source + tail_release_delay, tail_held, cap, w)
                        } else {
                            judged
                        };
                        let final_ = judged.max(delayed);
                        let barely_crossed = head_time > note.end_time && head_time <= note.end_time + PRECISE_EDGE_INTERVAL;
                        let late_start_past_ok = scoring_press > note.end_time
                            && keys <= 4
                            && speed <= 1.01
                            && median >= CROSSED_LN_HEAD_MEDIAN_TRANSITION
                            && !tail_held
                            && source > note.end_time + w.ok.floor()
                            && source <= tail_timeout
                            && final_ == 4;
                        let held_ok_timeout = tail_held && held_ok_timeout_as_miss && final_ == 4 && median <= 8.0 && !barely_crossed;
                        combined = if late_start_past_ok {
                            5
                        } else if held_ok_timeout {
                            6
                        } else {
                            final_
                        };
                        combined_time = if tail_held { tail_timeout } else { source.max(tail_early) };
                        release = (!tail_held).then_some(source - note.end_time);
                    }
                }
            }
            events.push(Event { note: index, judgment: combined, time: combined_time });

            let mut last_consumed = last_scanned;
            if broken_press_after_tail {
                if let Some(t) = tail_index {
                    last_consumed = last_consumed.min(t.wrapping_sub(1));
                }
            }
            let mut next_cursor = (matched + 1).max(last_consumed.wrapping_add(1));
            // a hold let go before its own head keeps the column until that head
            if release_time > 0.0 && release_time < note.time {
                while next_cursor < segs.len() && segs[next_cursor].start < note.time {
                    next_cursor += 1;
                }
            }
            states[index] = NoteState { pressed: true, head_judgment, head_offset, release, broke: body_break.is_some(), held: None };
            if next_cursor > cursor {
                cursor = next_cursor;
            }
        }
    }

    events.sort_by(|a, b| a.time.total_cmp(&b.time).then(a.note.cmp(&b.note)));
    Simulation { events, states }
}

// osu!'s accuracy from counts indexed like judgments, 1 PERFECT to 6 MISS
pub fn accuracy(counts: &[u32; 7], mode: Mode) -> f64 {
    let total: u32 = counts[1..].iter().sum();
    if total == 0 {
        return 100.0;
    }
    let perfect = if mode == Mode::Stable { 300.0 } else { 305.0 };
    let value = counts[1] as f64 * perfect + counts[2] as f64 * 300.0 + counts[3] as f64 * 200.0 + counts[4] as f64 * 100.0 + counts[5] as f64 * 50.0;
    value / (total as f64 * perfect) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(column: usize, time: f64, end: Option<f64>) -> Note {
        Note { column, time, end_time: end.unwrap_or(time), is_hold: end.is_some() }
    }

    fn judge(notes: &[Note], frames: &[(f64, u32)], mode: Mode) -> Simulation {
        let frames: Vec<Frame> = frames.iter().map(|&(time, keys)| Frame { time, keys }).collect();
        let r = Ruleset { mode, difficulty: 1.0, speed: 1.0, classic: mode == Mode::Stable };
        let w = windows(8.0, &r);
        let segments = build_segments(&frames, 4, 10_000.0);
        simulate(notes, &segments, 4, &w, mode, 1.0, false)
    }

    #[test]
    fn stable_windows_at_od8() {
        let w = windows(8.0, &ruleset(false, &[], None));
        assert_eq!([w.perfect, w.great, w.good, w.ok, w.meh, w.miss], [16.5, 40.5, 73.5, 103.5, 127.5, 164.5]);
        assert_eq!(windows(8.0, &ruleset(false, &["DT".into()], None)).great, 60.5);
    }

    #[test]
    fn taps_take_their_press() {
        let notes = [note(0, 1000.0, None), note(0, 1300.0, None)];
        let sim = judge(&notes, &[(500.0, 0), (1009.0, 0), (1010.0, 1), (1040.0, 0)], Mode::Stable);
        assert_eq!(sim.states[0].head_judgment, 1);
        assert_eq!(sim.states[0].head_offset, 10.0);
        assert!(!sim.states[1].pressed);
        assert_eq!(sim.events.iter().filter(|e| e.judgment == MISS).count(), 1);
    }

    #[test]
    fn lazer_judges_head_and_tail_apart() {
        let notes = [note(1, 1000.0, Some(2000.0))];
        let sim = judge(&notes, &[(0.0, 0), (1005.0, 2), (1950.0, 0)], Mode::Lazer);
        let judgments: Vec<Judgment> = sim.events.iter().map(|e| e.judgment).collect();
        // head, then the tail: 50ms early over the 1.5x tail window is 33ms, a GREAT at OD8
        assert_eq!(judgments, [1, 2]);
        assert_eq!(sim.states[0].release, Some(-50.0));
    }

    #[test]
    fn random_matches_dotnet() {
        // new System.Random(0).Next()
        let mut r = CSharpRandom::new(0);
        assert_eq!([r.next(), r.next(), r.next()], [1559595546, 1755192844, 1649316166]);
    }
}
