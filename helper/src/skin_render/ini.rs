use std::collections::HashMap;

pub(super) type Block = HashMap<String, String>;

pub(super) struct Ini {
    pub fonts: HashMap<String, String>,
    pub mania: Vec<Block>,
}

// osu! fills these per-column lists slot by slot, so a key declared twice keeps whatever the
// shorter list never reaches ("moj skin zielony" sets ColumnLineWidth to 0,0,0,0,0 and then 0,0)
const SLOT_LISTS: [&str; 4] = ["ColumnWidth", "ColumnSpacing", "ColumnLineWidth", "LightingNWidth"];

pub(super) fn parse(text: &str) -> Ini {
    let mut ini = Ini { fonts: HashMap::new(), mania: vec![] };
    let mut section = String::new();
    for raw in text.strip_prefix('\u{feff}').unwrap_or(text).split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let line = raw.find("//").map_or(raw, |i| &raw[..i]).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if !name.is_empty() && !name.contains(']') {
                section = name.trim().to_string();
                if section == "Mania" {
                    ini.mania.push(Block::new());
                }
                continue;
            }
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() {
            continue;
        }
        match section.as_str() {
            "Fonts" => {
                ini.fonts.insert(key.into(), value.into());
            }
            "Mania" => {
                let block = ini.mania.last_mut().expect("a [Mania] header pushed a block");
                match block.get_mut(key) {
                    Some(prev) if SLOT_LISTS.contains(&key) => {
                        let mut slots: Vec<&str> = prev.split(',').collect();
                        for (i, slot) in value.split(',').enumerate() {
                            if i < slots.len() {
                                slots[i] = slot;
                            } else {
                                slots.push(slot);
                            }
                        }
                        *prev = slots.join(",");
                    }
                    _ => {
                        block.insert(key.into(), value.into());
                    }
                }
            }
            _ => {}
        }
    }
    ini
}

// JavaScript's Number(): the TS renderer this mirrors reads "" as 0 and takes hex
pub(super) fn number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    let radix = |digits: &str, r| u64::from_str_radix(digits, r).ok().map(|v| v as f64);
    let v = match t.get(..2) {
        Some("0x" | "0X") => radix(&t[2..], 16),
        Some("0o" | "0O") => radix(&t[2..], 8),
        Some("0b" | "0B") => radix(&t[2..], 2),
        _ if t.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-')) => t.parse().ok(),
        _ => None,
    };
    v.filter(|v: &f64| v.is_finite())
}

pub(super) fn opt_number(s: Option<&String>) -> Option<f64> {
    s.and_then(|s| number(s))
}

// Math.round, which rounds halves up rather than away from zero
pub(super) fn round(x: f64) -> f64 {
    (x + 0.5).floor()
}

pub(super) fn clamp_int(x: f64, min: f64, max: f64) -> f64 {
    round(x).clamp(min, max)
}

pub(super) fn number_list(s: Option<&String>) -> Vec<f64> {
    match s {
        Some(s) if !s.is_empty() => s.split(',').filter_map(number).map(round).collect(),
        _ => vec![],
    }
}

pub(super) type Rgba = [u8; 4];

pub(super) fn color(s: Option<&String>) -> Option<Rgba> {
    let parts: Vec<Option<f64>> = s.filter(|s| !s.is_empty())?.split(',').map(number).collect();
    if parts.len() < 3 {
        return None;
    }
    let c = |i: usize| parts[i].map(|v| clamp_int(v, 0.0, 255.0) as u8);
    Some([c(0)?, c(1)?, c(2)?, 255])
}

// like color, but keeps a skin.ini alpha below 255
pub(super) fn color_alpha(s: Option<&String>) -> Option<Rgba> {
    let [r, g, b, _] = color(s)?;
    let a = s?.split(',').nth(3).and_then(number).map_or(255, |a| clamp_int(a, 0.0, 255.0) as u8);
    Some([r, g, b, a])
}

pub(super) fn hex(s: &str) -> Rgba {
    let v = u32::from_str_radix(&s[1..], 16).unwrap_or(0);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
}
