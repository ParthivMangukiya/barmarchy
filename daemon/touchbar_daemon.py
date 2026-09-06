#!/usr/bin/env python3
"""omarchy-touchbar daemon v0.1 — Omarchy UI on real M1 Touch Bar hardware.

Draws the workspace/app/theme strip with pycairo onto a 64x2008 surface
using tiny-dfr's exact transform (translate(60,0) + rotate90), then pushes
it through the DRM dumb-buffer path (same as poc/drm_fill.py).

Usage: stop tiny-dfr first, then
    ./touchbar_daemon.py --hold 8
"""
import ctypes
import datetime
import fcntl
import json
import mmap
import os
import struct
import subprocess
import sys
import time
import math

import cairo

import tomllib

C = ctypes
CARD = "/dev/dri/card2"
W, H = 64, 2008          # dumb buffer (tiny-dfr uses 64-wide for 60-wide mode)
LW, LH = 2008, 60        # logical strip coordinates
NERD = "JetBrainsMono Nerd Font"
ICONS = {"term": "\uf489", "chrome": "\uf268"}  # verified on-device via glyph-test

# Shared geometry (touchbar_live imports these for hit-testing)
WS_START, WS_W, WS_GAP, WS_N = 24, 140, 12, 5
TH1_X, TH1_W = LW - 24 - 140, 140       # single theme button (normal view)
WS_END = WS_START + WS_N * WS_W + (WS_N - 1) * WS_GAP
CTL_W, CTL_GAP, CTL_N = 130, 16, 6
# CTL_START is computed after the clock geometry (controls sit right of
# the clock); see below.
CTL_ORDER = ("display", "volume", "kbd", "media", "mic", "night")
FN_N, FN_W, FN_GAP = 12, 158, 9
FN_START = (LW - (FN_N * FN_W + (FN_N - 1) * FN_GAP)) // 2
MENU_N = 10                         # preferred cap (all themes if <= 10)
MENU_GAP = 12


def menu_geometry(n):
    """(x0, button_w) for n menu buttons spread across the strip."""
    n = max(1, n)
    w = (LW - (n - 1) * MENU_GAP) / n
    return (LW - (n * w + (n - 1) * MENU_GAP)) / 2, w


def menu_items(favs):
    """All themes when few, else preferred favs (capped)."""
    all_ = theme_list()
    if 0 < len(all_) <= 10:
        out = []
        for n in all_:
            c = colors_for(n)
            out.append((n, _hex(c.get("accent",
                                      c.get("blue", _FALLBACK["accent"])))))
        return out
    return favs[:MENU_N]
NEUTRAL_BG = (0x18 / 255, 0x18 / 255, 0x20 / 255)  # strip stays neutral
FAV_PATH = os.path.expanduser("~/.config/omarchy-touchbar/themes")

CTL_ICONS = {"display": "\uf185", "volume": "\uf028", "kbd": "\uf11c",
             "play": "\uf04b", "pause": "\uf04c",
             "mic": "\uf130", "mic_off": "\uf131", "night": "\uf186"}


def get_mic():
    """True when the microphone is muted."""
    try:
        out = subprocess.check_output(["pactl", "get-source-mute",
                                       "@DEFAULT_SOURCE@"],
                                      timeout=5, text=True)
        return "yes" in out.lower()
    except Exception:
        return False


def toggle_mic():
    subprocess.Popen(["omarchy", "audio", "input", "mute"],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def get_night():
    try:
        import json as _json
        out = subprocess.check_output(["omarchy", "toggle", "nightlight",
                                       "--status"], timeout=8, text=True)
        return bool(_json.loads(out).get("enabled"))
    except Exception:
        return False


def toggle_night():
    subprocess.Popen(["omarchy", "toggle", "nightlight"],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

# 3x5 pixel digits for the ASCII-art clock (screensaver flavor)
DIGITS = {"0": (1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1),
          "1": (0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1),
          "2": (1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1),
          "3": (1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1),
          "4": (1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1),
          "5": (1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1),
          "6": (1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1),
          "7": (1, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0),
          "8": (1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1),
          "9": (1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1),
          ":": (0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0)}
PX_CELL, PX_GAP = 9, 9
CLK_W = 4 * 3 * PX_CELL + 1 * PX_CELL + 4 * PX_GAP  # HH:MM = 153
# Centered button content keeps visual edges near-symmetric; -5px puts
# the glass-measured gaps equal (left read wider at 0 and +3).
CLK_NUDGE = -5
CLK_X0 = WS_END + 29 + CLK_NUDGE
CLK_END = CLK_X0 + CLK_W
CTL_START = CLK_END + 30  # controls sit right of the clock


def ascii_clock(ctx, theme, timestring):
    """Blocky pixel-digit clock (Omarchy screensaver flavor)."""
    FG, ACCENT = theme["fg"], theme["accent"]
    top = (LH - 5 * PX_CELL) / 2
    x = CLK_X0
    for ch in timestring:
        glyph = DIGITS.get(ch)
        if glyph is None:
            continue
        color = ACCENT if ch == ":" else FG
        ctx.set_source_rgb(*color)
        for r in range(5):
            for cc in range(3):
                if glyph[r * 3 + cc]:
                    ctx.rectangle(x + cc * PX_CELL, top + r * PX_CELL,
                                  PX_CELL - 1, PX_CELL - 1)
        ctx.fill()
        x += 3 * PX_CELL + PX_GAP


def mpris_players():
    try:
        out = subprocess.check_output(["busctl", "--user", "list"],
                                      timeout=5, text=True)
        return sorted({l.split()[0] for l in out.splitlines()
                       if "org.mpris.MediaPlayer2." in l})
    except Exception:
        return []


def mpris_status():
    """'Playing' / 'Paused' / None across players."""
    for bus in mpris_players():
        try:
            out = subprocess.check_output(
                ["busctl", "--user", "get-property", bus,
                 "/org/mpris/MediaPlayer2",
                 "org.mpris.MediaPlayer2.Player", "PlaybackStatus"],
                timeout=5, text=True)
            if '"Playing"' in out:
                return "Playing"
        except Exception:
            pass
    return "Paused" if mpris_players() else None


def mpris_toggle():
    for bus in mpris_players():
        subprocess.Popen(["busctl", "--user", "call", bus,
                          "/org/mpris/MediaPlayer2",
                          "org.mpris.MediaPlayer2.Player", "PlayPause"],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
SL_TX0, SL_TX1 = 300, LW - 140  # slider track geometry (hit-test mirror)


def _run(*args, timeout=8):
    try:
        return subprocess.check_output(list(args), timeout=timeout,
                                       text=True).strip()
    except Exception:
        return ""


def get_display():
    try:
        return int(float(_run("omarchy", "brightness", "display").rstrip("%")))
    except Exception:
        return 60


def set_display(n):
    n = max(1, min(100, int(n)))
    subprocess.Popen(["omarchy", "brightness", "display", "--no-osd",
                      f"{n}%"], stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL)


def get_volume():
    try:
        return int(float(_run("wpctl", "get-volume",
                              "@DEFAULT_AUDIO_SINK@").split()[1]) * 100)
    except Exception:
        return 50


def set_volume(n):
    n = max(0, min(100, int(n)))
    subprocess.Popen(["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@",
                      f"{n / 100:.2f}"], stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL)


def get_kbd():
    try:
        v = int(_run("brightnessctl", "-d", "kbd_backlight", "get"))
        m = int(_run("brightnessctl", "-d", "kbd_backlight", "max"))
        return int(v / max(m, 1) * 100)
    except Exception:
        return 20


def set_kbd(n):
    n = max(0, min(100, int(n)))
    subprocess.Popen(["brightnessctl", "-d", "kbd_backlight", "set",
                      str(int(n / 100 * 255))],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def IOWR(nr, size):
    return (3 << 30) | (size << 16) | (0x64 << 8) | nr


IOCTL_GETRES = IOWR(0xA0, 64)
IOCTL_SETCRTC = IOWR(0xA2, 104)
IOCTL_GETCONN = IOWR(0xA7, 80)
IOCTL_ADDFB2 = IOWR(0xB8, 100)
IOCTL_DIRTYFB = IOWR(0xB1, 24)
IOCTL_CREATE_DUMB = IOWR(0xB2, 32)
IOCTL_MAP_DUMB = IOWR(0xB3, 16)

DRM_FORMAT_XRGB8888 = 0x34325258
CONNECTED = 1
RES_FMT = "QQQQIIIIIIII"
CONN_FMT = "QQQQ" + "I" * 12

# Theme colors resolve from the live Omarchy theme (user overlay wins).
_FALLBACK = {"background": "#1e1e2e", "lighter_background": "#313244",
             "accent": "#89b4fa", "foreground": "#cdd6f4",
             "darker_background": "#101019", "red": "#f38ba8",
             "green": "#a6e3a1"}


def _hex(h):
    h = h.strip().lstrip("#")
    return tuple(int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))


_DIRS_CACHE = (0.0, {})


def _theme_dirs():
    """{lowername: path} for user + stock themes (user wins)."""
    global _DIRS_CACHE
    now = time.monotonic()
    if now - _DIRS_CACHE[0] < 60:
        return _DIRS_CACHE[1]
    out = {}
    for base in ("/usr/share/omarchy/themes",
                 os.path.expanduser("~/.config/omarchy/themes")):
        try:
            for e in os.listdir(base):
                if os.path.isdir(os.path.join(base, e)):
                    out.setdefault(e.lower(), os.path.join(base, e))
            # user dir second so it wins: rebuild with user priority
        except OSError:
            pass
    # user wins
    try:
        base = os.path.expanduser("~/.config/omarchy/themes")
        for e in os.listdir(base):
            if os.path.isdir(os.path.join(base, e)):
                out[e.lower()] = os.path.join(base, e)
    except OSError:
        pass
    _DIRS_CACHE = (now, out)
    return out


def _toml(path):
    try:
        with open(path, "rb") as f:
            return tomllib.load(f)
    except Exception:
        return {}


_COLORS_CACHE = {}


def colors_for(name):
    """Colors for a theme display name; mtime-aware cache. Falls back
    to alacritty.toml blue when there is no colors.toml (e.g. Mars)."""
    d = _theme_dirs().get(name.lower().replace(" ", "-"))
    if not d:
        return dict(_FALLBACK)
    cp = os.path.join(d, "colors.toml")
    try:
        mt = os.path.getmtime(cp)
        key = (name, mt)
    except OSError:
        key = (name, -1)
    hit = _COLORS_CACHE.get(key)
    if hit is not None:
        return hit
    colors = dict(_FALLBACK)
    c = _toml(cp)
    if c:
        colors.update(c)
    else:
        a = _toml(os.path.join(d, "alacritty.toml"))
        blue = (a.get("colors", {}).get("normal", {}).get("blue")
                or a.get("colors", {}).get("bright", {}).get("blue")
                or a.get("blue"))
        if isinstance(blue, str):
            colors["accent"] = blue
            colors["blue"] = blue
    _COLORS_CACHE[key] = colors
    return colors


def _luminance(hexcolor):
    h = hexcolor.strip().lstrip("#")
    r, g, b = (int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def is_light(name):
    """True for light themes (mode key, else background luminance)."""
    c = colors_for(name)
    if c.get("mode", "dark").lower() == "light":
        return True
    bg = c.get("background", "#000000")
    return _luminance(bg) > 0.55


def theme_list():
    try:
        out = subprocess.check_output(["omarchy", "theme", "list"],
                                      timeout=10, text=True)
        return [l.strip() for l in out.splitlines() if l.strip()]
    except Exception:
        return ["Catppuccin"]


def current_theme():
    try:
        return subprocess.check_output(["omarchy", "theme", "current"],
                                       timeout=5, text=True).strip()
    except Exception:
        return "Catppuccin"


def get_favs():
    """Preferred themes from ~/.config/omarchy-touchbar/themes (created
    with current + next ones on first run). Returns [(name, accent_rgb)],
    padded to MENU_N entries."""
    names, excluded = [], set()
    if os.path.exists(FAV_PATH):
        try:
            with open(FAV_PATH) as f:
                for l in f:
                    l = l.strip()
                    if not l or l.startswith("#"):
                        continue
                    if l.startswith("!"):
                        excluded.add(l[1:].strip())
                    else:
                        names.append(l)
        except Exception:
            pass
    dropped = [n for n in names if is_light(n)]
    names = [n for n in names if not is_light(n)]
    if dropped or len(names) < MENU_N:
        cur, all_ = current_theme(), theme_list()
        try:
            i = all_.index(cur)
            order = [all_[(i + k) % len(all_)] for k in range(len(all_))]
        except ValueError:
            order = all_
        # auto-fill never adds light or excluded themes
        for n in order:
            if n not in names and n not in excluded and not is_light(n):
                names.append(n)
            if len(names) >= MENU_N:
                break
        try:
            os.makedirs(os.path.dirname(FAV_PATH), exist_ok=True)
            with open(FAV_PATH, "w") as f:
                f.write("# preferred touchbar themes (one per line, shown "
                        "in the theme menu;\n# prefix with ! to exclude "
                        "one forever)\n"
                        + "".join(f"!{e}\n" for e in sorted(excluded))
                        + "\n".join(names[:MENU_N]) + "\n")
        except Exception:
            pass
    out = []
    for n in names[:MENU_N]:
        c = colors_for(n)
        accent = c.get("accent", c.get("blue", _FALLBACK["accent"]))
        out.append((n, _hex(accent)))
    return out


def load_theme():
    name = current_theme()
    colors = colors_for(name)

    def pick(*ks):
        for k in ks:
            if k in colors:
                return _hex(colors[k])
        return _hex(_FALLBACK.get(ks[0], "#ffffff"))

    return {"name": name,
            "bg": pick("background", "dark_background"),
            "pill": pick("lighter_background", "selection"),
            "accent": pick("accent", "blue"),
            "fg": pick("foreground", "light_foreground"),
            "dark": pick("darker_background", "dark_background"),
            "red": pick("red", "bright_red"),
            "green": pick("green", "bright_green"),
            "dim": pick("dark_foreground", "muted", "#6c7086")}


def hypr(cmd):
    try:
        out = subprocess.check_output(["hyprctl", cmd, "-j"],
                                      timeout=3, text=True)
        return json.loads(out)
    except Exception:
        return None


def rounded(ctx, x, y, w, h, r):
    ctx.new_sub_path()
    ctx.arc(x + r, y + r, r, math.pi, 1.5 * math.pi)
    ctx.arc(x + w - r, y + r, r, 1.5 * math.pi, 0)
    ctx.arc(x + w - r, y + h - r, r, 0, 0.5 * math.pi)
    ctx.arc(x + r, y + h - r, r, 0.5 * math.pi, math.pi)
    ctx.close_path()


def text_centered(ctx, cx, cy, s, color, font="Sans", size=24,
                  weight=cairo.FONT_WEIGHT_BOLD):
    ctx.select_font_face(font, cairo.FONT_SLANT_NORMAL, weight)
    ctx.set_font_size(size)
    ext = ctx.text_extents(s)
    ctx.move_to(cx - (ext.width / 2 + ext.x_bearing),
                cy - (ext.height / 2 + ext.y_bearing))
    ctx.set_source_rgb(*color)
    ctx.show_text(s)
    return ext.width


APP_GLYPHS = {"foot": (NERD, ICONS["term"]),
                "org.omarchy.agent": ("Sans", "▲"),
                "chrome-x.com__-Default": (NERD, ICONS["chrome"]),
                "chromium": (NERD, ICONS["chrome"])}

# Webapp icons (all verified on-device via glyph-test): omarchy webapps
# all run as `chromium --app=<url>`, so the WM class is usually a useless
# generic ("chromium") or a host-derived PWA class ("chrome-<host>__...").
# Match the host first, then fall back to title keywords.
WEB_ICONS = {"youtube": (NERD, "\uf16a"),      # play button
             "x": ("Sans", "X"),               # X brand
             "twitter": (NERD, "\uf099"),      # bird (older titles)
             "whatsapp": (NERD, "\uf232"),
             "discord": (NERD, "\U000f066f"),  # mdi-discord (\uf392 is tofu)
             "google": (NERD, "\uf1a0"),       # Maps/Photos/Contacts/Messages
             "maps": (NERD, "\uf1a0"),
             "photos": (NERD, "\uf1a0"),
             "contacts": (NERD, "\uf1a0"),
             "messages": (NERD, "\uf1a0"),
             "zoom": (NERD, "\uf03d"),
             "github": (NERD, "\uf09b")}
GENERIC_BROWSER = {"chromium", "chrome", "google-chrome", "brave-browser",
                   "microsoft-edge", "vivaldi", "helium"}


def _host_glyph(host):
    """Match a PWA-style host (x.com, youtube.com, ...) to an icon."""
    host = host.lower().split(":")[0]
    for key, glyph in WEB_ICONS.items():
        if key in host:
            # bare "x" would match everything ("linux"); require dot-boundary
            if key == "x" and "x.com" not in host:
                continue
            return glyph
    return None


def glyph_for_wmclass(wmclass, title=""):
    """(font, glyph) for a Hyprland client, or None to skip."""
    if wmclass in APP_GLYPHS:
        return APP_GLYPHS[wmclass]
    low = (wmclass or "").lower()
    # PWA class "chrome-<host>__<profile>-Default" carries the real site
    if low.startswith("chrome-") and "__" in low:
        g = _host_glyph(low[len("chrome-"):].split("__")[0].replace("_", "."))
        if g:
            return g
    if low in GENERIC_BROWSER or low.startswith("chrome-"):
        t = (title or "").lower().strip()
        if t in ("x", "home / x") or "x.com" in t:
            return WEB_ICONS["x"]
        for key, glyph in WEB_ICONS.items():
            if key == "x" or len(key) < 3:
                continue
            if key in t:
                return glyph
        return APP_GLYPHS.get("chromium")
    return None


def ws_button(ctx, x, y, w, h, theme, num, glyphs, is_active, has_windows):
    """Square workspace button: number + up to 3 app icons inside."""
    ACCENT, FG, DIM = theme["accent"], theme["fg"], theme["dim"]
    PILL = theme["pill"]
    rounded(ctx, x, y, w, h, 12)
    if is_active:
        ctx.set_source_rgba(*ACCENT, 0.32)
        ctx.fill_preserve()
        ctx.set_source_rgb(*ACCENT)
        ctx.set_line_width(2.5)
        ctx.stroke()
    else:
        ctx.set_source_rgba(*PILL, 0.90 if has_windows else 0.60)
        ctx.fill_preserve()
        ctx.set_source_rgba(*FG, 0.35)
        ctx.set_line_width(1.5)
        ctx.stroke()
    cy = y + h / 2
    num_color = ACCENT if is_active else (FG if has_windows else DIM)
    # number + icons centered as a group so visual edges sit symmetric
    ctx.select_font_face("Sans", cairo.FONT_SLANT_NORMAL,
                         cairo.FONT_WEIGHT_BOLD)
    ctx.set_font_size(27)
    ext = ctx.text_extents(str(num))
    icon_widths = []
    for font, g in glyphs[:3]:
        ctx.select_font_face(font, cairo.FONT_SLANT_NORMAL,
                             cairo.FONT_WEIGHT_BOLD)
        ctx.set_font_size(25)
        icon_widths.append(ctx.text_extents(g).width)
    total = ext.width + sum(icon_widths) + 14 + len(icon_widths) * 10
    gx = x + (w - total) / 2
    ctx.select_font_face("Sans", cairo.FONT_SLANT_NORMAL,
                         cairo.FONT_WEIGHT_BOLD)
    ctx.set_font_size(27)
    ctx.move_to(gx - ext.x_bearing, cy - (ext.height / 2 + ext.y_bearing))
    ctx.set_source_rgb(*num_color)
    ctx.show_text(str(num))
    gx += ext.width + 14
    for (font, g), iw in zip(glyphs[:3], icon_widths):
        if gx + iw > x + w - 8:
            break
        ctx.select_font_face(font, cairo.FONT_SLANT_NORMAL,
                             cairo.FONT_WEIGHT_BOLD)
        ctx.set_font_size(25)
        ge = ctx.text_extents(g)
        ctx.move_to(gx - ge.x_bearing, cy - (ge.height / 2 + ge.y_bearing))
        ctx.set_source_rgb(*FG)
        ctx.show_text(g)
        gx += iw + 10


def theme_button(ctx, x, y, w, h, name, accent, is_active, dark, size=20):
    rounded(ctx, x, y, w, h, 12)
    if is_active:
        ctx.set_source_rgb(*accent)
        ctx.fill_preserve()
        ctx.set_source_rgb(*accent)
        ctx.set_line_width(2)
        ctx.stroke()
        fg = dark
    else:
        ctx.set_source_rgba(*accent, 0.28)
        ctx.fill_preserve()
        ctx.set_source_rgba(*accent, 0.80)
        ctx.set_line_width(1.5)
        ctx.stroke()
        fg = accent
    text_centered(ctx, x + w / 2, y + h / 2, name[:13], fg, size=size)


def ctl_button(ctx, x, y, w, h, icon, fg, dim):
    rounded(ctx, x, y, w, h, 12)
    ctx.set_source_rgba(*fg, 0.16)
    ctx.fill_preserve()
    ctx.set_source_rgba(*fg, 0.40)
    ctx.set_line_width(1.5)
    ctx.stroke()
    text_centered(ctx, x + w / 2, y + h / 2, icon, dim, font=NERD, size=28)


def slider_view(ctx, theme, kind, label, icon, value):
    FG, DIM, ACCENT = theme["fg"], theme["dim"], theme["accent"]
    cy = LH / 2
    # explicit back button on the left
    rounded(ctx, 16, 8, 96, LH - 16, 12)
    ctx.set_source_rgba(*FG, 0.10)
    ctx.fill_preserve()
    ctx.set_source_rgba(*FG, 0.30)
    ctx.set_line_width(1.5)
    ctx.stroke()
    text_centered(ctx, 64, cy, "←", FG, size=26)
    text_centered(ctx, 172, cy, icon, ACCENT, font=NERD, size=28)
    ctx.select_font_face("Sans", cairo.FONT_SLANT_NORMAL,
                         cairo.FONT_WEIGHT_BOLD)
    ctx.set_font_size(22)
    ext = ctx.text_extents(label)
    ctx.move_to(214 - ext.x_bearing, cy - (ext.height / 2 + ext.y_bearing))
    ctx.set_source_rgb(*DIM)
    ctx.show_text(label)
    # track
    tx0, tx1, ty = SL_TX0, SL_TX1, cy
    rounded(ctx, tx0, ty - 5, tx1 - tx0, 10, 5)
    ctx.set_source_rgba(*FG, 0.18)
    ctx.fill()
    fx = tx0 + (tx1 - tx0) * max(0, min(100, value)) / 100
    if fx > tx0:
        rounded(ctx, tx0, ty - 5, fx - tx0, 10, 5)
        ctx.set_source_rgb(*ACCENT)
        ctx.fill()
    ctx.arc(fx, ty, 13, 0, 2 * math.pi)
    ctx.set_source_rgb(*ACCENT)
    ctx.fill()
    # percent on the right
    text_centered(ctx, LW - 60, cy, f"{int(value)}", FG, size=24)
    return tx0, tx1


def fn_view(ctx, theme):
    """F1-F12 layer while Fn is held (mirrors tiny-dfr Fn layer)."""
    FG, DIM, ACCENT = theme["fg"], theme["dim"], theme["accent"]
    cy = LH / 2
    for k in range(FN_N):
        x = FN_START + k * (FN_W + FN_GAP)
        rounded(ctx, x, 5, FN_W, LH - 10, 12)
        ctx.set_source_rgba(*FG, 0.16)
        ctx.fill_preserve()
        ctx.set_source_rgba(*FG, 0.40)
        ctx.set_line_width(1.5)
        ctx.stroke()
        text_centered(ctx, x + FN_W / 2, cy, f"F{k + 1}", DIM, size=26)


def render_ui(focused=None, favs=None, menu=False, slider=None, fn=False):
    theme = load_theme()
    ACCENT = theme["accent"]
    FG, DIM, DARK = theme["fg"], theme["dim"], theme["dark"]
    workspaces = hypr("workspaces") or [{"id": 1, "windows": 1}]
    clients = hypr("clients") or []
    open_ids = {w["id"] for w in workspaces}
    if focused is None:
        try:
            focused = hypr("activeworkspace")["id"]
        except Exception:
            focused = sorted(open_ids)[0] if open_ids else 1
    if favs is None:
        favs = get_favs()

    surf = cairo.ImageSurface(cairo.FORMAT_ARGB32, W, H)
    ctx = cairo.Context(surf)
    ctx.set_source_rgb(0, 0, 0)
    ctx.paint()
    # tiny-dfr transform: draw in 2008x60 logical space
    ctx.translate(LH, 0.0)
    ctx.rotate(math.pi / 2)

    # Strip background stays neutral; theme accent lives only in buttons.
    ctx.set_source_rgb(*NEUTRAL_BG)
    ctx.rectangle(0, 0, LW, LH)
    ctx.fill()
    by, bh = 5, LH - 10

    if menu:
        # Nested theme menu: all themes when few, else preferred favs.
        items = menu_items(favs)
        x0, bw = menu_geometry(len(items))
        cur = theme["name"]
        x = x0
        for name, accent in items:
            theme_button(ctx, x, by, bw, bh, name, accent,
                         name == cur, DARK,
                         size=20 if bw >= 260 else 15)
            x += bw + MENU_GAP
        surf.flush()
        return surf

    if slider is not None:
        slider_view(ctx, theme, slider["kind"], slider["label"],
                    slider["icon"], slider["value"])
        surf.flush()
        return surf

    if fn:
        fn_view(ctx, theme)
        surf.flush()
        return surf

    # apps per workspace id (webapps resolve via class+title)
    ws_apps = {}
    for c in clients:
        wid = c.get("workspace", {}).get("id")
        if wid in range(1, WS_N + 1):
            g = glyph_for_wmclass(c.get("class", "?"), c.get("title", ""))
            if g:
                ws_apps.setdefault(wid, []).append(g)

    # Workspace squares with app icons inside (no ESC: physical key)
    for k in range(WS_N):
        i = k + 1
        ws_button(ctx, WS_START + k * (WS_W + WS_GAP), by, WS_W, bh,
                  theme, i, ws_apps.get(i, []), i == focused,
                  i in open_ids)

    # Control buttons in the middle gap (state-aware where it matters)
    playing = mpris_status() == "Playing"
    muted = get_mic()
    night = get_night()
    for k, key in enumerate(CTL_ORDER):
        if key == "media":
            icon = CTL_ICONS["pause"] if playing else CTL_ICONS["play"]
            glyph = FG
        elif key == "mic":
            icon = CTL_ICONS["mic_off"] if muted else CTL_ICONS["mic"]
            glyph = theme["red"] if muted else DIM
        elif key == "night":
            icon = CTL_ICONS["night"]
            glyph = ACCENT if night else DIM
        else:
            icon, glyph = CTL_ICONS[key], DIM
        ctl_button(ctx, CTL_START + k * (CTL_W + CTL_GAP), by, CTL_W, bh,
                   icon, FG, glyph)

    # ASCII-art clock in the center pocket (screensaver flavor)
    ascii_clock(ctx, theme, datetime.datetime.now().strftime("%H:%M"))

    # Single theme button (current theme, accent colored); tap opens menu.
    cur_accent = ACCENT
    for name, accent in favs:
        if name == theme["name"]:
            cur_accent = accent
            break
    theme_button(ctx, TH1_X, by, TH1_W, bh, theme["name"], cur_accent,
                 False, DARK)
    surf.flush()  # mandatory: push cairo ops to the image buffer
    return surf


class DRMBackend:
    def __init__(self):
        self.fd = os.open(CARD, os.O_RDWR)
        self.mm = None

    def modeset(self):
        fd = self.fd
        res = bytearray(64)
        fcntl.ioctl(fd, IOCTL_GETRES, res)
        parts = struct.unpack(RES_FMT, res)
        (n_fb, n_crtc, n_conn, n_enc) = parts[4:8]
        crtc_arr = (C.c_uint32 * n_crtc)()
        conn_arr = (C.c_uint32 * n_conn)()
        enc_arr = (C.c_uint32 * n_enc)()
        fb_arr = (C.c_uint32 * n_fb)()
        struct.pack_into("QQQQ", res, 0, C.addressof(fb_arr),
                         C.addressof(crtc_arr), C.addressof(conn_arr),
                         C.addressof(enc_arr))
        fcntl.ioctl(fd, IOCTL_GETRES, res)
        crtc_id = int(crtc_arr[0])

        chosen, mode_bytes = None, None
        for cid in conn_arr:
            cid = int(cid)
            c = bytearray(80)
            struct.pack_into("I", c, 48, cid)
            fcntl.ioctl(fd, IOCTL_GETCONN, c)
            vals = struct.unpack(CONN_FMT, c)
            n_modes, connection = vals[4], vals[11]
            if connection != CONNECTED or n_modes == 0:
                continue
            mbuf = (C.c_char * (68 * n_modes))()
            struct.pack_into("Q", c, 8, C.addressof(mbuf))
            struct.pack_into("II", c, 36, 0, 0)
            fcntl.ioctl(fd, IOCTL_GETCONN, c)
            mode_bytes = bytes(mbuf[:68])
            chosen = cid
            break
        if chosen is None:
            raise RuntimeError("no connected connector")

        dumb = bytearray(struct.pack("IIIIIIQ", H, W, 32, 0, 0, 0, 0))
        fcntl.ioctl(fd, IOCTL_CREATE_DUMB, dumb)
        (_, _, _, _, handle, pitch, size) = struct.unpack("IIIIIIQ", dumb)

        fb2 = bytearray(100)
        struct.pack_into("IIIII", fb2, 0, 0, W, H, DRM_FORMAT_XRGB8888, 0)
        struct.pack_into("IIII", fb2, 20, handle, 0, 0, 0)
        struct.pack_into("IIII", fb2, 36, pitch, 0, 0, 0)
        fcntl.ioctl(fd, IOCTL_ADDFB2, fb2)
        self.fb_id = struct.unpack("I", fb2[:4])[0]

        conn_list = (C.c_uint32 * 1)(chosen)
        crtc = bytearray(104)
        struct.pack_into("Q", crtc, 0, C.addressof(conn_list))
        struct.pack_into("IIIIIII", crtc, 8, 1, crtc_id, self.fb_id,
                         0, 0, 0, 1)
        crtc[36:36 + 68] = mode_bytes
        fcntl.ioctl(fd, IOCTL_SETCRTC, crtc)

        md = bytearray(struct.pack("IIQ", handle, 0, 0))
        fcntl.ioctl(fd, IOCTL_MAP_DUMB, md)
        (_, _, offset) = struct.unpack("IIQ", md)
        self.mm = mmap.mmap(fd, size, offset=offset)
        print(f"modeset ok: conn={chosen} fb={self.fb_id} "
              f"pitch={pitch} size={size}", flush=True)

    def blit(self, surf):
        data = surf.get_data()
        n = len(data)
        self.mm[:n] = data
        clip = (C.c_char * 8).from_buffer_copy(struct.pack("HHHH", 0, 0, W, H))
        dirty = bytearray(struct.pack("IIIIQ", self.fb_id, 0, 0, 1,
                                      C.addressof(clip)))
        fcntl.ioctl(self.fd, IOCTL_DIRTYFB, dirty)
        print(f"blit ok ({n} bytes)", flush=True)


def main():
    hold = 8.0
    if "--hold" in sys.argv:
        hold = float(sys.argv[sys.argv.index("--hold") + 1])
    drm = DRMBackend()
    drm.modeset()
    drm.blit(render_ui())
    print(f"holding {hold}s", flush=True)
    time.sleep(hold)
    print("done (leaving framebuffer up)", flush=True)


if __name__ == "__main__":
    main()
