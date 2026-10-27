use super::{HEIGHT, WIDTH};

const STAGE_MAX_FRACTION: f64 = 0.94;
pub(super) const HIT_LINE_MIN_FRACTION: f64 = 0.62;
const SCROLL_TOP_FRACTION: f64 = 0.04;
// the synthetic notes have no clock, so their animation frame comes from height at about the
// scroll speed real charts land on
const PX_PER_MS: f64 = 0.55;

pub(super) struct Layout {
    pub stage_x: f64,
    pub stage_width: f64,
    pub lane_xs: Vec<f64>,
    pub lane_widths: Vec<f64>,
    pub scale: f64,
}

pub(super) fn layout(widths: &[f64], column_width: f64, spacing: f64, column_start: Option<f64>, keys: usize) -> Layout {
    let raw: Vec<f64> = (0..keys).map(|c| widths.get(c).copied().unwrap_or(column_width).max(8.0)).collect();
    let spacing = spacing.max(0.0);
    let raw_stage = raw.iter().sum::<f64>() + spacing * (keys - 1) as f64;
    // skin units are 480ths of the screen height, so on a 16:9 card ColumnStart lands where it
    // does in game
    let scale = (HEIGHT / 480.0).min(WIDTH * STAGE_MAX_FRACTION / raw_stage);
    let lane_widths: Vec<f64> = raw.iter().map(|w| w * scale).collect();
    let gap = spacing * scale;
    let stage_width = lane_widths.iter().sum::<f64>() + gap * (keys - 1) as f64;
    let stage_x = (column_start.unwrap_or(136.0) * scale).min(WIDTH - stage_width).max(0.0);
    let mut lane_xs = Vec::with_capacity(keys);
    let mut x = stage_x;
    for w in &lane_widths {
        lane_xs.push(x);
        x += w + gap;
    }
    Layout { stage_x, stage_width, lane_xs, lane_widths, scale }
}

pub(super) fn mulberry32(seed: u32) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state.wrapping_add(0x6d2b_79f5);
        let mut t = state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }
}

pub(super) struct Tap {
    pub column: usize,
    pub y: f64,
    pub time_ms: f64,
}

pub(super) struct Hold {
    pub column: usize,
    pub head_y: f64,
    pub tail_y: f64,
    pub time_ms: f64,
}

pub(super) struct Pattern {
    pub taps: Vec<Tap>,
    pub holds: Vec<Hold>,
    pub pressed: usize,
}

// seeded by the key count alone so every skin shows the same chart
pub(super) fn pattern(keys: usize, hit_line_y: f64, note_height: f64) -> Pattern {
    let note_height = note_height.max(12.0);
    let mut random = mulberry32((keys as u32).wrapping_mul(7919).wrapping_add(42));
    let k = keys as f64;
    let scroll_top = HEIGHT * SCROLL_TOP_FRACTION;
    let usable = hit_line_y - scroll_top;
    let min_y = scroll_top + note_height;
    let min_gap = note_height * 1.2;

    let columns = if keys >= 2 {
        let a = (random() * k).floor() as usize;
        let b = ((random() * (k - 1.0)).floor() as usize + 1 + keys / 2) % keys;
        vec![a, b]
    } else {
        vec![0]
    };
    let holds: Vec<Hold> = columns
        .into_iter()
        .enumerate()
        .map(|(i, column)| {
            let head_y = hit_line_y - usable * (0.08 + 0.4 * i as f64 + random() * 0.06);
            let length = (note_height * 2.6).max(usable * (0.26 + random() * 0.12));
            Hold { column, head_y, tail_y: min_y.max(head_y - length), time_ms: (hit_line_y - head_y) / PX_PER_MS }
        })
        .collect();

    let count = (keys * 2).clamp(6, 14);
    let mut taps: Vec<Tap> = vec![];
    let mut attempts = 0;
    while taps.len() < count && attempts < count * 24 {
        attempts += 1;
        let column = (random() * k).floor() as usize;
        let y = min_y + (hit_line_y - min_y) * random();
        if taps.iter().any(|t| t.column == column && (t.y - y).abs() < min_gap) {
            continue;
        }
        if holds.iter().any(|h| {
            h.column == column && y >= h.tail_y - min_gap * 0.6 && y - note_height <= h.head_y + min_gap * 0.6
        }) {
            continue;
        }
        taps.push(Tap { column, y, time_ms: (hit_line_y - y) / PX_PER_MS });
    }
    taps.sort_by(|a, b| a.y.total_cmp(&b.y));
    Pattern { taps, holds, pressed: 1.min(keys - 1) }
}

pub(super) struct Triangle {
    pub x: f64,
    pub y: f64,
    pub side: f64,
    pub tone: f64,
}

// one equilateral triangle per cell of a jittered grid with a ring past every edge, largest
// first, the way lazer layers its menu triangles
pub(super) fn backdrop() -> Vec<Triangle> {
    const SIDE: f64 = 215.0;
    const COLUMNS: i32 = 12;
    const ROWS: i32 = 7;
    let mut random = mulberry32(0x5eed);
    let cell_w = WIDTH / COLUMNS as f64;
    let cell_h = HEIGHT / ROWS as f64;
    let mut field = vec![];
    for row in -1..=ROWS {
        for col in -1..=COLUMNS {
            let stagger = if row.abs() % 2 == 1 { cell_w / 2.0 } else { 0.0 };
            let normal = (-2.0 * (1.0 - random()).ln()).sqrt() * (2.0 * std::f64::consts::PI * random()).cos();
            let scale = (1.0 + 0.42 * normal).clamp(0.26, 2.6);
            let x = (col as f64 + 0.5) * cell_w + stagger + (random() - 0.5) * cell_w * 0.8;
            let y = (row as f64 + 0.5) * cell_h + (random() - 0.5) * cell_h * 0.8;
            field.push(Triangle { x, y, side: SIDE * scale, tone: random() });
        }
    }
    field.sort_by(|a, b| b.side.total_cmp(&a.side));
    field
}

pub(super) struct Tile {
    pub top: f64,
    pub height: f64,
    pub rows: f64,
}

// Tiles snapped to whole pixels: two half-covered rows composited over each other come to 75%
// and show as a seam at every join
pub(super) fn body_tiles(top: f64, bottom: f64, source_height: f64, scale: f64) -> Vec<Tile> {
    let mut tiles = vec![];
    if !(scale > 0.0 && source_height > 0.0 && bottom > top) {
        return tiles;
    }
    let mut y = top;
    while y < bottom - 0.01 && tiles.len() < 4096 {
        let rows = source_height.min((bottom - y) / scale);
        let t = super::ini::round(y);
        let b = super::ini::round(y + rows * scale);
        tiles.push(Tile { top: t, height: (b - t).max(1.0), rows });
        y += rows * scale;
    }
    tiles
}

pub(super) struct HoldBoxes {
    pub body_top: f64,
    pub body_bottom: f64,
    pub head_top: f64,
    pub tail_top: f64,
}

// Both caps grow away from the receptor. The body stops at the head cap's centre but runs the full
// depth of the tail box, the edge a Percy body's transparent lead-in is measured from. Directional
// on purpose: a hold whose tail is within half a cap of the head comes out empty, not flipped.
pub(super) fn hold_boxes(upscroll: bool, head_end: f64, tail_end: f64, head_h: f64, tail_h: f64) -> HoldBoxes {
    let head_top = if upscroll { head_end } else { head_end - head_h };
    let tail_top = if upscroll { tail_end } else { tail_end - tail_h };
    let head_side = if upscroll { head_end + head_h / 2.0 } else { head_end - head_h / 2.0 };
    let tail_side = if upscroll { tail_top + tail_h } else { tail_top };
    HoldBoxes {
        body_top: if upscroll { head_side } else { tail_side },
        body_bottom: if upscroll { tail_side } else { head_side },
        head_top,
        tail_top,
    }
}
