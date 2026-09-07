//! User config: ~/.config/omarchy-touchbar/config.toml
//!
//! Users reorder/add/remove buttons freely. Example:
//!
//! ```toml
//! [bar]
//! workspaces = 5
//! show_clock = true
//! show_weather = true
//! show_theme = true
//! menu_timeout_secs = 5.0
//! slider_timeout_secs = 3.0
//!
//! [[button]]
//! id = "display"
//! kind = "slider"
//! target = "display"
//! icon = "\u{F185}"
//!
//! [[button]]
//! id = "lock"
//! kind = "lock"
//! icon = "󰌾"
//!
//! [[button]]
//! id = "screenshot"
//! kind = "command"
//! icon = ""
//! command = "omarchy screenshot"
//! ```

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".config/omarchy-touchbar/config.toml")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Bar {
    #[serde(default = "d_workspaces")]
    pub workspaces: usize,
    #[serde(default = "d_true")]
    pub show_clock: bool,
    /// pixel-style temperature next to the clock (tap = refresh now)
    #[serde(default = "d_true")]
    pub show_weather: bool,
    #[serde(default = "d_true")]
    pub show_theme: bool,
    /// Theme-picker and app-launcher buttons: true (default) stretches
    /// them to fill the bar; false gives them the same fixed width as
    /// the strip buttons, centered.
    #[serde(default = "d_true")]
    pub menu_expand: bool,
    #[serde(default = "d_menu_timeout")]
    pub menu_timeout_secs: f64,
    #[serde(default = "d_slider_timeout")]
    pub slider_timeout_secs: f64,
}

fn d_workspaces() -> usize {
    5
}
fn d_true() -> bool {
    true
}
fn d_menu_timeout() -> f64 {
    5.0
}
fn d_slider_timeout() -> f64 {
    3.0
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            workspaces: 5,
            show_clock: true,
            show_weather: true,
            show_theme: true,
            menu_expand: true,
            menu_timeout_secs: 5.0,
            slider_timeout_secs: 3.0,
        }
    }
}

/// A single control button in the middle of the strip.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Button {
    /// unique id, e.g. "display", "lock", "vpn"
    pub id: String,
    /// slider | media | mic | night | lock | command
    pub kind: String,
    /// for slider: display | volume | kbd
    #[serde(default)]
    pub target: Option<String>,
    /// nerd-font glyph rendered on the button
    #[serde(default)]
    pub icon: Option<String>,
    /// for command: shell command to run; for lock: override lock command
    #[serde(default)]
    pub command: Option<String>,
    /// human label (used in slider view / logs)
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub bar: Bar,
    #[serde(default = "default_buttons")]
    pub button: Vec<Button>,
    /// App launcher menu (Super+Shift overlay). Add your own entries freely:
    /// command = shell command, or url = opened via omarchy-launch-webapp.
    #[serde(default = "default_apps")]
    pub app: Vec<AppEntry>,
}

/// One entry in the app launcher menu (Super+Shift overlay).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// nerd-font glyph shown on the button (default per known id,
    /// else first letter of the name).
    #[serde(default)]
    pub icon: Option<String>,
    /// font for the icon: "nerd" (default) or "sans".
    #[serde(default)]
    pub icon_font: Option<String>,
}

pub fn default_apps() -> Vec<AppEntry> {
    let cmd = |id: &str, name: &str, command: &str, icon: &str| AppEntry {
        id: id.into(),
        name: name.into(),
        command: Some(command.into()),
        url: None,
        icon: Some(icon.into()),
        icon_font: None,
    };
    let web = |id: &str, name: &str, url: &str, icon: &str| AppEntry {
        id: id.into(),
        name: name.into(),
        command: None,
        url: Some(url.into()),
        icon: Some(icon.into()),
        icon_font: None,
    };
    vec![
        cmd("brave", "Brave", "brave", "\u{E639}"), // U+E639 script B
        cmd("chrome", "Chrome", "chromium", "\u{F268}"), // U+F268
        cmd("terminal", "Terminal", "xdg-terminal-exec", "\u{F489}"), // U+F489
        cmd("files", "Files", "nautilus", "\u{F07B}"), // U+F07B folder
        cmd("localsend", "LocalSend", "localsend", "\u{F1D8}"), // U+F1D8 paper plane
        web("youtube", "YouTube", "https://youtube.com/", "\u{F16A}"), // U+F16A
        web("whatsapp", "WhatsApp", "https://web.whatsapp.com/", "\u{F232}"), // U+F232
        cmd("obsidian", "Obsidian", "obsidian", "\u{E63A}"), // U+E63A cubes
        // X uses the Sans font (set below) — the brand glyph has no nerd slot.
        AppEntry {
            id: "x".into(),
            name: "X".into(),
            command: None,
            url: Some("https://x.com/".into()),
            icon: Some("X".into()),
            icon_font: Some("sans".into()),
        },
    ]
}

pub fn default_buttons() -> Vec<Button> {
    vec![
        Button {
            id: "display".into(),
            kind: "slider".into(),
            target: Some("display".into()),
            icon: Some("\u{F185}".into()),
            command: None,
            label: Some("Display".into()),
        },
        Button {
            id: "volume".into(),
            kind: "slider".into(),
            target: Some("volume".into()),
            icon: Some("".into()),
            command: None,
            label: Some("Volume".into()),
        },
        Button {
            id: "kbd".into(),
            kind: "slider".into(),
            target: Some("kbd".into()),
            icon: Some("".into()),
            command: None,
            label: Some("Keys".into()),
        },
        Button {
            id: "media".into(),
            kind: "media".into(),
            target: None,
            icon: None,
            command: None,
            label: Some("Media".into()),
        },
        Button {
            id: "mic".into(),
            kind: "mic".into(),
            target: None,
            icon: None,
            command: None,
            label: Some("Mic".into()),
        },
    ]
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bar: Bar::default(),
            button: default_buttons(),
            app: default_apps(),
        }
    }
}

const HEADER: &str = r#"# omarchy-touchbar config — edit freely, changes apply on restart.
# [bar] options: workspaces (1-9), show_clock, show_weather, show_theme,
# menu_timeout_secs, slider_timeout_secs, and menu_expand (true = theme/app
# menu buttons stretch to fill the bar; false = same fixed width as the
# strip buttons, centered).
# Reorder [[button]] entries to reorder the strip. Remove ones you don't
# want, add your own:
#
#   [[button]]
#   id = "vpn"
#   kind = "command"        # slider | media | mic | night | lock | command
#   icon = "\u{F0582}"
#   command = "omarchy toggle vpn"
#
# kinds:
#   slider  needs target = "display" | "volume" | "kbd"  (drag to adjust)
#   media   play/pause toggle (icon follows player state)
#   mic     mic mute toggle (red when muted)
#   night   nightlight toggle (accent when on)
#   lock    lock the session (command = override, default: hyprlock)
#   command runs `sh -c '<command>'`
#
# [[app]] entries are the Super+Shift launcher overlay (tap to launch).
# Each needs name + either command (shell) or url (opens as a webapp).
# Optional icon = nerd-font glyph (known ids have built-in icons),
# icon_font = "sans" to render the icon in the Sans font instead:
#
#   [[app]]
#   id = "calc"
#   name = "Calc"
#   command = "gnome-calculator"
#   icon = "\u{F1EC}"
#
"#;

pub fn load() -> Config {
    let path = config_path();
    if !path.exists() {
        let cfg = Config::default();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match toml::to_string_pretty(&cfg) {
            Ok(body) => {
                let _ = std::fs::write(&path, format!("{HEADER}{body}"));
                eprintln!("config: wrote defaults to {}", path.display());
            }
            Err(e) => eprintln!("config: serialize defaults failed: {e}"),
        }
        return cfg;
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            // migrate: append default [[app]] entries once so users can
            // discover + edit them (Super+Shift launcher).
            if !text.contains("[[app]]") {
                let apps_toml = toml::to_string_pretty(&Config {
                    bar: Bar::default(),
                    button: vec![],
                    app: default_apps(),
                })
                .unwrap_or_default()
                .lines()
                .skip_while(|l| !l.starts_with("[[app]]"))
                .collect::<Vec<_>>()
                .join("\n");
                if !apps_toml.is_empty() {
                    let _ = std::fs::write(&path, format!("{text}\n{apps_toml}\n"));
                    eprintln!("config: appended default [[app]] entries to {}", path.display());
                }
            }
            match toml::from_str::<Config>(&std::fs::read_to_string(&path).unwrap_or(text)) {
            Ok(mut cfg) => {
                if cfg.bar.workspaces == 0 || cfg.bar.workspaces > 9 {
                    eprintln!("config: workspaces out of range, using 5");
                    cfg.bar.workspaces = 5;
                }
                if cfg.button.is_empty() {
                    eprintln!("config: no buttons, restoring defaults");
                    cfg.button = default_buttons();
                }
                cfg
            }
            Err(e) => {
                eprintln!("config: parse error in {}: {e}; using defaults", path.display());
                Config::default()
            }
            }
        }
        Err(e) => {
            eprintln!("config: read error: {e}; using defaults");
            Config::default()
        }
    }
}
