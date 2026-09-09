//! Hardware probing so one binary runs on every Asahi Touch Bar model
//! (J293, J493, …): find the DRM card driving the strip, the touch input,
//! and the keyboard — instead of hardcoded /dev nodes.
//!
//! Env overrides (checked first): BARMARCHY_DRM, BARMARCHY_TOUCH, BARMARCHY_KBD.

use std::os::unix::io::RawFd;

// evdev ioctls (linux/input.h): _IOR('E', nr, len) = (2<<30)|(len<<16)|('E'<<8)|nr
fn ev_ioc(nr: u32, len: usize) -> u64 {
    (2u64 << 30) | ((len as u64) << 16) | (0x45u64 << 8) | nr as u64
}
fn eviocgname() -> u64 {
    ev_ioc(0x06, 256)
}
fn eviocgabs(axis: u32) -> u64 {
    ev_ioc(0x40 + axis, 32)
}

fn open_ro(path: &str) -> Option<RawFd> {
    let c = std::ffi::CString::new(path).ok()?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
    if fd < 0 { None } else { Some(fd) }
}

fn close(fd: RawFd) {
    unsafe { libc::close(fd) };
}

/// EVIOCGNAME(256) → device name, if any.
pub fn dev_name(path: &str) -> Option<String> {
    let fd = open_ro(path)?;
    let mut buf = [0u8; 256];
    let r = unsafe { libc::ioctl(fd, eviocgname() as _, buf.as_mut_ptr()) };
    close(fd);
    if r < 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..len].to_vec()).ok()
}

/// ABS axis maximum (EVIOCGABS), for touch-range scaling.
fn abs_max(path: &str, axis: u32) -> Option<f64> {
    let fd = open_ro(path)?;
    let mut buf = [0u8; 32];
    let r = unsafe { libc::ioctl(fd, eviocgabs(axis) as _, buf.as_mut_ptr()) };
    close(fd);
    if r < 0 {
        return None;
    }
    let max = i32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    if max > 0 { Some(max as f64) } else { None }
}

fn event_nodes() -> Vec<String> {
    let mut out = vec![];
    if let Ok(entries) = std::fs::read_dir("/dev/input") {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("event") && n[5..].chars().all(|c| c.is_ascii_digit()))
            .collect();
        names.sort_by_key(|n| n[5..].parse::<u32>().unwrap_or(999));
        for n in names {
            out.push(format!("/dev/input/{n}"));
        }
    }
    out
}

/// Touch device: name contains "Touch Bar" (model-specific prefix, e.g.
/// "MacBookPro17,1 Touch Bar"). Returns (node, x_max, y_max).
/// Returns None (instead of guessing) when the device is absent or
/// unreadable, so callers fail loudly with a non-zero exit rather than
/// driving the wrong /dev node.
pub fn touch_device() -> Option<(String, f64, f64)> {
    if let Ok(p) = std::env::var("BARMARCHY_TOUCH") {
        if !p.is_empty() {
            let x = abs_max(&p, 0).unwrap_or(23044.0);
            let y = abs_max(&p, 1).unwrap_or(639.0);
            return Some((p, x, y));
        }
    }
    let nodes = event_nodes();
    if nodes.is_empty() {
        eprintln!("probe: no /dev/input/event* nodes found (is udev running?)");
        return None;
    }
    let mut readable = 0usize;
    for node in &nodes {
        if let Some(name) = dev_name(node) {
            readable += 1;
            if name.contains("Touch Bar") {
                let x = abs_max(node, 0).unwrap_or(23044.0);
                let y = abs_max(node, 1).unwrap_or(639.0);
                eprintln!("probe: touch = {node} (\"{name}\", {x:.0}x{y:.0})");
                return Some((node.clone(), x, y));
            }
        }
    }
    if readable == 0 {
        eprintln!(
            "probe: cannot open any of {} input nodes (permission? need udev uaccess rule or 'input' group; see udev/99-barmarchy-touchbar.rules)",
            nodes.len()
        );
        return None;
    }
    eprintln!(
        "probe: no 'Touch Bar' input found among {readable} readable nodes (override with BARMARCHY_TOUCH=/dev/input/eventN)"
    );
    None
}

/// Keyboard: "Apple SPI Keyboard" holds Fn/Super/Shift on Asahi.
pub fn kbd_device() -> Option<String> {
    if let Ok(p) = std::env::var("BARMARCHY_KBD") {
        if !p.is_empty() {
            return Some(p);
        }
    }
    let mut fallback: Option<String> = None;
    for node in event_nodes() {
        if let Some(name) = dev_name(&node) {
            if name == "Apple SPI Keyboard" {
                eprintln!("probe: keyboard = {node}");
                return Some(node);
            }
            if fallback.is_none() && name.contains("Keyboard") {
                fallback = Some(node);
            }
        }
    }
    if let Some(f) = fallback {
        eprintln!("probe: keyboard = {f} (generic match)");
        return Some(f);
    }
    eprintln!("probe: no keyboard input found, falling back to /dev/input/event1");
    Some("/dev/input/event1".into())
}
