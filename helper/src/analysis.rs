use serde_json::{json, Value};

use crate::beatmap::{parse_chart, Chart, Note};
use crate::judge::{self, Mode, Mod, NoteState, Windows};
use crate::osr::parse_replay;

const STABLE_MODS: [(i64, &str); 31] = [
    (1 << 0, "NF"), (1 << 1, "EZ"), (1 << 2, "TD"), (1 << 3, "HD"), (1 << 4, "HR"), (1 << 5, "SD"), (1 << 6, "DT"), (1 << 7, "RX"),
    (1 << 8, "HT"), (1 << 9, "NC"), (1 << 10, "FL"), (1 << 11, "AT"), (1 << 12, "SO"), (1 << 13, "AP"), (1 << 14, "PF"), (1 << 15, "4K"),
    (1 << 16, "5K"), (1 << 17, "6K"), (1 << 18, "7K"), (1 << 19, "8K"), (1 << 20, "FI"), (1 << 21, "RD"), (1 << 22, "CN"), (1 << 23, "TP"),
    (1 << 24, "9K"), (1 << 25, "CO"), (1 << 26, "1K"), (1 << 27, "3K"), (1 << 28, "2K"), (1 << 29, "V2"), (1 << 30, "MR"),
];

fn stable_mods(bits: i64) -> Vec<String> {
    let mut mods: Vec<&str> = STABLE_MODS.iter().filter(|(b, _)| bits & b != 0).map(|(_, m)| *m).collect();
    // NC and PF come with DT and SD set too
    if mods.contains(&"NC") {
        mods.retain(|m| *m != "DT");
    } else if mods.contains(&"PF") {
        mods.retain(|m| *m != "SD");
    }
    mods.into_iter().map(String::from).collect()
}

// the one speed a lazer play ran at, None when it changes during the play
fn lazer_rate(mods: &[Value]) -> Option<f64> {
    let rate: Vec<&Value> = mods.iter().filter(|m| ["DT", "NC", "HT", "DC", "WU", "WD", "AS"].contains(&acronym(m))).collect();
    match rate.as_slice() {
        [] => Some(1.0),
        [m] => {
            let fast = match acronym(m) {
                "DT" | "NC" => true,
                "HT" | "DC" => false,
                _ => return None,
            };
            let Some(raw) = m.pointer("/settings/speed_change") else { return Some(if fast { 1.5 } else { 0.75 }) };
            let speed = raw.as_f64()?;
            let (min, max) = if fast { (1.01, 2.0) } else { (0.5, 0.99) };
            (speed >= min - 1e-9 && speed <= max + 1e-9).then(|| (speed * 100.0).round() / 100.0)
        }
        _ => None,
    }
}

fn acronym(m: &Value) -> &str {
    m.get("acronym").and_then(Value::as_str).unwrap_or("")
}

pub(crate) struct Judged {
    pub chart: Chart,
    // the notes as played, mods applied
    pub notes: Vec<Note>,
    pub mode: Mode,
    pub rate: f64,
    pub windows: Windows,
    pub states: Vec<NoteState>,
    pub segments: Vec<Vec<judge::Segment>>,
    no_release: bool,
    // each note's judgements, two for a lazer hold
    pub judgments: Vec<Vec<judge::Judgment>>,
    pub mods: Vec<String>,
    // the replay header's, PERFECT to MISS
    pub game: [u32; 6],
}

pub(crate) fn judge_play(replay: &[u8], chart: &[u8], lazer: bool) -> Result<Judged, String> {
    let replay = parse_replay(replay)?;
    if replay.mode != 3 {
        return Err("The replay isn't a mania play.".into());
    }
    let chart = parse_chart(&String::from_utf8_lossy(chart))?;
    let lazer_mods: Option<Vec<Value>> = replay.lazer.as_ref().filter(|_| lazer).map(|l| l.get("mods").and_then(Value::as_array).cloned().unwrap_or_default());
    let (mods, rate, applied) = match &lazer_mods {
        Some(list) => {
            let names: Vec<String> = list.iter().map(|m| acronym(m).to_string()).collect();
            if names.iter().any(|m| m == "DS") {
                return Err("Dual stage plays can't be analyzed.".into());
            }
            let rate = lazer_rate(list).ok_or("Plays where the speed changes as you go can't be analyzed.")?;
            let applied = list.iter().map(|m| Mod { acronym: acronym(m).into(), seed: m.pointer("/settings/seed").and_then(Value::as_f64) }).collect::<Vec<_>>();
            (names, Some(rate), applied)
        }
        None => {
            let names = stable_mods(replay.mods);
            // stable doesn't write Random's seed down, so the columns can't be put back
            if names.iter().any(|m| m == "RD") {
                return Err("Plays with Random can't be analyzed on stable.".into());
            }
            if names.iter().any(|m| m == "CO") {
                return Err("Dual stage plays can't be analyzed.".into());
            }
            let applied = names.iter().map(|m| Mod { acronym: m.clone(), seed: None }).collect();
            (names, None, applied)
        }
    };
    let rules = judge::ruleset(lazer_mods.is_some(), &mods, rate);
    let windows = judge::windows(chart.od, &rules);
    let notes = judge::apply_mods(&chart.notes, chart.keys, &applied, &chart.timing_points);
    let frames = judge::decode_frames(&replay.rows);
    if frames.is_empty() {
        return Err("The replay has no key presses in it.".into());
    }
    let mut end = frames.last().map_or(0.0, |f| f.time);
    for n in &notes {
        end = end.max(n.end_time);
    }
    let segments = judge::build_segments(&frames, chart.keys, end + windows.miss * 2.0);
    let no_release = mods.iter().any(|m| m == "NR");
    let sim = judge::simulate(&notes, &segments, chart.keys, &windows, rules.mode, rules.speed, no_release);
    let mut judgments = vec![vec![]; notes.len()];
    for e in &sim.events {
        if e.judgment >= 1 {
            judgments[e.note].push(e.judgment);
        }
    }
    let h = replay.counts;
    Ok(Judged {
        chart,
        notes,
        mode: rules.mode,
        rate: rules.speed,
        windows,
        states: sim.states,
        segments,
        no_release,
        judgments,
        mods,
        game: [h[3], h[0], h[4], h[1], h[2], h[5]],
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hand {
    Left,
    Right,
}

// a finger needs longer than this to let a key up and press it again. 7-9 ms gaps turn up in real
// fast jacks on rapid trigger keyboards, a bouncing switch is 1-5 ms
const BOUNCE_MS: f64 = 6.0;

// a roll is worth a tip when lining it up would gain this much accuracy
const ROLL_GAIN: f64 = 0.1;

// keys going down within this many ms count as pressed at once
const WIDE_MS: f64 = 15.0;
// this many keys at once, most hitting nothing, is pressing everything
const WIDE_KEYS: usize = 4;
// a few stray presses happen to everyone: the tip needs this many, and this share of the notes
const EMPTY_MIN: usize = 30;
const EMPTY_SHARE: f64 = 0.15;
// a real acc run's clean accuracy is within this of the play's own
const CLEAN_GOAL_GAP: f64 = 2.0;

pub(crate) const FINGERS: [&str; 5] = ["thumb", "index", "middle", "ring", "pinky"];

// the player's own fingers when they set them for this key count, the hands stay split down the middle
fn fingers(keys: usize, thumb_left: bool, own: Option<&[String]>) -> Vec<(Hand, &'static str)> {
    let mut out = usual_fingers(keys, thumb_left);
    if let Some(own) = own.filter(|o| o.len() == keys) {
        for (slot, name) in out.iter_mut().zip(own) {
            if let Some(f) = FINGERS.iter().find(|f| **f == name) {
                slot.1 = f;
            }
        }
    }
    out
}

// (hand, finger) for each column
pub(crate) fn column_fingers(keys: usize, thumb_left: bool, own: Option<&[String]>) -> Vec<(&'static str, &'static str)> {
    fingers(keys, thumb_left, own).into_iter().map(|(h, f)| (hand_name(h), f)).collect()
}

pub(crate) fn usual_finger_names(keys: usize) -> Vec<String> {
    usual_fingers(keys, true).into_iter().map(|(_, f)| f.to_string()).collect()
}

// which finger plays each column: hands split down the middle, the middle column of an odd count is a thumb
fn usual_fingers(keys: usize, thumb_left: bool) -> Vec<(Hand, &'static str)> {
    let m = keys / 2;
    // inner to outer
    let side: Vec<&'static str> = if m <= 4 {
        ["index", "middle", "ring", "pinky"][..m].to_vec()
    } else {
        let mut v = vec!["thumb", "index", "middle", "ring", "pinky"];
        v.resize(m, "pinky");
        v
    };
    let mut out: Vec<(Hand, &'static str)> = (0..m).map(|c| (Hand::Left, side[m - 1 - c])).collect();
    if keys % 2 == 1 {
        out.push((if thumb_left { Hand::Left } else { Hand::Right }, "thumb"));
    }
    out.extend((0..m).map(|c| (Hand::Right, side[c])));
    out
}

fn hand_name(h: Hand) -> &'static str {
    match h {
        Hand::Left => "left",
        Hand::Right => "right",
    }
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn sd(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = mean(v);
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    }
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn ms(x: f64) -> String {
    let x = x.abs();
    if x < 9.95 {
        format!("{x:.1}")
    } else {
        format!("{x:.0}")
    }
}

#[derive(Default, Clone)]
struct Group {
    units: [u32; 7],
    hits: Vec<f64>,
    releases: Vec<f64>,
    taps: Vec<f64>,
    breaks: u32,
    notes: u32,
    holds: u32,
}

impl Group {
    fn acc(&self, mode: Mode) -> Option<f64> {
        (self.units[1..].iter().sum::<u32>() > 0).then(|| judge::accuracy(&self.units, mode))
    }

    fn json(&self, mode: Mode) -> Value {
        json!({
            "notes": self.notes,
            "acc": self.acc(mode).map(r2),
            "counts": &self.units[1..],
            "hits": self.hits.len(),
            "mean": r1(mean(&self.hits)),
            "sd": r1(sd(&self.hits)),
            "early": r2(self.hits.iter().filter(|x| **x < 0.0).count() as f64 / self.hits.len().max(1) as f64),
            "release": (self.releases.len() >= 3).then(|| json!({ "n": self.releases.len(), "mean": r1(mean(&self.releases)), "sd": r1(sd(&self.releases)) })),
            "holds": self.holds,
            "breaks": self.breaks,
            "tapHold": (self.taps.len() >= 5).then(|| median(&self.taps).round()),
        })
    }
}

// times in real milliseconds
struct Seen {
    // press minus note, a press the game took and didn't call a miss
    hit: Option<f64>,
    release: Option<f64>,
    // how long a tap's key stayed down
    tap: Option<f64>,
    broke: bool,
    missed: bool,
}

struct Tip {
    good: bool,
    title: String,
    text: String,
    // what fixing it would have added to this play's accuracy, in points
    gain: Option<f64>,
    weight: f64,
    // the stretch it's about is marked on the charts
    marked: bool,
    drill: Option<Value>,
}

fn seen_notes(j: &Judged) -> Vec<Seen> {
    let rate = j.rate;
    j.notes
        .iter()
        .zip(&j.states)
        .zip(&j.judgments)
        .map(|((n, s), js)| Seen {
            hit: (s.pressed && (1..=5).contains(&s.head_judgment)).then(|| s.head_offset / rate),
            release: (n.is_hold && s.pressed && !j.no_release).then_some(s.release).flatten().map(|r| r / rate),
            // a key left down past a second wasn't a tap anymore
            tap: s.held.filter(|_| (1..=5).contains(&s.head_judgment)).map(|h| h / rate).filter(|h| *h < 1000.0),
            broke: s.broke,
            missed: js.contains(&judge::MISS),
        })
        .collect()
}

fn add_note(j: &Judged, seen: &[Seen], g: &mut Group, i: usize) {
    g.notes += 1;
    for &u in &j.judgments[i] {
        g.units[u as usize] += 1;
    }
    if let Some(h) = seen[i].hit {
        g.hits.push(h);
    }
    if let Some(t) = seen[i].tap {
        g.taps.push(t);
    }
    if j.notes[i].is_hold {
        g.holds += 1;
        if let Some(r) = seen[i].release {
            g.releases.push(r);
        }
        if seen[i].broke {
            g.breaks += 1;
        }
    }
}

pub(crate) fn analyze(j: &Judged, thumb_left: bool, own_fingers: Option<&[String]>) -> Value {
    let mode = j.mode;
    let rate = j.rate;
    let keys = j.chart.keys;
    let lazer_names = mode != Mode::Stable;
    let names: [&str; 6] = if lazer_names { ["Perfect", "Great", "Good", "Ok", "Meh", "Miss"] } else { ["MAX", "300", "200", "100", "50", "Miss"] };
    let fingers = fingers(keys, thumb_left, own_fingers);
    let finger_label = |c: usize| {
        let (h, f) = fingers[c];
        format!("{} {f}", hand_name(h))
    };

    let seen = seen_notes(j);
    let add = |g: &mut Group, i: usize| add_note(j, &seen, g, i);

    let mut all = Group::default();
    let mut columns = vec![Group::default(); keys];
    let mut hands = [Group::default(), Group::default()];
    for (i, n) in j.notes.iter().enumerate() {
        add(&mut all, i);
        add(&mut columns[n.column], i);
        add(&mut hands[(fingers[n.column].0 == Hand::Right) as usize], i);
    }

    // jacks repeat a column within 200 real ms, chords share a time with another column
    let mut patterns = [Group::default(), Group::default(), Group::default(), Group::default()];
    let mut last_in_column: Vec<Option<f64>> = vec![None; keys];
    let mut chord_spreads = vec![];
    let mut kinds = vec![0usize; j.notes.len()];
    let mut jack_gaps = vec![];
    let mut i = 0;
    while i < j.notes.len() {
        let mut k = i;
        while k + 1 < j.notes.len() && (j.notes[k + 1].time - j.notes[i].time).abs() <= 1.0 {
            k += 1;
        }
        let chord = k > i;
        if chord {
            let presses: Vec<f64> = (i..=k).filter_map(|x| seen[x].hit.map(|h| h + j.notes[x].time / rate)).collect();
            if presses.len() == k - i + 1 {
                let (lo, hi) = presses.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &p| (lo.min(p), hi.max(p)));
                chord_spreads.push((hi - lo, presses.len()));
            }
        }
        for x in i..=k {
            let n = &j.notes[x];
            let gap = last_in_column[n.column].map(|t| (n.time - t) / rate);
            let jack = gap.is_some_and(|g| g <= 200.0);
            if jack && !n.is_hold {
                jack_gaps.extend(gap);
            }
            let kind = if n.is_hold {
                3
            } else if jack {
                2
            } else if chord {
                1
            } else {
                0
            };
            kinds[x] = kind;
            add(&mut patterns[kind], x);
        }
        for x in i..=k {
            last_in_column[j.notes[x].column] = Some(j.notes[x].end_time);
        }
        i = k + 1;
    }

    let start = j.notes.first().map_or(0.0, |n| n.time);
    let end = j.notes.iter().map(|n| n.end_time).fold(start, f64::max);
    let length = (end - start) / rate / 1000.0;
    let slices = ((length / 8.0).round() as usize).clamp(12, 40);
    let slice_of = |t: f64| (((t - start) / (end - start).max(1.0)) * slices as f64).floor().clamp(0.0, slices as f64 - 1.0) as usize;
    let mut sections = vec![Group::default(); slices];
    for (i, n) in j.notes.iter().enumerate() {
        add(&mut sections[slice_of(n.time)], i);
    }
    let slice_seconds = (end - start) / rate / 1000.0 / slices as f64;
    let nps = |g: &Group| g.notes as f64 / slice_seconds.max(0.001);

    // the same play judged again with some timing put right
    let base = judge::accuracy(&all.units, mode);
    let rejudge = |shift: &dyn Fn(usize, &mut judge::Segment)| {
        let mut segments = j.segments.clone();
        for (c, list) in segments.iter_mut().enumerate() {
            for s in list.iter_mut() {
                shift(c, s);
            }
        }
        let sim = judge::simulate(&j.notes, &segments, keys, &j.windows, mode, rate, j.no_release);
        let mut units = [0u32; 7];
        for e in sim.events.iter().filter(|e| e.judgment >= 1) {
            units[e.judgment as usize] += 1;
        }
        judge::accuracy(&units, mode) - base
    };
    let move_presses = |s: &mut judge::Segment, by: f64| {
        s.start += by;
        s.end += by;
        s.start_previous = s.start_previous.map(|x| x + by);
        s.end_previous = s.end_previous.map(|x| x + by);
        for x in &mut s.samples {
            *x += by;
        }
    };
    // stable's frames are whole milliseconds
    let map_ms = |real: f64| (real * rate).round();

    let mut tips: Vec<Tip> = vec![];
    let hit_mean = mean(&all.hits);
    let hit_sd = sd(&all.hits);
    let enough = all.hits.len() >= 100;
    let great_real = j.windows.great.floor() / rate;

    // a lean every column shares is the offset
    if enough {
        let leaning: Vec<f64> = columns.iter().filter(|c| c.hits.len() >= 20).map(|c| mean(&c.hits)).collect();
        let same_way = leaning.iter().filter(|m| m.signum() == hit_mean.signum() && m.abs() >= 2.0).count();
        if hit_mean.abs() >= 4.0 && same_way * 4 >= leaning.len() * 3 {
            let late = hit_mean > 0.0;
            let by = map_ms(hit_mean);
            let gain = rejudge(&|_, s| move_presses(s, -by));
            tips.push(Tip {
                good: false,
                title: format!("You hit {} ms {} on average", ms(hit_mean), if late { "late" } else { "early" }),
                text: format!(
                    "{same_way} of {} columns are {}, so it's your offset, not your fingers. If your other plays are {} too, {} your {} offset by {} ms.",
                    leaning.len(),
                    if late { "late" } else { "early" },
                    if late { "late" } else { "early" },
                    if late { "lower" } else { "raise" },
                    if mode == Mode::Lazer { "audio" } else { "universal" },
                    ms(hit_mean)
                ),
                gain: Some(gain),
                weight: gain,
                marked: false,
                drill: None,
            });
        } else if hit_mean.abs() < 2.5 {
            tips.push(Tip { good: true, title: "Your timing is centered".into(), text: format!("Your average press is {} ms from the note, so your offset is right.", ms(hit_mean)), gain: None, weight: 0.3, marked: false, drill: None });
        }

        let outside = all.hits.iter().filter(|h| (*h - hit_mean).abs() > great_real).count() as f64 / all.hits.len() as f64;
        if outside >= 0.2 {
            tips.push(Tip {
                good: false,
                title: format!("{:.0}% of your presses miss the {} window", outside * 100.0, names[1]),
                text: format!(
                    "Your presses spread ±{} ms around your average, and the {} window is only ±{} ms wide.",
                    ms(hit_sd),
                    names[1],
                    ms(great_real)
                ),
                gain: None,
                weight: outside * 2.0,
                marked: false,
                drill: None,
            });
        } else if outside < 0.06 && hit_sd <= 12.0 {
            tips.push(Tip { good: true, title: "Your presses are consistent".into(), text: format!("They spread only ±{} ms around your average.", ms(hit_sd)), gain: None, weight: 0.4, marked: false, drill: None });
        }
    }

    let [left, right] = &hands;
    if left.hits.len() >= 40 && right.hits.len() >= 40 {
        let (lm, rm) = (mean(&left.hits), mean(&right.hits));
        if (lm - rm).abs() >= 6.0 {
            let later = if lm > rm { "left" } else { "right" };
            let earlier = if lm > rm { "right" } else { "left" };
            let center = hit_mean;
            let (lshift, rshift) = (map_ms(center - lm), map_ms(center - rm));
            let gain = rejudge(&|c, s| move_presses(s, if fingers[c].0 == Hand::Left { lshift } else { rshift }));
            tips.push(Tip {
                good: false,
                title: format!("Your {later} hand hits {} ms after your {earlier}", ms(lm - rm)),
                text: format!("Left hand averages {} ms, right hand {} ms. Press about {} ms earlier with your {later} hand to line them up.", signed(lm), signed(rm), ms(lm - rm)),
                gain: Some(gain),
                weight: gain,
                marked: false,
                drill: None,
            });
        }
    }

    let ranked: Vec<(usize, f64)> = columns.iter().enumerate().filter(|(_, c)| c.notes >= 30).filter_map(|(i, c)| c.acc(mode).map(|a| (i, a))).collect();
    if ranked.len() >= 2 {
        let (worst, worst_acc) = ranked.iter().copied().min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        let mut rest = Group::default();
        for (c, g) in columns.iter().enumerate().filter(|(c, _)| *c != worst) {
            let _ = c;
            for u in 1..7 {
                rest.units[u] += g.units[u];
            }
            rest.hits.extend(&g.hits);
        }
        let rest_acc = rest.acc(mode).unwrap_or(100.0);
        let w = &columns[worst];
        if rest_acc - worst_acc >= 1.0 {
            let lean = mean(&w.hits) - mean(&rest.hits);
            let why = if lean.abs() >= 5.0 {
                format!(" It presses {} ms {} than your other fingers, so press about that much {} with it.", ms(lean), if lean > 0.0 { "later" } else { "earlier" }, if lean > 0.0 { "earlier" } else { "later" })
            } else if sd(&w.hits) > sd(&rest.hits) * 1.25 {
                format!(" Its presses spread ±{} ms, your other fingers' ±{} ms.", ms(sd(&w.hits)), ms(sd(&rest.hits)))
            } else if w.units[6] > 0 && w.units[6] == all.units[6] {
                format!(" {} on it.", if w.units[6] == 1 { "The play's only miss is" } else { "Every miss in the play is" })
            } else if w.units[6] > 0 {
                format!(" {} of the play's {} misses {} on it.", w.units[6], all.units[6], if w.units[6] == 1 { "is" } else { "are" })
            } else {
                String::new()
            };
            // the ring finger shares tendons with the middle, the pinky is the weakest one
            let swap = match fingers[worst].1 {
                "ring" => " The ring finger can't move fully on its own, the middle finger pulls on it, so it tenses up. Playing this column with your middle finger often fixes it.",
                "pinky" => " The pinky is the weakest finger. Playing this column with a stronger finger often fixes it.",
                _ => "",
            };
            let gain = (lean.abs() >= 5.0).then(|| {
                let by = map_ms(-lean);
                rejudge(&|c, s| if c == worst { move_presses(s, by) })
            });
            let weight = (rest_acc - worst_acc) * w.notes as f64 / all.notes.max(1) as f64;
            tips.push(Tip {
                good: false,
                title: format!("Your {} is your weakest finger", finger_label(worst)),
                text: format!("Column {} gets {:.2}%, your other columns {:.2}%.{why}{swap}", worst + 1, worst_acc, rest_acc),
                gain,
                weight: gain.unwrap_or(weight),
                marked: false,
                drill: None,
            });
        }
    }

    let releases = &all.releases;
    if releases.len() >= 20 {
        let rm = mean(releases);
        let rs = sd(releases);
        if rm.abs() >= 12.0 {
            let by = map_ms(-rm);
            let gain = rejudge(&|_, s| {
                s.end = (s.end + by).max(s.start + 1.0);
                s.end_previous = s.end_previous.map(|x| x + by);
            });
            let early = rm < 0.0;
            tips.push(Tip {
                good: false,
                title: if early { format!("You let go of long notes {} ms early", ms(rm)) } else { format!("You hold long notes {} ms too long", ms(rm)) },
                text: format!(
                    "Your presses average {} ms, so it's the releases: let go about {} ms {}, right as the tail reaches the judgement line.",
                    signed(hit_mean),
                    ms(rm),
                    if early { "later" } else { "sooner" }
                ),
                gain: Some(gain),
                weight: gain,
                marked: false,
                drill: None,
            });
        } else if rs > hit_sd * 1.5 && rs > 25.0 {
            tips.push(Tip {
                good: false,
                title: "Your releases are less consistent than your presses".into(),
                text: format!("Your releases spread ±{} ms around their average, your presses only ±{} ms.", ms(rs), ms(hit_sd)),
                gain: None,
                weight: 0.5,
                marked: false,
                drill: None,
            });
        } else {
            tips.push(Tip { good: true, title: "Your releases are on time".into(), text: format!("They average {} ms from the tail, nothing to change there.", signed(rm)), gain: None, weight: 0.3, marked: false, drill: None });
        }
    }
    if all.breaks >= 3 && all.breaks as f64 >= all.holds as f64 * 0.02 {
        let cap = if mode == Mode::Stable { names[2] } else { names[4] };
        tips.push(Tip {
            good: false,
            title: format!("You let go in the middle of {} long notes", all.breaks),
            text: format!("Each of those scores {cap} at most, even if you press it again before the tail. Keep the key down until the tail."),
            gain: None,
            weight: all.breaks as f64 / all.notes.max(1) as f64 * 30.0,
            marked: false,
            drill: None,
        });
    }

    let filled: Vec<&Group> = sections.iter().filter(|g| g.notes >= 8).collect();
    if filled.len() >= 8 {
        let mut by_density: Vec<&Group> = filled.clone();
        by_density.sort_by(|a, b| nps(b).total_cmp(&nps(a)));
        let top = by_density.len() / 4;
        let merge = |gs: &[&Group]| {
            let mut g = Group::default();
            for x in gs {
                for u in 1..7 {
                    g.units[u] += x.units[u];
                }
                g.notes += x.notes;
            }
            g
        };
        let (dense, rest) = (merge(&by_density[..top]), merge(&by_density[top..]));
        let threshold = nps(by_density[top - 1]);
        if let (Some(da), Some(ra)) = (dense.acc(mode), rest.acc(mode)) {
            if da >= ra - 0.3 && ra < 99.5 && threshold >= 8.0 {
                tips.push(Tip {
                    good: true,
                    title: "You keep your accuracy in dense parts".into(),
                    text: format!("Counted on their own, the notes in parts with {:.0} or more a second get {:.2}% and the rest of the map {:.2}%.", threshold, da, ra),
                    gain: None,
                    weight: 0.2,
                    marked: false,
                    drill: None,
                });
            }
        }

        let third = slices / 3;
        let merge_range = |a: usize, b: usize| {
            let mut g = Group::default();
            for x in &sections[a..b] {
                for u in 1..7 {
                    g.units[u] += x.units[u];
                }
                g.notes += x.notes;
                g.hits.extend(&x.hits);
            }
            g
        };
        let (first, last) = (merge_range(0, third), merge_range(slices - third, slices));
        let per_second = |g: &Group| g.notes as f64 / (slice_seconds * third as f64).max(0.001);
        if let (Some(fa), Some(la)) = (first.acc(mode), last.acc(mode)) {
            if fa - la >= 2.0 && per_second(&last) <= per_second(&first) * 1.15 {
                tips.push(Tip {
                    good: false,
                    title: "Your accuracy drops at the end of the map".into(),
                    text: format!("Counted on their own, the notes in the last third get {:.2}% and the first third {:.2}%, with about as many notes a second, so it's stamina rather than harder patterns.", la, fa),
                    gain: None,
                    weight: (fa - la) / 3.0,
                    marked: false,
                    drill: None,
                });
            }
        }
        if first.hits.len() >= 30 && last.hits.len() >= 30 {
            let drift = mean(&last.hits) - mean(&first.hits);
            if drift.abs() >= 6.0 {
                tips.push(Tip {
                    good: false,
                    title: format!("Your timing gets {} as the map goes on", if drift > 0.0 { "later" } else { "earlier" }),
                    text: format!("Your presses average {} ms in the first third and {} ms in the last.", signed(mean(&first.hits)), signed(mean(&last.hits))),
                    gain: None,
                    weight: drift.abs() / 10.0,
                    marked: false,
                    drill: None,
                });
            }
        }
    }

    // how far apart a chord's keys would land from your timing spread alone: the expected range of
    // that many independent hits
    let spread = mean(&chord_spreads.iter().map(|c| c.0).collect::<Vec<_>>());
    if chord_spreads.len() >= 15 && hit_sd > 0.0 {
        const RANGE: [f64; 9] = [0.0, 0.0, 1.128, 1.693, 2.059, 2.326, 2.534, 2.704, 2.847];
        let expected = mean(&chord_spreads.iter().map(|c| RANGE[c.1.min(8)] * hit_sd).collect::<Vec<_>>());
        if spread >= 10.0 && spread >= expected * 1.25 {
            tips.push(Tip {
                good: false,
                title: "Your chord keys don't go down together".into(),
                text: format!("A chord's keys land {} ms apart on average. With how much your presses usually spread, about {} ms would be normal.", ms(spread), ms(expected)),
                gain: None,
                weight: (spread - expected) / 10.0,
                marked: false,
                drill: None,
            });
        } else if spread <= 6.0 {
            tips.push(Tip { good: true, title: "Your chord keys go down together".into(), text: format!("A chord's keys land {} ms apart on average.", ms(spread)), gain: None, weight: 0.2, marked: false, drill: None });
        }
    }
    // a tilted hand lands one finger first, which the spread over every key averages away
    let mut pairs: Vec<Vec<f64>> = vec![vec![]; keys.saturating_sub(1)];
    let mut pair_times: Vec<Vec<f64>> = vec![vec![]; keys.saturating_sub(1)];
    let mut i = 0;
    while i < j.notes.len() {
        let mut k = i;
        while k + 1 < j.notes.len() && (j.notes[k + 1].time - j.notes[i].time).abs() <= 1.0 {
            k += 1;
        }
        if k > i {
            let mut at = vec![None; keys];
            for x in i..=k {
                at[j.notes[x].column] = seen[x].hit;
            }
            for c in 0..keys.saturating_sub(1) {
                if let (Some(a), Some(b)) = (at[c], at[c + 1]) {
                    if fingers[c].0 == fingers[c + 1].0 {
                        pairs[c].push(a - b);
                        pair_times[c].push(j.notes[i].time);
                    }
                }
            }
        }
        i = k + 1;
    }
    for hand in [Hand::Left, Hand::Right] {
        let rolled = pairs
            .iter()
            .enumerate()
            .filter(|(c, p)| fingers[*c].0 == hand && p.len() >= 30)
            .map(|(c, p)| (c, mean(p), sd(p), p.len()))
            // about half the Perfect window, and there in this many chords, not chance. a hand pressing
            // as one leans under 2 ms
            .filter(|(_, m, s, n)| m.abs() >= 7.0 && m.abs() >= 3.0 * s / (*n as f64).sqrt())
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()));
        // a steady lead inside the judgement windows costs next to nothing, a lead that changes chord to
        // chord does: the play judged again with both fingers of those chords meeting halfway
        let rolled = rolled.map(|(c, m, s, n)| {
            let times = &pair_times[c];
            let in_chord = |t: f64| times.binary_search_by(|x| x.total_cmp(&t)).map_or_else(|i| [i.wrapping_sub(1), i].iter().any(|&k| times.get(k).is_some_and(|x| (x - t).abs() <= j.windows.miss)), |_| true);
            let half = map_ms(m / 2.0);
            let gain = rejudge(&|col, seg| {
                if col == c && in_chord(seg.start) {
                    move_presses(seg, -half);
                } else if col == c + 1 && in_chord(seg.start) {
                    move_presses(seg, half);
                }
            });
            (c, m, s, n, gain)
        });
        // how much the gap swings is mostly each finger's own spread, the other tips cover that
        if let Some((c, m, s, n, gain)) = rolled.filter(|r| r.4 >= ROLL_GAIN) {
            let (first, second) = if m > 0.0 { (c + 1, c) } else { (c, c + 1) };
            tips.push(Tip {
                good: false,
                title: format!("Your {} hand rolls its chords", hand_name(hand)),
                text: format!(
                    "When columns {} and {} go down together, your {} lands {} ms before your {} on average, over {n} chords. Press them as one motion from the wrist so both land at once. A tilted hand makes one finger lead.",
                    c + 1,
                    c + 2,
                    finger_label(first),
                    ms(m),
                    finger_label(second)
                ),
                gain: Some(gain),
                weight: gain,
                marked: false,
                drill: Some(json!({ "columns": [c, c + 1], "mean": r1(m), "sd": r1(s), "chords": n })),
            });
        }
    }

    let [singles, chords, jacks, _] = &patterns;
    let mut others = singles.clone();
    others.hits.extend(&chords.hits);
    for u in 1..7 {
        others.units[u] += chords.units[u];
    }
    // on a jack map the notes that aren't jacks are mostly each run's first chord, so jacks
    // against the rest says nothing there
    if jacks.hits.len() >= 30 && others.hits.len() >= 30 && jacks.notes * 2 <= all.notes {
        let lag = mean(&jacks.hits) - mean(&others.hits);
        let (ja, oa) = (jacks.acc(mode).unwrap_or(100.0), others.acc(mode).unwrap_or(100.0));
        if lag >= 6.0 && ja < oa {
            tips.push(Tip {
                good: false,
                title: "You hit jacks late".into(),
                text: format!("You press jacks {} ms later than your other notes, and they get {:.2}% against {:.2}% for the rest. Press them about {} ms earlier.", ms(lag), ja, oa, ms(lag)),
                gain: None,
                weight: (oa - ja).max(lag / 10.0) * jacks.notes as f64 / all.notes.max(1) as f64,
                marked: false,
                drill: None,
            });
        } else if oa - ja >= 2.0 {
            tips.push(Tip {
                good: false,
                title: "You lose accuracy on jacks".into(),
                text: format!("Counted on their own, jacks get {:.2}% and the rest {:.2}%.", ja, oa),
                gain: None,
                weight: (oa - ja) * jacks.notes as f64 / all.notes.max(1) as f64,
                marked: false,
                drill: None,
            });
        }
    }

    let early_misses = j.states.iter().zip(&seen).filter(|(s, x)| x.missed && s.pressed && s.head_judgment == judge::MISS && s.head_offset < 0.0).count();
    if early_misses >= 5 && early_misses * 10 >= all.units[6] as usize * 3 {
        tips.push(Tip {
            good: false,
            title: format!("{early_misses} of your misses were early presses"),
            text: format!("You pressed before the {} window opened, and osu! counts that as a miss.", names[4]),
            gain: None,
            weight: early_misses as f64 / all.notes.max(1) as f64 * 20.0,
            marked: false,
            drill: None,
        });
    }

    // fingers that stay down on taps: a key has to come back up before the next note in its column
    let held: Vec<(usize, f64)> = columns.iter().enumerate().filter(|(_, c)| c.taps.len() >= 20).map(|(i, c)| (i, median(&c.taps))).collect();
    if held.len() >= 2 {
        let (slowest, h) = held.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
        let others: Vec<f64> = columns.iter().enumerate().filter(|(i, _)| *i != slowest).flat_map(|(_, c)| c.taps.iter().copied()).collect();
        let o = median(&others);
        if h - o >= 25.0 && h >= o * 1.4 {
            tips.push(Tip {
                good: false,
                title: format!("You hold taps longer with your {}", finger_label(slowest)),
                text: format!("Taps on column {} stay down {:.0} ms, on your other columns {:.0} ms. Lift that finger sooner, it has to come back up before the next note in that column.", slowest + 1, h, o),
                gain: None,
                weight: 0.5 + (h - o) / 100.0,
                marked: false,
                drill: None,
            });
        }
    }
    let (jacks, others_acc) = (&patterns[2], {
        let mut g = patterns[0].clone();
        for u in 1..7 {
            g.units[u] += patterns[1].units[u];
        }
        g.acc(mode)
    });
    if jack_gaps.len() >= 30 && all.taps.len() >= 30 {
        let (gap, hold) = (median(&jack_gaps), median(&all.taps));
        let ja = jacks.acc(mode).unwrap_or(100.0);
        if gap - hold <= 40.0 && others_acc.is_some_and(|oa| ja < oa) {
            tips.push(Tip {
                good: false,
                title: "Your taps leave little room for this map's jacks".into(),
                text: format!(
                    "Your taps stay down {:.0} ms and the jacks here are {:.0} ms apart, so a key has {:.0} ms to come up and go down again. Lift your fingers sooner after each tap.",
                    hold,
                    gap,
                    (gap - hold).max(0.0)
                ),
                gain: None,
                weight: (others_acc.unwrap_or(100.0) - ja) * jacks.notes as f64 / all.notes.max(1) as f64 + 0.3,
                marked: false,
                drill: None,
            });
        }
    }

    // the eight seconds that cost the most
    let practice = {
        let window = 8000.0 * rate;
        let n = j.notes.len();
        let mut sums = vec![[0u32; 7]; n + 1];
        for i in 0..n {
            sums[i + 1] = sums[i];
            for &u in &j.judgments[i] {
                sums[i + 1][u as usize] += 1;
            }
        }
        let units = |a: usize, b: usize| {
            let mut u = [0u32; 7];
            for k in 1..7 {
                u[k] = sums[b][k] - sums[a][k];
            }
            u
        };
        let mut worst: Option<(usize, usize, f64)> = None;
        let mut e = 0;
        for st in 0..n {
            while e < n && j.notes[e].time < j.notes[st].time + window {
                e += 1;
            }
            // near the end the window runs past the last note, and a final burst can be under a second long
            if e - st < 20 || j.notes[e - 1].time - j.notes[st].time < window / 2.0 {
                continue;
            }
            let a = judge::accuracy(&units(st, e), mode);
            if worst.is_none_or(|w| a < w.2) {
                worst = Some((st, e, a));
            }
        }
        worst.and_then(|(st, e, acc)| {
            let inside = units(st, e);
            let mut outside = units(0, n);
            for k in 1..7 {
                outside[k] -= inside[k];
            }
            let rest = judge::accuracy(&outside, mode);
            (rest - acc >= 1.5).then(|| {
                let from = j.notes[st].time;
                let to = j.notes[st..e].iter().map(|x| x.end_time).fold(from, f64::max);
                let mut count = [0usize; 4];
                for &k in &kinds[st..e] {
                    count[k] += 1;
                }
                let (kind, most) = count.iter().copied().enumerate().max_by_key(|x| x.1).unwrap();
                let mut by_column = vec![[0u32; 7]; keys];
                for x in st..e {
                    for &u in &j.judgments[x] {
                        by_column[j.notes[x].column][u as usize] += 1;
                    }
                }
                let weak = by_column
                    .iter()
                    .enumerate()
                    .filter(|(_, u)| u[1..].iter().sum::<u32>() >= 5)
                    .map(|(c, u)| (c, judge::accuracy(u, mode)))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .filter(|w| w.1 < acc - 2.0);
                (from / rate / 1000.0, to / rate / 1000.0, acc, rest, (e - st) as f64 / ((to - from) / rate / 1000.0).max(1.0), kind, most as f64 / (e - st) as f64, weak, e - st)
            })
        })
    };
    if let Some((from, to, acc, rest, per_second, kind, share, weak, count)) = practice {
        let pattern = if share >= 0.45 {
            format!(" Most of it is {} ({:.0}% of its notes).", ["single notes", "chords", "jacks", "long notes"][kind], share * 100.0)
        } else {
            " It's a mix of patterns.".into()
        };
        let finger = weak.map_or(String::new(), |(c, a)| format!(" Your {} does worst there, {:.2}% on its notes.", finger_label(c), a));
        tips.push(Tip {
            good: false,
            title: format!("Practice {} to {}", clock(from), clock(to)),
            text: format!("Counted on their own, the notes in this part get {acc:.2}% and the rest of the map {rest:.2}%. It has {per_second:.0} notes a second.{pattern}{finger}"),
            gain: None,
            weight: (rest - acc) * count as f64 / all.notes.max(1) as f64 * 3.0,
            marked: true,
            drill: None,
        });
    }

    // presses with no note in their column anywhere near: osu! lets them go, so pressing every key on
    // every beat clears a jack map at full accuracy without any finger learning which notes are its own
    let Clean { count: empty_count, presses, wide, wide_empty, clean, shown: empty_shown, goal: clean_goal } = clean_stats(j);
    if empty_shown {
        let share = empty_count as f64 / presses.max(1) as f64;
        let how = if wide_empty * 2 >= empty_count { format!(", mostly {}+ keys at once", WIDE_KEYS.min(keys)) } else { String::new() };
        tips.push(Tip {
            good: false,
            title: "You're pressing keys with no notes".into(),
            text: format!(
                "{empty_count} of your {presses} presses hit nothing ({:.0}%){how}. Clean accuracy, with those as misses: {clean:.2}%. Goal: a real acc run, {clean_goal:.0}% clean. Press only the notes, slower if you need to.",
                share * 100.0
            ),
            gain: None,
            weight: 2.0 + share * 5.0,
            marked: false,
            drill: None,
        });
    }

    let bounces: Vec<usize> = j
        .segments
        .iter()
        .map(|list| list.windows(2).filter(|w| (w[1].start - w[0].end) / rate < BOUNCE_MS).count())
        .collect();
    if let Some((c, &n)) = bounces.iter().enumerate().max_by_key(|x| *x.1) {
        let others = bounces.iter().sum::<usize>() - n;
        if n >= 3 && n >= others * 2 {
            tips.push(Tip {
                good: false,
                title: format!("The key on column {} may be faulty", c + 1),
                text: format!(
                    "It came up and went down again within {BOUNCE_MS:.0} ms {n} times, faster than a finger can. That's usually a worn or chattering switch, or a rapid trigger setting so sensitive that a shaky finger counts as a new press. Test the key in a keyboard tester before training your {}. If two of your keys play this column, ignore this.",
                    finger_label(c)
                ),
                gain: None,
                weight: 1.0 + n as f64 / 10.0,
                marked: false,
                drill: None,
            });
        }
    }

    tips.sort_by(|a, b| a.good.cmp(&b.good).then(b.weight.total_cmp(&a.weight)));

    let mut game = [0u32; 7];
    game[1..].copy_from_slice(&j.game);
    let pattern_names = ["Singles", "Chords", "Jacks", "Long notes"];
    let windows = [j.windows.perfect, j.windows.great, j.windows.good, j.windows.ok, j.windows.meh].map(|w| r1(w.floor() / rate));
    json!({
        "mode": match mode { Mode::Stable => "stable", Mode::StableV2 => "scorev2", Mode::Lazer => "lazer" },
        "names": names,
        "keys": keys,
        "od": j.chart.od,
        "rate": rate,
        "mods": j.mods,
        "length": r1(length),
        // the song's time at the first note, the charts count from there
        "offset": r2(start / rate / 1000.0),
        "practice": practice.map(|(from, to, acc, rest, ..)| json!({ "from": r2(from - start / rate / 1000.0), "to": r2(to - start / rate / 1000.0), "acc": r2(acc), "rest": r2(rest) })),
        "tapHold": (all.taps.len() >= 5).then(|| median(&all.taps).round()),
        "accuracy": r2(judge::accuracy(&game, mode)),
        "judged": r2(base),
        "counts": j.game,
        "windows": windows,
        "timing": {
            "n": all.hits.len(),
            "mean": r1(hit_mean),
            "median": r1(median(&all.hits)),
            "sd": r1(hit_sd),
            "ur": r1(hit_sd * 10.0),
            "early": r2(all.hits.iter().filter(|x| **x < 0.0).count() as f64 / all.hits.len().max(1) as f64),
        },
        "release": (releases.len() >= 3).then(|| json!({
            "n": releases.len(),
            "mean": r1(mean(releases)),
            "median": r1(median(releases)),
            "sd": r1(sd(releases)),
            "early": r2(releases.iter().filter(|x| **x < 0.0).count() as f64 / releases.len() as f64),
            "holds": all.holds,
            "breaks": all.breaks,
        })),
        "thumb": (keys % 2 == 1).then_some(if thumb_left { "left" } else { "right" }),
        "columns": columns.iter().enumerate().map(|(c, g)| {
            let mut v = g.json(mode);
            v["hand"] = hand_name(fingers[c].0).into();
            v["finger"] = fingers[c].1.into();
            v
        }).collect::<Vec<_>>(),
        "hands": { "left": hands[0].json(mode), "right": hands[1].json(mode) },
        "patterns": patterns.iter().zip(pattern_names).filter(|(g, _)| g.notes > 0).map(|(g, name)| {
            let mut v = g.json(mode);
            v["name"] = name.into();
            v
        }).collect::<Vec<_>>(),
        "chordSpread": (chord_spreads.len() >= 5).then(|| r1(spread)),
        "empty": { "count": empty_count, "presses": presses, "clean": r2(clean), "wide": wide, "shown": empty_shown, "goal": clean_goal },
        "pairs": pairs.iter().enumerate().filter(|(_, p)| p.len() >= 10).map(|(c, p)| json!({
            "columns": [c, c + 1],
            "hand": hand_name(fingers[c].0),
            "mean": r1(mean(p)),
            "sd": r1(sd(p)),
            "chords": p.len(),
        })).collect::<Vec<_>>(),
        "sections": sections.iter().enumerate().map(|(i, g)| json!({
            "at": r1(i as f64 * slice_seconds),
            "acc": g.acc(mode).map(r2),
            "mean": (!g.hits.is_empty()).then(|| r1(mean(&g.hits))),
            "nps": r1(nps(g)),
            "misses": g.units[6],
        })).collect::<Vec<_>>(),
        "sectionLength": r1(slice_seconds),
        // [seconds, offset, judgment, column]
        "hits": j.notes.iter().zip(&j.states).filter(|(_, s)| s.pressed && s.head_judgment >= 1).map(|(n, s)| {
            json!([r2((n.time - start) / rate / 1000.0), r1(s.head_offset / rate), s.head_judgment, n.column])
        }).collect::<Vec<_>>(),
        "releases": j.notes.iter().zip(&seen).filter_map(|(n, x)| x.release.map(|r| json!([r2((n.end_time - start) / rate / 1000.0), r1(r), n.column]))).collect::<Vec<_>>(),
        "misses": j.notes.iter().zip(&seen).filter(|(_, x)| x.missed).map(|(n, _)| json!([r2((n.time - start) / rate / 1000.0), n.column])).collect::<Vec<_>>(),
        "tips": tips.iter().map(|t| json!({
            "tone": if t.good { "good" } else { "work" },
            "title": t.title,
            "text": t.text,
            "gain": t.gain.filter(|g| *g >= 0.05).map(r2),
            "marked": t.marked,
            "drill": t.drill,
        })).collect::<Vec<_>>(),
    })
}

pub(crate) struct Clean {
    pub count: usize,
    pub presses: usize,
    pub wide: usize,
    pub wide_empty: usize,
    pub clean: f64,
    pub shown: bool,
    pub goal: f64,
}

pub(crate) fn clean_stats(j: &Judged) -> Clean {
    let mut units = [0u32; 7];
    for js in &j.judgments {
        for &u in js {
            units[u as usize] += 1;
        }
    }
    let base = judge::accuracy(&units, j.mode);
    let start = j.notes.first().map_or(0.0, |n| n.time);
    let end = j.notes.iter().map(|n| n.end_time).fold(start, f64::max);
    let (count, presses, wide, wide_empty) = empty_presses(&j.notes, &j.segments, j.windows.miss, j.rate, (start - 1000.0 * j.rate, end + 1000.0 * j.rate));
    units[6] += count as u32;
    Clean {
        count,
        presses,
        wide,
        wide_empty,
        clean: judge::accuracy(&units, j.mode),
        shown: count >= EMPTY_MIN && count as f64 >= j.notes.len() as f64 * EMPTY_SHARE,
        goal: (base - CLEAN_GOAL_GAP).floor().max(0.0),
    }
}

// a tap with no note near in any column is a ghost tap to the rhythm, which is fine
fn empty_presses(notes: &[Note], segments: &[Vec<judge::Segment>], miss: f64, rate: f64, span: (f64, f64)) -> (usize, usize, usize, usize) {
    let keys = segments.len();
    let mut edges: Vec<f64> = notes.iter().flat_map(|n| if n.is_hold { vec![n.time, n.end_time] } else { vec![n.time] }).collect();
    edges.sort_by(f64::total_cmp);
    let busy = |t: f64| {
        let i = edges.partition_point(|e| *e < t - miss);
        edges.get(i).is_some_and(|e| *e <= t + miss)
    };
    let mut count = 0usize;
    let mut presses = 0usize;
    let mut starts: Vec<(f64, bool)> = vec![];
    for (c, list) in segments.iter().enumerate() {
        let mine: Vec<&Note> = notes.iter().filter(|n| n.column == c).collect();
        for s in list.iter().filter(|s| s.start >= span.0 && s.start <= span.1) {
            presses += 1;
            // a hold counts until its tail, a broken one can be pressed again
            let near = mine.iter().any(|n| s.start >= n.time - miss && s.start <= n.end_time.max(n.time) + miss);
            let empty = !near && busy(s.start);
            count += empty as usize;
            starts.push((s.start, empty));
        }
    }
    starts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let together = WIDE_MS * rate;
    let (mut wide, mut wide_empty, mut i) = (0usize, 0usize, 0usize);
    while i < starts.len() {
        let mut k = i;
        while k + 1 < starts.len() && starts[k + 1].0 - starts[i].0 <= together {
            k += 1;
        }
        let group = &starts[i..=k];
        let hollow = group.iter().filter(|x| x.1).count();
        if group.len() >= WIDE_KEYS.min(keys) && hollow * 2 >= group.len() {
            wide += 1;
            wide_empty += hollow;
        }
        i = k + 1;
    }
    (count, presses, wide, wide_empty)
}

// m:ss of the song, at the speed it was played
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn signed(x: f64) -> String {
    let s = ms(x);
    if x >= 0.05 {
        format!("+{s}")
    } else if x <= -0.05 {
        format!("−{s}")
    } else {
        s
    }
}

pub(crate) async fn for_play(b: &std::sync::Arc<crate::Bridge>, id: &str) -> (u16, Value) {
    let item = b.st().queue.items.iter().find(|i| i.id == id).cloned();
    let Some(item) = item else { return (404, json!({ "error": "That play isn't here anymore." })) };
    let (Some(replay), Some(chart)) = (&item.replay, &item.chart) else {
        return (404, json!({ "error": "This play has no replay to read." }));
    };
    let (replay, chart) = match (crate::replays::read_blob(b, replay).await, crate::replays::read_blob(b, chart).await) {
        (Ok(r), Ok(c)) => (r, c),
        (Err(e), _) | (_, Err(e)) => {
            crate::log!("analysis: {e}");
            return (500, json!({ "error": "Couldn't read this play's replay." }));
        }
    };
    let lazer = item.play.is_lazer();
    let (thumb_left, own) = {
        let s = b.st();
        let settings = s.config.settings.as_ref();
        (!settings.is_some_and(|s| s.thumb_right), settings.map(|s| s.fingers.clone()).unwrap_or_default())
    };
    let practice_maps = crate::practice::list_sync(b);
    let practice = practice_maps.iter().find(|p| p.md5 == item.play.beatmap_md5).cloned();
    let made = practice_maps.iter().filter(|p| p.from_play == item.id).map(|p| json!({ "version": p.version, "from": p.from, "to": p.to, "createdAt": p.created_at })).collect::<Vec<_>>();
    let earlier = earlier_plays(b, &item, if practice.is_some() { 10 } else { 1 }).await;
    let p2 = practice.clone();
    let result = tokio::task::spawn_blocking(move || {
        let j = judge_play(&replay, &chart, lazer)?;
        let own_here = own.get(&j.chart.keys.to_string()).map(Vec::as_slice);
        let mut v = analyze(&j, thumb_left, own_here);
        v["practiceMaps"] = json!(made);
        // a play that can't be read just isn't compared
        let earlier: Vec<(String, Judged)> = earlier.into_iter().filter_map(|(at, replay, chart, lazer)| Some((at, judge_play(&replay, &chart, lazer).ok()?))).collect();
        if let Some(p) = &p2 {
            let mut b = crate::practice::breakdown(p, &j);
            b["attempts"] = earlier.iter().map(|(at, a)| json!({ "capturedAt": at, "repeats": crate::practice::breakdown(p, a)["repeats"] })).collect::<Vec<_>>().into();
            v["practiceRun"] = b;
        }
        if let Some((at, p)) = earlier.first().filter(|(_, p)| p.chart.keys == j.chart.keys) {
            v["previous"] = summary(p, thumb_left, own_here, at);
            real_acc_run(&mut v);
        }
        Ok::<_, String>(v)
    })
    .await;
    match result {
        Ok(Ok(mut v)) => {
            // plays from before goals were kept count when they're opened
            let (title, version) = (crate::types::text(&item.play.title), crate::types::text(&item.play.version));
            match practice {
                Some(p) => {
                    if let (Some(goal), Some(value)) = (p.goal, crate::goals::full_speed(&v["practiceRun"])) {
                        let seen = crate::goals::Seen { kind: "practice", md5: &p.md5, title: &p.title, version: &p.version, target: goal, play: Some((&item.id, &item.captured_at, value)), opens: true };
                        crate::goals::record(b, seen).await;
                    }
                }
                None => {
                    if let (Some(clean), Some(goal)) = (v["empty"]["clean"].as_f64(), v["empty"]["goal"].as_f64()) {
                        let opens = v["empty"]["shown"] == json!(true);
                        let seen = crate::goals::Seen { kind: "clean", md5: &item.play.beatmap_md5, title: &title, version: &version, target: goal, play: Some((&item.id, &item.captured_at, clean)), opens };
                        crate::goals::record(b, seen).await;
                    }
                }
            }
            v["goal"] = crate::goals::for_map(b, &item.play.beatmap_md5);
            (200, v)
        }
        Ok(Err(e)) => (422, json!({ "error": e })),
        Err(_) => (500, json!({ "error": "Couldn't analyze this play." })),
    }
}

fn real_acc_run(v: &mut Value) {
    let (prev, now) = (&v["previous"], &v["empty"]);
    let (Some(goal), Some(was), Some(clean)) = (prev["cleanGoal"].as_f64(), prev["clean"].as_f64(), now["clean"].as_f64()) else { return };
    if prev["emptyShown"] != json!(true) || now["shown"] == json!(true) || clean < goal {
        return;
    }
    let count = now["count"].as_u64().unwrap_or(0);
    let tip = json!({
        "tone": "good",
        "title": "Real acc run",
        "text": format!("Clean accuracy {clean:.2}%, up from {was:.2}% last time. {} hit nothing.", match count { 0 => "Not one press".to_string(), 1 => "Only 1 press".to_string(), n => format!("Only {n} presses") }),
        "gain": null,
        "marked": false,
        "drill": null,
    });
    if let Some(tips) = v["tips"].as_array_mut() {
        tips.insert(0, tip);
    }
}

async fn earlier_plays(b: &std::sync::Arc<crate::Bridge>, item: &crate::types::Item, n: usize) -> Vec<(String, Vec<u8>, Vec<u8>, bool)> {
    let mut before: Vec<crate::types::Item> = b
        .st()
        .queue
        .items
        .iter()
        .filter(|i| i.id != item.id && i.play.beatmap_md5 == item.play.beatmap_md5 && i.captured_at < item.captured_at && i.replay.is_some() && i.chart.is_some())
        .cloned()
        .collect();
    before.sort_by(|a, b| b.captured_at.cmp(&a.captured_at));
    let mut out = vec![];
    for i in before.into_iter().take(n) {
        if let (Some(r), Some(c)) = (i.replay.as_ref(), i.chart.as_ref()) {
            if let (Ok(r), Ok(c)) = (crate::replays::read_blob(b, r).await, crate::replays::read_blob(b, c).await) {
                out.push((i.captured_at.clone(), r, c, i.play.is_lazer()));
            }
        }
    }
    out
}

fn summary(j: &Judged, thumb_left: bool, own_fingers: Option<&[String]>, at: &str) -> Value {
    let mode = j.mode;
    let fingers = fingers(j.chart.keys, thumb_left, own_fingers);
    let seen = seen_notes(j);
    let mut columns = vec![Group::default(); j.chart.keys];
    let mut hands = [Group::default(), Group::default()];
    for (i, n) in j.notes.iter().enumerate() {
        add_note(j, &seen, &mut columns[n.column], i);
        add_note(j, &seen, &mut hands[(fingers[n.column].0 == Hand::Right) as usize], i);
    }
    let clean = clean_stats(j);
    let mut game = [0u32; 7];
    game[1..].copy_from_slice(&j.game);
    let part = |g: &Group| json!({ "acc": g.acc(mode).map(r2), "mean": r1(mean(&g.hits)), "sd": r1(sd(&g.hits)) });
    json!({
        "capturedAt": at,
        "rate": j.rate,
        "mods": j.mods,
        "accuracy": r2(judge::accuracy(&game, mode)),
        "clean": r2(clean.clean),
        "emptyShown": clean.shown,
        "cleanGoal": clean.goal,
        "columns": columns.iter().zip(&fingers).map(|(g, f)| json!({ "acc": g.acc(mode).map(r2), "finger": f.1 })).collect::<Vec<_>>(),
        "hands": { "left": part(&hands[0]), "right": part(&hands[1]) },
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_presses_catch_pressing_everything() {
        use crate::beatmap::Note;
        use crate::judge::Segment;
        let seg = |t: f64| Segment { start: t, end: t + 40.0, end_previous: None, samples: vec![t], start_previous: None };
        // 100 two-note chords on columns 2 and 4, 250 ms apart
        let notes: Vec<Note> = (0..100).flat_map(|i| [2, 4].map(|c| Note { column: c, time: 1000.0 + i as f64 * 250.0, end_time: 1000.0 + i as f64 * 250.0, is_hold: false })).collect();
        let span = (0.0, 30_000.0);
        let mut clean = vec![vec![]; 7];
        for n in &notes {
            clean[n.column].push(seg(n.time + 3.0));
        }
        let (empty, presses, wide, _) = super::empty_presses(&notes, &clean, 150.0, 1.0, span);
        assert_eq!((empty, presses, wide), (0, 200, 0));
        // every key on every beat, a couple of ms apart
        let mut mash = vec![vec![]; 7];
        for i in 0..100 {
            for (c, list) in mash.iter_mut().enumerate() {
                list.push(seg(1000.0 + i as f64 * 250.0 + c as f64));
            }
        }
        let (empty, presses, wide, wide_empty) = super::empty_presses(&notes, &mash, 150.0, 1.0, span);
        assert_eq!((empty, presses, wide, wide_empty), (500, 700, 100, 500));
        // played clean, plus ghost taps to the beat through a break after the last note
        let mut ghost = clean.clone();
        for i in 0..40 {
            ghost[i % 7].push(seg(27_000.0 + i as f64 * 125.0));
        }
        let (empty_ghost, _, wide_ghost, _) = super::empty_presses(&notes, &ghost, 150.0, 1.0, (0.0, 40_000.0));
        assert_eq!((empty_ghost, wide_ghost), (0, 0));
        assert!(empty >= super::EMPTY_MIN && empty as f64 >= notes.len() as f64 * super::EMPTY_SHARE);
    }

    // checks the port against mania tracker's own judge, over the replays on this computer. make the
    // reference first, from mania-hub/backend: node_modules/.bin/tsx /tmp/hashi-parity/dump.mts
    // then: cargo test -p mania-bridge-helper same_as_mania_tracker -- --ignored --nocapture
    #[test]
    #[ignore]
    fn same_as_mania_tracker() {
        let reference: serde_json::Value = serde_json::from_str(&std::fs::read_to_string("/tmp/hashi-parity/ts.json").unwrap()).unwrap();
        let (mut same, mut total, mut notes_off) = (0, 0, 0);
        for case in reference.as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let replay = std::fs::read(case["replay"].as_str().unwrap()).unwrap();
            let chart = std::fs::read(case["chart"].as_str().unwrap()).unwrap();
            let judged = match super::judge_play(&replay, &chart, case["lazer"].as_bool().unwrap()) {
                Ok(j) => j,
                Err(e) => {
                    println!("skip {name}: {e}");
                    continue;
                }
            };
            total += 1;
            let want: Vec<Vec<u8>> = serde_json::from_value(case["judgments"].clone()).unwrap();
            let heads = case["heads"].as_array().unwrap();
            let mut off = 0;
            for (i, (ours, theirs)) in judged.judgments.iter().zip(&want).enumerate() {
                let head = &heads[i];
                let head_same = head.is_null() || (head[0].as_u64() == Some(judged.states[i].head_judgment as u64) && (head[1].as_f64().unwrap() - judged.states[i].head_offset).abs() < 1e-6);
                if ours != theirs || !head_same {
                    if off < 3 {
                        let n = judged.notes[i];
                        println!("  {name} note {i} col {} t {} end {} ours {ours:?} {:?} theirs {theirs:?} {head}", n.column, n.time, n.end_time, judged.states[i]);
                    }
                    off += 1;
                }
            }
            if want.len() != judged.judgments.len() {
                println!("  {name}: {} notes here, {} there", judged.judgments.len(), want.len());
                off += 1;
            }
            notes_off += off;
            if off == 0 {
                same += 1;
            } else {
                println!("DIFF {name}: {off} notes");
            }
        }
        println!("{same}/{total} the same, {notes_off} notes differ");
        assert_eq!(notes_off, 0);
    }
}
