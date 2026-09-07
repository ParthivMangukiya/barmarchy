//! Touch-bar screensaver — mirrors the desktop TTE screensaver, but
//! designed for a 2008x60 strip. Multiple effects (assemble, decrypt,
//! rain, beams, blackhole) cycle at random, like `ttfx --random-effect`.
//! Each cycle alternates the word between OMARCHY and the clock.
//!
//! Detection matches what omarchy-launch-screensaver itself greps for:
//! a process running as `org.omarchy.screensaver`.

use cairo::{Context, FontSlant, FontWeight, Format, ImageSurface, Operator};

use crate::theme::{self, Theme};

pub const LW: f64 = 2008.0;
pub const LH: f64 = 60.0;
const N_PARTICLES: usize = 650;
const NEEDLE: &[u8] = b"org.omarchy.screensaver";

/// True while the Omarchy desktop screensaver is running.
/// Native /proc scan (no fork) so the live loop can call it every second.
/// Matches only whole argv entries, so transient processes that merely
/// mention the class (shell one-liners, jq filters, pgrep/grep wrappers)
/// can't trip the detector: the real terminal carries it as its own
/// --class= / --app-id= argument.
pub fn saver_active() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.is_empty() || !name.bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(cmd) = std::fs::read(format!("/proc/{name}/cmdline")) else {
            continue;
        };
        for arg in cmd.split(|b| *b == 0) {
            if arg == b"--class=org.omarchy.screensaver"
                || arg == b"--app-id=org.omarchy.screensaver"
                || arg == NEEDLE
            {
                return true;
            }
        }
    }
    false
}

fn clock_hhmm() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

// Tiny xorshift RNG — no new deps for decorative randomness.
struct Rng(u64);
impl Rng {
    fn seed() -> Self {
        let mut t: libc::timespec = unsafe { std::mem::zeroed() };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
        let s = (t.tv_sec as u64).wrapping_mul(0x9e3779b97f4a7c15)
            ^ (t.tv_nsec as u64)
            ^ (unsafe { libc::getpid() } as u64).wrapping_mul(0xbf58476d1ce4e5b9);
        Self(s | 1)
    }
    fn fixed(s: u64) -> Self {
        Self(s | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f()
    }
    fn pick(&mut self, n: usize) -> usize {
        (self.f() * n as f64) as usize % n
    }
}

/// Rasterize `word` centered on a 2008x60 mask and sample N target points.
fn sample_word(word: &str, n: usize, rng: &mut Rng) -> Vec<(f64, f64)> {
    let mut surf = ImageSurface::create(Format::A8, LW as i32, LH as i32).expect("mask surface");
    {
        let ctx = Context::new(&surf).expect("mask ctx");
        // A8 stores alpha only: "black paint" would be alpha=1 everywhere.
        // Clear to transparent, then draw the word opaque.
        ctx.set_operator(Operator::Clear);
        ctx.paint().ok();
        ctx.set_operator(Operator::Over);
        ctx.set_source_rgb(1.0, 1.0, 1.0);
        let mut size = 44.0;
        let tracking = 14.0;
        let measure = |size: f64| -> f64 {
            ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
            ctx.set_font_size(size);
            let mut w = 0.0;
            for ch in word.chars() {
                let s = ch.to_string();
                w += ctx.text_extents(&s).map(|e| e.x_advance()).unwrap_or(0.0) + tracking;
            }
            w - tracking
        };
        let mut w = measure(size);
        if w > 1400.0 {
            size *= 1400.0 / w;
            w = measure(size);
        }
        ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
        ctx.set_font_size(size);
        let mut x = (LW - w) / 2.0;
        let cy = LH / 2.0;
        for ch in word.chars() {
            let s = ch.to_string();
            let ext = ctx.text_extents(&s).expect("extents");
            ctx.move_to(
                x - ext.x_bearing(),
                cy - (ext.height() / 2.0 + ext.y_bearing()),
            );
            ctx.show_text(&s).ok();
            x += ext.x_advance() + tracking;
        }
    }
    surf.flush();
    let stride = surf.stride() as usize;
    let lit: Vec<(f64, f64)> = surf
        .data()
        .map(|d| {
            let mut v = vec![];
            for y in 0..LH as usize {
                for x in 0..LW as usize {
                    if d[y * stride + x] > 110 {
                        v.push((x as f64 + 0.5, y as f64 + 0.5));
                    }
                }
            }
            v
        })
        .unwrap_or_default();
    if lit.is_empty() {
        return vec![(LW / 2.0, LH / 2.0); n];
    }
    if std::env::var("SAVER_DEBUG").is_ok() {
        let (mut x0, mut x1) = (LW, 0.0f64);
        for &(x, _) in &lit {
            x0 = x0.min(x);
            x1 = x1.max(x);
        }
        eprintln!("saver-debug: word={word:?} lit={} x=[{x0:.0},{x1:.0}]", lit.len());
    }
    // Evenly spaced picks across the glyph pixels (+ jitter) so strokes
    // fill uniformly instead of clumping at the first strokes.
    (0..n)
        .map(|i| {
            let p = lit[(i * lit.len() / n) % lit.len()];
            (
                (p.0 + rng.range(-1.5, 1.5)).clamp(0.0, LW - 1.0),
                (p.1 + rng.range(-1.5, 1.5)).clamp(0.0, LH - 1.0),
            )
        })
        .collect()
}

/// One screensaver effect. `update` returns true when the effect is done
/// and the saver should move to the next random one.
trait Effect {
    fn update(&mut self, dt: f64) -> bool;
    fn render(&self, ctx: &Context, theme: &Theme, time: f64);
}

fn dot(ctx: &Context, x: f64, y: f64, s: f64, c: (f64, f64, f64), a: f64) {
    ctx.set_source_rgba(c.0, c.1, c.2, a.clamp(0.0, 1.0));
    ctx.rectangle(x - s / 2.0, y - s / 2.0, s, s);
    ctx.fill().ok();
}

// ---------------------------------------------------------------------------
// assemble: particles stream in from both edges, form the word, hold, disperse
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Assemble,
    Hold,
    Disperse,
    Gap,
}

struct AssemblePt {
    x: f64,
    y: f64,
    tx: f64,
    ty: f64,
    dx: f64,
    dy: f64,
    speed: f64,
    delay: f64,
    size: f64,
    white: bool,
    tw: f64,
    vx: f64,
    vy: f64,
    side: f64,
}

struct Assemble {
    pts: Vec<AssemblePt>,
    stage: Stage,
    t: f64,
    rng: Rng,
}

impl Assemble {
    fn new(word: &str, rng_seed: Option<u64>) -> Self {
        let mut rng = match rng_seed {
            Some(s) => Rng::fixed(s),
            None => Rng::seed(),
        };
        let pts = sample_word(word, N_PARTICLES, &mut rng)
            .into_iter()
            .map(|(tx, ty)| {
                let left = rng.f() < 0.5;
                let (sx, side) = if left { (-12.0, -1.0) } else { (LW + 12.0, 1.0) };
                let sy = rng.range(0.0, LH);
                let mut dx = tx - sx;
                let mut dy = ty - sy;
                let len = (dx * dx + dy * dy).sqrt().max(1.0);
                dx /= len;
                dy /= len;
                AssemblePt {
                    x: sx,
                    y: sy,
                    tx,
                    ty,
                    dx,
                    dy,
                    speed: rng.range(1.6, 3.4),
                    delay: rng.range(0.0, 0.9),
                    size: rng.range(1.6, 3.0),
                    white: rng.f() < 0.30,
                    tw: rng.range(0.0, 6.28),
                    vx: 0.0,
                    vy: 0.0,
                    side,
                }
            })
            .collect();
        Self {
            pts,
            stage: Stage::Assemble,
            t: 0.0,
            rng,
        }
    }
}

impl Effect for Assemble {
    fn update(&mut self, dt: f64) -> bool {
        self.t += dt;
        match self.stage {
            Stage::Assemble => {
                let ks: Vec<f64> =
                    self.pts.iter().map(|p| 1.0 - (-p.speed * dt).exp()).collect();
                let mut settled = true;
                for (p, k) in self.pts.iter_mut().zip(ks) {
                    if self.t < p.delay {
                        settled = false;
                        continue;
                    }
                    p.x += (p.tx - p.x) * k;
                    p.y += (p.ty - p.y) * k;
                    if (p.tx - p.x).abs() > 2.5 || (p.ty - p.y).abs() > 2.5 {
                        settled = false;
                    }
                }
                if settled || self.t > 4.5 {
                    for p in self.pts.iter_mut() {
                        p.x = p.tx;
                        p.y = p.ty;
                    }
                    self.stage = Stage::Hold;
                    self.t = 0.0;
                }
                false
            }
            Stage::Hold => {
                if self.t > 2.5 {
                    for p in self.pts.iter_mut() {
                        p.vx = p.side * self.rng.range(250.0, 700.0);
                        p.vy = self.rng.range(-90.0, 90.0);
                    }
                    self.stage = Stage::Disperse;
                    self.t = 0.0;
                }
                false
            }
            Stage::Disperse => {
                for p in self.pts.iter_mut() {
                    p.x += p.vx * dt;
                    p.y += p.vy * dt;
                }
                if self.t > 1.6 {
                    self.stage = Stage::Gap;
                    self.t = 0.0;
                }
                false
            }
            Stage::Gap => self.t > 0.5,
        }
    }

    fn render(&self, ctx: &Context, theme: &Theme, time: f64) {
        match self.stage {
            Stage::Gap => {}
            Stage::Assemble => {
                for p in &self.pts {
                    if self.t < p.delay {
                        continue;
                    }
                    let dx = p.tx - p.x;
                    let dy = p.ty - p.y;
                    let dist = (dx * dx + dy * dy).sqrt();
                    let a = (1.0 - dist / 1200.0).clamp(0.25, 1.0);
                    let c = if p.white { theme.fg } else { theme.accent };
                    dot(ctx, p.x - p.dx * 14.0, p.y - p.dy * 14.0, 2.0, theme.accent, a * 0.25);
                    dot(ctx, p.x, p.y, p.size, c, a);
                }
            }
            Stage::Hold => {
                for p in &self.pts {
                    let shimmer = 0.72 + 0.28 * (time * 3.0 + p.tw).sin();
                    let ox = (time * 5.0 + p.tw).sin() * 0.8;
                    let c = if p.white { theme.fg } else { theme.accent };
                    dot(ctx, p.x + ox, p.y, p.size, c, shimmer);
                }
            }
            Stage::Disperse => {
                let fade = (1.0 - self.t / 1.6).max(0.0);
                for p in &self.pts {
                    let c = if p.white { theme.fg } else { theme.accent };
                    dot(ctx, p.x, p.y, p.size, c, fade * 0.9);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// decrypt: movie-style glyph noise locks into the word left-to-right
// ---------------------------------------------------------------------------

struct DecryptPt {
    tx: f64,
    ty: f64,
    lock_at: f64,
    white: bool,
    tw: f64,
    size: f64,
}

struct Decrypt {
    word: String,
    pts: Vec<DecryptPt>,
    t: f64,
    sweep: f64,
}

impl Decrypt {
    fn new(word: &str, rng_seed: Option<u64>) -> Self {
        let mut rng = match rng_seed {
            Some(s) => Rng::fixed(s),
            None => Rng::seed(),
        };
        let sweep = 2.6;
        let pts = sample_word(word, N_PARTICLES, &mut rng)
            .into_iter()
            .map(|(tx, ty)| DecryptPt {
                tx,
                ty,
                lock_at: (tx / LW) * sweep + rng.range(0.0, 0.25),
                white: rng.f() < 0.30,
                tw: rng.range(0.0, 6.28),
                size: rng.range(1.6, 3.0),
            })
            .collect();
        Self {
            word: word.to_string(),
            pts,
            t: 0.0,
            sweep,
        }
    }

    fn ghost(&self, ctx: &Context, theme: &Theme, a: f64) {
        // dim backdrop of the real word, like an encrypted message
        ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
        ctx.set_font_size(44.0);
        let ext = ctx.text_extents(&self.word).expect("extents");
        ctx.move_to(
            (LW - ext.width()) / 2.0 - ext.x_bearing(),
            LH / 2.0 - (ext.height() / 2.0 + ext.y_bearing()),
        );
        ctx.set_source_rgba(theme.accent.0, theme.accent.1, theme.accent.2, a);
        ctx.show_text(&self.word).ok();
    }
}

impl Effect for Decrypt {
    fn update(&mut self, dt: f64) -> bool {
        self.t += dt;
        self.t > self.sweep + 2.0 + 0.8
    }

    fn render(&self, ctx: &Context, theme: &Theme, time: f64) {
        let fade = if self.t > self.sweep + 2.0 {
            (1.0 - (self.t - self.sweep - 2.0) / 0.8).max(0.0)
        } else {
            1.0
        };
        self.ghost(ctx, theme, 0.10 * fade);
        for p in &self.pts {
            let c = if p.white { theme.fg } else { theme.accent };
            if self.t < p.lock_at {
                // unsolved: jittering noise around the target
                let jx = (time * 13.0 + p.tw * 3.0).sin() * 7.0;
                let jy = (time * 17.0 + p.tw * 5.0).cos() * 5.0;
                let flicker = 0.10 + 0.30 * (0.5 + 0.5 * (time * 23.0 + p.tw * 7.0).sin());
                dot(ctx, p.tx + jx, (p.ty + jy).clamp(1.0, LH - 1.0), p.size, c, flicker * fade);
            } else {
                let since = (self.t - p.lock_at).min(0.3) / 0.3;
                let shimmer = 0.80 + 0.20 * (time * 3.0 + p.tw).sin();
                dot(ctx, p.tx, p.ty, p.size, c, since * shimmer * fade);
            }
        }
        // scanline at the decrypt frontier
        if self.t < self.sweep {
            let fx = (self.t / self.sweep) * LW;
            ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, 0.55 * fade);
            ctx.rectangle(fx - 1.0, 2.0, 2.0, LH - 4.0);
            ctx.fill().ok();
            ctx.set_source_rgba(theme.accent.0, theme.accent.1, theme.accent.2, 0.20 * fade);
            ctx.rectangle(fx - 9.0, 2.0, 18.0, LH - 4.0);
            ctx.fill().ok();
        }
    }
}

// ---------------------------------------------------------------------------
// rain: matrix-style columns fall while the word glows through
// ---------------------------------------------------------------------------

struct Drop {
    x: f64,
    y: f64,
    speed: f64,
    len: f64,
}

struct RainPt {
    tx: f64,
    ty: f64,
    white: bool,
    tw: f64,
    size: f64,
}

struct Rain {
    drops: Vec<Drop>,
    pts: Vec<RainPt>,
    t: f64,
    rng: Rng,
}

impl Rain {
    fn new(word: &str, rng_seed: Option<u64>) -> Self {
        let mut rng = match rng_seed {
            Some(s) => Rng::fixed(s),
            None => Rng::seed(),
        };
        let mut drops = vec![];
        let mut x = 4.0;
        while x < LW {
            drops.push(Drop {
                x,
                y: rng.range(-LH, LH),
                speed: rng.range(180.0, 480.0),
                len: rng.range(12.0, 30.0),
            });
            x += 8.0;
        }
        let pts = sample_word(word, N_PARTICLES, &mut rng)
            .into_iter()
            .map(|(tx, ty)| RainPt {
                tx,
                ty,
                white: rng.f() < 0.35,
                tw: rng.range(0.0, 6.28),
                size: rng.range(1.8, 3.2),
            })
            .collect();
        Self {
            drops,
            pts,
            t: 0.0,
            rng,
        }
    }
}

impl Effect for Rain {
    fn update(&mut self, dt: f64) -> bool {
        self.t += dt;
        for d in self.drops.iter_mut() {
            d.y += d.speed * dt;
            if d.y - d.len > LH {
                d.y = -self.rng.range(4.0, 40.0);
                d.speed = self.rng.range(180.0, 480.0);
            }
        }
        self.t > 3.0 + 2.0 + 0.8
    }

    fn render(&self, ctx: &Context, theme: &Theme, time: f64) {
        let reveal = (self.t / 3.0).min(1.0);
        let fade = if self.t > 5.0 {
            (1.0 - (self.t - 5.0) / 0.8).max(0.0)
        } else {
            1.0
        };
        // rain streaks behind
        for d in &self.drops {
            let y1 = (d.y - d.len).max(0.0);
            let y0 = d.y.min(LH);
            if y0 > y1 {
                ctx.set_source_rgba(theme.accent.0, theme.accent.1, theme.accent.2, 0.22 * fade);
                ctx.rectangle(d.x - 1.0, y1, 2.0, y0 - y1);
                ctx.fill().ok();
            }
            if d.y > 0.0 && d.y < LH {
                dot(ctx, d.x, d.y, 2.4, theme.fg, 0.55 * fade);
            }
        }
        // word glows through as the rain "lands"
        for p in &self.pts {
            let gate = ((reveal * 1.3) - (p.tx / LW) * 0.3).clamp(0.0, 1.0);
            if gate <= 0.0 {
                continue;
            }
            let shimmer = 0.70 + 0.30 * (time * 4.0 + p.tw).sin();
            let c = if p.white { theme.fg } else { theme.accent };
            dot(ctx, p.tx, p.ty, p.size, c, gate * shimmer * fade);
        }
    }
}

// ---------------------------------------------------------------------------
// beams: bright bars sweep across, illuminating the dim word
// ---------------------------------------------------------------------------

struct Beam {
    x: f64,
    vx: f64,
    w: f64,
}

struct Beams {
    pts: Vec<(f64, f64, bool, f64, f64)>, // tx, ty, white, tw, size
    beams: Vec<Beam>,
    t: f64,
}

impl Beams {
    fn new(word: &str, rng_seed: Option<u64>) -> Self {
        let mut rng = match rng_seed {
            Some(s) => Rng::fixed(s),
            None => Rng::seed(),
        };
        let pts = sample_word(word, N_PARTICLES, &mut rng)
            .into_iter()
            .map(|(tx, ty)| (tx, ty, rng.f() < 0.30, rng.range(0.0, 6.28), rng.range(1.6, 3.0)))
            .collect();
        let beams = vec![
            Beam { x: -120.0, vx: 420.0, w: 46.0 },
            Beam { x: LW + 120.0, vx: -520.0, w: 38.0 },
            Beam { x: LW / 2.0, vx: 300.0, w: 30.0 },
        ];
        Self { pts, beams, t: 0.0 }
    }

    fn glow(&self, x: f64) -> f64 {
        self.beams
            .iter()
            .map(|b| {
                let d = x - b.x;
                (-d * d / (2.0 * (b.w * b.w))).exp()
            })
            .fold(0.0f64, |a, b| (a + b).min(1.5))
    }
}

impl Effect for Beams {
    fn update(&mut self, dt: f64) -> bool {
        self.t += dt;
        for b in self.beams.iter_mut() {
            b.x += b.vx * dt;
        }
        self.t > 4.2 + 1.0 + 0.8
    }

    fn render(&self, ctx: &Context, theme: &Theme, time: f64) {
        let flash = self.t > 4.2;
        let fade = if self.t > 5.2 {
            (1.0 - (self.t - 5.2) / 0.8).max(0.0)
        } else {
            1.0
        };
        // beam bars
        for b in &self.beams {
            if b.x < -b.w * 2.0 || b.x > LW + b.w * 2.0 {
                continue;
            }
            ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, 0.30 * fade);
            ctx.rectangle(b.x - 2.0, 0.0, 4.0, LH);
            ctx.fill().ok();
            ctx.set_source_rgba(theme.accent.0, theme.accent.1, theme.accent.2, 0.16 * fade);
            ctx.rectangle(b.x - b.w, 0.0, b.w * 2.0, LH);
            ctx.fill().ok();
        }
        for (tx, ty, white, tw, size) in &self.pts {
            let g = self.glow(*tx);
            let (a, ox) = if flash {
                (0.85 + 0.15 * (time * 5.0 + tw).sin(), 0.0)
            } else {
                (0.22 + 0.78 * (g * 1.4).min(1.0), g.min(1.0) * 7.0 * (*tx - LW / 2.0).signum())
            };
            let c = if *white { theme.fg } else { theme.accent };
            dot(ctx, tx + ox, *ty, *size, c, a * fade);
        }
    }
}

// ---------------------------------------------------------------------------
// blackhole: the formed word is consumed, then explodes outward
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum HoleStage {
    Suck,
    Boom,
}

struct HolePt {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    white: bool,
    tw: f64,
    size: f64,
}

struct Blackhole {
    pts: Vec<HolePt>,
    stage: HoleStage,
    t: f64,
    rng: Rng,
}

impl Blackhole {
    fn new(word: &str, rng_seed: Option<u64>) -> Self {
        let mut rng = match rng_seed {
            Some(s) => Rng::fixed(s),
            None => Rng::seed(),
        };
        // start with the word formed so there is something to consume
        let pts = sample_word(word, N_PARTICLES, &mut rng)
            .into_iter()
            .map(|(tx, ty)| HolePt {
                x: tx,
                y: ty,
                vx: 0.0,
                vy: 0.0,
                white: rng.f() < 0.30,
                tw: rng.range(0.0, 6.28),
                size: rng.range(1.6, 3.0),
            })
            .collect();
        Self {
            pts,
            stage: HoleStage::Suck,
            t: 0.0,
            rng,
        }
    }
}

impl Effect for Blackhole {
    fn update(&mut self, dt: f64) -> bool {
        self.t += dt;
        let cx = LW / 2.0;
        let cy = LH / 2.0;
        match self.stage {
            HoleStage::Suck => {
                // 0.4s hold so the word reads, then accelerating differential
                // spiral: inner particles orbit faster (like an accretion disk)
                if self.t > 0.4 {
                    let k = ((self.t - 0.4) / 1.1).min(1.0);
                    for p in self.pts.iter_mut() {
                        let dx = cx - p.x;
                        let dy = cy - p.y;
                        let dist = (dx * dx + dy * dy).sqrt().max(6.0);
                        p.x += dx * (0.6 + 4.0 * k) * dt;
                        p.y += dy * (0.6 + 4.0 * k) * dt;
                        let tang = 45000.0 / dist * k * dt;
                        p.x += -dy / dist * tang;
                        p.y += dx / dist * tang;
                    }
                }
                if self.t > 1.5 {
                    // explode outward from the singularity
                    for p in self.pts.iter_mut() {
                        let mut dx = p.x - cx;
                        let mut dy = (p.y - cy) * 2.0; // strip is short: spread vertically too
                        if dx.abs() + dy.abs() < 4.0 {
                            let a = self.rng.range(0.0, 6.28);
                            dx = a.cos() * 8.0;
                            dy = a.sin() * 8.0;
                        }
                        let len = (dx * dx + dy * dy).sqrt().max(1.0);
                        let sp = self.rng.range(300.0, 850.0);
                        p.vx = dx / len * sp;
                        p.vy = dy / len * sp;
                    }
                    self.stage = HoleStage::Boom;
                    self.t = 0.0;
                }
                false
            }
            HoleStage::Boom => {
                for p in self.pts.iter_mut() {
                    p.x += p.vx * dt;
                    p.y += p.vy * dt;
                }
                self.t > 1.4
            }
        }
    }

    fn render(&self, ctx: &Context, theme: &Theme, time: f64) {
        match self.stage {
            HoleStage::Suck => {
                let k = (self.t / 1.5).min(1.0);
                // event-horizon glow tightening on the center
                ctx.set_source_rgba(theme.accent.0, theme.accent.1, theme.accent.2, 0.25 * (1.0 - k * 0.5));
                ctx.arc(LW / 2.0, LH / 2.0, 26.0 - 18.0 * k, 0.0, 6.29);
                ctx.fill().ok();
                for p in &self.pts {
                    let c = if p.white { theme.fg } else { theme.accent };
                    dot(ctx, p.x, p.y.clamp(1.0, LH - 1.0), p.size, c, 0.9);
                }
                if k > 0.85 {
                    // singularity flash
                    ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, (k - 0.85) / 0.15 * 0.8);
                    ctx.arc(LW / 2.0, LH / 2.0, 8.0, 0.0, 6.29);
                    ctx.fill().ok();
                }
                let _ = time;
            }
            HoleStage::Boom => {
                let fade = (1.0 - self.t / 1.4).max(0.0);
                // shockwave ring
                let r = 10.0 + self.t * 900.0;
                ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, fade * 0.35);
                ctx.set_line_width(2.0);
                ctx.arc(LW / 2.0, LH / 2.0, r.min(1200.0), 0.0, 6.29);
                ctx.stroke().ok();
                for p in &self.pts {
                    if p.x < -10.0 || p.x > LW + 10.0 || p.y < -10.0 || p.y > LH + 10.0 {
                        continue;
                    }
                    let c = if p.white { theme.fg } else { theme.accent };
                    dot(ctx, p.x, p.y, p.size, c, fade);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// saver: owns the current effect, picks the next at random (no repeats)
// ---------------------------------------------------------------------------

pub const EFFECTS: &[&str] = &["assemble", "decrypt", "rain", "beams", "blackhole"];

/// Good snapshot second per effect for `--view saver:EFFECT` previews.
pub fn default_t(name: &str) -> f64 {
    match name {
        "assemble" => 2.2,
        "decrypt" => 1.8,
        "rain" => 2.2,
        "beams" => 2.0,
        "blackhole" => 0.9,
        _ => 2.0,
    }
}

pub struct Saver {
    effect: Box<dyn Effect>,
    idx: usize,
    cycle: usize,
    time: f64,
    theme: Theme,
    rng: Rng,
}

fn make_effect(idx: usize, word: &str, seed: Option<u64>) -> Box<dyn Effect> {
    match EFFECTS[idx % EFFECTS.len()] {
        "decrypt" => Box::new(Decrypt::new(word, seed)),
        "rain" => Box::new(Rain::new(word, seed)),
        "beams" => Box::new(Beams::new(word, seed)),
        "blackhole" => Box::new(Blackhole::new(word, seed)),
        _ => Box::new(Assemble::new(word, seed)),
    }
}

fn effect_index(name: &str) -> Option<usize> {
    EFFECTS.iter().position(|e| *e == name)
}

impl Saver {
    /// Live mode: starts with assemble, then cycles randomly.
    pub fn new(word: &str) -> Self {
        Self {
            effect: make_effect(0, word, None),
            idx: 0,
            cycle: 0,
            time: 0.0,
            theme: theme::load_theme(),
            rng: Rng::seed(),
        }
    }

    /// Pinned effect (for `--view saver:EFFECT` previews); deterministic seed.
    pub fn new_effect(name: &str, word: &str) -> Self {
        let idx = effect_index(name).unwrap_or(0);
        Self {
            effect: make_effect(idx, word, Some(0xC0FFEE)),
            idx,
            cycle: 0,
            time: 0.0,
            theme: theme::load_theme(),
            rng: Rng::fixed(0xBEEF),
        }
    }

    /// Theme captured at cycle start (follows theme switches between cycles).
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    fn next_word(&self) -> String {
        if self.cycle % 2 == 0 {
            "OMARCHY".to_string()
        } else {
            clock_hhmm()
        }
    }

    fn advance(&mut self) {
        self.cycle += 1;
        if theme::current_theme() != self.theme.name {
            self.theme = theme::load_theme();
        }
        // random next effect, never the same twice in a row
        let n = EFFECTS.len();
        self.idx = (self.idx + 1 + self.rng.pick(n - 1)) % n;
        let word = self.next_word();
        self.effect = make_effect(self.idx, &word, None);
    }

    /// Advance the animation by dt seconds.
    pub fn update(&mut self, dt: f64) {
        let dt = dt.clamp(0.0, 0.1);
        self.time += dt;
        if self.effect.update(dt) {
            self.advance();
        }
    }

    /// Draw the current frame. Caller sets up the 2008x60 logical transform.
    pub fn render(&self, ctx: &Context, theme: &Theme) {
        ctx.set_source_rgb(0.0, 0.0, 0.0);
        ctx.paint().ok();
        self.effect.render(ctx, theme, self.time);
    }
}
