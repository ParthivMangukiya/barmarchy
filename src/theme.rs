//! Omarchy theme resolution (port of touchbar_daemon.py theme section).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

pub type Rgb = (f64, f64, f64);

pub fn hex(s: &str) -> Rgb {
    let h = s.trim().trim_start_matches('#');
    let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0xff) as f64 / 255.0;
    if h.len() < 6 {
        return (1.0, 1.0, 1.0);
    }
    (v(0), v(2), v(4))
}

pub const FALLBACK: &[(&str, &str)] = &[
    ("background", "#1e1e2e"),
    ("lighter_background", "#313244"),
    ("accent", "#89b4fa"),
    ("foreground", "#cdd6f4"),
    ("darker_background", "#101019"),
    ("red", "#f38ba8"),
    ("green", "#a6e3a1"),
];

fn fallback(key: &str) -> String {
    FALLBACK
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| "#ffffff".into())
}

fn theme_bases() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    // System dir follows OMARCHY_PATH like omarchy's own helpers do
    // (omarchy_env::ensure sets it at startup; dev-link overrides honored).
    let system = std::env::var("OMARCHY_PATH")
        .map(|p| format!("{}/themes", p.trim_end_matches('/')))
        .unwrap_or_else(|_| "/usr/share/omarchy/themes".into());
    vec![
        PathBuf::from(system),
        PathBuf::from(format!("{home}/.config/omarchy/themes")),
    ]
}

/// {lowername-with-dashes: path}, user dir wins.
pub fn theme_dirs() -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    for base in theme_bases() {
        let entries = std::fs::read_dir(&base);
        if let Ok(entries) = entries {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() {
                    out.entry(name.to_lowercase()).or_insert(e.path());
                }
            }
        }
    }
    // user wins
    if let Some(user) = theme_bases().last() {
        if let Ok(entries) = std::fs::read_dir(user) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() {
                    out.insert(name.to_lowercase(), e.path());
                }
            }
        }
    }
    out
}

fn read_toml_table(path: &PathBuf) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    let Ok(val) = text.parse::<toml::Value>() else {
        return out;
    };
    fn walk(prefix: &str, v: &toml::Value, out: &mut HashMap<String, String>) {
        match v {
            toml::Value::String(s) => {
                out.insert(prefix.to_string(), s.clone());
            }
            toml::Value::Table(t) => {
                for (k, v) in t {
                    let key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(&key, v, out);
                }
            }
            _ => {}
        }
    }
    walk("", &val, &mut out);
    out
}

/// Flat color map for a theme display name.
pub fn colors_for(name: &str) -> HashMap<String, String> {
    let mut colors: HashMap<String, String> =
        FALLBACK.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let key = name.to_lowercase().replace(' ', "-");
    let dirs = theme_dirs();
    let Some(dir) = dirs.get(&key) else {
        return colors;
    };
    let flat = read_toml_table(&dir.join("colors.toml"));
    if !flat.is_empty() {
        for (k, v) in flat {
            // keep only top-level color keys (skip nested like colors.normal.blue)
            if !k.contains('.') {
                colors.insert(k, v);
            }
        }
    } else {
        // fallback to alacritty blue (e.g. Mars has no colors.toml)
        let a = read_toml_table(&dir.join("alacritty.toml"));
        for k in ["colors.normal.blue", "colors.bright.blue", "blue"] {
            if let Some(b) = a.get(k) {
                colors.insert("accent".into(), b.clone());
                colors.insert("blue".into(), b.clone());
                break;
            }
        }
    }
    colors
}

fn luminance(hexcolor: &str) -> f64 {
    let (r, g, b) = hex(hexcolor);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

pub fn is_light(name: &str) -> bool {
    let c = colors_for(name);
    if c.get("mode").map(|m| m.to_lowercase()) == Some("light".into()) {
        return true;
    }
    luminance(c.get("background").map(|s| s.as_str()).unwrap_or("#000000")) > 0.55
}

pub fn theme_list() -> Vec<String> {
    let out = Command::new("omarchy")
        .args(["theme", "list"])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => vec!["Catppuccin".into()],
    }
}

pub fn current_theme() -> String {
    let out = Command::new("omarchy")
        .args(["theme", "current"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                "Catppuccin".into()
            } else {
                s
            }
        }
        _ => "Catppuccin".into(),
    }
}

pub fn set_theme(name: &str) {
    // Detached: theme switching runs long post-hooks (retints, preloads).
    // Reaped on a side thread so the child never lingers as a zombie.
    if let Ok(child) = Command::new("omarchy")
        .args(["theme", "set", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        crate::omarchy_env::detach(child);
    }
}

pub fn fav_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.config/omarchy-touchbar/themes"))
}

const MENU_CAP: usize = 10;

/// Preferred themes -> [(name, accent_rgb)], padded to MENU_CAP.
pub fn get_favs() -> Vec<(String, Rgb)> {
    let mut names: Vec<String> = vec![];
    let mut excluded: Vec<String> = vec![];
    if let Ok(text) = std::fs::read(&fav_path()).map(|b| String::from_utf8_lossy(&b).into_owned()) {
        for l in text.lines() {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if let Some(rest) = l.strip_prefix('!') {
                excluded.push(rest.trim().to_string());
            } else {
                names.push(l.to_string());
            }
        }
    }
    names.retain(|n| !is_light(n));
    if names.len() < MENU_CAP {
        let cur = current_theme();
        let all = theme_list();
        let start = all.iter().position(|n| *n == cur).unwrap_or(0);
        for k in 0..all.len() {
            let n = &all[(start + k) % all.len().max(1)];
            if !names.contains(n) && !excluded.contains(n) && !is_light(n) {
                names.push(n.clone());
            }
            if names.len() >= MENU_CAP {
                break;
            }
        }
        if let Some(parent) = fav_path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut body = String::from(
            "# preferred touchbar themes (one per line, shown in the theme menu;\n# prefix with ! to exclude one forever)\n",
        );
        let mut excl = excluded.clone();
        excl.sort();
        for e in &excl {
            body.push_str(&format!("!{e}\n"));
        }
        body.push_str(&names.iter().take(MENU_CAP).cloned().collect::<Vec<_>>().join("\n"));
        body.push('\n');
        let _ = std::fs::write(fav_path(), body);
    }
    names
        .into_iter()
        .take(MENU_CAP)
        .map(|n| {
            let c = colors_for(&n);
            let accent = c
                .get("accent")
                .or_else(|| c.get("blue"))
                .cloned()
                .unwrap_or_else(|| fallback("accent"));
            (n, hex(&accent))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub bg: Rgb,
    pub pill: Rgb,
    pub accent: Rgb,
    pub fg: Rgb,
    pub dark: Rgb,
    pub red: Rgb,
    pub green: Rgb,
    pub dim: Rgb,
}

pub fn load_theme() -> Theme {
    let name = current_theme();
    let colors = colors_for(&name);
    let pick = |keys: &[&str]| -> Rgb {
        for k in keys {
            if let Some(v) = colors.get(*k) {
                return hex(v);
            }
        }
        hex(&fallback(keys[0]))
    };
    Theme {
        name,
        bg: pick(&["background", "dark_background"]),
        pill: pick(&["lighter_background", "selection"]),
        accent: pick(&["accent", "blue"]),
        fg: pick(&["foreground", "light_foreground"]),
        dark: pick(&["darker_background", "dark_background"]),
        red: pick(&["red", "bright_red"]),
        green: pick(&["green", "bright_green"]),
        dim: pick(&["dark_foreground", "muted"]),
    }
}

/// Throttle helper for theme-dir rescans (60s in python; kept simple here).
pub struct Throttle {
    last: Option<Instant>,
    interval: Duration,
}

impl Throttle {
    pub fn new(secs: u64) -> Self {
        Self {
            last: None,
            interval: Duration::from_secs(secs),
        }
    }
    pub fn ready(&mut self) -> bool {
        match self.last {
            None => {
                self.last = Some(Instant::now());
                true
            }
            Some(t) if t.elapsed() >= self.interval => {
                self.last = Some(Instant::now());
                true
            }
            _ => false,
        }
    }
}
