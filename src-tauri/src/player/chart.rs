use mania_bridge::player::ChartNote;

#[derive(Clone, Copy)]
pub struct Note {
    pub c: usize,
    pub t: f64,
    pub e: Option<f64>,
    // its index in the helper's order, which the judge goes by
    pub i: Option<usize>,
}

impl Note {
    pub fn end(&self) -> f64 {
        self.e.unwrap_or(self.t)
    }
}

pub struct MapNotes {
    notes: Vec<Note>,
    // the latest end of every note up to each one, to find the first still on screen
    reach: Vec<f64>,
    pub first: f64,
    pub end: f64,
}

impl MapNotes {
    pub fn new(chart: &[ChartNote]) -> MapNotes {
        let mut notes: Vec<Note> = chart.iter().enumerate().map(|(i, &(c, t, e))| Note { c, t, e, i: Some(i) }).collect();
        notes.sort_by(|a, b| a.t.total_cmp(&b.t));
        let mut reach = Vec::with_capacity(notes.len());
        let mut most = f64::NEG_INFINITY;
        for n in &notes {
            most = most.max(n.end());
            reach.push(most);
        }
        let first = notes.first().map_or(0.0, |n| n.t);
        MapNotes { notes, reach, first, end: most.max(first) }
    }

    // from `from` on, ending after `lo` and starting before `hi`
    pub fn visible(&self, from: f64, lo: f64, hi: f64, out: &mut Vec<Note>) {
        let start = self.reach.partition_point(|&r| r < lo);
        for n in &self.notes[start..] {
            if n.t > hi {
                break;
            }
            if n.t >= from && n.end() >= lo {
                out.push(*n);
            }
        }
    }
}

// skin.js's skinRandom
struct Random(u32);

impl Random {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4294967296.0
    }
}

// skin.js's skinChart, the same notes for a keymode every time
pub struct Made {
    keys: usize,
    holds_only: bool,
    random: Random,
    notes: Vec<Note>,
    next: f64,
    free: Vec<f64>,
    last: Vec<usize>,
    div: f64,
    section_end: f64,
}

const BEAT: f64 = 60000.0 / 170.0;

impl Made {
    pub fn new(keys: usize, holds_only: bool, start: f64) -> Made {
        Made {
            keys,
            holds_only,
            random: Random((keys as u32).wrapping_mul(7919).wrapping_add(7)),
            notes: vec![],
            next: start,
            free: vec![f64::NEG_INFINITY; keys],
            last: vec![],
            div: 2.0,
            section_end: 0.0,
        }
    }

    fn pick(&mut self, t: f64, n: usize, avoid: &[usize]) -> Vec<usize> {
        let mut open: Vec<usize> = (0..self.keys).filter(|&c| self.free[c] <= t && !avoid.contains(&c)).collect();
        let mut out = vec![];
        while out.len() < n && !open.is_empty() {
            let i = (self.random.next() * open.len() as f64).floor() as usize;
            out.push(open.remove(i.min(open.len() - 1)));
        }
        out
    }

    fn until(&mut self, to: f64) {
        while self.next < to {
            let t = self.next;
            if t >= self.section_end {
                self.div = if self.random.next() < 0.3 { 4.0 } else { 2.0 };
                self.section_end = t + BEAT * if self.div == 4.0 { 4.0 } else { 8.0 };
            }
            let step = BEAT / self.div;
            let r = self.random.next();
            let last = self.last.clone();
            let mut hold = 0.0;
            let columns = if self.holds_only {
                // long notes of every length, some together, with room between them to read the tails
                let columns = if r < 0.3 {
                    vec![]
                } else {
                    let n = if self.random.next() < 0.3 { 2 } else { 1 };
                    self.pick(t, n, &last)
                };
                hold = BEAT * (0.5 + (self.random.next() * 7.0).floor() / 2.0);
                columns
            } else if r < 0.08 {
                vec![]
            } else if r < 0.22 && self.div == 2.0 {
                let n = if self.random.next() < 0.25 { 2 } else { 1 };
                let columns = self.pick(t, n, &[]);
                hold = BEAT * (1.0 + (self.random.next() * 3.0).floor());
                columns
            } else if r < 0.42 {
                let n = if self.keys >= 6 && self.random.next() < 0.35 { 3 } else { 2 };
                self.pick(t, n, &last)
            } else if r < 0.55 && !last.is_empty() {
                last.iter().copied().filter(|&c| self.free[c] <= t).collect()
            } else {
                self.pick(t, 1, &last)
            };
            for &c in &columns {
                self.notes.push(Note { c, t, e: (hold > 0.0).then_some(t + hold), i: None });
                self.free[c] = t + if hold > 0.0 { hold + step } else { step * 0.9 };
            }
            self.last = columns;
            self.next = t + step;
        }
    }

    pub fn visible(&mut self, lo: f64, hi: f64, out: &mut Vec<Note>) {
        self.until(hi);
        self.notes.retain(|n| n.end() >= lo);
        out.extend(self.notes.iter().filter(|n| n.t <= hi));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_matches_the_page() {
        // skinRandom(4 * 7919 + 7) in the page, its first three draws
        let mut r = Random(4 * 7919 + 7);
        let got: Vec<f64> = (0..3).map(|_| r.next()).collect();
        assert_eq!(got, [0.4565261541865766, 0.8936199666932225, 0.6563205348793417]);
    }

    #[test]
    fn map_notes_still_on_screen() {
        let m = MapNotes::new(&[(0, 0.0, Some(5000.0)), (1, 1000.0, None), (2, 3000.0, None)]);
        let mut out = vec![];
        m.visible(0.0, 2000.0, 4000.0, &mut out);
        let at: Vec<f64> = out.iter().map(|n| n.t).collect();
        assert_eq!(at, [0.0, 3000.0]);
    }
}
