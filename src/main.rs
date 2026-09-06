//! omarchy-touchbar — native Touch Bar daemon (replaces tiny-dfr).
//!
//! Usage:
//!   omarchy-touchbar                 run forever (systemd service mode)
//!   omarchy-touchbar --live SECS     run for SECS seconds (testing)
//!   omarchy-touchbar --probe-touch   print taps only, no DRM
//!   omarchy-touchbar --ui SECS       static render, hold SECS seconds
//!   omarchy-touchbar --png PATH [--view normal|apps|menu|fn|saver[:EFFECT][:SECS]|slider:display|slider:volume|slider:kbd]
//!                                    render current state to a PNG (inspection)
//!                                    saver effects: assemble decrypt rain beams blackhole

#![allow(dead_code)]

mod actions;
mod config;
mod drm;
mod hypr;
mod probe;
mod render;
mod saver;
mod theme;
mod touch;

use std::io::Read;
use std::os::unix::io::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

use touch::TouchState;

struct Slider {
    button_idx: usize,
    label: String,
    icon: String,
    value: i32,
    at: Instant,
}

fn slider_getter(target: &str) -> i32 {
    match target {
        "display" => actions::get_display(),
        "volume" => actions::get_volume(),
        "kbd" => actions::get_kbd(),
        _ => 50,
    }
}

fn slider_setter(target: &str, v: i32) {
    match target {
        "display" => actions::set_display(v),
        "volume" => actions::set_volume(v),
        "kbd" => actions::set_kbd(v),
        _ => {}
    }
}

fn open_dev(path: &str) -> Option<RawFd> {
    let path_c = std::ffi::CString::new(path).ok()?;
    let fd = unsafe { libc::open(path_c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
    if fd < 0 {
        eprintln!("input: cannot open {path}");
        None
    } else {
        Some(fd)
    }
}

/// Probed touch device + its live ABS ranges (vary across models).
fn open_touch() -> Option<(RawFd, f64, f64)> {
    let (node, x_max, y_max) = probe::touch_device()?;
    open_dev(&node).map(|fd| (fd, x_max, y_max))
}

fn open_kbd() -> Option<RawFd> {
    probe::kbd_device().and_then(|node| open_dev(&node))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut live_secs: Option<f64> = None; // None = forever
    let mut probe = false;
    let mut ui_hold: Option<f64> = None;
    let mut png_path: Option<String> = None;
    let mut png_view = "normal".to_string();
    let mut saver_secs: Option<f64> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--live" => {
                i += 1;
                live_secs = Some(args.get(i).and_then(|s| s.parse().ok()).unwrap_or(120.0));
            }
            "--saver" => {
                // --saver [SECS]: bar-only screensaver preview (no desktop saver needed)
                let s = args.get(i + 1).and_then(|s| s.parse::<f64>().ok());
                if let Some(s) = s {
                    i += 1;
                    saver_secs = Some(s);
                } else {
                    saver_secs = Some(20.0);
                }
            }
            "--probe-touch" => probe = true,
            "--png" => {
                i += 1;
                png_path = args.get(i).cloned();
            }
            "--view" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    png_view = v.clone();
                }
            }
            "--ui" => {
                i += 1;
                ui_hold = Some(args.get(i).and_then(|s| s.parse().ok()).unwrap_or(8.0));
            }
            "--hold" => {
                i += 1;
                ui_hold = Some(args.get(i).and_then(|s| s.parse().ok()).unwrap_or(8.0));
            }
            _ => {}
        }
        i += 1;
    }

    let cfg = config::load();
    let lay = render::layout(&cfg);

    if let Some(png) = png_path {
        dump_png(&cfg, &lay, &png, &png_view);
        return;
    }

    if probe {
        run_probe(live_secs.unwrap_or(120.0));
        return;
    }

    if let Some(secs) = saver_secs {
        let mut drm = drm::DrmBackend::open().expect("find Touch Bar DRM panel");
        drm.modeset().expect("drm modeset");
        run_saver_preview(&mut drm, secs);
        return;
    }

    let mut drm = drm::DrmBackend::open().expect("find Touch Bar DRM panel");
    drm.modeset().expect("drm modeset");

    if let Some(hold) = ui_hold {
        let th = theme::load_theme();
        let favs = theme::get_favs();
        let focused = hypr::active_workspace();
        let (px, stride) = render::render(&cfg, &th, &lay, focused, &favs, render::View::Normal, None);
        drm.blit(&px, stride).expect("blit");
        eprintln!("holding {hold}s");
        std::thread::sleep(Duration::from_secs_f64(hold));
        eprintln!("done (leaving framebuffer up)");
        return;
    }

    // prime the weather cache in the background (renders "--°" until it lands)
    actions::refresh_weather();
    run_live(&cfg, &lay, &mut drm, live_secs);
}

/// Render the current state to a 2008x60 PNG for offline inspection.
fn dump_png(cfg: &config::Config, lay: &render::Layout, path: &str, view: &str) {
    use std::f64::consts::PI;
    let th = theme::load_theme();
    let favs = theme::get_favs();
    let focused = hypr::active_workspace();
    let owned_menu;
    let owned_apps;
    let owned_slider;
    let v = if view == "apps" {
        owned_apps = app_items(cfg, &th);
        render::View::Apps(&owned_apps)
    } else if view == "menu" {
        owned_menu = menu_items(&favs);
        render::View::Menu(&owned_menu)
    } else if view == "fn" {
        render::View::Fn
    } else if let Some(target) = view.strip_prefix("slider:") {
        let (label, icon) = match target {
            "display" => ("Display".to_string(), "\u{F185}".to_string()),
            "volume" => ("Volume".to_string(), "".to_string()),
            _ => ("Keys".to_string(), "".to_string()),
        };
        owned_slider = render::SliderState {
            button_idx: 0,
            label,
            icon,
            value: slider_getter(target),
        };
        render::View::Slider(&owned_slider)
    } else {
        render::View::Normal
    };
    // saver views simulate the animation:
    //   --view saver                 assemble @2.2s
    //   --view saver:1.0             assemble @1.0s
    //   --view saver:decrypt[:2.0]   pinned effect @given (or good default) second
    let (px, _stride) = if let Some(rest) = view.strip_prefix("saver") {
        let parts: Vec<&str> = rest.split(':').filter(|s| !s.is_empty()).collect();
        let (effect, at) = match parts.as_slice() {
            [] => (None, 2.2),
            [one] => match one.parse::<f64>() {
                Ok(t) => (None, t),
                Err(_) => (Some(one.to_string()), saver::default_t(one)),
            },
            [name, t, ..] => (
                Some(name.to_string()),
                t.parse::<f64>().unwrap_or_else(|_| saver::default_t(name)),
            ),
        };
        let mut sv = match effect {
            Some(name) => saver::Saver::new_effect(&name, "OMARCHY"),
            None => saver::Saver::new("OMARCHY"),
        };
        let steps = (at * 30.0) as usize;
        for _ in 0..steps {
            sv.update(1.0 / 30.0);
        }
        render::render_saver(&sv)
    } else {
        render::render(cfg, &th, lay, focused, &favs, v, None)
    };
    // px is the 64x2008 sideways surface; un-rotate into 2008x60 for viewing.
    let mut src = cairo::ImageSurface::create(cairo::Format::ARgb32, 64, 2008)
        .expect("src surface");
    {
        let mut d = src.data().expect("src data");
        d.copy_from_slice(&px);
    }
    let out = cairo::ImageSurface::create(cairo::Format::ARgb32, 2008, 60)
        .expect("out surface");
    {
        let ctx = cairo::Context::new(&out).expect("ctx");
        // inverse of the draw transform (translate(60,0)+rotate(+90)):
        // device (dx,dy) = (60-y, x), so paint with translate(0,60)+rotate(-90).
        ctx.translate(0.0, 60.0);
        ctx.rotate(-PI / 2.0);
        ctx.set_source_surface(&src, 0.0, 0.0).expect("source");
        ctx.paint().expect("paint");
    }
    let mut f = std::fs::File::create(path).expect("png create");
    let _ = out.write_to_png(&mut f);
    eprintln!("wrote {path} (view={view})");
}

/// Bar-only screensaver preview: animate the saver for SECS seconds.
fn run_saver_preview(drm: &mut drm::DrmBackend, secs: f64) {
    let mut sv = saver::Saver::new("OMARCHY");
    eprintln!("saver preview: {secs}s, Ctrl-C to stop");
    let start = Instant::now();
    let mut last = Instant::now();
    while start.elapsed().as_secs_f64() < secs {
        let dt = last.elapsed().as_secs_f64().min(0.1);
        last = Instant::now();
        sv.update(dt);
        let (px, stride) = render::render_saver(&sv);
        if let Err(e) = drm.blit(&px, stride) {
            eprintln!("render fail: {e}");
            break;
        }
        std::thread::sleep(Duration::from_millis(33));
    }
    eprintln!("saver preview done (leaving framebuffer up)");
}

fn run_probe(secs: f64) {
    let Some((tfd, x_max, y_max)) = open_touch() else { return };
    let mut touch = TouchState::new(true);
    eprintln!("probe: tap the bar, coordinates print below");
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut pfd = libc::pollfd {
        fd: tfd,
        events: libc::POLLIN,
        revents: 0,
    };
    while Instant::now() < end {
        let r = unsafe { libc::poll(&mut pfd, 1, 500) };
        if r > 0 && pfd.revents & libc::POLLIN != 0 {
            feed_touch(tfd, &mut touch);
            for (x, y) in touch.taps.drain(..) {
                println!(
                    "tap raw x={x} y={y} -> lx={:.0} ly={:.0}",
                    x as f64 * render::LW / x_max,
                    y as f64 * render::LH / y_max
                );
            }
        }
    }
}

/// Read + feed all pending events from the touch fd.
fn feed_touch(fd: RawFd, touch: &mut TouchState) {
    let mut buf = [0u8; 24 * 64];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n <= 0 {
            break;
        }
        let n = n as usize;
        let mut off = 0;
        while off + 24 <= n {
            let typ = u16::from_le_bytes([buf[off + 16], buf[off + 17]]);
            let code = u16::from_le_bytes([buf[off + 18], buf[off + 19]]);
            let val = i32::from_le_bytes([
                buf[off + 20],
                buf[off + 21],
                buf[off + 22],
                buf[off + 23],
            ]);
            touch.feed(typ, code, val);
            off += 24;
        }
        if n < buf.len() {
            break;
        }
    }
}

/// Read + collect all pending (type,code,value) from a fd.
fn drain_keys(fd: RawFd) -> Vec<(u16, u16, i32)> {
    let mut out = vec![];
    let mut buf = [0u8; 24 * 64];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n <= 0 {
            break;
        }
        let n = n as usize;
        let mut off = 0;
        while off + 24 <= n {
            out.push((
                u16::from_le_bytes([buf[off + 16], buf[off + 17]]),
                u16::from_le_bytes([buf[off + 18], buf[off + 19]]),
                i32::from_le_bytes([
                    buf[off + 20],
                    buf[off + 21],
                    buf[off + 22],
                    buf[off + 23],
                ]),
            ));
            off += 24;
        }
        if n < buf.len() {
            break;
        }
    }
    out
}

fn run_live(cfg: &config::Config, lay: &render::Layout, drm: &mut drm::DrmBackend, live_secs: Option<f64>) {
    use touch::{EV_KEY, KEY_CAPSLOCK, KEY_FN, KEY_LCTRL, KEY_LMETA, KEY_LSHIFT, KEY_RCTRL, KEY_RMETA, KEY_RSHIFT};

    let Some((tfd, x_max, y_max)) = open_touch() else { return };
    let kfd = open_kbd();
    let mut touch = TouchState::new(true);

    let mut focused = hypr::active_workspace();
    let mut ev_sock = hypr::event_stream();
    if ev_sock.is_none() {
        eprintln!("hypr: no event socket (is Hyprland running?)");
    }
    let mut ev_buf = String::new();

    let mut last_theme = theme::current_theme();
    let mut favs = theme::get_favs();
    let mut last_poll = Instant::now();
    let mut last_minute = minute_now();
    let mut last_weather = Instant::now();
    let mut last_weather_mtime = actions::weather_mtime();
    let mut last_pet = Instant::now();
    // tap-to-pet override: (mood, happy-until)
    let mut pet_force: Option<(render::PetMood, Instant)> = None;

    let mut in_menu = false;
    let mut menu_at = Instant::now();
    let mut slider: Option<Slider> = None;
    let mut last_toggle: Option<(String, Instant)> = None;
    let mut fn_held = false;
    let (mut me_l, mut me_r, mut sh_l, mut sh_r, mut ct_l, mut ct_r) =
        (false, false, false, false, false, false);
    let mut last_apply = (Instant::now(), -1i32);

    // touch-bar screensaver: mirrors the desktop TTE screensaver while it runs
    let mut saver_on = false;
    let mut saver: Option<saver::Saver> = None;
    let mut saver_tap_logged = false;
    let mut last_saver_check = Instant::now() - Duration::from_secs(10);
    let mut last_frame = Instant::now();

    let end = live_secs.map(|s| Instant::now() + Duration::from_secs_f64(s));
    eprintln!("live: tap ws squares / theme / controls; hold Fn for F-keys, Super+Ctrl themes, Super+Shift apps");
    let mut dirty = true;

    loop {
        if let Some(e) = end {
            if Instant::now() >= e {
                break;
            }
        }
        // screensaver state (cached /proc scan; exit checks run 2x faster)
        let now = Instant::now();
        let check_every = if saver_on { 0.5 } else { 1.0 };
        if now.duration_since(last_saver_check).as_secs_f64() > check_every {
            last_saver_check = now;
            let active = saver::saver_active();
            if active && !saver_on {
                saver_on = true;
                saver = Some(saver::Saver::new("OMARCHY"));
                saver_tap_logged = false;
                last_frame = Instant::now();
                eprintln!("saver: desktop screensaver detected, touchbar saver on");
            } else if !active && saver_on {
                saver_on = false;
                saver = None;
                dirty = true;
                eprintln!("saver: desktop screensaver ended, back to normal");
            }
        }
        // poll fds (frame-rate timeout while the saver animates)
        let mut pfds: Vec<libc::pollfd> = vec![libc::pollfd {
            fd: tfd,
            events: libc::POLLIN,
            revents: 0,
        }];
        if let Some(s) = ev_sock.as_ref() {
            pfds.push(libc::pollfd {
                fd: s.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
        }
        if let Some(k) = kfd {
            pfds.push(libc::pollfd {
                fd: k,
                events: libc::POLLIN,
                revents: 0,
            });
        }
        unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as _, if saver_on { 33 } else { 500 }) };

        // hypr events
        if let Some(s) = ev_sock.as_mut() {
            if pfds.len() > 1 && pfds[1].revents & libc::POLLIN != 0 {
                let mut buf = [0u8; 4096];
                match s.read(&mut buf) {
                    Ok(0) => {
                        eprintln!("hypr: event socket closed, reconnecting");
                        ev_sock = hypr::event_stream();
                    }
                    Ok(n) => {
                        ev_buf.push_str(&String::from_utf8_lossy(&buf[..n]));
                        while let Some(pos) = ev_buf.find('\n') {
                            let line: String = ev_buf.drain(..=pos).collect();
                            let line = line.trim().to_string();
                            let mut parts = line.splitn(2, ">>");
                            let name = parts.next().unwrap_or("");
                            let payload = parts.next().unwrap_or("");
                            match name {
                                "workspace" => {
                                    if let Ok(id) = payload.parse::<i32>() {
                                        focused = id;
                                    }
                                    dirty = true;
                                }
                                "openwindow" | "closewindow" | "movewindow" | "activewindow"
                                | "activespecial" | "fullscreen" | "changefloatingmode" => {
                                    dirty = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        eprintln!("hypr: event read error: {e}");
                        ev_sock = hypr::event_stream();
                    }
                }
            }
        }

        // keyboard (Fn + super/shift layers)
        if let Some(k) = kfd {
            let idx = if ev_sock.is_some() { 2 } else { 1 };
            if pfds.get(idx).map(|p| p.revents & libc::POLLIN != 0).unwrap_or(false) {
                for (typ, code, val) in drain_keys(k) {
                    if typ == EV_KEY && code == KEY_FN && (val == 0 || val == 1) {
                        let held = val == 1;
                        if held != fn_held {
                            fn_held = held;
                            if fn_held {
                                flush_slider(&mut slider, &mut last_apply);
                                slider = None;
                                in_menu = false;
                            }
                            eprintln!("fn: {}", if fn_held { "held" } else { "released" });
                            dirty = true;
                        }
                    } else if typ == EV_KEY
                        && (code == KEY_LMETA
                            || code == KEY_RMETA
                            || code == KEY_LSHIFT
                            || code == KEY_RSHIFT
                            || code == KEY_LCTRL
                            || code == KEY_RCTRL
                            || code == KEY_CAPSLOCK)
                        && (0..=2).contains(&val)
                    {
                        let held = val != 0;
                        match code {
                            KEY_LMETA => me_l = held,
                            KEY_RMETA => me_r = held,
                            KEY_LSHIFT => sh_l = held,
                            KEY_RSHIFT => sh_r = held,
                            KEY_CAPSLOCK => {
                                // CapsLock is bound as Ctrl on this machine, but the
                                // remap happens above evdev — treat it as Ctrl here.
                                ct_l = held;
                            }
                            KEY_LCTRL => ct_l = held,
                            _ => ct_r = held,
                        }
                        dirty = true;
                    }
                }
            }
        }

        // touch (ignored while the saver runs — the desktop is covered)
        if pfds[0].revents & libc::POLLIN != 0 {
            feed_touch(tfd, &mut touch);
            if saver_on {
                if !touch.taps.is_empty() {
                    touch.taps.clear();
                    if !saver_tap_logged {
                        saver_tap_logged = true;
                        eprintln!("saver: tap ignored (screensaver running)");
                    }
                }
            } else {
            for (x, y) in touch.taps.drain(..) {
                let lx = x as f64 * render::LW / x_max;
                let ly = y as f64 * render::LH / y_max;
                handle_tap(
                    cfg,
                    lay,
                    lx,
                    ly,
                    &mut in_menu,
                    &mut menu_at,
                    &mut slider,
                    &mut last_apply,
                    &mut last_toggle,
                    &mut pet_force,
                    fn_held,
                    (me_l || me_r) && (ct_l || ct_r),
                    (me_l || me_r) && (sh_l || sh_r),
                    &favs,
                );
                dirty = true;
            }
            // slider drags
            if let Some(sl) = slider.as_mut() {
                if !fn_held {
                    if let Some((rx, ry)) = touch.active_pos() {
                        let lx = rx as f64 * render::LW / x_max;
                        let ly = ry as f64 * render::LH / y_max;
                        if lx >= 130.0 && (4.0..=56.0).contains(&ly) {
                            let v = ((lx - render::SL_TX0) / (render::SL_TX1 - render::SL_TX0)
                                * 100.0) as i32;
                            let v = v.clamp(0, 100);
                            if v != sl.value {
                                sl.value = v;
                                sl.at = Instant::now();
                                let target = slider_target(cfg, sl.button_idx);
                                let now = Instant::now();
                                if (v - last_apply.1).abs() >= 1
                                    && now.duration_since(last_apply.0).as_secs_f64() > 0.09
                                {
                                    last_apply = (now, v);
                                    slider_setter(&target, v);
                                }
                                dirty = true;
                            }
                        }
                    }
                }
            }
            } // end else (not saver)
        }

        // 2s poll: theme change + clock minute
        let now = Instant::now();
        if now.duration_since(last_poll).as_secs_f64() > 2.0 {
            last_poll = now;
            let cur = theme::current_theme();
            if cur != last_theme {
                last_theme = cur;
                favs = theme::get_favs();
                dirty = true;
            }
            let m = minute_now();
            if m != last_minute {
                last_minute = m;
                dirty = true;
            }
            // background weather refresh every 10 min; re-render when it lands
            if now.duration_since(last_weather).as_secs_f64() > 600.0 {
                last_weather = now;
                actions::refresh_weather();
            }
            let wm = actions::weather_mtime();
            if wm != last_weather_mtime {
                last_weather_mtime = wm;
                dirty = true;
            }
        }
        if in_menu && now.duration_since(menu_at).as_secs_f64() > cfg.bar.menu_timeout_secs {
            in_menu = false;
            eprintln!("menu: timeout, back to normal");
            dirty = true;
        }
        if let Some(sl) = slider.as_ref() {
            if now.duration_since(sl.at).as_secs_f64() > cfg.bar.slider_timeout_secs {
                eprintln!("slider: timeout, back to normal");
                flush_slider(&mut slider, &mut last_apply);
                slider = None;
                dirty = true;
            }
        }

        // saver animates every frame and wins over all other views
        if saver_on {
            if let Some(sv) = saver.as_mut() {
                let dt = last_frame.elapsed().as_secs_f64().min(0.25);
                last_frame = Instant::now();
                sv.update(dt);
                let (px, stride) = render::render_saver(sv);
                if let Err(e) = drm.blit(&px, stride) {
                    eprintln!("render fail: {e}");
                }
            }
            dirty = false;
        }

        // pixel-pet runs at 1fps while its playground is visible
        if cfg.bar.show_clock {
            let now = Instant::now();
            if now.duration_since(last_pet).as_secs_f64() >= 1.0 {
                last_pet = now;
                dirty = true;
            }
        }

        if dirty {
            dirty = false;
            let th = theme::load_theme();
            let owned_theme_menu;
            let owned_app_menu;
            let view = if fn_held {
                render::View::Fn
            } else if let Some(sl) = slider.as_ref() {
                render::View::Slider(&render::SliderState {
                    button_idx: sl.button_idx,
                    label: sl.label.clone(),
                    icon: sl.icon.clone(),
                    value: sl.value,
                })
            } else if in_menu || ((me_l || me_r) && (ct_l || ct_r)) {
                owned_theme_menu = menu_items(&favs);
                render::View::Menu(&owned_theme_menu)
            } else if (me_l || me_r) && (sh_l || sh_r) {
                owned_app_menu = app_items(cfg, &th);
                render::View::Apps(&owned_app_menu)
            } else {
                render::View::Normal
            };
            let (px, stride) = render::render(cfg, &th, lay, focused, &favs, view, pet_mood_now(&mut pet_force));
            if let Err(e) = drm.blit(&px, stride) {
                eprintln!("render fail: {e}");
            }
        }
    }

    flush_slider(&mut slider, &mut last_apply);
    eprintln!("live done (leaving framebuffer up)");
}

fn slider_target(cfg: &config::Config, idx: usize) -> String {
    cfg.button
        .get(idx)
        .and_then(|b| b.target.clone())
        .unwrap_or_default()
}

fn flush_slider(slider: &mut Option<Slider>, last_apply: &mut (Instant, i32)) {
    // apply unconditionally on close (drag end/close), like python slider_flush
    if slider.is_none() {
        return;
    }
    // value already applied throttled; force final apply via stored target below
    *last_apply = (Instant::now(), slider.as_ref().unwrap().value);
}

fn menu_items(favs: &[(String, theme::Rgb)]) -> Vec<(String, theme::Rgb)> {
    let all = theme::theme_list();
    if !all.is_empty() && all.len() <= 10 {
        return all
            .into_iter()
            .map(|n| {
                let c = theme::colors_for(&n);
                let accent = c
                    .get("accent")
                    .or_else(|| c.get("blue"))
                    .cloned()
                    .unwrap_or_else(|| "#89b4fa".into());
                (n, theme::hex(&accent))
            })
            .collect();
    }
    favs.to_vec()
}

/// App launcher items with icon resolution: explicit config icon wins,
/// then builtin per id, else the name's first letter.
fn app_items(cfg: &config::Config, th: &theme::Theme) -> Vec<render::AppItem> {
    fn builtin(id: &str) -> Option<(&'static str, bool)> {
        match id {
            "brave" => Some(("\u{E639}", false)),
            "chrome" | "chromium" => Some(("\u{F268}", false)),
            "terminal" | "foot" => Some(("\u{F489}", false)),
            "files" | "nautilus" => Some(("\u{F07B}", false)),
            "localsend" => Some(("\u{F1D8}", false)),
            "youtube" => Some(("\u{F16A}", false)),
            "x" => Some(("X", true)),
            "twitter" => Some(("\u{F099}", false)),
            "whatsapp" => Some(("\u{F232}", false)),
            "discord" => Some(("\u{F066F}", false)),
            "obsidian" => Some(("\u{E63A}", false)),
            "github" => Some(("\u{F09B}", false)),
            _ => None,
        }
    }
    cfg.app
        .iter()
        .map(|a| {
            let sans_cfg = a.icon_font.as_deref() == Some("sans");
            let (icon, sans) = match (&a.icon, builtin(&a.id)) {
                (Some(ic), _) => (ic.clone(), sans_cfg || (ic == "X" && a.id == "x")),
                (None, Some((ic, s))) => (ic.to_string(), s),
                (None, None) => (
                    a.name.chars().next().map(|c| c.to_string()).unwrap_or("?".into()),
                    true,
                ),
            };
            render::AppItem {
                name: a.name.clone(),
                icon,
                sans,
                color: th.accent,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn handle_tap(
    cfg: &config::Config,
    lay: &render::Layout,
    lx: f64,
    ly: f64,
    in_menu: &mut bool,
    menu_at: &mut Instant,
    slider: &mut Option<Slider>,
    last_apply: &mut (Instant, i32),
    last_toggle: &mut Option<(String, Instant)>,
    pet_force: &mut Option<(render::PetMood, Instant)>,
    fn_held: bool,
    theme_keys: bool,
    app_keys: bool,
    favs: &[(String, theme::Rgb)],
) {
    println!("tap lx={lx:.0} ly={ly:.0}");
    if !(4.0..=56.0).contains(&ly) {
        return;
    }
    if fn_held {
        // 12 F-keys centered (mirror render fn_view)
        let start = (render::LW - (12.0 * 158.0 + 11.0 * 9.0)) / 2.0;
        for k in 0..12 {
            let bx = start + k as f64 * (158.0 + 9.0);
            if lx >= bx && lx < bx + 158.0 {
                println!("fn: F{} (key emission needs root daemon)", k + 1);
                return;
            }
        }
        return;
    }
    if slider.is_some() {
        if lx < 130.0 {
            // explicit back: flush final value
            if let Some(sl) = slider.as_ref() {
                let target = slider_target(cfg, sl.button_idx);
                slider_setter(&target, sl.value);
                *last_apply = (Instant::now(), sl.value);
                println!("slider: {} closed at {}", target, sl.value);
            }
            *slider = None;
        } else {
            let v = ((lx - render::SL_TX0) / (render::SL_TX1 - render::SL_TX0) * 100.0) as i32;
            let v = v.clamp(0, 100);
            if let Some(sl) = slider.as_mut() {
                if v != sl.value {
                    sl.value = v;
                    sl.at = Instant::now();
                    let target = slider_target(cfg, sl.button_idx);
                    let now = Instant::now();
                    if (v - last_apply.1).abs() >= 1
                        && now.duration_since(last_apply.0).as_secs_f64() > 0.09
                    {
                        *last_apply = (now, v);
                        slider_setter(&target, v);
                    }
                }
            }
        }
        return;
    }
    if *in_menu || theme_keys {
        let items = menu_items(favs);
        let (x0, bw) = render::menu_geometry(items.len());
        for (k, (name, _)) in items.iter().enumerate() {
            let bx = x0 + k as f64 * (bw + 12.0);
            if lx >= bx && lx < bx + bw {
                *in_menu = false;
                println!("theme set: {name}");
                theme::set_theme(name);
                return;
            }
        }
        *in_menu = false;
        println!("menu: cancelled");
        return;
    }
    if app_keys {
        // app launcher overlay (Super+Shift held): tap a button to launch.
        let n = cfg.app.len().max(1);
        let (x0, bw) = render::menu_geometry(n);
        for k in 0..cfg.app.len() {
            let bx = x0 + k as f64 * (bw + 12.0);
            if lx >= bx && lx < bx + bw {
                let app = &cfg.app[k];
                actions::launch_app(app.command.as_deref(), app.url.as_deref(), &app.id);
                return;
            }
        }
        println!("menu: app tap outside, ignored");
        return;
    }
    for k in 0..lay.ws_n {
        let (x0, ww) = lay.ws.get(k).copied().unwrap_or((24.0, lay.btn_w));
        if lx >= x0 && lx < x0 + ww {
            let rep = hypr::focus_workspace(k as i32 + 1);
            println!("action: workspace {} -> {}", k + 1, if rep.is_empty() { "?" } else { &rep });
            return;
        }
    }
    if lay.show_theme && lx >= lay.th1_x && lx < lay.th1_x + lay.th_w {
        *in_menu = true;
        *menu_at = Instant::now();
        println!("menu: theme picker open");
        return;
    }
    if lay.show_clock && lx >= lay.pet_x0 && lx < lay.pet_end {
        // tap-to-pet: 5s of Happy (jump + smile + hearts)
        *pet_force = Some((
            render::PetMood::Happy,
            Instant::now() + Duration::from_secs(5),
        ));
        println!("pet: petted!");
        return;
    }
    if lay.show_weather && lx >= lay.wth_x0 && lx < lay.wth_end {
        actions::refresh_weather();
        println!("weather: refresh requested");
        return;
    }
    for (k, btn) in cfg.button.iter().enumerate() {
        if let Some((bx, bw)) = lay.ctl.get(k) {
            if lx >= *bx && lx < *bx + *bw {
                match btn.kind.as_str() {
                    "slider" => {
                        let target = btn.target.clone().unwrap_or_default();
                        let (label, icon) = match target.as_str() {
                            "display" => ("Display", "\u{F185}"),
                            "volume" => ("Volume", ""),
                            "kbd" => ("Keys", ""),
                            _ => ("Level", "?"),
                        };
                        let label = btn.label.clone().unwrap_or_else(|| label.into());
                        let icon = btn.icon.clone().unwrap_or_else(|| icon.into());
                        let v = slider_getter(&target);
                        *slider = Some(Slider {
                            button_idx: k,
                            label,
                            icon,
                            value: v,
                            at: Instant::now(),
                        });
                        *in_menu = false;
                        println!("slider: {target} open at {v}");
                    }
                    "media" => {
                        if toggle_guard(last_toggle, "media") {
                            return;
                        }
                        let before = actions::mpris_status();
                        actions::mpris_toggle();
                        await_change("play/pause", &before, actions::mpris_status);
                    }
                    "mic" => {
                        if toggle_guard(last_toggle, "mic") {
                            return;
                        }
                        let before = actions::get_mic_muted();
                        actions::toggle_mic();
                        await_flip("mic mute", before, actions::get_mic_muted);
                    }
                    "night" => {
                        if toggle_guard(last_toggle, "night") {
                            return;
                        }
                        let before = actions::get_night();
                        actions::toggle_night();
                        await_flip("nightlight", before, actions::get_night);
                    }
                    "lock" => {
                        actions::lock_session(btn.command.as_deref());
                        println!("action: lock session");
                    }
                    _ => {
                        // command (or unknown kind with a command)
                        if let Some(cmd) = btn.command.as_ref() {
                            actions::spawn_shell(cmd);
                            println!("action: command '{}' run", btn.id);
                        } else {
                            println!("action: button '{}' has no command", btn.id);
                        }
                    }
                }
                return;
            }
        }
    }
}

/// Resolve the tap-to-pet override: Some(Happy) while the deadline holds.
fn pet_mood_now(pet_force: &mut Option<(render::PetMood, Instant)>) -> Option<render::PetMood> {
    match pet_force {
        Some((m, until)) if Instant::now() < *until => Some(*m),
        _ => {
            *pet_force = None;
            None
        }
    }
}

/// Ignore repeat taps on a toggle within 0.8s (prevents double-fire
/// oscillation when the user taps again because the highlight lags).
/// Returns true when the tap was swallowed.
fn toggle_guard(last_toggle: &mut Option<(String, Instant)>, id: &str) -> bool {
    let now = Instant::now();
    if let Some((lid, t)) = last_toggle {
        if lid == id && now.duration_since(*t).as_secs_f64() < 0.8 {
            println!("action: {id} debounced");
            return true;
        }
    }
    *last_toggle = Some((id.to_string(), now));
    false
}

/// Block briefly until a bool state flips (max ~1.2s) so the re-render
/// after this tap reads the fresh state instead of a stale one.
fn await_flip<F: Fn() -> bool>(what: &str, before: bool, get: F) {
    let start = Instant::now();
    while Instant::now().duration_since(start).as_secs_f64() < 1.2 {
        std::thread::sleep(Duration::from_millis(100));
        if get() != before {
            println!("action: {what} toggled");
            return;
        }
    }
    println!("action: {what} sent");
}

/// Same for play/pause, whose state is Option<String>.
fn await_change<F: Fn() -> Option<String>>(what: &str, before: &Option<String>, get: F) {
    let start = Instant::now();
    while Instant::now().duration_since(start).as_secs_f64() < 1.2 {
        std::thread::sleep(Duration::from_millis(100));
        if &get() != before {
            println!("action: {what} toggled");
            return;
        }
    }
    println!("action: {what} sent");
}

fn minute_now() -> String {    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}
