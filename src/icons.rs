//! Real app icons, the way the Omarchy menu does it:
//! WM_CLASS -> .desktop file -> Icon= -> icon-theme file -> rasterized PNG.
//!
//! Anything that fails at any step falls back to the nerd-font glyphs in
//! render.rs, so the bar never shows a hole.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/root".into())
}

/// Directories searched for .desktop files.
fn desktop_dirs() -> Vec<PathBuf> {
    let h = home();
    [
        format!("{h}/.local/share/applications"),
        "/usr/share/applications".into(),
        format!("{h}/.local/share/flatpak/exports/share/applications"),
        "/var/lib/flatpak/exports/share/applications".into(),
        "/var/lib/snapd/desktop/applications".into(),
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

/// Base dirs searched for themed icons.
fn icon_bases() -> Vec<PathBuf> {
    let h = home();
    [
        format!("{h}/.icons"),
        format!("{h}/.local/share/icons"),
        "/usr/share/icons".into(),
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn icon_cache_dir() -> PathBuf {
    PathBuf::from(home()).join(".cache/omarchy-touchbar/icons")
}

/// Current GTK icon theme (what the Omarchy menu uses), cached.
fn icon_theme() -> String {
    static THEME: OnceLock<String> = OnceLock::new();
    THEME
        .get_or_init(|| {
            let out = std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.interface", "icon-theme"])
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    let s = String::from_utf8_lossy(&o.stdout)
                        .trim()
                        .trim_matches('\'')
                        .to_string();
                    if s.is_empty() { "hicolor".into() } else { s }
                }
                _ => "hicolor".into(),
            }
        })
        .clone()
}

/// Theme + its Inherited chain (index.theme), always ending at hicolor.
fn theme_chain() -> Vec<String> {
    let mut chain = vec![icon_theme()];
    for _ in 0..4 {
        let last = chain.last().cloned().unwrap_or_default();
        if last.eq_ignore_ascii_case("hicolor") {
            break;
        }
        let mut inherited: Vec<String> = vec![];
        for base in icon_bases() {
            let idx = base.join(&last).join("index.theme");
            let Ok(text) = std::fs::read_to_string(&idx) else {
                continue;
            };
            for line in text.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("Inherits=") {
                    inherited = rest
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    break;
                }
            }
            if !inherited.is_empty() {
                break;
            }
        }
        if inherited.is_empty() {
            break;
        }
        for t in inherited {
            if !chain.iter().any(|c| c.eq_ignore_ascii_case(&t)) {
                chain.push(t);
            }
        }
    }
    if !chain.iter().any(|c| c.eq_ignore_ascii_case("hicolor")) {
        chain.push("hicolor".into());
    }
    chain
}

#[derive(Default)]
struct DesktopInfo {
    wmclass: String,
    name: String,
    icon: String,
}

fn parse_desktop(path: &Path) -> Option<DesktopInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut info = DesktopInfo::default();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "StartupWMClass" => info.wmclass = v.trim().to_string(),
                "Name" => {
                    if info.name.is_empty() {
                        info.name = v.trim().to_string();
                    }
                }
                "Icon" => {
                    if info.icon.is_empty() {
                        info.icon = v.trim().to_string();
                    }
                }
                _ => {}
            }
        }
    }
    if info.icon.is_empty() {
        return None;
    }
    Some(info)
}

/// Icon= value from the .desktop file best matching a WM class.
fn desktop_icon_for_class(class: &str) -> Option<String> {
    let low = class.to_lowercase();
    let short = low.rsplit('.').next().unwrap_or(&low).to_string();
    let mut name_hit: Option<String> = None;
    let mut stem_hit: Option<String> = None;
    for dir in desktop_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("desktop") {
                continue;
            }
            let stem = p
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("")
                .to_lowercase();
            let Some(info) = parse_desktop(&p) else {
                continue;
            };
            if !info.wmclass.is_empty() && info.wmclass.to_lowercase() == low {
                return Some(info.icon);
            }
            if stem_hit.is_none() && (stem == low || stem == short) {
                stem_hit = Some(info.icon.clone());
            }
            if name_hit.is_none() && info.name.to_lowercase() == low {
                name_hit = Some(info.icon.clone());
            }
        }
    }
    stem_hit.or(name_hit)
}

/// Rank a "<N>x<N>" size dir (lower is better).
fn size_rank(dir: &str) -> i32 {
    match dir {
        "48x48" => 0,
        "32x32" => 1,
        "64x64" => 2,
        "24x24" => 3,
        "scalable" => 4,
        "22x22" => 5,
        "16x16" => 6,
        "symbolic" => 7,
        _ => {
            if dir.ends_with("x48") || dir.ends_with("x32") {
                return 2;
            }
            8
        }
    }
}

fn ext_rank(ext: &str) -> i32 {
    match ext {
        "png" => 0,
        "svg" => 1,
        "xpm" => 2,
        _ => 9,
    }
}

/// Resolve a themed icon name to an image file, like GtkIconTheme.
fn find_icon_file(name: &str) -> Option<PathBuf> {
    fn consider(best: &mut Option<(i32, PathBuf)>, score: i32, path: PathBuf) {
        if path.is_file() && best.as_ref().map(|(s, _)| score < *s).unwrap_or(true) {
            *best = Some((score, path));
        }
    }
    // candidates: exact, then progressively stripped namespaces
    let mut candidates = vec![name.to_string()];
    if let Some(rest) = name.strip_prefix("org.gnome.") {
        candidates.push(rest.to_string());
    }
    if let Some(rest) = name.strip_prefix("com.") {
        candidates.push(rest.to_string());
    }
    if let Some(rest) = name.strip_prefix("io.") {
        candidates.push(rest.to_string());
    }
    if name.contains('.') {
        // last component
        candidates.push(name.rsplit('.').next().unwrap().to_string());
    }
    // dedup
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| seen.insert(c.clone()));

    if name.contains('/') {
        // absolute (or relative) path: try as-is, then with extensions
        let p = PathBuf::from(name);
        if p.is_file() {
            return Some(p);
        }
        for ext in ["png", "svg", "xpm"] {
            let q = p.with_extension(ext);
            if q.is_file() {
                return Some(q);
            }
        }
        return None;
    }
    let mut best: Option<(i32, PathBuf)> = None;
    for theme in theme_chain() {
        for base in icon_bases() {
            let tdir = base.join(&theme);
            let entries = std::fs::read_dir(&tdir).ok();
            // preferred: <size>/apps/<name>.<ext>
            for cand in &candidates {
                for size in [
                    "48x48", "32x32", "64x64", "24x24", "scalable", "22x22", "16x16",
                ] {
                    for ext in ["png", "svg", "xpm"] {
                        consider(
                            &mut best,
                            size_rank(size) * 10 + ext_rank(ext),
                            tdir.join(size).join("apps").join(format!("{cand}.{ext}")),
                        );
                    }
                }
            }
            // any other context dir (devices, mimetypes, ...) as fallback
            if let Some(entries) = entries {
                for e in entries.flatten() {
                    let sd = e.file_name().to_string_lossy().to_string();
                    if !e.path().is_dir() {
                        continue;
                    }
                    for cand in &candidates {
                        for ctx in ["apps", "devices", "actions", "places", "status"] {
                            for ext in ["png", "svg", "xpm"] {
                                consider(
                                    &mut best,
                                    100 + size_rank(&sd) * 10 + ext_rank(ext),
                                    tdir.join(&sd).join(ctx).join(format!("{cand}.{ext}")),
                                );
                            }
                        }
                    }
                    // also search any subdirectory (e.g., scalable/org.gnome.Nautilus/)
                    for cand in &candidates {
                        for ext in ["png", "svg", "xpm"] {
                            consider(
                                &mut best,
                                200 + size_rank(&sd) * 10 + ext_rank(ext),
                                tdir.join(&sd).join(format!("{cand}.{ext}")),
                            );
                        }
                    }
                }
            }
        }
        if best.is_some() {
            return best.map(|(_, p)| p);
        }
    }
    // last resort: pixmaps + bare theme dirs
    for cand in &candidates {
        for ext in ["png", "svg", "xpm"] {
            let p = PathBuf::from("/usr/share/pixmaps").join(format!("{cand}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Rasterize to a cached PNG when the source isn't one cairo reads.
/// Returns the PNG path to draw (source itself for .png).
fn rasterized_png(src: &Path, name: &str) -> Option<PathBuf> {
    let ext = src
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_lowercase();
    if ext == "png" {
        return Some(src.to_path_buf());
    }
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dir = icon_cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let dst = dir.join(format!("{safe}.png"));
    if dst.is_file() {
        return Some(dst);
    }
    let ok = if ext == "svg" {
        std::process::Command::new("rsvg-convert")
            .args([
                "-w",
                "96",
                "-h",
                "96",
                "-o",
                &dst.to_string_lossy(),
                &src.to_string_lossy(),
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    } else {
        // xpm and friends via ImageMagick
        std::process::Command::new("convert")
            .args([src.as_os_str(), dst.as_os_str()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if ok && dst.is_file() {
        Some(dst)
    } else {
        None
    }
}

/// Cached WM_CLASS -> drawable PNG (or solved-no-icon).
pub struct IconCache {
    solved: HashMap<String, Option<PathBuf>>,
    surfaces: HashMap<PathBuf, cairo::ImageSurface>,
}

impl IconCache {
    pub fn new() -> Self {
        Self {
            solved: HashMap::new(),
            surfaces: HashMap::new(),
        }
    }

    /// Real app icon for a WM class, like the Omarchy menu shows.
    /// Cloned surfaces are refcounted, so this stays cheap per frame.
    pub fn surface_for_class(&mut self, class: &str) -> Option<cairo::ImageSurface> {        let key = class.to_lowercase();
        let path = self
            .solved
            .entry(key.clone())
            .or_insert_with(|| {
                let icon = desktop_icon_for_class(class)?;
                let file = find_icon_file(&icon)?;
                rasterized_png(&file, &icon)
            })
            .clone()?;
        if let Some(s) = self.surfaces.get(&path) {
            let s: cairo::ImageSurface = s.clone();
            return Some(s);
        }
        let s = std::fs::File::open(&path)
            .ok()
            .and_then(|mut f| cairo::ImageSurface::create_from_png(&mut f).ok())?;
        self.surfaces.insert(path, s.clone());
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localsend_resolves_to_image() {
        let mut c = IconCache::new();
        let s = c.surface_for_class("localsend");
        assert!(s.is_some(), "localsend should resolve via localsend.desktop");
        let s = s.unwrap();
        assert!(s.width() > 0 && s.height() > 0);
    }

    #[test]
    fn nautilus_reverse_dns_resolves() {
        let mut c = IconCache::new();
        assert!(c.surface_for_class("org.gnome.Nautilus").is_some());
    }

    #[test]
    fn unknown_class_is_none() {
        let mut c = IconCache::new();
        assert!(c.surface_for_class("no-such-app-xyz-123").is_none());
    }

    #[test]
    fn saver_detector_ignores_substring_wrappers() {
        // the strict argv matcher must not fire on shells/greps that merely
        // mention the class (regression test for the flip-flop)
        let wrapper = b"bash\x00-lc\x00foot --app-id=org.omarchy.screensaver --config=x\x00";
        let real = b"foot\x00--app-id=org.omarchy.screensaver\x00--config=x\x00";
        let hit = |cmd: &[u8]| {
            cmd.split(|b| *b == 0).any(|arg| {
                arg == b"--class=org.omarchy.screensaver"
                    || arg == b"--app-id=org.omarchy.screensaver"
                    || arg == b"org.omarchy.screensaver"
            })
        };
        assert!(!hit(wrapper));
        assert!(hit(real));
    }
}
