#!/usr/bin/env python3
"""omarchy-touchbar live v0.2 — realtime + interactive.

- Hyprland state via .socket.sock, live updates via .socket2.sock events
- Touch taps via /dev/input/event3 (MT protocol, stdlib only)
- Actions: tap workspace -> focus, tap theme -> next theme (instant),
  tap ESC -> Escape key via wtype
- Theme changes re-render automatically (poll current name every 2s)

Share layout constants with render_ui in touchbar_daemon.
"""
import os
import select
import socket
import struct
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from touchbar_daemon import (DRMBackend, render_ui, LW, LH, WS_START, WS_W,
                               WS_GAP, WS_N, TH1_X, TH1_W, MENU_N, MENU_GAP,
                               get_favs, menu_geometry, menu_items,
                               CTL_START, CTL_W, CTL_GAP,
                               CTL_ORDER, SL_TX0, SL_TX1, FN_START, FN_W,
                               FN_GAP, FN_N, CTL_ICONS, toggle_mic,
                               toggle_night, get_display, set_display,
                               get_volume, set_volume, get_kbd, set_kbd,
                               mpris_toggle)  # noqa: E402

RUN = f"/run/user/{os.getuid()}"
SIG = os.environ.get("HYPRLAND_INSTANCE_SIGNATURE", "")
TOUCH = "/dev/input/event3"
X_MAX, Y_MAX = 23044, 639

# Hit regions mirror render_ui geometry (no ESC: physical key exists).
# FAVS refreshed by the main loop when the theme changes.
FAVS = []
# Nested theme menu state: IN_MENU + MENU_AT (5s timeout back to normal).
IN_MENU, MENU_AT, MENU_TIMEOUT = False, 0.0, 5.0
# Slider state: None or dict(kind/label/icon/value/at); 3s after last
# change it disappears. Mutually exclusive with IN_MENU.
SLIDER, SLIDER_TIMEOUT = None, 3.0
SLIDER_DEFS = {"display": ("Display", get_display, set_display),
               "volume": ("Volume", get_volume, set_volume),
               "kbd": ("Keys", get_kbd, set_kbd)}
SLIDER_ORDER = ("display", "volume", "kbd")
_last_apply_t, _last_apply_v = 0.0, -1


def slider_val_from_x(lx):
    return max(0, min(100, int((lx - SL_TX0) / (SL_TX1 - SL_TX0) * 100)))


def slider_open(kind):
    global SLIDER, IN_MENU
    IN_MENU = False
    label, getter, _ = SLIDER_DEFS[kind]
    SLIDER = {"kind": kind, "label": label, "icon": CTL_ICONS[kind],
              "value": getter(), "at": time.monotonic()}
    print(f"slider: {kind} open at {SLIDER['value']}", flush=True)


def slider_flush():
    """Apply the current slider value unconditionally (drag end/close)."""
    global _last_apply_t, _last_apply_v
    if SLIDER is None:
        return
    _last_apply_t, _last_apply_v = time.monotonic(), SLIDER["value"]
    SLIDER_DEFS[SLIDER["kind"]][2](SLIDER["value"])


def slider_close():
    global SLIDER
    if SLIDER is not None:
        slider_flush()
        print(f"slider: {SLIDER['kind']} closed at {SLIDER['value']}",
              flush=True)
    SLIDER = None


def slider_set(value):
    """Apply throttled; returns True if value changed."""
    global _last_apply_t, _last_apply_v
    if SLIDER is None or value == SLIDER["value"]:
        return False
    SLIDER["value"] = value
    SLIDER["at"] = time.monotonic()
    setter = SLIDER_DEFS[SLIDER["kind"]][2]
    now = time.monotonic()
    if abs(value - _last_apply_v) >= 1 and now - _last_apply_t > 0.09:
        _last_apply_t, _last_apply_v = now, value
        setter(value)
    return True


def menu_layout():
    n = min(MENU_N, len(FAVS)) or 1
    total = n * MENU_W + (n - 1) * MENU_GAP
    return (LW - total) / 2


def hypr_cmd(cmd):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(3)
    s.connect(os.path.join(RUN, "hypr", SIG, ".socket.sock"))
    s.sendall(cmd.encode())
    out = b""
    try:
        while True:
            chunk = s.recv(65536)
            if not chunk:
                break
            out += chunk
            if len(chunk) < 65536:
                break
    except socket.timeout:
        pass
    s.close()
    return out.decode()


def theme_list():
    try:
        out = subprocess.check_output(["omarchy", "theme", "list"],
                                      timeout=10, text=True)
        return [l.strip() for l in out.splitlines() if l.strip()]
    except Exception:
        return []


def theme_current():
    try:
        return subprocess.check_output(["omarchy", "theme", "current"],
                                       timeout=5, text=True).strip()
    except Exception:
        return ""


def theme_set(name):
    print(f"theme set: {name}", flush=True)
    subprocess.Popen(["omarchy", "theme", "set", name],
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


EV_SYN, EV_KEY, EV_ABS = 0, 1, 3
BTN_TOUCH = 330
KEY_FN = 464
KEY_LSHIFT, KEY_RSHIFT = 42, 54
KEY_LMETA, KEY_RMETA = 125, 126
KBD = "/dev/input/event1"
MT_SLOT, MT_POS_X, MT_POS_Y, MT_TRACKING = 47, 53, 54, 57
# Fn layer: held state (F1-F12 view; emission needs root, see Rust daemon)
FN_HELD = False
# Super+Shift held -> theme menu overlay (like Fn layer)
ME_L, ME_R, SH_L, SH_R = False, False, False, False


def sup_held():
    return (ME_L or ME_R) and (SH_L or SH_R)# (Shift layer removed: holding shift that long was annoying. The theme
# menu stays available by tapping the theme button.)


def menu_layout():
    items = menu_items(FAVS)
    x0, bw = menu_geometry(len(items))
    return x0, bw, [n for n, _ in items]


class TouchState:
    def __init__(self, debug=False):
        self.slot = 0
        self.fingers = {}
        self.taps = []
        self.debug = debug
        self.st_down = None
        self.st_x = self.st_y = 0
        self.st_ax = self.st_ay = 0
        self.st_seen = False
        self.last_mt_tap = 0.0

    def _log(self, msg):
        if self.debug:
            print(f"touch: {msg}", flush=True)

    def active_pos(self):
        """Current position of an in-progress contact, else None."""
        for slot in sorted(self.fingers):
            f = self.fingers[slot]
            if "down" in f and "x" in f and "y" in f:
                return f["x"], f["y"]
        return None

    def _tap(self, x, y, how):
        now = time.monotonic()
        if how == "st" and now - self.last_mt_tap < 0.15:
            return  # MT already caught this contact
        if how == "mt":
            self.last_mt_tap = now
        self.taps.append((x, y))
        self._log(f"TAP x={x} y={y} via {how}")

    def feed(self, typ, code, val):
        now = time.monotonic()
        if typ == EV_ABS and code == MT_SLOT:
            self.slot = val
            return
        if typ == EV_KEY and code == BTN_TOUCH:
            if val == 1:
                self.st_down = now
                self.st_seen = False
                self.st_ax, self.st_ay = self.st_x, self.st_y
            elif self.st_down is not None:
                dt = now - self.st_down
                self.st_down = None
                if not self.st_seen:
                    return  # MT path owns this contact; stay quiet
                dx = abs(self.st_x - self.st_ax)
                dy = abs(self.st_y - self.st_ay)
                if dt < 0.6 and dx < 3000 and dy < 300:
                    self._tap(self.st_x, self.st_y, "st")
                else:
                    self._log(f"st reject dt={dt:.2f} dx={dx} dy={dy}")
            return
        if typ != EV_ABS:
            return
        if code == 0:  # ABS_X single-touch
            self.st_x = val
            self.st_seen = True
            return
        if code == 1:  # ABS_Y single-touch
            self.st_y = val
            self.st_seen = True
            return
        f = self.fingers.setdefault(self.slot, {})
        if code == MT_TRACKING:
            if val == -1:
                down = f.pop("down", None)
                if down is None:
                    self._log(f"slot {self.slot} lift w/o down")
                else:
                    ax = f.get("ax", f.get("x", 0))
                    ay = f.get("ay", f.get("y", 0))
                    dx = abs(f.get("x", ax) - ax)
                    dy = abs(f.get("y", ay) - ay)
                    dt = now - down
                    if dt < 0.6 and dx < 3000 and dy < 300:
                        self._tap(f.get("x", ax), f.get("y", ay), "mt")
                    else:
                        self._log(f"slot {self.slot} reject "
                                  f"dt={dt:.2f} dx={dx} dy={dy}")
                f.pop("ax", None)
                f.pop("ay", None)
            else:
                f["down"] = now
                f.pop("ax", None)
                f.pop("ay", None)
                self._log(f"slot {self.slot} down id={val} "
                          f"x={f.get('x', '?')} y={f.get('y', '?')}")
        elif code == MT_POS_X:
            f["x"] = val
            f.setdefault("ax", val)
        elif code == MT_POS_Y:
            f["y"] = val
            f.setdefault("ay", val)


def handle_tap(lx, ly):
    global IN_MENU, MENU_AT
    print(f"tap lx={lx:.0f} ly={ly:.0f}", flush=True)
    if not (4 <= ly <= 56):
        return True
    if FN_HELD:
        for k in range(FN_N):
            bx = FN_START + k * (FN_W + FN_GAP)
            if bx <= lx < bx + FN_W:
                print(f"fn: F{k + 1} (key emission needs root daemon)",
                      flush=True)
                return True
        return True
    if SLIDER is not None:
        if lx < 130:  # explicit back button
            slider_close()
        else:
            slider_set(slider_val_from_x(lx))
        return True
    if IN_MENU or sup_held():
        x0, bw, names = menu_layout()
        for k, name in enumerate(names):
            bx = x0 + k * (bw + MENU_GAP)
            if bx <= lx < bx + bw:
                IN_MENU = False
                theme_set(name)
                return True
        IN_MENU = False  # tap outside: back to normal
        print("menu: cancelled", flush=True)
        return True
    for k in range(WS_N):
        x0 = WS_START + k * (WS_W + WS_GAP)
        if x0 <= lx < x0 + WS_W:
            rep = hypr_cmd(
                f'dispatch hl.dsp.focus({{ workspace = "{k + 1}" }})'
            ).strip().split("\n")[0]
            print(f"action: workspace {k + 1} -> {rep or '?'}", flush=True)
            return True
    if TH1_X <= lx < TH1_X + TH1_W:
        IN_MENU = True
        MENU_AT = time.monotonic()
        print("menu: theme picker open", flush=True)
        return True
    for k, key in enumerate(CTL_ORDER):
        bx = CTL_START + k * (CTL_W + CTL_GAP)
        if bx <= lx < bx + CTL_W:
            if key == "media":
                mpris_toggle()
                print("action: play/pause toggle", flush=True)
            elif key == "mic":
                toggle_mic()
                print("action: mic mute toggle", flush=True)
            elif key == "night":
                toggle_night()
                print("action: nightlight toggle", flush=True)
            else:
                slider_open(key)
            return True
    return True


def main():
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--live", type=float, default=120)
    ap.add_argument("--probe-touch", action="store_true",
                    help="print taps only, no actions, no DRM")
    args = ap.parse_args()

    tfd = os.open(TOUCH, os.O_RDONLY | os.O_NONBLOCK)
    touch = TouchState(debug=True)

    if args.probe_touch:
        print("probe: tap the bar, coordinates print below", flush=True)
        end = time.monotonic() + args.live
        while time.monotonic() < end:
            r, _, _ = select.select([tfd], [], [], 0.5)
            if r:
                try:
                    data = os.read(tfd, 24 * 64)
                except BlockingIOError:
                    continue
                for off in range(0, len(data) - 23, 24):
                    _, _, typ, code, val = struct.unpack("qqHHi",
                                                         data[off:off + 24])
                    touch.feed(typ, code, val)
                for x, y in touch.taps:
                    print(f"tap raw x={x} y={y} -> "
                          f"lx={x * LW / X_MAX:.0f} ly={y * LH / Y_MAX:.0f}",
                          flush=True)
                touch.taps.clear()
        return

    drm = DRMBackend()
    drm.modeset()

    ev = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    ev.settimeout(5)
    ev.connect(os.path.join(RUN, "hypr", SIG, ".socket.sock"))
    ev.sendall(b"j/activeworkspace")
    try:
        active_ws = int(__import__("json").loads(ev.recv(4096))["id"])
    except Exception:
        active_ws = 1
    ev.close()

    ev = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    ev.connect(os.path.join(RUN, "hypr", SIG, ".socket2.sock"))
    ev.setblocking(False)

    last_theme, last_poll = theme_current(), time.monotonic()
    import datetime as _dt
    last_minute = _dt.datetime.now().strftime("%H:%M")
    global IN_MENU, MENU_AT, FAVS, SLIDER, FN_HELD
    global ME_L, ME_R, SH_L, SH_R
    FAVS = get_favs()
    try:
        kbd = os.open(KBD, os.O_RDONLY | os.O_NONBLOCK)
    except OSError as e:
        print(f"fn: kbd watch unavailable: {e}", flush=True)
        kbd = -1
    focused_ws = active_ws
    dirty, buf = True, b""
    end = time.monotonic() + args.live
    print("live: tap ws squares / theme / controls; hold Fn for F-keys",
          flush=True)
    while time.monotonic() < end:
        watch = [ev, tfd] + ([kbd] if kbd >= 0 else [])
        r, _, _ = select.select(watch, [], [], 0.5)
        if ev in r:
            try:
                buf += ev.recv(4096)
            except BlockingIOError:
                pass
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                try:
                    name, _, payload = line.decode().partition(">>")
                except Exception:
                    continue
                if name == "workspace":
                    try:
                        focused_ws = int(payload)
                    except ValueError:
                        pass
                    dirty = True
                elif name in ("openwindow", "closewindow", "movewindow",
                              "activewindow", "activespecial", "fullscreen",
                              "changefloatingmode"):
                    dirty = True
        if kbd >= 0 and kbd in r:
            try:
                data = os.read(kbd, 24 * 64)
            except BlockingIOError:
                data = b""
            for off in range(0, len(data) - 23, 24):
                _, _, typ, code, val = struct.unpack("qqHHi",
                                                     data[off:off + 24])
                if typ == EV_KEY and code == KEY_FN and val in (0, 1):
                    if bool(val) != FN_HELD:
                        FN_HELD = bool(val)
                        if FN_HELD:
                            slider_close()
                            IN_MENU = False
                        print(f"fn: {'held' if FN_HELD else 'released'}",
                              flush=True)
                        dirty = True
                elif typ == EV_KEY and code in (KEY_LMETA, KEY_RMETA,
                                                KEY_LSHIFT, KEY_RSHIFT) \
                        and val in (0, 1, 2):
                    held = bool(val)
                    if code == KEY_LMETA:
                        ME_L = held
                    elif code == KEY_RMETA:
                        ME_R = held
                    elif code == KEY_LSHIFT:
                        SH_L = held
                    else:
                        SH_R = held
                    dirty = True
        if tfd in r:
            try:
                data = os.read(tfd, 24 * 64)
            except BlockingIOError:
                data = b""
            for off in range(0, len(data) - 23, 24):
                _, _, typ, code, val = struct.unpack("qqHHi",
                                                     data[off:off + 24])
                touch.feed(typ, code, val)
            for x, y in touch.taps:
                handle_tap(x * LW / X_MAX, y * LH / Y_MAX)
                dirty = True
            touch.taps.clear()
            if SLIDER is not None and not FN_HELD:
                pos = touch.active_pos()
                if pos is not None:
                    lx = pos[0] * LW / X_MAX
                    ly = pos[1] * LH / Y_MAX
                    # back button zone never drives the value
                    if lx >= 130 and 4 <= ly <= 56:
                        if slider_set(slider_val_from_x(lx)):
                            dirty = True
        now = time.monotonic()
        if now - last_poll > 2:
            last_poll = now
            cur = theme_current()
            if cur != last_theme:
                last_theme = cur
                FAVS = get_favs()
                dirty = True
            minute = _dt.datetime.now().strftime("%H:%M")
            if minute != last_minute:
                last_minute = minute
                dirty = True
        if IN_MENU and now - MENU_AT > MENU_TIMEOUT:
            IN_MENU = False
            print("menu: timeout, back to normal", flush=True)
            dirty = True
        if SLIDER is not None and now - SLIDER["at"] > SLIDER_TIMEOUT:
            slider_close()
            print("slider: timeout, back to normal", flush=True)
            dirty = True
        if dirty:
            dirty = False
            try:
                drm.blit(render_ui(focused_ws, FAVS, menu=(IN_MENU or
                                   sup_held()), slider=SLIDER, fn=FN_HELD))
            except Exception as e:
                print(f"render fail: {e}", flush=True)
    slider_close()
    print("live done (leaving framebuffer up)", flush=True)


if __name__ == "__main__":
    main()
