# omarchy-touchbar

Native Omarchy Touch Bar for Apple Silicon Macs (M1/M2).

Hardware (this machine, MacBookPro17,1):
- Display: DRM `/dev/dri/card2`, connector `DSI-1` (id 39), mode `60x2008`
- Input: `/dev/input/event3` ("MacBookPro17,1 Touch Bar")
- Current driver: `tiny-dfr` 0.3.7 (static F-keys / media keys only)

Goal (inspired by T1Bridge "render whatever you want" API):
- Show open workspaces, tap to focus
- Show window app icons (terminal / chrome / ...)
- Theme switcher button following Omarchy `colors.toml`
- Fully native Omarchy look (Catppuccin first)

Plan (step by step):
1. `poc/` — offline cairo render (no root) + hardware modeset test via `modetest` (needs sudo, user-run)
2. `daemon/` — Python daemon: DRM dumb-buffer + cairo render, evdev tap handling, Hyprland IPC, theme watcher (next step)
3. Polish: backlight, systemd unit, escape-key safety

The T1Bridge post you linked is T1/USB hardware; this repo targets M1/DRM.
The architecture (DRM + cairo + feeds + IPC) is borrowed from the T1 `dfrd` idea,
reimplemented against `tiny-dfr`'s DRM backend (`display.rs`).
