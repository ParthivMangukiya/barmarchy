# barmarchy

Native Touch Bar daemon for Omarchy on Apple Silicon Macs — workspaces, app
icons, sliders, F-keys, themes, a pixel pet, and a particle screensaver,
all rendered straight to the bar in Rust. Replaces `tiny-dfr`.

## Install (one command)

```bash
curl -fsSL https://raw.githubusercontent.com/ParthivMangukiya/barmarchy/main/install.sh | bash
```

Requirements: Apple Silicon MacBook Pro with Touch Bar running
Asahi/Omarchy (M1 13" J293 verified; M2 13" J493 uses the same panel and is
expected to work — please report). No hardcoded `/dev` nodes: the daemon
probes at startup for the 2008x60 DRM panel, the `* Touch Bar` input, its
live touch ranges, and the `Apple SPI Keyboard`. User must be in `video` +
`input` groups (the installer adds you; log out/in once if it did). The installer builds the release binary,
installs `~/.local/bin/omarchy-touchbar`, enables the user service, and
masks stock `tiny-dfr`. Uninstall: `./install.sh --uninstall`.

## What's on the bar

- **Workspaces** — tap to focus, app icons per workspace, accent on active
- **Clock + pixel pet** — a pixel cat that idles, winks, dances, plays ball,
  eats, sleeps, and goes Happy (hearts + blush) when you tap it
- **Weather** — pixel temperature next to brightness, tap to refresh
- **Controls** — display/volume/keyboard sliders, media, mic, all accent
- **Theme button + picker** — follows your Omarchy theme
- **Overlays** — hold `Fn` for F1–F12, `Super+Ctrl` themes, `Super+Shift` apps
- **Screensaver mirror** — while the desktop TTE screensaver runs, the bar
  plays its own 5-effect particle show (assemble, decrypt, rain, beams,
  blackhole), alternating OMARCHY and the clock

## Configure

`~/.config/omarchy-touchbar/config.toml` (auto-created with defaults):

```toml
[bar]
workspaces = 5
show_clock = true
show_weather = true
show_theme = true

[[button]]          # slider | media | mic | night | lock | command
id = "volume"
kind = "slider"
target = "volume"

[[app]]             # Super+Shift launcher
id = "yt"
name = "YouTube"
url = "https://youtube.com/"
```

Restart after edits: `systemctl --user restart omarchy-touchbar.service`.

Hardware overrides (only needed if probing picks wrong on your model):
`BARMARCHY_DRM=/dev/dri/card1`, `BARMARCHY_TOUCH=/dev/input/event5`,
`BARMARCHY_KBD=/dev/input/event2` — set via
`systemctl --user edit omarchy-touchbar.service` (`[Service] Environment=`).

## Hack on it

```bash
touchtest --live 60   # build + run on hardware (service auto-restores)
touchtest --saver 20  # screensaver preview on the bar
./target/release/omarchy-touchbar --png /tmp/bar.png --view saver:rain:2.2
```

`dev/` has the systemd unit template and service installer.
`daemon/` + `poc/` are the Python ancestors, kept for reference.

MIT — vibecoded with Muse Spark.
