//! Cairo rendering (port of render_ui in touchbar_daemon.py).
//!
//! Logical space is 2008x60; the surface is 64x2008 with tiny-dfr's
//! translate(60,0)+rotate90 transform.

use cairo::{Context, FontSlant, FontWeight, Format, ImageSurface};
use std::f64::consts::PI;

use crate::actions;
use crate::config::{Button, Config};
use crate::hypr;
use crate::saver::Saver;
use crate::theme::{Rgb, Theme};

pub const W: i32 = 64;
pub const H: i32 = 2008;
pub const LW: f64 = 2008.0;
pub const LH: f64 = 60.0;

const WS_START: f64 = 24.0;
const WS_W: f64 = 140.0;
const WS_GAP: f64 = 12.0;
const TH1_W: f64 = 140.0;
const MENU_GAP: f64 = 12.0;
const CTL_GAP: f64 = 16.0;
const CTL_DEFAULT_W: f64 = 130.0;
const FN_N: usize = 12;
const FN_W: f64 = 158.0;
const FN_GAP: f64 = 9.0;

const PX_CELL: f64 = 9.0;
const PX_GAP: f64 = 9.0;
const CLK_W: f64 = 5.0 * 3.0 * PX_CELL + 4.0 * PX_GAP; // 171, full "HH:MM"
const CLK_NUDGE: f64 = -5.0;
// pet playground after the clock; weather sits past it, next to brightness
const PET_GAP: f64 = 16.0;
const PET_W: f64 = 240.0;
// weather block fits 5 glyphs ("-12°C") so controls never shift
const WTH_W: f64 = 5.0 * 3.0 * PX_CELL + 4.0 * PX_GAP; // 171
const WTH_GAP: f64 = 16.0;

pub const SL_TX0: f64 = 300.0;
pub const SL_TX1: f64 = LW - 140.0;

const NERD: &str = "JetBrainsMono Nerd Font";
const NEUTRAL_BG: Rgb = (0x18 as f64 / 255.0, 0x18 as f64 / 255.0, 0x20 as f64 / 255.0);

/// Shared layout: render and hit-testing both use this.
#[derive(Debug, Clone)]
pub struct Layout {
    pub ws_n: usize,
    pub ws_end: f64,
    pub clk_x0: f64,
    pub clk_end: f64,
    pub show_clock: bool,
    pub pet_x0: f64,
    pub pet_end: f64,
    pub wth_x0: f64,
    pub wth_end: f64,
    pub show_weather: bool,
    pub ctl: Vec<(f64, f64)>, // (x, w) per config button
    pub th1_x: f64,
    pub show_theme: bool,
}

pub fn layout(cfg: &Config) -> Layout {
    let ws_n = cfg.bar.workspaces.max(1).min(9);
    let ws_end = WS_START + ws_n as f64 * WS_W + (ws_n as f64 - 1.0) * WS_GAP;
    let show_clock = cfg.bar.show_clock;
    let clk_x0 = ws_end + 29.0 + CLK_NUDGE;
    let clk_end = if show_clock { clk_x0 + CLK_W } else { ws_end };
    let show_weather = cfg.bar.show_weather;
    let pet_x0 = clk_end + PET_GAP;
    let pet_end = pet_x0 + PET_W;
    let wth_x0 = pet_end + WTH_GAP;
    let wth_end = if show_weather { wth_x0 + WTH_W } else { pet_end };
    let show_theme = cfg.bar.show_theme;
    let th1_x = LW - 24.0 - TH1_W;
    let right = if show_theme { th1_x - 16.0 } else { LW - 24.0 };
    let ctl_start = wth_end + 30.0;
    let n = cfg.button.len().max(1);
    let avail = (right - ctl_start).max(60.0);
    let w = (avail - (n as f64 - 1.0) * CTL_GAP) / n as f64;
    let w = w.min(CTL_DEFAULT_W).max(40.0);
    // center the row in the available space so few buttons don't hug left
    let total = n as f64 * w + (n as f64 - 1.0) * CTL_GAP;
    let x0 = ctl_start + ((avail - total) / 2.0).max(0.0);
    let ctl = (0..cfg.button.len())
        .map(|k| (x0 + k as f64 * (w + CTL_GAP), w))
        .collect();
    Layout {
        ws_n,
        ws_end,
        clk_x0,
        clk_end,
        show_clock,
        pet_x0,
        pet_end,
        wth_x0,
        wth_end,
        show_weather,
        ctl,
        th1_x,
        show_theme,
    }
}

pub fn menu_geometry(n: usize) -> (f64, f64) {
    let n = n.max(1) as f64;
    let w = (LW - (n - 1.0) * MENU_GAP) / n;
    ((LW - (n * w + (n - 1.0) * MENU_GAP)) / 2.0, w)
}

fn set_rgb(ctx: &Context, c: Rgb) {
    ctx.set_source_rgb(c.0, c.1, c.2);
}

fn rounded(ctx: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    ctx.new_sub_path();
    ctx.arc(x + r, y + r, r, PI, 1.5 * PI);
    ctx.arc(x + w - r, y + r, r, 1.5 * PI, 0.0);
    ctx.arc(x + w - r, y + h - r, r, 0.0, 0.5 * PI);
    ctx.arc(x + r, y + h - r, r, 0.5 * PI, PI);
    ctx.close_path();
}

fn text_centered(
    ctx: &Context,
    cx: f64,
    cy: f64,
    s: &str,
    color: Rgb,
    font: &str,
    size: f64,
) -> f64 {
    ctx.select_font_face(font, FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(size);
    let ext = ctx.text_extents(s).unwrap_or_else(|_| {
        // unreachable in practice; return zero extents
        ctx.text_extents(" ").expect("text extents")
    });
    ctx.move_to(
        cx - (ext.width() / 2.0 + ext.x_bearing()),
        cy - (ext.height() / 2.0 + ext.y_bearing()),
    );
    set_rgb(ctx, color);
    ctx.show_text(s).ok();
    ext.width()
}

/// Pixel pet moods on a 54s loop: idle, dance, play, eat, sleep.
/// Happy is never scheduled — it only triggers when you tap (pet) the cat.
#[derive(Clone, Copy, PartialEq)]
pub enum PetMood {
    Idle,
    Dance,
    Play,
    Eat,
    Sleep,
    Happy,
}

/// (mood, seconds into the current slot), derived from wall time so no
/// daemon state is needed. PET_DEBUG="mood:secs" pins a mood for previews.
fn pet_mood() -> (PetMood, f64) {
    if let Ok(dbg) = std::env::var("PET_DEBUG") {
        let mut it = dbg.split(':');
        let mood = match it.next().unwrap_or("") {
            "dance" => PetMood::Dance,
            "play" => PetMood::Play,
            "eat" => PetMood::Eat,
            "sleep" => PetMood::Sleep,
            "happy" => PetMood::Happy,
            _ => PetMood::Idle,
        };
        let s: f64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        return (mood, s);
    }
    let t = unsafe { libc::time(std::ptr::null_mut()) } as f64 % 54.0;
    if t < 8.0 {
        (PetMood::Idle, t)
    } else if t < 14.0 {
        (PetMood::Dance, t - 8.0)
    } else if t < 20.0 {
        (PetMood::Idle, t - 14.0)
    } else if t < 26.0 {
        (PetMood::Play, t - 20.0)
    } else if t < 32.0 {
        (PetMood::Eat, t - 26.0)
    } else if t < 38.0 {
        (PetMood::Idle, t - 32.0)
    } else if t < 48.0 {
        (PetMood::Sleep, t - 38.0)
    } else {
        (PetMood::Idle, t - 48.0)
    }
}

const PET_OPEN: [u8; 25] = [
    1, 0, 0, 0, 1, //
    1, 1, 1, 1, 1, //
    1, 0, 1, 0, 1, //
    1, 1, 1, 1, 1, //
    0, 1, 1, 1, 0, //
];
const PET_SHUT: [u8; 25] = [
    1, 0, 0, 0, 1, //
    1, 1, 1, 1, 1, //
    1, 1, 1, 1, 1, //
    1, 1, 1, 1, 1, //
    0, 1, 1, 1, 0, //
];
const PET_EAT_SHUT: [u8; 25] = [
    1, 0, 0, 0, 1, //
    1, 1, 1, 1, 1, //
    1, 0, 1, 0, 1, //
    0, 1, 1, 1, 0, //
    0, 0, 0, 0, 0, //
];
const PET_EAT_OPEN: [u8; 25] = [
    1, 0, 0, 0, 1, //
    1, 1, 1, 1, 1, //
    1, 0, 1, 0, 1, //
    0, 1, 0, 1, 0, //
    0, 0, 0, 0, 0, //
];
const PET_WINK_L: [u8; 25] = [
    1, 0, 0, 0, 1, //
    1, 1, 1, 1, 1, //
    1, 0, 1, 1, 1, //
    1, 1, 1, 1, 1, //
    0, 1, 1, 1, 0, //
];
const PET_WINK_R: [u8; 25] = [
    1, 0, 0, 0, 1, //
    1, 1, 1, 1, 1, //
    1, 1, 1, 0, 1, //
    1, 1, 1, 1, 1, //
    0, 1, 1, 1, 0, //
];
const PET_SMILE: [u8; 25] = [
    1, 0, 0, 0, 1, //
    1, 1, 1, 1, 1, //
    1, 1, 1, 1, 1, //
    0, 1, 0, 1, 0, //
    0, 1, 1, 1, 0, //
];
const PET_HEART: [u8; 9] = [
    1, 0, 1, //
    1, 1, 1, //
    0, 1, 0, //
];
const PET_Z: [u8; 9] = [
    1, 1, 1, //
    0, 1, 0, //
    1, 1, 1, //
];

fn pet_cells(ctx: &Context, x0: f64, y0: f64, cell: f64, cells: &[u8], cols: usize, c: Rgb, a: f64) {
    ctx.set_source_rgba(c.0, c.1, c.2, a.clamp(0.0, 1.0));
    for (i, v) in cells.iter().enumerate() {
        if *v == 1 {
            ctx.rectangle(
                x0 + (i % cols) as f64 * cell,
                y0 + (i / cols) as f64 * cell,
                cell - 1.0,
                cell - 1.0,
            );
        }
    }
    ctx.fill().ok();
}

/// The pixel pet. `cx` = playground center, `half` = roaming room.
/// `force` (tap-to-pet) overrides the schedule with Happy.
fn pixel_pet(ctx: &Context, theme: &Theme, cx: f64, half: f64, force: Option<PetMood>) {
    let wall = unsafe { libc::time(std::ptr::null_mut()) } as f64;
    let (mood, s) = match force {
        Some(PetMood::Happy) => (PetMood::Happy, wall),
        _ => pet_mood(),
    };
    let sec = s as usize;
    let x0 = cx - 2.5 * PX_CELL;
    let y0 = (LH - 5.0 * PX_CELL) / 2.0;
    match mood {
        PetMood::Idle => {
            // blink + alternating winks across the 8s slot
            let grid = match s % 8.0 {
                v if v < 3.0 => &PET_OPEN,
                v if v < 4.0 => &PET_SHUT,
                v if v < 5.0 => &PET_OPEN,
                v if v < 6.0 => &PET_WINK_L,
                v if v < 7.0 => &PET_WINK_R,
                _ => &PET_OPEN,
            };
            pet_cells(ctx, x0, y0, PX_CELL, grid, 5, theme.accent, 1.0);
        }
        PetMood::Dance => {
            // big side-step hops across the playground
            let hop = sec % 2 == 1;
            let dx = if hop { 10.0 } else { -10.0 };
            let dy = if hop { -6.0 } else { 0.0 };
            pet_cells(ctx, x0 + dx, y0 + dy, PX_CELL, &PET_SHUT, 5, theme.accent, 1.0);
        }
        PetMood::Play => {
            // chases a bouncing ball sweeping the playground
            let bx = cx + (s / 6.0 * 6.2832).sin() * (half - 20.0);
            let gy = y0 + 5.0 * PX_CELL - 7.0;
            let by = gy - (s / 6.0 * 12.5664).sin().abs() * 16.0;
            ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, 1.0);
            ctx.rectangle(bx - 3.5, by - 3.5, 7.0, 7.0);
            ctx.fill().ok();
            let dx = ((bx - cx) * 0.5).clamp(-(half - 50.0), half - 50.0);
            let hop = (bx - cx).abs() > 12.0 && sec % 2 == 1;
            pet_cells(
                ctx,
                x0 + dx,
                y0 + if hop { -4.0 } else { 0.0 },
                PX_CELL,
                &PET_OPEN,
                5,
                theme.accent,
                1.0,
            );
        }
        PetMood::Eat => {
            // munches from a bowl (dim row under the chin), bobbing slightly
            let grid = if sec % 2 == 0 { &PET_EAT_SHUT } else { &PET_EAT_OPEN };
            let bob = if sec % 2 == 0 { 0.0 } else { 1.5 };
            pet_cells(ctx, x0, y0 + bob, PX_CELL, grid, 5, theme.accent, 1.0);
            ctx.set_source_rgba(theme.dim.0, theme.dim.1, theme.dim.2, 1.0);
            ctx.rectangle(x0, y0 + bob + 4.0 * PX_CELL, 5.0 * PX_CELL - 1.0, PX_CELL - 1.0);
            ctx.fill().ok();
        }
        PetMood::Sleep => {
            // breathing (slow bob) + two Z's drifting through the margin
            let bob = (sec % 2) as f64 * 1.0;
            pet_cells(ctx, x0, y0 + 8.0 + bob, PX_CELL, &PET_SHUT, 5, theme.accent, 0.85);
            let p = s / 10.0;
            for i in 0..2 {
                let lp = p * 2.0 - i as f64;
                if (0.0..1.0).contains(&lp) {
                    pet_cells(
                        ctx,
                        x0 + 46.0 + 8.0 * lp,
                        y0 + 20.0 - 6.0 * lp,
                        4.0,
                        &PET_Z,
                        3,
                        theme.accent,
                        (1.0 - lp) * 0.85,
                    );
                }
            }
        }
        PetMood::Happy => {
            // petted! jumping smile with blush + a burst of hearts
            let jump = (sec % 2 == 1) as usize as f64 * -6.0;
            pet_cells(ctx, x0, y0 + jump, PX_CELL, &PET_SMILE, 5, theme.accent, 1.0);
            // blush cheeks
            ctx.set_source_rgba(theme.red.0, theme.red.1, theme.red.2, 0.9);
            ctx.rectangle(x0, y0 + jump + 2.0 * PX_CELL, PX_CELL - 1.0, PX_CELL - 1.0);
            ctx.rectangle(
                x0 + 4.0 * PX_CELL,
                y0 + jump + 2.0 * PX_CELL,
                PX_CELL - 1.0,
                PX_CELL - 1.0,
            );
            ctx.fill().ok();
            for i in 0..3 {
                let ph = ((wall + i as f64 * 1.7) % 5.0) / 5.0;
                pet_cells(
                    ctx,
                    x0 + 4.0 + i as f64 * 14.0 + (wall * 2.0 + i as f64).sin() * 3.0,
                    y0 - 4.0 - ph * 22.0,
                    4.0,
                    &PET_HEART,
                    3,
                    theme.red,
                    (1.0 - ph) * 0.95,
                );
            }
        }
    }
}

fn now_hhmm() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

// 3x5 pixel glyphs (clock + weather)
fn digit(ch: char) -> Option<[u8; 15]> {
    let d: [u8; 15] = match ch {
        '0' => [1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1],
        '1' => [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1],
        '2' => [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1],
        '3' => [1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1],
        '4' => [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1],
        '5' => [1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1],
        '6' => [1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1],
        '7' => [1, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0],
        '8' => [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1],
        '9' => [1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1],
        ':' => [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0],
        '-' => [0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0],
        '°' => [1, 1, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        'C' => [1, 1, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 1],
        'F' => [1, 1, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 0, 0],
        _ => return None,
    };
    Some(d)
}

fn pixel_text(ctx: &Context, theme: &Theme, mut x: f64, s: &str) {
    let top = (LH - 5.0 * PX_CELL) / 2.0;
    for ch in s.chars() {
        let Some(g) = digit(ch) else { continue };
        let color = if ch == ':' || ch == '°' { theme.accent } else { theme.fg };
        set_rgb(ctx, color);
        for r in 0..5 {
            for c in 0..3 {
                if g[r * 3 + c] == 1 {
                    ctx.rectangle(
                        x + c as f64 * PX_CELL,
                        top + r as f64 * PX_CELL,
                        PX_CELL - 1.0,
                        PX_CELL - 1.0,
                    );
                }
            }
        }
        ctx.fill().ok();
        x += 3.0 * PX_CELL + PX_GAP;
    }
}

/// Webapp icons, all verified on-device. Omarchy webapps all run as
/// `chromium --app=<url>`, so WM class is usually generic ("chromium") or
/// a host-derived PWA class ("chrome-<host>__..."): match host first,
/// then title keywords, then fall back to the chrome icon.
fn host_glyph(host: &str) -> Option<(&'static str, &'static str)> {
    let host = host.to_lowercase();
    let host = host.split(':').next().unwrap_or(&host);
    // (keyword, font, glyph) — order matters, check specific before generic
    const TABLE: &[(&str, &str, &str)] = &[
        ("youtube", NERD, "\u{F16A}"),     // \uf16a play button
        ("twitter", NERD, ""),     // \uf099 bird
        ("whatsapp", NERD, ""),    // \uf232
        ("discord", NERD, "󰙯"),     // mdi \U000f066f (\uf392 is tofu here)
        ("github", NERD, ""),      // \uf09b
        ("zoom", NERD, ""),        // \uf03d
        ("google", NERD, ""),      // \uf1a0 Maps/Photos/...
        ("maps", NERD, ""),
        ("photos", NERD, ""),
        ("contacts", NERD, ""),
        ("messages", NERD, ""),
    ];
    for (key, font, g) in TABLE {
        if host.contains(key) {
            return Some((*font, *g));
        }
    }
    if host.contains("x.com") {
        return Some(("Sans", "X"));
    }
    None
}

fn is_generic_browser(low: &str) -> bool {
    matches!(
        low,
        "chromium" | "chrome" | "google-chrome" | "brave-browser" | "microsoft-edge" | "vivaldi" | "helium"
    ) || low.starts_with("chrome-")
}

pub fn glyph_for_wmclass(class: &str, title: &str) -> Option<(&'static str, &'static str)> {
    // exact app classes first
    if class == "foot" {
        return Some((NERD, "\u{F489}"));
    }
    if class == "org.omarchy.agent" {
        return Some(("Sans", "▲"));
    }
    let low = class.to_lowercase();
    // PWA class "chrome-<host>__..." carries the real site
    if low.starts_with("chrome-") {
        if let Some(rest) = low.strip_prefix("chrome-") {
            if let Some(host) = rest.split("__").next() {
                let host = host.replace('_', ".");
                if let Some(g) = host_glyph(&host) {
                    return Some(g);
                }
            }
        }
    }
    if is_generic_browser(&low) {
        let t = title.to_lowercase();
        let t = t.trim();
        if t == "x" || t == "home / x" || t.contains("x.com") {
            return Some(("Sans", "X"));
        }
        const TITLE_KEYS: &[(&str, &str, &str)] = &[
            ("youtube", NERD, "\u{F16A}"),
            ("twitter", NERD, ""),
            ("whatsapp", NERD, ""),
            ("discord", NERD, "󰙯"),
            ("github", NERD, ""),
            ("zoom", NERD, ""),
            ("maps", NERD, ""),
            ("photos", NERD, ""),
            ("contacts", NERD, ""),
            ("messages", NERD, ""),
            ("google", NERD, ""),
        ];
        for (key, font, g) in TITLE_KEYS {
            if t.contains(key) {
                return Some((*font, *g));
            }
        }
        return Some((NERD, "")); // generic chrome \uf268
    }
    None
}

fn ws_button(
    ctx: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    theme: &Theme,
    num: usize,
    glyphs: &[(&str, &str)],
    is_active: bool,
    has_windows: bool,
) {
    rounded(ctx, x, y, w, h, 12.0);
    if is_active {
        ctx.set_source_rgba(theme.accent.0, theme.accent.1, theme.accent.2, 0.32);
        ctx.fill_preserve().ok();
        set_rgb(ctx, theme.accent);
        ctx.set_line_width(2.5);
        ctx.stroke().ok();
    } else {
        let a = if has_windows { 0.90 } else { 0.60 };
        ctx.set_source_rgba(theme.pill.0, theme.pill.1, theme.pill.2, a);
        ctx.fill_preserve().ok();
        ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, 0.35);
        ctx.set_line_width(1.5);
        ctx.stroke().ok();
    }
    let cy = y + h / 2.0;
    let num_color = if is_active {
        theme.accent
    } else if has_windows {
        theme.fg
    } else {
        theme.dim
    };
    ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(27.0);
    let ext = ctx.text_extents(&num.to_string()).expect("extents");
    let mut icon_widths = vec![];
    for (font, g) in glyphs.iter().take(3) {
        ctx.select_font_face(font, FontSlant::Normal, FontWeight::Bold);
        ctx.set_font_size(25.0);
        icon_widths.push(ctx.text_extents(g).map(|e| e.width()).unwrap_or(0.0));
    }
    let total = ext.width() + icon_widths.iter().sum::<f64>() + 14.0 + icon_widths.len() as f64 * 10.0;
    let mut gx = x + (w - total) / 2.0;
    ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(27.0);
    ctx.move_to(
        gx - ext.x_bearing(),
        cy - (ext.height() / 2.0 + ext.y_bearing()),
    );
    set_rgb(ctx, num_color);
    ctx.show_text(&num.to_string()).ok();
    gx += ext.width() + 14.0;
    for ((font, g), iw) in glyphs.iter().take(3).zip(icon_widths.iter()) {
        if gx + iw > x + w - 8.0 {
            break;
        }
        ctx.select_font_face(font, FontSlant::Normal, FontWeight::Bold);
        ctx.set_font_size(25.0);
        let ge = ctx.text_extents(g).expect("extents");
        ctx.move_to(
            gx - ge.x_bearing(),
            cy - (ge.height() / 2.0 + ge.y_bearing()),
        );
        set_rgb(ctx, theme.fg);
        ctx.show_text(g).ok();
        gx += iw + 10.0;
    }
}

fn theme_button(
    ctx: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    name: &str,
    accent: Rgb,
    is_active: bool,
    dark: Rgb,
    size: f64,
) {
    rounded(ctx, x, y, w, h, 12.0);
    let fg;
    if is_active {
        set_rgb(ctx, accent);
        ctx.fill_preserve().ok();
        set_rgb(ctx, accent);
        ctx.set_line_width(2.0);
        ctx.stroke().ok();
        fg = dark;
    } else {
        ctx.set_source_rgba(accent.0, accent.1, accent.2, 0.28);
        ctx.fill_preserve().ok();
        ctx.set_source_rgba(accent.0, accent.1, accent.2, 0.80);
        ctx.set_line_width(1.5);
        ctx.stroke().ok();
        fg = accent;
    }
    let short: String = name.chars().take(13).collect();
    text_centered(ctx, x + w / 2.0, y + h / 2.0, &short, fg, "Sans", size);
}

/// One launcher button: icon + name grouped and centered.
#[derive(Debug, Clone)]
pub struct AppItem {
    pub name: String,
    pub icon: String,
    pub sans: bool,
    pub color: Rgb,
}

fn app_button(ctx: &Context, x: f64, y: f64, w: f64, h: f64, app: &AppItem) {
    let accent = app.color;
    rounded(ctx, x, y, w, h, 12.0);
    ctx.set_source_rgba(accent.0, accent.1, accent.2, 0.28);
    ctx.fill_preserve().ok();
    ctx.set_source_rgba(accent.0, accent.1, accent.2, 0.80);
    ctx.set_line_width(1.5);
    ctx.stroke().ok();
    let cy = y + h / 2.0;
    let font = if app.sans { "Sans" } else { NERD };
    // measure icon + name as a group so the pair sits centered
    ctx.select_font_face(font, FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(26.0);
    let ie = ctx.text_extents(&app.icon).expect("extents");
    ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(17.0);
    let short: String = app.name.chars().take(10).collect();
    let ne = ctx.text_extents(&short).expect("extents");
    let total = ie.width() + 10.0 + ne.width();
    let mut gx = x + (w - total) / 2.0;
    ctx.select_font_face(font, FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(26.0);
    ctx.move_to(
        gx - ie.x_bearing(),
        cy - (ie.height() / 2.0 + ie.y_bearing()),
    );
    set_rgb(ctx, accent);
    ctx.show_text(&app.icon).ok();
    gx += ie.width() + 10.0;
    ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(17.0);
    ctx.move_to(
        gx - ne.x_bearing(),
        cy - (ne.height() / 2.0 + ne.y_bearing()),
    );
    set_rgb(ctx, accent);
    ctx.show_text(&short).ok();
}

fn ctl_button(ctx: &Context, x: f64, y: f64, w: f64, h: f64, icon: &str, fg: Rgb, dim: Rgb) {
    rounded(ctx, x, y, w, h, 12.0);
    ctx.set_source_rgba(fg.0, fg.1, fg.2, 0.16);
    ctx.fill_preserve().ok();
    ctx.set_source_rgba(fg.0, fg.1, fg.2, 0.40);
    ctx.set_line_width(1.5);
    ctx.stroke().ok();
    text_centered(ctx, x + w / 2.0, y + h / 2.0, icon, dim, NERD, 28.0);
}

fn slider_view(ctx: &Context, theme: &Theme, label: &str, icon: &str, value: i32) {
    let cy = LH / 2.0;
    rounded(ctx, 16.0, 8.0, 96.0, LH - 16.0, 12.0);
    ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, 0.10);
    ctx.fill_preserve().ok();
    ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, 0.30);
    ctx.set_line_width(1.5);
    ctx.stroke().ok();
    text_centered(ctx, 64.0, cy, "←", theme.fg, "Sans", 26.0);
    text_centered(ctx, 172.0, cy, icon, theme.accent, NERD, 28.0);
    ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(22.0);
    let ext = ctx.text_extents(label).expect("extents");
    ctx.move_to(
        214.0 - ext.x_bearing(),
        cy - (ext.height() / 2.0 + ext.y_bearing()),
    );
    set_rgb(ctx, theme.dim);
    ctx.show_text(label).ok();
    let (tx0, tx1, ty) = (SL_TX0, SL_TX1, cy);
    rounded(ctx, tx0, ty - 5.0, tx1 - tx0, 10.0, 5.0);
    ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, 0.18);
    ctx.fill().ok();
    let fx = tx0 + (tx1 - tx0) * value.clamp(0, 100) as f64 / 100.0;
    if fx > tx0 {
        rounded(ctx, tx0, ty - 5.0, fx - tx0, 10.0, 5.0);
        set_rgb(ctx, theme.accent);
        ctx.fill().ok();
    }
    ctx.arc(fx, ty, 13.0, 0.0, 2.0 * PI);
    set_rgb(ctx, theme.accent);
    ctx.fill().ok();
    text_centered(
        ctx,
        LW - 60.0,
        cy,
        &format!("{}", value.clamp(0, 100)),
        theme.fg,
        "Sans",
        24.0,
    );
}

fn fn_view(ctx: &Context, theme: &Theme) {
    let cy = LH / 2.0;
    let start = (LW - (FN_N as f64 * FN_W + (FN_N as f64 - 1.0) * FN_GAP)) / 2.0;
    for k in 0..FN_N {
        let x = start + k as f64 * (FN_W + FN_GAP);
        rounded(ctx, x, 5.0, FN_W, LH - 10.0, 12.0);
        ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, 0.16);
        ctx.fill_preserve().ok();
        ctx.set_source_rgba(theme.fg.0, theme.fg.1, theme.fg.2, 0.40);
        ctx.set_line_width(1.5);
        ctx.stroke().ok();
        text_centered(
            ctx,
            x + FN_W / 2.0,
            cy,
            &format!("F{}", k + 1),
            theme.dim,
            "Sans",
            26.0,
        );
    }
}

pub struct SliderState {
    pub button_idx: usize,
    pub label: String,
    pub icon: String,
    pub value: i32,
}

pub enum View<'a> {
    Normal,
    Menu(&'a [(String, Rgb)]),
    Apps(&'a [AppItem]),
    Slider(&'a SliderState),
    Fn,
}

/// Render a frame. Returns (pixel bytes ARGB32, stride).
pub fn render(
    cfg: &Config,
    theme: &Theme,
    lay: &Layout,
    focused: i32,
    favs: &[(String, Rgb)],
    view: View,
    pet_force: Option<PetMood>,
) -> (Vec<u8>, usize) {
    let mut surf =
        ImageSurface::create(Format::ARgb32, W, H).expect("cairo surface");
    {
        let ctx = Context::new(&surf).expect("cairo context");
        ctx.set_source_rgb(0.0, 0.0, 0.0);
        ctx.paint().ok();
        ctx.translate(LH, 0.0);
        ctx.rotate(PI / 2.0);

        set_rgb(&ctx, NEUTRAL_BG);
        ctx.rectangle(0.0, 0.0, LW, LH);
        ctx.fill().ok();
        let (by, bh) = (5.0, LH - 10.0);

        match view {
            View::Menu(items) => {
                let (x0, bw) = menu_geometry(items.len());
                let mut x = x0;
                for (name, accent) in items {
                    theme_button(
                        &ctx,
                        x,
                        by,
                        bw,
                        bh,
                        name,
                        *accent,
                        *name == theme.name,
                        theme.dark,
                        if bw >= 260.0 { 20.0 } else { 15.0 },
                    );
                    x += bw + MENU_GAP;
                }
            }
            View::Apps(apps) => {
                let (x0, bw) = menu_geometry(apps.len());
                let mut x = x0;
                for app in apps {
                    app_button(&ctx, x, by, bw, bh, app);
                    x += bw + MENU_GAP;
                }
            }
            View::Slider(s) => {
                slider_view(&ctx, theme, &s.label, &s.icon, s.value);
            }
            View::Fn => fn_view(&ctx, theme),
            View::Normal => {
                let ws = hypr::workspaces();
                let clients = hypr::clients();
                let open: std::collections::HashSet<i32> =
                    ws.iter().map(|w| w.id).collect();
                let mut ws_apps: std::collections::HashMap<i32, Vec<(&str, &str)>> =
                    std::collections::HashMap::new();
                for c in &clients {
                    if (1..=lay.ws_n as i32).contains(&c.ws_id) {
                        if let Some(g) = glyph_for_wmclass(&c.class, &c.title) {
                            ws_apps.entry(c.ws_id).or_default().push(g);
                        }
                    }
                }
                for k in 0..lay.ws_n {
                    let i = k as i32 + 1;
                    let empty: Vec<(&str, &str)> = vec![];
                    let glyphs = ws_apps.get(&i).unwrap_or(&empty);
                    ws_button(
                        &ctx,
                        WS_START + k as f64 * (WS_W + WS_GAP),
                        by,
                        WS_W,
                        bh,
                        theme,
                        k + 1,
                        glyphs,
                        i == focused,
                        open.contains(&i),
                    );
                }

                if lay.show_clock {
                    pixel_text(&ctx, theme, lay.clk_x0, &now_hhmm());
                    pixel_pet(
                        &ctx,
                        theme,
                        (lay.pet_x0 + lay.pet_end) / 2.0,
                        PET_W / 2.0,
                        pet_force,
                    );
                }

                if lay.show_weather {
                    // pixel temperature ("21°C"); "--°" until first fetch lands
                    let w = actions::weather_read();
                    let s = if w.is_empty() { "--°".to_string() } else { w };
                    pixel_text(&ctx, theme, lay.wth_x0, &s);
                }

                // state-aware icons for toggle buttons
                let playing = actions::mpris_status().as_deref() == Some("Playing");
                let muted = actions::get_mic_muted();
                let night = actions::get_night();
                for (k, btn) in cfg.button.iter().enumerate() {
                    let Some((x, w)) = lay.ctl.get(k) else { continue };
                    let (icon, glyph): (String, Rgb) = match btn.kind.as_str() {
                        "slider" => (
                            btn.icon.clone().unwrap_or_else(|| "?".into()),
                            theme.accent,
                        ),
                        "media" => (
                            if playing {
                                "\u{F04B}".into()
                            } else {
                                "\u{F04C}".into()
                            },
                            theme.accent,
                        ),
                        "mic" => (
                            if muted { "\u{F131}".into() } else { "\u{F130}".into() },
                            if muted { theme.red } else { theme.accent },
                        ),
                        "night" => (
                            btn.icon.clone().unwrap_or_else(|| "\u{F186}".into()),
                            if night { theme.accent } else { theme.dim },
                        ),
                        "lock" => (
                            btn.icon.clone().unwrap_or_else(|| "󰌾".into()),
                            theme.accent,
                        ),
                        _ => (
                            btn.icon.clone().unwrap_or_else(|| "?".into()),
                            theme.accent,
                        ),
                    };
                    ctl_button(&ctx, *x, by, *w, bh, &icon, theme.fg, glyph);
                }

                if lay.show_theme {
                    let mut cur_accent = theme.accent;
                    for (name, accent) in favs {
                        if *name == theme.name {
                            cur_accent = *accent;
                            break;
                        }
                    }
                    theme_button(
                        &ctx,
                        lay.th1_x,
                        by,
                        TH1_W,
                        bh,
                        &theme.name,
                        cur_accent,
                        false,
                        theme.dark,
                        20.0,
                    );
                }
            }
        }
    }
    surf.flush();
    let stride = surf.stride() as usize;
    let data = surf
        .data()
        .map(|d| d.to_vec())
        .unwrap_or_else(|_| vec![0u8; stride * H as usize]);
    (data, stride)
}

/// Render one touch-bar screensaver frame. Returns (pixel bytes ARGB32, stride).
pub fn render_saver(sv: &Saver) -> (Vec<u8>, usize) {
    let theme = sv.theme();
    let mut surf = ImageSurface::create(Format::ARgb32, W, H).expect("cairo surface");
    {
        let ctx = Context::new(&surf).expect("cairo context");
        ctx.set_source_rgb(0.0, 0.0, 0.0);
        ctx.paint().ok();
        ctx.translate(LH, 0.0);
        ctx.rotate(PI / 2.0);
        sv.render(&ctx, theme);
    }
    surf.flush();
    let stride = surf.stride() as usize;
    let data = surf
        .data()
        .map(|d| d.to_vec())
        .unwrap_or_else(|_| vec![0u8; stride * H as usize]);
    (data, stride)
}

/// Default icon/label for builtin slider targets (when config omits them).
pub fn slider_defaults(target: &str) -> (&'static str, &'static str) {
    match target {
        "display" => ("Display", "\u{F185}"),
        "volume" => ("Volume", ""),
        "kbd" => ("Keys", ""),
        _ => ("Level", "?"),
    }
}

pub type ButtonRef<'a> = &'a Button;
