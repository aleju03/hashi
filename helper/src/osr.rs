pub struct OsrHeader {
    pub mode: u8,
    pub beatmap_md5: Option<String>,
    pub player_name: Option<String>,
    pub mods: i64,
    pub played_at: i64,
    pub complete: bool,
}

struct Reader<'a> {
    buf: &'a [u8],
    o: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.o.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or("short file")?;
        let s = &self.buf[self.o..end];
        self.o = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn uleb(&mut self) -> Result<usize, String> {
        let (mut result, mut shift) = (0usize, 0u32);
        loop {
            let byte = self.u8()?;
            if shift < 63 {
                result |= ((byte & 0x7f) as usize) << shift;
            }
            shift += 7;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
    }

    fn string(&mut self) -> Result<Option<String>, String> {
        match self.u8()? {
            0x00 => Ok(None),
            0x0b => {
                let len = self.uleb()?;
                Ok(Some(String::from_utf8_lossy(self.take(len)?).into_owned()))
            }
            _ => Err("bad string marker".into()),
        }
    }
}

pub fn parse_osr(buf: &[u8]) -> Result<OsrHeader, String> {
    let mut r = Reader { buf, o: 0 };
    let mode = r.u8()?;
    let _version = r.i32()?;
    let beatmap_md5 = r.string()?;
    let player_name = r.string()?;
    let _replay_md5 = r.string()?;
    r.take(6 * 2)?; // judgement counts
    let _score = r.i32()?;
    r.take(2 + 1)?; // max combo, perfect
    let mods = r.i32()? as i64;
    r.string()?; // life bar
    let ticks = r.i64()?;
    let frame_bytes = r.i32()?;
    let complete = frame_bytes > 0 && buf.len() >= r.o + frame_bytes as usize + 8;
    Ok(OsrHeader { mode, beatmap_md5, player_name, mods, played_at: ticks / 10000 - 62135596800000, complete })
}

pub struct Replay {
    pub mode: u8,
    pub mods: i64,
    // 300, 100, 50, geki (PERFECT), katu (GOOD), miss
    pub counts: [u32; 6],
    // (ms since the last row, x, y) as written, x holds the pressed columns as bits
    pub rows: Vec<(f64, f64, f64)>,
    // what lazer adds after the frames: mods with their settings, the exact judgement counts
    pub lazer: Option<serde_json::Value>,
}

pub fn parse_replay(buf: &[u8]) -> Result<Replay, String> {
    let mut r = Reader { buf, o: 0 };
    let mode = r.u8()?;
    let version = r.i32()?;
    r.string()?;
    r.string()?;
    r.string()?;
    let mut counts = [0u32; 6];
    for c in &mut counts {
        *c = r.u16()? as u32;
    }
    let _score = r.i32()?;
    r.take(2 + 1)?;
    let mods = r.i32()? as u32 as i64;
    r.string()?;
    r.i64()?;
    let len = r.i32()?;
    let data = r.take(usize::try_from(len).map_err(|_| "bad frame length")?)?;
    let rows = parse_rows(&decompress(data)?)?;
    let mut lazer = None;
    if version >= 30000001 && r.i64().is_ok() {
        if let Ok(len) = r.i32() {
            if let Some(data) = usize::try_from(len).ok().and_then(|n| r.take(n).ok()) {
                lazer = decompress(data).ok().and_then(|text| serde_json::from_slice(&text).ok());
            }
        }
    }
    Ok(Replay { mode, mods, counts, rows, lazer })
}

fn decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = vec![];
    // a replay's frames come to a few MB at most, this stops a crafted one from asking for gigabytes
    let opts = lzma_rs::decompress::Options { memlimit: Some(64 << 20), ..Default::default() };
    lzma_rs::lzma_decompress_with_options(&mut std::io::Cursor::new(data), &mut out, &opts).map_err(|e| format!("frames: {e}"))?;
    Ok(out)
}

// "w|x|y|z," rows, the "-12345" one at the end carries the RNG seed
fn parse_rows(text: &[u8]) -> Result<Vec<(f64, f64, f64)>, String> {
    let text = std::str::from_utf8(text).map_err(|_| "frames aren't text")?;
    let number = |s: &str| {
        let s = s.trim();
        if s.is_empty() {
            Some(0.0)
        } else {
            s.parse::<f64>().ok().filter(|n| n.is_finite())
        }
    };
    let mut rows = vec![];
    for entry in text.split(',').filter(|e| !e.is_empty()) {
        let parts: Vec<&str> = entry.split('|').collect();
        if parts.len() < 4 || parts[0] == "-12345" {
            continue;
        }
        let (Some(w), Some(x), Some(y)) = (number(parts[0]), number(parts[1]), number(parts[2])) else {
            return Err("The replay has a corrupt input frame.".into());
        };
        rows.push((w, x, y));
    }
    Ok(rows)
}

// Data/r/<md5>-<ticks>.osr, ticks are a windows FILETIME (1601), not .NET ticks
pub fn parse_replay_name(name: &str) -> Option<(String, i64)> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".osr")?;
    let (md5, ticks) = stem.split_once('-')?;
    if md5.len() != 32 || !md5.chars().all(|c| c.is_ascii_hexdigit()) || ticks.is_empty() || !ticks.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let ticks: u128 = ticks.parse().ok()?;
    Some((md5.to_string(), (ticks / 10000) as i64 - 11644473600000))
}

// lazer: mode, a lazer version (30000000+), then the chart md5 as an osu! string
pub fn lazer_replay_md5(head: &[u8]) -> Option<String> {
    if head.len() < 39 || head[0] > 3 {
        return None;
    }
    let version = i32::from_le_bytes(head[1..5].try_into().ok()?);
    if !(30000000..40000000).contains(&version) || head[5] != 0x0b || head[6] != 32 {
        return None;
    }
    let md5 = std::str::from_utf8(&head[7..39]).ok()?;
    md5.chars().all(|c| c.is_ascii_hexdigit()).then(|| md5.to_ascii_lowercase())
}
