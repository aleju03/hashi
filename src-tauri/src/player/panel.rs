use super::gpu::{Frame, Gpu};
use super::settings::{key_name, Fps, Game, FPS, SPEED};
use super::text::Text;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    Gear,
    Auto(bool),
    Lazer(bool),
    Bind,
    DefaultKeys,
    Speed(f64),
    Volume,
    Offset(f64),
    // the hits' average lateness into the offset
    UseOffset,
    MuteOsu(bool),
    Fps(Fps),
    // inside the panel, on nothing
    Panel,
}

pub struct Hit {
    // x0, y0, x1, y1 in scene units
    pub r: [f64; 4],
    pub action: Action,
}

impl Hit {
    pub fn holds(&self, (x, y): (f64, f64)) -> bool {
        x >= self.r[0] && x <= self.r[2] && y >= self.r[1] && y <= self.r[3]
    }
}

pub struct Binding {
    pub keys: usize,
    pub codes: Vec<String>,
}

pub struct Look<'a> {
    pub game: &'a Game,
    pub keys: usize,
    pub lazer: bool,
    pub od: Option<String>,
    pub speed: f64,
    // how late the hits have been on average since this was last used
    pub late: Option<f64>,
    pub binding: Option<&'a Binding>,
    pub open: bool,
    pub alpha: f64,
    pub accent: [f64; 3],
}

const W: f64 = 430.0;
const ROW: f64 = 40.0;
const PAD: f64 = 14.0;
const GEAR: f64 = 36.0;
const TEXT: f64 = 13.0;
const VOLUME_W: f64 = 140.0;

pub struct Painter<'a> {
    pub frame: &'a mut Frame,
    pub gpu: &'a mut Gpu,
    pub text: &'a mut Text,
    pub k: f64,
    pub ox: f64,
    pub oy: f64,
    pub hits: Vec<Hit>,
}

impl Painter<'_> {
    fn px(&self, r: [f64; 4]) -> [f32; 4] {
        [(self.ox + r[0] * self.k) as f32, (self.oy + r[1] * self.k) as f32, (self.ox + r[2] * self.k) as f32, (self.oy + r[3] * self.k) as f32]
    }

    fn rrect(&mut self, r: [f64; 4], radius: f64, c: [f64; 4]) {
        let a = c[3];
        let rect = self.px(r);
        self.frame.round_rect(rect, (radius * self.k) as f32, [(c[0] * a) as f32, (c[1] * a) as f32, (c[2] * a) as f32, a as f32]);
    }

    fn width(&mut self, s: &str, size: f64) -> f64 {
        self.text.get(self.gpu, s, (size * self.k) as f32).map_or(0.0, |d| f64::from(d.width) / self.k)
    }

    // left-aligned at x, its middle at y
    fn label(&mut self, s: &str, size: f64, x: f64, y: f64, c: [f64; 4]) {
        let Some(d) = self.text.get(self.gpu, s, (size * self.k) as f32) else { return };
        let (x0, y0) = ((self.ox + x * self.k) as f32, (self.oy + y * self.k) as f32 - d.height / 2.0);
        let a = c[3] as f32;
        self.frame.quad(d.tex, false, [x0, y0, x0 + d.width, y0 + d.height], [0.0, 0.0, 1.0, 1.0], [c[0] as f32 * a, c[1] as f32 * a, c[2] as f32 * a, a]);
    }

    fn hit(&mut self, r: [f64; 4], action: Action) {
        self.hits.push(Hit { r, action });
    }

    // a row of choices ending at `right`, the picked one lit
    fn segs(&mut self, right: f64, y: f64, options: &[(&str, bool, Action)], alpha: f64) {
        let widths: Vec<f64> = options.iter().map(|(s, ..)| self.width(s, TEXT) + 20.0).collect();
        let total: f64 = widths.iter().sum::<f64>() + 4.0;
        let (h, mut x) = (28.0, right - total);
        self.rrect([x, y - h / 2.0, right, y + h / 2.0], 8.0, [1.0, 1.0, 1.0, 0.07 * alpha]);
        x += 2.0;
        for ((s, on, action), w) in options.iter().zip(&widths) {
            if *on {
                self.rrect([x, y - h / 2.0 + 2.0, x + w, y + h / 2.0 - 2.0], 6.0, [1.0, 1.0, 1.0, 0.16 * alpha]);
            }
            let tw = w - 20.0;
            self.label(s, TEXT, x + (w - tw) / 2.0, y, [1.0, 1.0, 1.0, if *on { alpha } else { 0.6 * alpha }]);
            self.hit([x, y - h / 2.0, x + w, y + h / 2.0], *action);
            x += w;
        }
    }

    fn gear(&mut self, cx: f64, cy: f64, r: f64, c: [f64; 4]) {
        let teeth = 8;
        let mut pts = vec![];
        for i in 0..teeth * 4 {
            let a = i as f64 / (teeth * 4) as f64 * std::f64::consts::TAU;
            let out = matches!(i % 4, 1 | 2);
            let rr = if out { r } else { r * 0.74 };
            pts.push([(self.ox + (cx + a.cos() * rr) * self.k) as f32, (self.oy + (cy + a.sin() * rr) * self.k) as f32]);
        }
        let a = c[3] as f32;
        let color = [c[0] as f32 * a, c[1] as f32 * a, c[2] as f32 * a, a];
        let centre = [(self.ox + cx * self.k) as f32, (self.oy + cy * self.k) as f32];
        for i in 0..pts.len() {
            self.frame.triangle([centre, pts[i], pts[(i + 1) % pts.len()]], color);
        }
        let hole = r * 0.34;
        self.rrect([cx - hole, cy - hole, cx + hole, cy + hole], hole, [0.08, 0.08, 0.1, 1.0]);
    }

    // `width` and `height` are the scene's
    pub fn draw(&mut self, look: &Look, width: f64, height: f64) {
        let a = if look.open { 1.0 } else { look.alpha };
        if a <= 0.0 {
            return;
        }
        let (gx1, gy1) = (width - 20.0, height - 24.0);
        let (gx0, gy0) = (gx1 - GEAR, gy1 - GEAR);
        self.rrect([gx0, gy0, gx1, gy1], 10.0, [0.0, 0.0, 0.0, 0.6 * a]);
        let lit = if look.open { look.accent } else { [1.0, 1.0, 1.0] };
        self.gear(gx0 + GEAR / 2.0, gy0 + GEAR / 2.0, 11.0, [lit[0], lit[1], lit[2], 0.9 * a]);
        self.hit([gx0, gy0, gx1, gy1], Action::Gear);
        if !look.open {
            return;
        }

        let rows = 8.0;
        let (x1, y1) = (gx1, gy0 - 10.0);
        let (x0, y0) = (x1 - W, y1 - PAD * 2.0 - ROW * rows);
        self.hit([x0, y0, x1, y1], Action::Panel);
        self.rrect([x0, y0, x1, y1], 12.0, [0.05, 0.04, 0.07, 0.92]);
        let (left, right) = (x0 + 18.0, x1 - 16.0);
        let row_y = |i: f64| y0 + PAD + ROW * i + ROW / 2.0;
        let soft = [1.0, 1.0, 1.0, 0.55];
        let white = [1.0, 1.0, 1.0, 0.9];
        let g = look.game;

        let y = row_y(0.0);
        self.label("Who plays", 14.0, left, y, white);
        self.segs(right, y, &[("Autoplay", g.auto, Action::Auto(true)), ("Play", !g.auto, Action::Auto(false))], 1.0);

        let y = row_y(1.0);
        self.label("Judged like", 14.0, left, y, white);
        if let Some(od) = &look.od {
            let w = self.width("Judged like", 14.0) + 10.0;
            self.label(od, 12.0, left + w, y, soft);
        }
        self.segs(right, y, &[("Stable", !look.lazer, Action::Lazer(false)), ("Lazer", look.lazer, Action::Lazer(true))], 1.0);

        let y = row_y(2.0);
        match look.binding {
            Some(b) => {
                self.label(&format!("Press column {} of {}", b.codes.len() + 1, b.keys), 14.0, left, y, [look.accent[0], look.accent[1], look.accent[2], 1.0]);
                let mut caps: Vec<String> = b.codes.iter().map(|c| key_name(c)).collect();
                caps.push("…".into());
                self.caps(right, y, &caps, Some(caps.len() - 1), look.accent);
            }
            None => {
                self.label("Keys", 14.0, left, y, white);
                if g.own_keys(look.keys) {
                    let w = self.width("Keys", 14.0) + 10.0;
                    self.label("Default", 12.0, left + w, y, [look.accent[0], look.accent[1], look.accent[2], 0.9]);
                    let dw = self.width("Default", 12.0);
                    self.hit([left + w, y - 10.0, left + w + dw, y + 10.0], Action::DefaultKeys);
                }
                let caps: Vec<String> = g.keys_for(look.keys).iter().map(|c| key_name(c)).collect();
                let r = self.caps(right, y, &caps, None, look.accent);
                self.hit(r, Action::Bind);
            }
        }

        let y = row_y(3.0);
        self.label("Scroll speed", 14.0, left, y, white);
        let value = format!("{:.0}", look.speed);
        let (slower, faster) = ((look.speed - 1.0).max(SPEED.0), (look.speed + 1.0).min(SPEED.1));
        self.segs(right, y, &[("−", false, Action::Speed(slower)), (&value, false, Action::Panel), ("+", false, Action::Speed(faster))], 1.0);

        let y = row_y(4.0);
        self.label("Song volume", 14.0, left, y, white);
        let shown = format!("{:.0}%", g.volume);
        let tx1 = right - 44.0;
        let tx0 = tx1 - VOLUME_W;
        self.label(&shown, TEXT, tx1 + 10.0, y, white);
        self.rrect([tx0, y - 2.0, tx1, y + 2.0], 2.0, [1.0, 1.0, 1.0, 0.18]);
        let at = tx0 + VOLUME_W * g.volume / 100.0;
        self.rrect([tx0, y - 2.0, at, y + 2.0], 2.0, [look.accent[0], look.accent[1], look.accent[2], 1.0]);
        self.rrect([at - 6.0, y - 6.0, at + 6.0, y + 6.0], 6.0, [1.0, 1.0, 1.0, 1.0]);
        self.hit([tx0 - 8.0, y - 12.0, tx1 + 8.0, y + 12.0], Action::Volume);

        let y = row_y(5.0);
        self.label("Offset", 14.0, left, y, white);
        let w = self.width("Offset", 14.0) + 10.0;
        match look.late {
            Some(late) if late.round() != 0.0 => {
                let said = format!("you hit {:.0} ms {}", late.abs(), if late > 0.0 { "late" } else { "early" });
                self.label(&said, 12.0, left + w, y, soft);
                let x = left + w + self.width(&said, 12.0) + 8.0;
                self.label("Use it", 12.0, x, y, [look.accent[0], look.accent[1], look.accent[2], 0.95]);
                let uw = self.width("Use it", 12.0);
                self.hit([x, y - 10.0, x + uw, y + 10.0], Action::UseOffset);
            }
            _ => self.label("higher plays the notes later", 12.0, left + w, y, soft),
        }
        let value = format!("{}{} ms", if g.offset > 0.0 { "+" } else { "" }, g.offset);
        self.segs(right, y, &[("−", false, Action::Offset(-5.0)), (&value, false, Action::Panel), ("+", false, Action::Offset(5.0))], 1.0);

        let y = row_y(6.0);
        self.label("Mute osu!", 14.0, left, y, white);
        let w = self.width("Mute osu!", 14.0) + 10.0;
        self.label("while the song plays here", 12.0, left + w, y, soft);
        self.segs(right, y, &[("On", g.mute_osu, Action::MuteOsu(true)), ("Off", !g.mute_osu, Action::MuteOsu(false))], 1.0);

        let y = row_y(7.0);
        self.label("Frame limiter", 14.0, left, y, white);
        let options: Vec<(&str, bool, Action)> = FPS.iter().map(|&(f, s)| (s, g.fps == f, Action::Fps(f))).collect();
        self.segs(right, y, &options, 1.0);
    }

    // ending at `right`, returning their whole span
    fn caps(&mut self, right: f64, y: f64, caps: &[String], lit: Option<usize>, accent: [f64; 3]) -> [f64; 4] {
        let widths: Vec<f64> = caps.iter().map(|c| self.width(c, 12.0).max(10.0) + 14.0).collect();
        let total = widths.iter().sum::<f64>() + 4.0 * (caps.len().saturating_sub(1)) as f64;
        let (h, mut x) = (24.0, right - total);
        let start = x;
        for (i, (c, w)) in caps.iter().zip(&widths).enumerate() {
            let bg = if lit == Some(i) { [accent[0], accent[1], accent[2], 0.35] } else { [1.0, 1.0, 1.0, 0.12] };
            self.rrect([x, y - h / 2.0, x + w, y + h / 2.0], 6.0, bg);
            let tw = w - 14.0;
            self.label(c, 12.0, x + (w - tw) / 2.0, y, [1.0, 1.0, 1.0, 0.95]);
            x += w + 4.0;
        }
        [start, y - h / 2.0, right, y + h / 2.0]
    }
}

pub fn volume_at(hits: &[Hit], x: f64) -> Option<f64> {
    let h = hits.iter().find(|h| h.action == Action::Volume)?;
    let (x0, x1) = (h.r[0] + 8.0, h.r[2] - 8.0);
    Some((((x - x0) / (x1 - x0)).clamp(0.0, 1.0) * 100.0).round())
}
