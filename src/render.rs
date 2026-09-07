//! Cairo rendering (port of render_ui in touchbar_daemon.py).
//!
//! Logical space is 2008x60; the surface is 64x2008 with tiny-dfr's
//! translate(60,0)+rotate90 transform.

use cairo::{Context, FontSlant, FontWeight, Format, ImageSurface};
use std::f64::consts::PI;

use crate::actions;
use crate::config::{Button, Config};
use crate::hypr;
use crate::icons::IconCache;
use crate::deck;
use crate::saver::Saver;
use crate::theme::{Rgb, Theme};

pub const W: i32 = 64;
pub const H: i32 = 2008;
pub const LW: f64 = 2008.0;
pub const LH: f64 = 60.0;

const EDGE: f64 = 20.0;
const WS_GAP: f64 = 10.0; // uniform inner gap shared by ALL buttons
const SEC_GAP: f64 = 12.0; // gaps between strip sections
const MENU_GAP: f64 = 12.0;
const FN_N: usize = 12;
const FN_W: f64 = 158.0;
const FN_GAP: f64 = 9.0;

const CLK_W: f64 = 5.0 * 3.0 * deck::TXT_CELL + 4.0 * deck::TXT_GAP; // ~133, full "HH:MM"
// pet playground after the clock; weather sits past it, next to brightness
const PET_W: f64 = 200.0;
// weather block is sized tight to the actual text (see wth_width) so a
// short reading like "56°F" leaves no trailing gap before the controls

/// Pixel width of a weather/clock string: one 3-wide cell per glyph plus
/// the inter-glyph gap. Callers pass the exact string being drawn so the
/// block hugs the text with no trailing gap.
pub fn pixel_width(n_glyphs: usize) -> f64 {
    let n = n_glyphs.max(1) as f64;
    n * 3.0 * deck::TXT_CELL + (n - 1.0) * deck::TXT_GAP
}

/// Width of the weather block for the string about to be drawn.
fn wth_width(s: &str) -> f64 {
    pixel_width(s.chars().count())
}

pub const SL_TX0: f64 = 300.0;
pub const SL_TX1: f64 = LW - 140.0;

const NERD: &str = "JetBrainsMono Nerd Font";
const NEUTRAL_BG: Rgb = (0x18 as f64 / 255.0, 0x18 as f64 / 255.0, 0x20 as f64 / 255.0);

/// Shared layout: render and hit-testing both use this.
/// Every button (workspaces, controls, theme) shares one uniform width.
#[derive(Debug, Clone)]
pub struct Layout {
    pub ws_n: usize,
    pub ws: Vec<(f64, f64)>, // (x, w) per workspace button
    pub ws_end: f64,
    pub btn_w: f64, // the uniform button width
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
    pub th_w: f64,
    pub show_theme: bool,
}

pub fn layout(cfg: &Config) -> Layout {
    // Geometry sizes the weather block from the same string the weather
    // plugin draws, so they always agree (see deck::weather_shown).
    layout_with_weather(cfg, &deck::weather_shown())
}

/// Same as layout(), but sizes the weather block for an explicit string.
/// Callers that already hold the string render() will draw should prefer
/// this so geometry and drawing agree exactly.
pub fn layout_with_weather(cfg: &Config, weather: &str) -> Layout {
    let ws_n = cfg.bar.workspaces.max(1).min(9);
    let n_ctl = cfg.button.len();
    let show_clock = cfg.bar.show_clock;
    let show_weather = cfg.bar.show_weather;
    let show_theme = cfg.bar.show_theme;
    // weather hugs its text: no trailing gap before the controls
    let wth_w = if show_weather { wth_width(weather) } else { 0.0 };
    // one width for every button on the strip
    let n_btns = (ws_n + n_ctl + if show_theme { 1 } else { 0 }).max(1) as f64;
    let inner_gaps = ((ws_n as f64 - 1.0).max(0.0) + (n_ctl as f64 - 1.0).max(0.0)) * WS_GAP;
    let fixed = if show_clock { CLK_W } else { 0.0 } + PET_W + wth_w;
    let n_sec = 2.0 + if show_clock { 1.0 } else { 0.0 } + if show_weather { 1.0 } else { 0.0 }
        + if show_theme { 1.0 } else { 0.0 };
    let btn_w = ((LW - 2.0 * EDGE - fixed - n_sec * SEC_GAP - inner_gaps) / n_btns).clamp(48.0, 160.0);
    let ws = (0..ws_n)
        .map(|k| (EDGE + k as f64 * (btn_w + WS_GAP), btn_w))
        .collect::<Vec<_>>();
    let ws_end = EDGE + ws_n as f64 * btn_w + (ws_n as f64 - 1.0).max(0.0) * WS_GAP;
    let clk_x0 = ws_end + SEC_GAP;
    let clk_end = if show_clock { clk_x0 + CLK_W } else { ws_end };
    let pet_base = if show_clock { clk_end } else { ws_end };
    let pet_x0 = pet_base + SEC_GAP;
    let pet_end = pet_x0 + PET_W;
    let wth_x0 = pet_end + SEC_GAP;
    let wth_end = if show_weather { wth_x0 + wth_w } else { pet_end };
    let ctl_start = wth_end + SEC_GAP;
    let ctl = (0..n_ctl)
        .map(|k| (ctl_start + k as f64 * (btn_w + WS_GAP), btn_w))
        .collect();
    let ctl_end = if n_ctl > 0 {
        ctl_start + n_ctl as f64 * btn_w + (n_ctl as f64 - 1.0) * WS_GAP
    } else {
        ctl_start - SEC_GAP
    };
    let th_w = btn_w;
    let th1_x = LW - EDGE - th_w;
    // if buttons overflow into the theme slot (extreme configs), theme wins:
    // widths already clamped to min, strip just runs edge to edge.
    let _ = ctl_end;
    Layout {
        ws_n,
        ws,
        ws_end,
        btn_w,
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
        th_w,
        show_theme,
    }
}

pub fn menu_geometry(n: usize) -> (f64, f64) {
    let n = n.max(1) as f64;
    let w = (LW - (n - 1.0) * MENU_GAP) / n;
    ((LW - (n * w + (n - 1.0) * MENU_GAP)) / 2.0, w)
}

/// Geometry for the theme-picker and app-launcher overlays. Both always
/// share one width: expanded to fill the bar by default, or the fixed
/// strip-button width (centered) when `menu_expand = false`.
pub fn menu_geometry_cfg(n: usize, cfg: &Config, btn_w: f64) -> (f64, f64) {
    if cfg.bar.menu_expand {
        return menu_geometry(n);
    }
    let n = n.max(1) as f64;
    let w = btn_w.clamp(48.0, 200.0);
    let total = n * w + (n - 1.0) * MENU_GAP;
    (((LW - total) / 2.0).max(EDGE), w)
}

pub(crate) fn set_rgb(ctx: &Context, c: Rgb) {
    ctx.set_source_rgb(c.0, c.1, c.2);
}

pub(crate) fn rounded(ctx: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    ctx.new_sub_path();
    ctx.arc(x + r, y + r, r, PI, 1.5 * PI);
    ctx.arc(x + w - r, y + r, r, 1.5 * PI, 0.0);
    ctx.arc(x + w - r, y + h - r, r, 0.0, 0.5 * PI);
    ctx.arc(x + r, y + h - r, r, 0.5 * PI, PI);
    ctx.close_path();
}

pub(crate) fn text_centered(
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

// Middle-zone (center deck) drawing lives in `deck.rs`: plugins own their
// pixels, the bar only grants bounds via `deck::render_middle()`.

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
        ("calendar", NERD, ""), // calendar
        ("contacts", NERD, ""), // contacts
        ("maps", NERD, ""), // maps pin
        ("settings", NERD, ""), // settings
        ("github", NERD, ""),      // \uf09b
        ("zoom", NERD, ""),        // \uf03d
        ("gmail", NERD, ""), // mail
        ("mail", NERD, ""), // mail
        ("drive", NERD, ""), // drive
        ("meet", NERD, ""), // video call
        ("docs", NERD, ""), // docs
        ("google", NERD, ""),      // \uf1a0 Maps/Photos/...
        ("photos", NERD, ""), // photos
        ("messages", NERD, ""), // messages
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
        return Some((NERD, ""));
    }
    if class == "org.omarchy.agent" {
        return Some(("Sans", "▲"));
    }
    let low = class.to_lowercase();
    // common desktop apps by class keyword (order matters: specific first).
    // Nautilus (Files) reports class "org.gnome.Nautilus".
    const CLASS_TABLE: &[(&str, &str, &str)] = &[
        ("nautilus", NERD, "\u{F07B}"),       // files
        ("thunar", NERD, "\u{F07B}"),         // files
        ("dolphin", NERD, "\u{F07B}"),        // files
        ("nemo", NERD, "\u{F07B}"),           // files
        ("alacritty", NERD, "\u{F489}"),      // terminal
        ("kitty", NERD, "\u{F489}"),          // terminal
        ("ghostty", NERD, "\u{F489}"),        // terminal
        ("wezterm", NERD, "\u{F489}"),        // terminal
        ("gnome-terminal", NERD, "\u{F489}"), // terminal
        ("ptyxis", NERD, "\u{F489}"),         // terminal
        ("konsole", NERD, "\u{F489}"),        // terminal
        ("xterm", NERD, "\u{F489}"),          // terminal
        ("vscodium", NERD, "\u{E70C}"),       // vscode
        ("code", NERD, "\u{E70C}"),           // vscode
        ("neovim", NERD, "\u{E62B}"),         // vim
        ("nvim", NERD, "\u{E62B}"),           // vim
        ("vim", NERD, "\u{E62B}"),            // vim
        ("emacs", NERD, "\u{E632}"),          // emacs
        ("firefox", NERD, "\u{F269}"),        // browser
        ("brave", NERD, "\u{E639}"),          // browser
        ("chromium", NERD, "\u{F268}"),       // browser
        ("obsidian", NERD, "\u{E63A}"),       // notes
        ("spotify", NERD, "\u{F1BC}"),        // music
        ("discord", NERD, "\u{F066F}"),       // chat (mdi)
        ("telegram", NERD, "\u{F2C6}"),       // chat
        ("slack", NERD, "\u{F198}"),          // chat
        ("whatsapp", NERD, "\u{F232}"),       // chat
        ("zoom", NERD, "\u{F03D}"),           // meeting
        ("vlc", NERD, "\u{F07C4}"),           // video (mdi)
        ("gimp", NERD, "\u{F1C5}"),           // image
        ("steam", NERD, "\u{F1B6}"),          // games
        ("calendar", NERD, ""), // calendar
        ("contacts", NERD, ""), // contacts
        ("maps", NERD, ""), // maps pin
        ("settings", NERD, ""), // settings
        ("localsend", NERD, ""),       // send
        ("github", NERD, "\u{F09B}"),         // dev
    ];
    for (key, font, g) in CLASS_TABLE {
        if low.contains(key) {
            return Some((*font, *g));
        }
    }
    // zen browser (kept out of the table: bare "zen" would also match zenity)
    if low == "zen" || low.contains("zen-browser") {
        return Some((NERD, ""));
    }
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
            ("calendar", NERD, ""), // calendar
            ("contacts", NERD, ""), // contacts
            ("maps", NERD, ""), // maps pin
            ("settings", NERD, ""), // settings
            ("github", NERD, ""),
            ("zoom", NERD, ""),
            ("photos", NERD, ""), // photos
            ("messages", NERD, ""), // messages
            ("gmail", NERD, ""), // mail
            ("mail", NERD, ""), // mail
            ("drive", NERD, ""), // drive
            ("meet", NERD, ""), // video call
            ("docs", NERD, ""), // docs
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

/// One app slot on a workspace button: a real theme icon when the
/// .desktop lookup succeeds, otherwise the nerd-font fallback glyph.
#[derive(Clone)]
pub enum WsIcon {
    Glyph(&'static str, &'static str),
    Image(cairo::ImageSurface),
}

fn ws_cell_width(ctx: &Context, icon: &WsIcon, glyph_size: f64, img_box: f64) -> f64 {
    match icon {
        WsIcon::Glyph(font, g) => {
            ctx.select_font_face(font, FontSlant::Normal, FontWeight::Bold);
            ctx.set_font_size(glyph_size);
            ctx.text_extents(g).map(|e| e.width()).unwrap_or(0.0)
        }
        WsIcon::Image(s) => {
            let (w, h) = (s.width() as f64, s.height() as f64);
            if w <= 0.0 || h <= 0.0 {
                img_box
            } else {
                (img_box * w / h).clamp(10.0, img_box * 1.6)
            }
        }
    }
}

fn ws_cell_widths(ctx: &Context, icons: &[WsIcon], glyph_size: f64, img_box: f64) -> Vec<f64> {
    icons
        .iter()
        .map(|i| ws_cell_width(ctx, i, glyph_size, img_box))
        .collect()
}

fn ws_icon_widths(ctx: &Context, icons: &[(&str, &str)], size: f64) -> Vec<f64> {
    icons
        .iter()
        .map(|(font, g)| {
            ctx.select_font_face(font, FontSlant::Normal, FontWeight::Bold);
            ctx.set_font_size(size);
            ctx.text_extents(g).map(|e| e.width()).unwrap_or(0.0)
        })
        .collect()
}

fn ws_draw_cell(
    ctx: &Context,
    theme: &Theme,
    icon: &WsIcon,
    gx: f64,
    center_y: f64,
    glyph_size: f64,
    img_box: f64,
) {
    match icon {
        WsIcon::Glyph(font, g) => ws_draw_icon(ctx, theme, gx, center_y, font, g, glyph_size),
        WsIcon::Image(s) => {
            let (w, h) = (s.width() as f64, s.height() as f64);
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let sc = img_box / h;
            let _ = ctx.save();
            ctx.translate(gx, center_y - img_box / 2.0);
            ctx.scale(sc, sc);
            ctx.set_source_surface(s, 0.0, 0.0).ok();
            ctx.paint().ok();
            let _ = ctx.restore();
        }
    }
}

fn ws_draw_icon(
    ctx: &Context,
    theme: &Theme,
    gx: f64,
    center_y: f64,
    font: &str,
    g: &str,
    size: f64,
) {
    ctx.select_font_face(font, FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(size);
    let ge = ctx.text_extents(g).expect("extents");
    ctx.move_to(
        gx - ge.x_bearing(),
        center_y - (ge.height() / 2.0 + ge.y_bearing()),
    );
    set_rgb(ctx, theme.fg);
    ctx.show_text(g).ok();
}

fn ws_button(
    ctx: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    theme: &Theme,
    num: usize,
    glyphs: &[WsIcon],
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
    // the number is always left-aligned; app icons flow next to it.
    // up to 4 icons: the first two sit beside the number, the 3rd/4th
    // drop to a bottom row so nothing ever leaves the button.
    const PAD_L: f64 = 10.0;
    const PAD_R: f64 = 8.0;
    ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(27.0);
    let ext = ctx.text_extents(&num.to_string()).expect("extents");
    ctx.move_to(
        x + PAD_L - ext.x_bearing(),
        cy - (ext.height() / 2.0 + ext.y_bearing()),
    );
    set_rgb(ctx, num_color);
    ctx.show_text(&num.to_string()).ok();
    let icons: &[WsIcon] = &glyphs[..glyphs.len().min(4)];
    if icons.is_empty() {
        return;
    }
    // clip icon drawing to the pill so icons can never bleed over neighbors
    let _ = ctx.save();
    rounded(ctx, x, y, w, h, 12.0);
    ctx.clip();
    let ix = x + PAD_L + ext.width() + 8.0;
    let avail = (x + w - PAD_R - ix).max(0.0);
    let edge = x + w - PAD_R + 1.0;
    if icons.len() <= 2 {
        // single row beside the number, vertically centered
        let mut size = 25.0;
        let mut widths = ws_cell_widths(ctx, icons, size, 30.0);
        let mut total = widths.iter().sum::<f64>() + (widths.len() as f64 - 1.0) * 8.0;
        while total > avail && size > 16.0 {
            size -= 1.0;
            widths = ws_cell_widths(ctx, icons, size, 30.0);
            total = widths.iter().sum::<f64>() + (widths.len() as f64 - 1.0) * 8.0;
        }
        let mut gx = ix;
        for (icon, iw) in icons.iter().zip(widths.iter()) {
            if gx + iw > edge {
                break;
            }
            ws_draw_cell(ctx, theme, icon, gx, cy, size, 30.0);
            gx += iw + 8.0;
        }
    } else {
        // 2x2 grid: first pair on top, 3rd/4th on the bottom row
        let (top, bot) = icons.split_at(2);
        let mut size = 18.0;
        let (c0, _c1) = loop {
            let wt = ws_cell_widths(ctx, top, size, 21.0);
            let wb = ws_cell_widths(ctx, bot, size, 21.0);
            let c0 = wt[0].max(*wb.first().unwrap_or(&0.0));
            let c1 = wt[1].max(wb.get(1).copied().unwrap_or(0.0));
            if c0 + 6.0 + c1 <= avail || size <= 13.0 {
                break (c0, c1);
            }
            size -= 1.0;
        };
        let top_cy = y + h * 0.30;
        let bot_cy = y + h * 0.72;
        let wt = ws_cell_widths(ctx, top, size, 21.0);
        let wb = ws_cell_widths(ctx, bot, size, 21.0);
        for (k, (icon, iw)) in top.iter().zip(wt.iter()).enumerate() {
            let gx = if k == 0 { ix } else { ix + c0 + 6.0 };
            if gx + iw > edge {
                continue;
            }
            ws_draw_cell(ctx, theme, icon, gx, top_cy, size, 21.0);
        }
        for (k, (icon, iw)) in bot.iter().zip(wb.iter()).enumerate() {
            let gx = if k == 0 { ix } else { ix + c0 + 6.0 };
            if gx + iw > edge {
                continue;
            }
            ws_draw_cell(ctx, theme, icon, gx, bot_cy, size, 21.0);
        }
    }
    let _ = ctx.restore();
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
    // fit the label inside any button width; clip so long theme names
    // can never bleed over neighbors at small uniform sizes
    let mut size = size;
    ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    loop {
        ctx.set_font_size(size);
        let ext = ctx.text_extents(&short).expect("extents");
        if ext.width() <= w - 16.0 || size <= 10.0 {
            break;
        }
        size -= 1.0;
    }
    let _ = ctx.save();
    rounded(ctx, x, y, w, h, 12.0);
    ctx.clip();
    text_centered(ctx, x + w / 2.0, y + h / 2.0, &short, fg, "Sans", size);
    let _ = ctx.restore();
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
    // measure icon + name as a group so the pair sits centered;
    // shrink the name until it fits inside fixed-width buttons
    ctx.select_font_face(font, FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(26.0);
    let ie = ctx.text_extents(&app.icon).expect("extents");
    ctx.select_font_face("Sans", FontSlant::Normal, FontWeight::Bold);
    ctx.set_font_size(17.0);
    let mut short: String = app.name.chars().take(10).collect();
    let mut ne = ctx.text_extents(&short).expect("extents");
    let mut total = ie.width() + 10.0 + ne.width();
    while total > w - 16.0 && short.len() > 1 {
        short.pop();
        ne = ctx.text_extents(&short).expect("extents");
        total = ie.width() + 10.0 + ne.width();
    }
    let _ = ctx.save();
    rounded(ctx, x, y, w, h, 12.0);
    ctx.clip();
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
    let _ = ctx.restore();
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
/// The bar grants the middle-zone bounds; deck plugins own all drawing.
/// `forced` previews one center plugin (offline `--view`); live passes None.
pub fn render(
    cfg: &Config,
    theme: &Theme,
    lay: &Layout,
    focused: i32,
    favs: &[(String, Rgb)],
    view: View,
    zone: &deck::ZoneState,
    icons: &mut IconCache,
    forced: Option<deck::Center>,
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
                let (x0, bw) = menu_geometry_cfg(items.len(), cfg, lay.btn_w);
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
                let (x0, bw) = menu_geometry_cfg(apps.len(), cfg, lay.btn_w);
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
                let mut ws_apps: std::collections::HashMap<i32, Vec<WsIcon>> =
                    std::collections::HashMap::new();
                for c in &clients {
                    if (1..=lay.ws_n as i32).contains(&c.ws_id) {
                        // real theme icon first (like the Omarchy menu),
                        // nerd-font glyph as fallback
                        if let Some(img) = icons.surface_for_class(&c.class) {
                            ws_apps.entry(c.ws_id).or_default().push(WsIcon::Image(img));
                        } else if let Some(g) = glyph_for_wmclass(&c.class, &c.title) {
                            ws_apps
                                .entry(c.ws_id)
                                .or_default()
                                .push(WsIcon::Glyph(g.0, g.1));
                        }
                    }
                }
                for k in 0..lay.ws_n {
                    let i = k as i32 + 1;
                    let empty: Vec<WsIcon> = vec![];
                    let glyphs = ws_apps.get(&i).unwrap_or(&empty);
                    let (wx, ww) = lay.ws.get(k).copied().unwrap_or((EDGE, lay.btn_w));
                    ws_button(
                        &ctx,
                        wx,
                        by,
                        ww,
                        bh,
                        theme,
                        k + 1,
                        glyphs,
                        i == focused,
                        open.contains(&i),
                    );
                }

                // Center deck: the bar grants bounds, plugins draw.
                deck::render_middle(&ctx, theme, lay, zone, forced);

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
                            // F04B = play glyph, F04C = pause glyph: show
                            // what the tap will do (pause while playing).
                            if playing {
                                "\u{F04C}".into()
                            } else {
                                "\u{F04B}".into()
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
                        lay.th_w,
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
