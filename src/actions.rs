//! Hardware + desktop actions (port of touchbar_daemon helpers).

use std::process::{Command, Stdio};

fn run_out(args: &[&str], timeout_secs: u64) -> String {
    // std has no timeout; commands used here are all fast local queries.
    let _ = timeout_secs;
    let Ok(out) = Command::new(args[0]).args(&args[1..]).output() else {
        return String::new();
    };
    if out.status.success() {
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        String::new()
    }
}

fn spawn(args: &[&str]) {
    if args.is_empty() {
        return;
    }
    let _ = Command::new(args[0])
        .args(&args[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub fn spawn_shell(cmd: &str) {
    let _ = Command::new("sh")
        .args(["-c", cmd])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

// --- sliders ---

pub fn get_display() -> i32 {
    run_out(&["omarchy", "brightness", "display"], 8)
        .trim_end_matches('%')
        .parse::<f64>()
        .map(|v| v as i32)
        .unwrap_or(60)
}

pub fn set_display(n: i32) {
    let n = n.clamp(1, 100);
    spawn(&["omarchy", "brightness", "display", "--no-osd", &format!("{n}%")]);
}

pub fn get_volume() -> i32 {
    run_out(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"], 8)
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| (v * 100.0) as i32)
        .unwrap_or(50)
}

pub fn set_volume(n: i32) {
    let n = n.clamp(0, 100);
    spawn(&[
        "wpctl",
        "set-volume",
        "@DEFAULT_AUDIO_SINK@",
        &format!("{:.2}", n as f64 / 100.0),
    ]);
}

pub fn get_kbd() -> i32 {
    let v: i32 = run_out(&["brightnessctl", "-d", "kbd_backlight", "get"], 8)
        .parse()
        .unwrap_or(51);
    let m: i32 = run_out(&["brightnessctl", "-d", "kbd_backlight", "max"], 8)
        .parse()
        .unwrap_or(255);
    if m <= 0 {
        return 20;
    }
    (v * 100 / m).clamp(0, 100)
}

pub fn set_kbd(n: i32) {
    let n = n.clamp(0, 100);
    spawn(&[
        "brightnessctl",
        "-d",
        "kbd_backlight",
        "set",
        &format!("{}", (n as f64 / 100.0 * 255.0) as i32),
    ]);
}

// --- mic ---

pub fn get_mic_muted() -> bool {
    run_out(&["pactl", "get-source-mute", "@DEFAULT_SOURCE@"], 5)
        .to_lowercase()
        .contains("yes")
}

pub fn toggle_mic() {
    spawn(&["omarchy", "audio", "input", "mute"]);
}

// --- night light ---

pub fn get_night() -> bool {
    let out = run_out(&["omarchy", "toggle", "nightlight", "--status"], 8);
    serde_json::from_str::<serde_json::Value>(&out)
        .ok()
        .and_then(|v| v.get("enabled").and_then(|e| e.as_bool()))
        .unwrap_or(false)
}

pub fn toggle_night() {
    spawn(&["omarchy", "toggle", "nightlight"]);
}

// --- media (MPRIS) ---

pub fn mpris_players() -> Vec<String> {
    let out = run_out(&["busctl", "--user", "list"], 5);
    let mut players: Vec<String> = out
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|s| s.contains("org.mpris.MediaPlayer2."))
        .map(|s| s.to_string())
        .collect();
    players.sort();
    players.dedup();
    players
}

/// "Playing" | "Paused" | None
pub fn mpris_status() -> Option<String> {
    let mut any = false;
    for bus in mpris_players() {
        any = true;
        let out = run_out(
            &[
                "busctl",
                "--user",
                "get-property",
                &bus,
                "/org/mpris/MediaPlayer2",
                "org.mpris.MediaPlayer2.Player",
                "PlaybackStatus",
            ],
            5,
        );
        if out.contains("\"Playing\"") {
            return Some("Playing".into());
        }
    }
    if any {
        Some("Paused".into())
    } else {
        None
    }
}

pub fn mpris_toggle() {
    for bus in mpris_players() {
        spawn(&[
            "busctl",
            "--user",
            "call",
            &bus,
            "/org/mpris/MediaPlayer2",
            "org.mpris.MediaPlayer2.Player",
            "PlayPause",
        ]);
    }
}

// --- app launcher ---

/// Launch a configured app: url opens via omarchy-launch-webapp,
/// otherwise the shell command runs detached.
pub fn launch_app(command: Option<&str>, url: Option<&str>, id: &str) {
    if let Some(u) = url.map(str::trim).filter(|s| !s.is_empty()) {
        // absolute path: the service PATH already has omarchy/bin,
        // but don't depend on it.
        let launcher = "/usr/share/omarchy/bin/omarchy-launch-webapp";
        if std::path::Path::new(launcher).exists() {
            spawn(&[launcher, u]);
        } else {
            spawn(&["omarchy-launch-webapp", u]);
        }
        println!("action: launch {id} ({u})");
        return;
    }
    if let Some(c) = command.map(str::trim).filter(|s| !s.is_empty()) {
        spawn_shell(c);
        println!("action: launch {id}");
        return;
    }
    eprintln!("launch: app '{id}' has neither command nor url");
}

// --- lock ---

/// Lock the session. `override_cmd` from config wins; otherwise try
/// hyprlock, then loginctl.
pub fn lock_session(override_cmd: Option<&str>) {
    if let Some(cmd) = override_cmd {
        if !cmd.trim().is_empty() {
            spawn_shell(cmd);
            return;
        }
    }
    for candidate in ["hyprlock", "omarchy lock", "loginctl lock-session"] {
        // probe existence for bare binaries
        let first = candidate.split_whitespace().next().unwrap_or(candidate);
        let probe = Command::new("sh")
            .args(["-c", &format!("command -v {first}")])
            .output();
        let found = probe.map(|o| o.status.success()).unwrap_or(false);
        if found {
            spawn_shell(candidate);
            return;
        }
    }
    eprintln!("lock: no lock command found (install hyprlock?)");
}

// --- weather ---

fn weather_cache_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    std::path::PathBuf::from(home).join(".cache/omarchy-touchbar/weather")
}

/// Cached temperature like "21°C" (max 5 chars), "" when never fetched.
pub fn weather_read() -> String {
    let raw = std::fs::read_to_string(weather_cache_path()).unwrap_or_default();
    raw.chars()
        .filter(|c| c.is_ascii_digit() || *c == '-' || *c == '°' || *c == 'C' || *c == 'F')
        .take(5)
        .collect()
}

/// Cache mtime (unix secs, 0 when missing) — the live loop watches this
/// to re-render when a background refresh lands.
pub fn weather_mtime() -> u64 {
    std::fs::metadata(weather_cache_path())
        .and_then(|m| m.modified())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// Fire-and-forget wttr.in refresh into the cache (never blocks the bar).
/// Uses Omarchy's stored location, else wttr.in auto-detects by IP.
pub fn refresh_weather() {
    let cache = weather_cache_path();
    let dir = cache.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let file = cache.to_string_lossy().to_string();
    spawn_shell(&format!(
        "loc=$(omarchy-weather-location 2>/dev/null); \
         if [ -n \"$loc\" ]; then q=$(jq -rn --arg p \"$loc\" '$p|@uri'); else q=\"\"; fi; \
         t=$(curl -fsS --max-time 4 \"https://wttr.in/${{q}}?format=%t\" 2>/dev/null | tr -d '+ \\n'); \
         case \"$t\" in *°C|*°F) mkdir -p \"{dir}\" && printf '%s' \"$t\" > \"{file}.tmp\" \
           && mv \"{file}.tmp\" \"{file}\" && echo \"weather: $t\" >&2;; \
         *) echo \"weather: fetch failed\" >&2;; esac"
    ));
}
