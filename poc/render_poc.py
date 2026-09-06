#!/usr/bin/env python3
"""Offline Touch Bar PoC render — no root needed.

Draws a 2008x60 strip with pycairo using live Hyprland state +
current Omarchy theme colors, proving the render path before we
take over the real DRM framebuffer.
"""
import json
import subprocess
import datetime
import os

import cairo

W, H = 2008, 60

# Catppuccin (dark) — from /usr/share/omarchy/themes/catppuccin/colors.toml
BG = (0x1E / 255, 0x1E / 255, 0x2E / 255)
PILL = (0x31 / 255, 0x32 / 255, 0x44 / 255)
ACCENT = (0x89 / 255, 0xB4 / 255, 0xFA / 255)
FG = (0xCD / 255, 0xD6 / 255, 0xF4 / 255)
DARK = (0x10 / 255, 0x10 / 255, 0x19 / 255)
RED = (0xF3 / 255, 0x8B / 255, 0xA8 / 255)
GREEN = (0xA6 / 255, 0xE3 / 255, 0xA1 / 255)


def hypr(cmd):
    try:
        out = subprocess.check_output(
            ["hyprctl", cmd, "-j"], timeout=3, text=True)
        return json.loads(out)
    except Exception:
        return None


def rounded(ctx, x, y, w, h, r):
    ctx.new_sub_path()
    ctx.arc(x + r, y + r, r, 3.14159, 1.5 * 3.14159)
    ctx.arc(x + w - r, y + r, r, 1.5 * 3.14159, 0)
    ctx.arc(x + w - r, y + h - r, r, 0, 0.5 * 3.14159)
    ctx.arc(x + r, y + h - r, r, 0.5 * 3.14159, 3.14159)
    ctx.close_path()


def button(ctx, x, y, w, h, fill, label, fg, bold=True):
    rounded(ctx, x, y, w, h, 10)
    ctx.set_source_rgb(*fill)
    ctx.fill_preserve()
    ctx.set_source_rgba(*FG, 0.25)
    ctx.set_line_width(1.5)
    ctx.stroke()
    ctx.select_font_face("Sans", cairo.FONT_SLANT_NORMAL,
                         cairo.FONT_WEIGHT_BOLD if bold else cairo.FONT_WEIGHT_NORMAL)
    ctx.set_font_size(22)
    ext = ctx.text_extents(label)
    ctx.move_to(x + (w - ext.width) / 2 - ext.x_bearing,
                y + (h - ext.height) / 2 - ext.y_bearing)
    ctx.set_source_rgb(*fg)
    ctx.show_text(label)


def main():
    workspaces = hypr("workspaces") or [{"id": 1, "windows": 2}]
    clients = hypr("clients") or []
    open_ids = sorted({w["id"] for w in workspaces})
    active = open_ids[0] if open_ids else 1
    apps = [c.get("class", "?") for c in clients][:4]

    surf = cairo.ImageSurface(cairo.FORMAT_ARGB32, W, H)
    ctx = cairo.Context(surf)
    ctx.set_source_rgb(*BG)
    ctx.paint()

    x = 8
    # ESC — safety, always present
    button(ctx, x, 8, 110, 44, (0x45 / 255, 0x28 / 255, 0x35 / 255),
           "ESC", RED)
    x += 122

    # Workspaces 1..5, open ones lit, active filled
    for i in range(1, 6):
        is_open = i in open_ids
        is_active = i == active
        fill = ACCENT if is_active else (PILL if is_open else DARK)
        fg = DARK if is_active else FG
        label = f"{i} {'●' if is_open else '○'}"
        button(ctx, x, 8, 120, 44, fill, label, fg)
        x += 130

    # Center: window icons as text pills
    ctx.select_font_face("Sans", cairo.FONT_SLANT_NORMAL,
                        cairo.FONT_WEIGHT_NORMAL)
    ctx.set_font_size(22)
    icon_text = "  ".join(
        {"org.omarchy.agent": "◈ agent",
         "foot": "▮ terminal",
         "chrome-x.com__-Default": "◉ chrome"}.get(a, f"○ {a}")
        for a in apps) or "○ empty"
    ext = ctx.text_extents(icon_text)
    ctx.set_source_rgb(*FG)
    ctx.move_to((W - ext.width) / 2, 39)
    ctx.show_text(icon_text)

    # Right: theme switcher + clock
    now = datetime.datetime.now().strftime("%H:%M")
    button(ctx, W - 330, 8, 190, 44, PILL, "◑ theme", FG)
    button(ctx, W - 130, 8, 122, 44, PILL, now, GREEN)

    out = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "touchbar-poc.png")
    surf.write_to_png(out)
    print(f"wrote {out} ({W}x{H})")
    print(f"workspaces={open_ids} active={active} apps={apps}")


if __name__ == "__main__":
    main()
