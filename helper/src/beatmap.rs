// read the way mania tracker's beatmap-parser.ts reads charts, so the judge sees the same notes

#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub column: usize,
    pub time: f64,
    // the same as time for a tap
    pub end_time: f64,
    pub is_hold: bool,
}

#[derive(Clone, Copy)]
pub struct TimingPoint {
    pub time: f64,
    pub beat_length: f64,
}

pub struct Chart {
    pub keys: usize,
    pub od: f64,
    pub notes: Vec<Note>,
    pub timing_points: Vec<TimingPoint>,
}

// javascript's parseInt: the leading integer, NaN without one
fn parse_int(s: &str) -> f64 {
    let s = s.trim_start();
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    let len = digits.bytes().take_while(u8::is_ascii_digit).count();
    if len == 0 {
        return f64::NAN;
    }
    let n: f64 = digits[..len].parse().unwrap_or(f64::NAN);
    if s.starts_with('-') {
        -n
    } else {
        n
    }
}

// parseFloat's leading number
fn parse_float(s: &str) -> f64 {
    let s = s.trim();
    (1..=s.len()).rev().filter(|&i| s.is_char_boundary(i)).find_map(|i| s[..i].parse::<f64>().ok()).unwrap_or(f64::NAN)
}

// osu! reads the key count as C#'s Math.Round, halves to even
fn key_count(cs: f64) -> usize {
    let floor = cs.floor();
    let diff = cs - floor;
    let rounded = if diff > 0.5 {
        floor + 1.0
    } else if diff < 0.5 || floor % 2.0 == 0.0 {
        floor
    } else {
        floor + 1.0
    };
    rounded.clamp(1.0, 18.0) as usize
}

pub fn parse_chart(text: &str) -> Result<Chart, String> {
    let mut section = "";
    let mut mode = 0;
    let mut cs = 4.0;
    let mut od = 8.0;
    let mut notes = vec![];
    let mut timing_points = vec![];
    for line in text.split('\n').map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
            section = &line[1..line.len() - 1];
            continue;
        }
        let value = || line.split(':').nth(1).unwrap_or("").trim();
        match section {
            "General" if line.starts_with("Mode:") => mode = parse_int(value()) as i64,
            "Difficulty" if line.starts_with("CircleSize:") => cs = parse_float(value()),
            "Difficulty" if line.starts_with("OverallDifficulty:") => od = parse_float(value()),
            "TimingPoints" if line.contains(',') => {
                let parts: Vec<&str> = line.split(',').collect();
                let (time, beat_length) = (parse_float(parts[0]), parse_float(parts[1]));
                let uninherited = parts.len() < 7 || parts[6].trim() != "0";
                if beat_length > 0.0 && uninherited {
                    timing_points.push(TimingPoint { time, beat_length });
                }
            }
            "HitObjects" if line.contains(',') => {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() < 5 {
                    continue;
                }
                let time = parse_int(parts[2]);
                // a gimmick's fake notes have no real time and are never played
                if !time.is_finite() {
                    continue;
                }
                let keys = key_count(cs);
                let x = parse_int(parts[0]);
                let column = ((x * keys as f64) / 512.0).floor();
                let is_hold = parse_int(parts[3]) as i64 & 128 != 0;
                let mut end_time = time;
                if is_hold && parts.len() >= 6 {
                    let end = parse_int(parts[5].split(':').next().unwrap_or(""));
                    end_time = if end.is_finite() && end != 0.0 { end } else { time };
                }
                let column = if column.is_finite() { column.max(0.0).min(keys as f64 - 1.0) as usize } else { 0 };
                notes.push(Note { column, time, end_time, is_hold });
            }
            _ => {}
        }
    }
    if mode != 3 {
        return Err("Only maps made for mania can be analyzed, not converts.".into());
    }
    if notes.is_empty() {
        return Err("The map has no notes.".into());
    }
    notes.sort_by(|a, b| a.time.total_cmp(&b.time));
    Ok(Chart { keys: key_count(cs), od, notes, timing_points })
}

#[cfg(test)]
mod tests {
    #[test]
    fn columns_and_holds() {
        let chart = super::parse_chart(
            "osu file format v14\r\n[General]\r\nMode: 3\r\n[Difficulty]\r\nCircleSize:4\r\nOverallDifficulty:8.5\r\n[TimingPoints]\r\n0,500,4,2,0,50,1,0\r\n[HitObjects]\r\n64,192,1000,1,0,0:0:0:0:\r\n448,192,1200,128,0,1500:0:0:0:0:\r\n192,192,900,1,0,0:0:0:0:\r\n",
        )
        .unwrap();
        assert_eq!(chart.keys, 4);
        assert_eq!(chart.od, 8.5);
        let cols: Vec<usize> = chart.notes.iter().map(|o| o.column).collect();
        assert_eq!(cols, [1, 0, 3]);
        assert_eq!(chart.notes[2].end_time, 1500.0);
        assert!(chart.notes[2].is_hold);
        assert_eq!(chart.timing_points.len(), 1);
        assert!(super::parse_chart("[General]\nMode: 0\n[HitObjects]\n64,192,1000,1,0\n").is_err());
    }

    #[test]
    fn key_counts_round_half_to_even() {
        assert_eq!(super::key_count(4.5), 4);
        assert_eq!(super::key_count(7.5), 8);
        assert_eq!(super::key_count(6.6), 7);
    }
}
