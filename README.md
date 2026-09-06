# barmarchy

Native Touch Bar daemon for Omarchy on Apple Silicon Macs — workspaces, app
icons, sliders, F-keys, themes, a pixel pet, and a particle screensaver,
all rendered straight to the bar in Rust. Replaces `tiny-dfr`.

## Install (one command)

```bash
curl -fsSL https://raw.githubusercontent.com/ParthivMangukiya/barmarchy/main/install.sh | bash
```

Requirements: Apple Silicon MacBook Pro with Touch Bar (M1/M2 13",
Asahi/Omarchy), user in `video` + `input` groups (the installer adds you;
log out/in once if it did). The installer builds the release binary,
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

## Hack on it

```bash
touchtest --live 60   # build + run on hardware (service auto-restores)
touchtest --saver 20  # screensaver preview on the bar
./target/release/omarchy-touchbar --png /tmp/bar.png --view saver:rain:2.2
```

`dev/` has the systemd unit template and service installer.
`daemon/` + `poc/` are the Python ancestors, kept for reference.

MIT — vibecoded with Muse Spark.
