//! Center deck: clock + center slot + weather.
//!
//! Ownership rule: **the bar knows nothing about what deck plugins draw.**
//! The bar (`render.rs` / `main.rs`) only grants bounds — it calls
//! `render_middle()` with the layout and `ZoneState`, and executes the
//! generic `TapEffect` a tap resolves to. Everything else (who is
//! available, who wins the slot, what pixels, what a tap means) lives here.
//!
//! ```text
//! [clock] [center: pet | levels | pomodoro] [weather]
//! ```
//!
//! Adding a plugin: implement `CenterPlugin`, push it in `registry()`.
//! No bar file changes needed.
//!
//! Tap = the visible plugin's action (pet-the-cat, cycle visualizer
//! style, start/pause); double-tap cycles pinned plugins. Playback is
//! owned by the media button — a tap on the visualizer never plays/pauses.
//!
//! Feed files (written by apps, read by plugins):
//! - `~/.cache/omarchy-touchbar/deck/levels.json`
//!   `{"levels":[0.0..1.0,…],"label":"Spotify"}` (fresh = <2s old)
//! - `~/.cache/omarchy-touchbar/deck/pomodoro.json`
//!   `{"active":true,"label":"24:59"}`

use std::time::{Duration, Instant};

use cairo::Context;

use crate::actions;
use crate::render::{rounded, set_rgb, text_centered, Layout, LH};
use crate::theme::{Rgb, Theme};

// ---------------------------------------------------------------------------
// tiny pixel text (clock + weather side plugins, ~35px tall in a 60px strip)
// ---------------------------------------------------------------------------

pub const TXT_CELL: f64 = 7.0;
pub const TXT_GAP: f64 = 7.0;

// Pet pixel cells stay big (9px): only clock/weather shrank.
const PX_CELL: f64 = 9.0;

// ---------------------------------------------------------------------------
// plugin ids / registry
// ---------------------------------------------------------------------------

/// Center-slot plugin ids in priority order (highest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Center {
    Levels,
    Pomodoro,
    Pet,
}

impl Center {
    pub fn id(self) -> &'static str {
        match self {
            Center::Levels => "levels",
            Center::Pomodoro => "pomodoro",
            Center::Pet => "pet",
        }
    }
}

/// Opaque preview lookup: CLI `--view <id>` resolves without the bar
/// naming any plugin concretely.
pub fn center_by_id(id: &str) -> Option<Center> {
    match id {
        "levels" => Some(Center::Levels),
        "pomodoro" => Some(Center::Pomodoro),
        "pet" => Some(Center::Pet),
        _ => None,
    }
}

/// Side plugins of the middle zone. They share the zone but never compete
/// for the center slot.
pub const SIDE_IDS: &[&str] = &["clock", "weather"];
/// All middle-zone plugin ids (side + center).
pub const ALL_IDS: &[&str] = &["clock", "weather", "pet", "levels", "pomodoro"];

/// Generic tap outcomes. The bar executes these; it never learns which
/// plugin produced them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapEffect {
    TogglePlayback,
    PomodoroTap,
    RefreshWeather,
}

/// Visualizer look. Single-tap on the visualizer cycles this
/// (tap never touches playback — the media button owns play/pause).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VizStyle {
    #[default]
    Bars,
    Segments,
    Wave,
}

impl VizStyle {
    fn next(self) -> VizStyle {
        match self {
            VizStyle::Bars => VizStyle::Segments,
            VizStyle::Segments => VizStyle::Wave,
            VizStyle::Wave => VizStyle::Bars,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            VizStyle::Bars => "bars",
            VizStyle::Segments => "segments",
            VizStyle::Wave => "wave",
        }
    }
    fn from_name(s: &str) -> Option<VizStyle> {
        match s {
            "bars" => Some(VizStyle::Bars),
            "segments" | "seg" => Some(VizStyle::Segments),
            "wave" => Some(VizStyle::Wave),
            _ => None,
        }
    }
}
/// One center-slot plugin. The bar grants `(x0, w)`; the plugin owns every
/// pixel and the meaning of a tap.
pub trait CenterPlugin {
    fn center(&self) -> Center;
    fn priority(&self) -> u8;
    fn available(&self) -> bool;
    fn draw(&self, ctx: &Context, theme: &Theme, x0: f64, w: f64);
    fn tap(&self, zone: &mut ZoneState) -> Option<TapEffect>;
}

/// All center plugins with fresh state. The bar never enumerates these.
fn registry(zone: &ZoneState) -> Vec<Box<dyn CenterPlugin>> {
    // VIZ_DEBUG="segments" pins a style for offline `--view levels` previews.
    let viz = std::env::var("VIZ_DEBUG")
        .ok()
        .and_then(|s| VizStyle::from_name(s.trim().to_lowercase().as_str()))
        .unwrap_or(zone.viz);
    vec![
        Box::new(LevelsPlugin {
            feed: read_levels(),
            playing: is_playing(),
            style: viz,
            viz_secs: zone.viz_secs,
            marquee_secs: zone.marquee_secs,
        }),
        Box::new(PomodoroPlugin { feed: read_pomodoro() }),
        Box::new(PetPlugin { force: zone.pet_override() }),
    ]
}

fn plugin_for(c: Center, zone: &ZoneState) -> Box<dyn CenterPlugin> {
    registry(zone)
        .into_iter()
        .find(|p| p.center() == c)
        .expect("registry holds every Center")
}

// ---------------------------------------------------------------------------
// feeds (written by apps, read by plugins — same pattern as weather cache)
// ---------------------------------------------------------------------------

/// Feed pushed by an external helper for the levels visualizer.
#[derive(Debug, Clone)]
pub struct LevelsFeed {
    pub levels: Vec<f32>,
    pub label: String,
    pub age: Duration,
}

/// Pomodoro state pushed by an external timer app (stub until one exists).
#[derive(Debug, Clone)]
pub struct PomodoroFeed {
    pub label: String,
}

fn deck_file(name: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    std::path::PathBuf::from(home).join(".cache/omarchy-touchbar/deck").join(name)
}

fn file_age(path: &std::path::Path) -> Option<Duration> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()?
        .elapsed()
        .ok()
}

/// Latest levels frame from a helper app. `None` when no helper has ever
/// written, or the frame is stale (>2s).
pub fn read_levels() -> Option<LevelsFeed> {
    let path = deck_file("levels.json");
    let age = file_age(&path)?;
    if age > Duration::from_secs(2) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let levels = v
        .get("levels")
        .and_then(|l| l.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|n| n.as_f64().map(|f| f.clamp(0.0, 1.0) as f32))
                .take(48)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if levels.is_empty() {
        return None;
    }
    Some(LevelsFeed {
        levels,
        label: v
            .get("label")
            .and_then(|l| l.as_str())
            .unwrap_or("music")
            .to_string(),
        age,
    })
}

/// Pomodoro timer state. `None` = no timer app active → slot stays hidden.
pub fn read_pomodoro() -> Option<PomodoroFeed> {
    let text = std::fs::read_to_string(deck_file("pomodoro.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    if v.get("active").and_then(|a| a.as_bool()) != Some(true) {
        return None;
    }
    Some(PomodoroFeed {
        label: v
            .get("label")
            .and_then(|l| l.as_str())
            .unwrap_or("25:00")
            .to_string(),
    })
}

pub fn is_playing() -> bool {
    // Cached: the live loop polls this at ~30fps while the visualizer is
    // up, and each uncached call forks `busctl` twice. A 1s TTL keeps
    // play/pause transitions snappy without the fork storm.
    static CACHE: std::sync::OnceLock<std::sync::Mutex<(bool, Option<Instant>)>> =
        std::sync::OnceLock::new();
    let lock = CACHE.get_or_init(|| std::sync::Mutex::new((false, None)));
    if let Ok(guard) = lock.lock() {
        if let Some(t) = guard.1 {
            if t.elapsed() < Duration::from_secs(1) {
                return guard.0;
            }
        }
    }
    let v = actions::mpris_status().as_deref() == Some("Playing");
    if let Ok(mut guard) = lock.lock() {
        *guard = (v, Some(Instant::now()));
    }
    v
}

/// Cached now-playing (title, artist), 2s TTL — a metadata miss forks
/// busctl twice, and the visualizer draws at ~30fps.
pub fn track_info() -> Option<(String, String)> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<(Option<(String, String)>, Option<Instant>)>,
    > = std::sync::OnceLock::new();
    let lock = CACHE.get_or_init(|| std::sync::Mutex::new((None, None)));
    if let Ok(guard) = lock.lock() {
        if let Some(t) = guard.1 {
            if t.elapsed() < Duration::from_secs(2) {
                return guard.0.clone();
            }
        }
    }
    let v = actions::mpris_metadata();
    if let Ok(mut guard) = lock.lock() {
        *guard = (v.clone(), Some(Instant::now()));
    }
    v
}
/// Wall clock with millisecond precision (libc::time() is 1s-granular —
/// too steppy for the visualizer idle wave / smoothing).
fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Lerp two theme colors.
fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

/// VU-meter color for a bar value: green (quiet) → accent (mid) →
/// red (loud). Theme-aware, "slightly colourful" without going rainbow.
fn level_color(theme: &Theme, v: f64) -> Rgb {
    let v = v.clamp(0.0, 1.0);
    if v < 0.5 {
        mix(theme.green, theme.accent, v * 2.0)
    } else {
        mix(theme.accent, theme.red, (v - 0.5) * 2.0)
    }
}

/// Center plugins currently available, in priority order.
pub fn available() -> Vec<Center> {
    let zone = ZoneState::default();
    let mut plugins = registry(&zone);
    plugins.sort_by_key(|p| std::cmp::Reverse(p.priority()));
    plugins
        .into_iter()
        .filter(|p| p.available())
        .map(|p| p.center())
        .collect()
}

/// Which center plugin to show. A double-tap pin wins while it is still
/// available; otherwise the highest-priority available plugin shows.
pub fn select(pinned: Option<Center>) -> Center {
    let avail = available();
    if let Some(p) = pinned {
        if avail.contains(&p) {
            return p;
        }
    }
    avail.into_iter().next().unwrap_or(Center::Pet)
}

// ---------------------------------------------------------------------------
// zone state (lives in the live loop; owned by the plugin layer)
// ---------------------------------------------------------------------------

/// Middle-zone interaction state: double-tap pin/cycle + tap-to-pet mood
/// + visualizer style and rotation timing.
#[derive(Debug)]
pub struct ZoneState {
    pub pinned: Option<Center>,
    pub viz: VizStyle,
    /// Seconds of visualizer per rotation cycle (from `[deck] viz_secs`).
    pub viz_secs: f64,
    /// Seconds of now-playing title per cycle (`0` = title disabled).
    pub marquee_secs: f64,
    last_tap: Option<Instant>,
    pet_until: Option<Instant>,
}

impl Default for ZoneState {
    fn default() -> Self {
        Self {
            pinned: None,
            viz: VizStyle::default(),
            viz_secs: 8.0,
            marquee_secs: 4.0,
            last_tap: None,
            pet_until: None,
        }
    }
}

impl ZoneState {
    /// Returns true when this tap is a double-tap (caller should cycle
    /// instead of firing the primary action).
    pub fn is_double_tap(&mut self) -> bool {
        let now = Instant::now();
        let dbl = matches!(self.last_tap, Some(t) if now.duration_since(t) < Duration::from_millis(350));
        self.last_tap = Some(now);
        dbl
    }

    /// Pin the next available center plugin after the one showing now.
    pub fn cycle(&mut self, showing: Center) {
        let avail = available();
        if avail.len() < 2 {
            return;
        }
        let i = avail.iter().position(|c| *c == showing).unwrap_or(0);
        self.pinned = Some(avail[(i + 1) % avail.len()]);
    }

    /// Tap on the center slot: the visible plugin decides the effect.
    /// `None` = fully handled inside (pet-the-cat, visualizer style).
    pub fn tap_center(&mut self) -> Option<TapEffect> {
        let showing = select(self.pinned);
        plugin_for(showing, self).tap(self)
    }

    /// Advance the visualizer style. Returns the new style name for logging.
    pub fn cycle_viz(&mut self) -> &'static str {
        self.viz = self.viz.next();
        self.viz.name()
    }

    /// Tap on the clock side: no action.
    pub fn tap_clock(&mut self) -> Option<TapEffect> {
        None
    }

    /// Tap on the weather side: refresh now.
    pub fn tap_weather(&mut self) -> Option<TapEffect> {
        Some(TapEffect::RefreshWeather)
    }

    fn poke(&mut self) {
        self.pet_until = Some(Instant::now() + Duration::from_secs(5));
    }

    fn pet_override(&self) -> Option<PetMood> {
        match self.pet_until {
            Some(t) if Instant::now() < t => Some(PetMood::Happy),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// bar entry point: grant bounds, plugins draw
// ---------------------------------------------------------------------------

/// Draw the whole middle zone into the bounds the bar grants via `lay`.
/// `forced` previews one center plugin (offline `--view`); live passes None.
pub fn render_middle(
    ctx: &Context,
    theme: &Theme,
    lay: &Layout,
    zone: &ZoneState,
    forced: Option<Center>,
) {
    if lay.show_clock {
        ClockPlugin.draw_side(ctx, theme, lay.clk_x0);
    }
    if lay.show_clock || lay.show_weather {
        let showing = forced.unwrap_or_else(|| select(zone.pinned));
        let w = lay.pet_end - lay.pet_x0;
        plugin_for(showing, zone).draw(ctx, theme, lay.pet_x0, w);
    }
    if lay.show_weather {
        WeatherPlugin.draw_side(ctx, theme, lay.wth_x0);
    }
}

// ---------------------------------------------------------------------------
// side plugins: clock + weather (small pixel text)
// ---------------------------------------------------------------------------

struct ClockPlugin;
struct WeatherPlugin;

fn now_hhmm() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

/// The weather string drawn (cached temp, "--°" until the first fetch
/// lands). Layout sizes the block from this same string so geometry and
/// drawing always agree.
pub fn weather_shown() -> String {
    let w = actions::weather_read();
    if w.is_empty() {
        "--°".to_string()
    } else {
        w
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
    let top = (LH - 5.0 * TXT_CELL) / 2.0;
    for ch in s.chars() {
        let Some(g) = digit(ch) else { continue };
        let color = if ch == ':' || ch == '°' { theme.accent } else { theme.fg };
        set_rgb(ctx, color);
        for r in 0..5 {
            for c in 0..3 {
                if g[r * 3 + c] == 1 {
                    ctx.rectangle(
                        x + c as f64 * TXT_CELL,
                        top + r as f64 * TXT_CELL,
                        TXT_CELL - 1.0,
                        TXT_CELL - 1.0,
                    );
                }
            }
        }
        ctx.fill().ok();
        x += 3.0 * TXT_CELL + TXT_GAP;
    }
}

impl ClockPlugin {
    fn draw_side(&self, ctx: &Context, theme: &Theme, x0: f64) {
        pixel_text(ctx, theme, x0, &now_hhmm());
    }
}

impl WeatherPlugin {
    fn draw_side(&self, ctx: &Context, theme: &Theme, x0: f64) {
        // pixel temperature ("21°C"); "--°" until first fetch lands
        pixel_text(ctx, theme, x0, &weather_shown());
    }
}

// ---------------------------------------------------------------------------
// center plugin: pet
// ---------------------------------------------------------------------------

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

struct PetPlugin {
    force: Option<PetMood>,
}

impl CenterPlugin for PetPlugin {
    fn center(&self) -> Center {
        Center::Pet
    }
    fn priority(&self) -> u8 {
        0
    }
    fn available(&self) -> bool {
        true
    }
    /// The pixel pet. `x0` = granted slot start, `w` = granted width.
    /// `force` (tap-to-pet) overrides the schedule with Happy.
    fn draw(&self, ctx: &Context, theme: &Theme, x0: f64, w: f64) {
        let cx = x0 + w / 2.0;
        let half = w / 2.0;
        let wall = unsafe { libc::time(std::ptr::null_mut()) } as f64;
        let (mood, s) = match self.force {
            Some(PetMood::Happy) => (PetMood::Happy, wall),
            _ => pet_mood(),
        };
        let sec = s as usize;
        let px0 = cx - 2.5 * PX_CELL;
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
                pet_cells(ctx, px0, y0, PX_CELL, grid, 5, theme.accent, 1.0);
            }
            PetMood::Dance => {
                // big side-step hops across the playground
                let hop = sec % 2 == 1;
                let dx = if hop { 10.0 } else { -10.0 };
                let dy = if hop { -6.0 } else { 0.0 };
                pet_cells(ctx, px0 + dx, y0 + dy, PX_CELL, &PET_SHUT, 5, theme.accent, 1.0);
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
                    px0 + dx,
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
                pet_cells(ctx, px0, y0 + bob, PX_CELL, grid, 5, theme.accent, 1.0);
                ctx.set_source_rgba(theme.dim.0, theme.dim.1, theme.dim.2, 1.0);
                ctx.rectangle(px0, y0 + bob + 4.0 * PX_CELL, 5.0 * PX_CELL - 1.0, PX_CELL - 1.0);
                ctx.fill().ok();
            }
            PetMood::Sleep => {
                // breathing (slow bob) + two Z's drifting through the margin
                let bob = (sec % 2) as f64 * 1.0;
                pet_cells(ctx, px0, y0 + 8.0 + bob, PX_CELL, &PET_SHUT, 5, theme.accent, 0.85);
                let p = s / 10.0;
                for i in 0..2 {
                    let lp = p * 2.0 - i as f64;
                    if (0.0..1.0).contains(&lp) {
                        pet_cells(
                            ctx,
                            px0 + 46.0 + 8.0 * lp,
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
                pet_cells(ctx, px0, y0 + jump, PX_CELL, &PET_SMILE, 5, theme.accent, 1.0);
                // blush cheeks
                ctx.set_source_rgba(theme.red.0, theme.red.1, theme.red.2, 0.9);
                ctx.rectangle(px0, y0 + jump + 2.0 * PX_CELL, PX_CELL - 1.0, PX_CELL - 1.0);
                ctx.rectangle(
                    px0 + 4.0 * PX_CELL,
                    y0 + jump + 2.0 * PX_CELL,
                    PX_CELL - 1.0,
                    PX_CELL - 1.0,
                );
                ctx.fill().ok();
                for i in 0..3 {
                    let ph = ((wall + i as f64 * 1.7) % 5.0) / 5.0;
                    pet_cells(
                        ctx,
                        px0 + 4.0 + i as f64 * 14.0 + (wall * 2.0 + i as f64).sin() * 3.0,
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
    fn tap(&self, zone: &mut ZoneState) -> Option<TapEffect> {
        zone.poke();
        println!("pet: petted!");
        None
    }
}

// ---------------------------------------------------------------------------
// center plugin: file-fed audio levels
// ---------------------------------------------------------------------------

struct LevelsPlugin {
    feed: Option<LevelsFeed>,
    playing: bool,
    style: VizStyle,
    viz_secs: f64,
    marquee_secs: f64,
}

impl LevelsPlugin {
    /// Resample the feed (any length) to `n` normalized 0..1 values with
    /// linear interpolation, applying stale decay. Falls back to the idle
    /// wave when playing with no helper attached.
    fn values(&self, n: usize, wall: f64) -> Vec<f32> {
        if let Some(f) = &self.feed {
            if !f.levels.is_empty() {
                // decay stale frames so a dead helper settles instead of freezing
                let decay =
                    (1.0 - f.age.as_secs_f32().clamp(0.0, 2.0) / 2.0 * 0.5).clamp(0.5, 1.0);
                let m = f.levels.len();
                return (0..n)
                    .map(|i| {
                        if m == 1 {
                            return (f.levels[0] * decay).clamp(0.0, 1.0);
                        }
                        let pos = i as f32 * (m - 1) as f32 / (n - 1).max(1) as f32;
                        let lo = pos.floor() as usize;
                        let hi = (lo + 1).min(m - 1);
                        let t = pos - lo as f32;
                        ((f.levels[lo] * (1.0 - t) + f.levels[hi] * t) * decay).clamp(0.0, 1.0)
                    })
                    .collect();
            }
        }
        // idle wave while playing with no helper attached
        // (now_secs is ms-precise; libc::time() stepped at 1Hz)
        (0..n)
            .map(|i| (0.32 + 0.26 * ((wall * 3.0 + i as f64 * 0.7).sin() * 0.5 + 0.5)) as f32)
            .collect()
    }
}

/// LED-segment color by height zone: green (low) → amber (mid) → red
/// (hot). Discrete zones like a hardware meter: the color you see
/// depends on how high the amplitude climbs.
fn seg_color(theme: &Theme, t: f64) -> Rgb {
    if t < 0.55 {
        theme.green
    } else if t < 0.8 {
        mix(theme.green, theme.red, 0.6)
    } else {
        theme.red
    }
}

impl LevelsPlugin {
    /// Rotation state: which title the current cycle belongs to and when
    /// its viz phase started. Returns (show_marquee, marquee_elapsed).
    /// The title phase always lasts at least one full scroll pass
    /// (`(slot + text) / SPEED`), however long the title is — the cycle
    /// never cuts a title mid-scroll. A new track restarts the cycle at
    /// its title, so you read the new song immediately.
    fn rotation(&self, text: &str, tw: f64, w: f64) -> (bool, f64) {
        static ROT: std::sync::OnceLock<std::sync::Mutex<(String, Option<Instant>)>> =
            std::sync::OnceLock::new();
        let lock = ROT.get_or_init(|| std::sync::Mutex::new((String::new(), None)));
        let Ok(mut g) = lock.lock() else {
            return (true, 0.0);
        };
        let now_i = Instant::now();
        let viz = self.viz_secs.max(0.0);
        if g.0 != text {
            g.0 = text.to_string();
            // pretend the viz phase already elapsed: title shows first
            g.1 = now_i
                .checked_sub(Duration::from_secs_f64(viz))
                .or(Some(now_i));
        }
        let start = g.1.unwrap_or(now_i);
        let pass = (w + tw) / MARQ_SPEED; // one full enter→exit scroll
        let marquee_len = self.marquee_secs.max(0.0).max(pass);
        let total = viz + marquee_len;
        let mut elapsed = now_i.duration_since(start).as_secs_f64();
        if total > 0.0 && elapsed >= total {
            g.1 = Some(now_i);
            elapsed = 0.0;
        }
        (elapsed >= viz, (elapsed - viz).max(0.0))
    }

    /// Scrolling title, clipped to the granted slot. Short titles sit
    /// centered; long ones marquee in from the right at MARQ_SPEED, with
    /// an empty breather before wrapping (wrap lands on empty, no pop).
    fn draw_marquee(
        &self,
        ctx: &Context,
        theme: &Theme,
        x0: f64,
        w: f64,
        text: &str,
        tw: f64,
        th: f64,
        tyb: f64,
        t_m: f64,
        scroll: bool,
    ) {
        use cairo::{FontSlant, FontWeight};
        let _ = ctx.save();
        ctx.rectangle(x0, 0.0, w, LH);
        ctx.clip();
        ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
        ctx.set_font_size(20.0);
        let y = LH / 2.0 - (th / 2.0 + tyb);
        set_rgb(ctx, theme.fg);
        if !scroll {
            ctx.move_to(x0 + (w - tw) / 2.0, y);
        } else {
            let off = (t_m * MARQ_SPEED) % (w + tw + MARQ_BREATHE);
            ctx.move_to(x0 + w - off, y);
        }
        ctx.show_text(text).ok();
        let _ = ctx.restore();
    }
    /// "Title — Artist", title/artist alone, or the helper feed label.
    /// `None` = nothing to show (stay on the visualizer).
    fn marquee_text(&self) -> Option<String> {
        if let Some((t, a)) = track_info() {
            let (t, a) = (t.trim(), a.trim());
            if !t.is_empty() && !a.is_empty() {
                return Some(format!("{t} — {a}"));
            } else if !t.is_empty() {
                return Some(t.to_string());
            } else if !a.is_empty() {
                return Some(a.to_string());
            }
        }
        let label = self
            .feed
            .as_ref()
            .map(|f| f.label.trim().to_string())
            .unwrap_or_default();
        if label.is_empty() {
            None
        } else {
            Some(label)
        }
    }

}

/// Marquee scroll: px per second, and empty px before wrapping.
const MARQ_SPEED: f64 = 45.0;
const MARQ_BREATHE: f64 = 80.0;

impl CenterPlugin for LevelsPlugin {
    fn center(&self) -> Center {
        Center::Levels
    }
    fn priority(&self) -> u8 {
        2
    }
    fn available(&self) -> bool {
        self.playing
    }
    /// Audio visualizer, rotating with the now-playing title: `viz_secs`
    /// of bars per cycle, then `marquee_secs` of scrolling title
    /// (`marquee_secs = 0` disables the title). The bar never captures
    /// audio: a helper app pushes normalized levels; here they are just
    /// drawn, decaying stale frames.
    /// Falls back to a gentle idle wave when playing with no fresh feed.
    fn draw(&self, ctx: &Context, theme: &Theme, x0: f64, w: f64) {
        // now-playing rotation (MARQUEE_DEBUG pins a phase for previews).
        // The title phase always runs one full scroll pass; a zero
        // marquee_secs (or no title) means visualizer only.
        let force = std::env::var("MARQUEE_DEBUG")
            .ok()
            .map(|s| s.trim().to_lowercase())
            .unwrap_or_default();
        if force != "viz" && self.marquee_secs > 0.0 {
            if let Some(text) = self.marquee_text() {
                use cairo::{FontSlant, FontWeight};
                ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
                ctx.set_font_size(20.0);
                let (tw, th, tyb) = ctx
                    .text_extents(&text)
                    .map(|e| (e.width(), e.height(), e.y_bearing()))
                    .unwrap_or((0.0, 0.0, 0.0));
                let scroll = tw > w;
                let (show, t_m) = if force == "marquee" {
                    (true, now_secs())
                } else {
                    self.rotation(&text, tw, w)
                };
                if show {
                    self.draw_marquee(ctx, theme, x0, w, &text, tw, th, tyb, t_m, scroll);
                    return;
                }
            }
        }
        let wall = now_secs();
        let n = 24;
        let gap = 4.0;
        let bw = ((w - (n as f64 - 1.0) * gap) / n as f64).clamp(3.0, 8.0);
        let total = n as f64 * bw + (n as f64 - 1.0) * gap;
        let x_start = x0 + (w - total) / 2.0;
        // bottom-aligned meter: baseline near the strip edge so bars
        // grow upward like a real visualizer.
        let baseline = LH - 8.0;
        let max_h = LH - 18.0;
        let vals = self.values(n, wall);
        match self.style {
            VizStyle::Bars => {
                let mut x = x_start;
                for v in &vals {
                    let h = (4.0 + *v as f64 * max_h).clamp(4.0, max_h + 4.0);
                    let y = baseline - h;
                    let c = level_color(theme, *v as f64);
                    rounded(ctx, x, y, bw, h, (bw / 2.0).min(2.5));
                    ctx.set_source_rgba(c.0, c.1, c.2, 0.95);
                    ctx.fill().ok();
                    // peak cap: bright 2px tip so peaks read at a glance
                    rounded(ctx, x, y, bw, 2.5, 1.2);
                    ctx.set_source_rgba(
                        (c.0 + 0.35).min(1.0),
                        (c.1 + 0.35).min(1.0),
                        (c.2 + 0.35).min(1.0),
                        0.95,
                    );
                    ctx.fill().ok();
                    x += bw + gap;
                }
            }
            VizStyle::Segments => {
                // hardware LED meter: 10 chunky bars, each a stack of 8
                // segments that light green → amber → red as the amplitude
                // climbs, with peak-hold caps on top.
                let n = 10;
                let gap = 6.0;
                let bw = ((w - (n as f64 - 1.0) * gap) / n as f64).clamp(8.0, 20.0);
                let total = n as f64 * bw + (n as f64 - 1.0) * gap;
                let mut x = x0 + (w - total) / 2.0;
                let baseline = LH - 8.0;
                let max_h = LH - 18.0;
                let nsegs = 8usize;
                let sgap = 2.0;
                let sh = (max_h - (nsegs as f64 - 1.0) * sgap) / nsegs as f64;
                let vals = self.values(n, wall);
                // peak-hold caps: per-bar recent max, falling slowly.
                // (static: plugins are rebuilt every frame, so the hold
                // lives here — same pattern as the MPRIS cache above.)
                static PEAKS: std::sync::OnceLock<std::sync::Mutex<(Vec<f32>, Option<Instant>)>> =
                    std::sync::OnceLock::new();
                let peaks = PEAKS
                    .get_or_init(|| std::sync::Mutex::new((vec![0.0; 10], None)))
                    .lock()
                    .map(|mut g| {
                        let now = Instant::now();
                        let dt = g
                            .1
                            .map(|t| now.duration_since(t).as_secs_f32())
                            .unwrap_or(0.0)
                            .clamp(0.0, 1.0);
                        g.1 = Some(now);
                        if g.0.len() != n {
                            g.0 = vec![0.0; n];
                        }
                        for i in 0..n {
                            g.0[i] = vals[i].max(g.0[i] - dt * 1.4).clamp(0.0, 1.0);
                        }
                        g.0.clone()
                    })
                    .unwrap_or_else(|_| vals.clone());
                for (i, v) in vals.iter().enumerate() {
                    let lit = (*v as f64 * nsegs as f64).round().clamp(0.0, nsegs as f64) as usize;
                    for s in 0..nsegs {
                        let y = baseline - (s + 1) as f64 * (sh + sgap) + sgap;
                        let c = seg_color(theme, s as f64 / (nsegs - 1) as f64);
                        rounded(ctx, x, y, bw, sh, 1.2);
                        if s < lit {
                            ctx.set_source_rgba(c.0, c.1, c.2, 0.95);
                        } else {
                            ctx.set_source_rgba(c.0, c.1, c.2, 0.16);
                        }
                        ctx.fill().ok();
                    }
                    let ph = (peaks[i] as f64 * max_h).clamp(0.0, max_h);
                    if ph > 1.0 {
                        let c = seg_color(theme, peaks[i] as f64);
                        rounded(ctx, x, baseline - ph - 2.0, bw, 2.5, 1.2);
                        ctx.set_source_rgba(c.0, c.1, c.2, 1.0);
                        ctx.fill().ok();
                    }
                    x += bw + gap;
                }
            }
            VizStyle::Wave => {
                // smooth waveform polyline through the levels + soft fill
                let step = if n > 1 { total / (n - 1) as f64 } else { total };
                let pts: Vec<(f64, f64)> = vals
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let x = x_start + i as f64 * step;
                        let y = baseline - (4.0 + *v as f64 * max_h).clamp(4.0, max_h + 4.0);
                        (x, y)
                    })
                    .collect();
                if let Some(&(fx, fy)) = pts.first() {
                    // fill under the curve
                    ctx.new_sub_path();
                    ctx.move_to(fx, baseline);
                    ctx.line_to(fx, fy);
                    for &(x, y) in &pts[1..] {
                        ctx.line_to(x, y);
                    }
                    if let Some(&(lx, _)) = pts.last() {
                        ctx.line_to(lx, baseline);
                        ctx.close_path();
                    }
                    ctx.set_source_rgba(theme.accent.0, theme.accent.1, theme.accent.2, 0.25);
                    ctx.fill().ok();
                    // the wave line itself
                    ctx.new_sub_path();
                    ctx.move_to(fx, fy);
                    for &(x, y) in &pts[1..] {
                        ctx.line_to(x, y);
                    }
                    ctx.set_source_rgba(theme.accent.0, theme.accent.1, theme.accent.2, 1.0);
                    ctx.set_line_width(3.0);
                    ctx.set_line_cap(cairo::LineCap::Round);
                    ctx.set_line_join(cairo::LineJoin::Round);
                    ctx.stroke().ok();
                    // dots on each sample, VU-colored
                    for (i, &(x, y)) in pts.iter().enumerate() {
                        let c = level_color(theme, vals[i] as f64);
                        ctx.arc(x, y, 2.5, 0.0, 2.0 * std::f64::consts::PI);
                        ctx.set_source_rgba(c.0, c.1, c.2, 1.0);
                        ctx.fill().ok();
                    }
                }
            }
        }
    }
    /// Tap cycles the visualizer style. Playback is owned by the media
    /// button — a tap here never plays/pauses.
    fn tap(&self, zone: &mut ZoneState) -> Option<TapEffect> {
        println!("viz: style -> {}", zone.cycle_viz());
        None
    }
}

// ---------------------------------------------------------------------------
// center plugin: pomodoro stub
// ---------------------------------------------------------------------------

struct PomodoroPlugin {
    feed: Option<PomodoroFeed>,
}

impl CenterPlugin for PomodoroPlugin {
    fn center(&self) -> Center {
        Center::Pomodoro
    }
    fn priority(&self) -> u8 {
        1
    }
    fn available(&self) -> bool {
        self.feed.is_some()
    }
    /// Renders the label pushed by a timer app.
    fn draw(&self, ctx: &Context, theme: &Theme, x0: f64, w: f64) {
        let label = self
            .feed
            .as_ref()
            .map(|p| p.label.as_str())
            .unwrap_or("--:--");
        text_centered(ctx, x0 + w / 2.0, LH / 2.0, label, theme.fg, "Sans", 22.0);
    }
    fn tap(&self, _zone: &mut ZoneState) -> Option<TapEffect> {
        Some(TapEffect::PomodoroTap)
    }
}
