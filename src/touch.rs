//! MT touch parsing (port of TouchState in touchbar_live.py).

use std::collections::HashMap;
use std::time::Instant;

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_ABS: u16 = 3;
pub const BTN_TOUCH: u16 = 330;
pub const KEY_FN: u16 = 464;
pub const KEY_LSHIFT: u16 = 42;
pub const KEY_RSHIFT: u16 = 54;
pub const KEY_LCTRL: u16 = 29;
pub const KEY_RCTRL: u16 = 97;
pub const KEY_CAPSLOCK: u16 = 58;
pub const KEY_LMETA: u16 = 125;
pub const KEY_RMETA: u16 = 126;
pub const MT_SLOT: u16 = 47;
pub const MT_POS_X: u16 = 53;
pub const MT_POS_Y: u16 = 54;
pub const MT_TRACKING: u16 = 57;

#[derive(Default)]
struct Finger {
    down: Option<Instant>,
    x: Option<i32>,
    y: Option<i32>,
    ax: Option<i32>,
    ay: Option<i32>,
}

pub struct TouchState {
    slot: i32,
    fingers: HashMap<i32, Finger>,
    pub taps: Vec<(i32, i32)>,
    debug: bool,
    st_down: Option<Instant>,
    st_x: i32,
    st_y: i32,
    st_ax: i32,
    st_ay: i32,
    st_seen: bool,
    last_mt_tap: Option<Instant>,
}

impl TouchState {
    pub fn new(debug: bool) -> Self {
        Self {
            slot: 0,
            fingers: HashMap::new(),
            taps: vec![],
            debug,
            st_down: None,
            st_x: 0,
            st_y: 0,
            st_ax: 0,
            st_ay: 0,
            st_seen: false,
            last_mt_tap: None,
        }
    }

    fn log(&self, msg: &str) {
        if self.debug {
            eprintln!("touch: {msg}");
        }
    }

    /// Current position of an in-progress contact (for slider drags).
    pub fn active_pos(&self) -> Option<(i32, i32)> {
        let mut slots: Vec<i32> = self.fingers.keys().cloned().collect();
        slots.sort();
        for s in slots {
            if let Some(f) = self.fingers.get(&s) {
                if f.down.is_some() {
                    if let (Some(x), Some(y)) = (f.x, f.y) {
                        return Some((x, y));
                    }
                }
            }
        }
        None
    }

    fn tap(&mut self, x: i32, y: i32, how: &str) {
        let now = Instant::now();
        if how == "st" {
            if let Some(t) = self.last_mt_tap {
                if now.duration_since(t).as_secs_f64() < 0.15 {
                    return;
                }
            }
        }
        if how == "mt" {
            self.last_mt_tap = Some(now);
        }
        self.taps.push((x, y));
        self.log(&format!("TAP x={x} y={y} via {how}"));
    }

    pub fn feed(&mut self, typ: u16, code: u16, val: i32) {
        let now = Instant::now();
        if typ == EV_ABS && code == MT_SLOT {
            self.slot = val;
            return;
        }
        if typ == EV_KEY && code == BTN_TOUCH {
            if val == 1 {
                self.st_down = Some(now);
                self.st_seen = false;
                self.st_ax = self.st_x;
                self.st_ay = self.st_y;
            } else if let Some(t0) = self.st_down.take() {
                if !self.st_seen {
                    return; // MT path owns this contact
                }
                let dt = now.duration_since(t0).as_secs_f64();
                let dx = (self.st_x - self.st_ax).abs();
                let dy = (self.st_y - self.st_ay).abs();
                if dt < 0.6 && dx < 3000 && dy < 300 {
                    self.tap(self.st_x, self.st_y, "st");
                } else {
                    self.log(&format!("st reject dt={dt:.2} dx={dx} dy={dy}"));
                }
            }
            return;
        }
        if typ != EV_ABS {
            return;
        }
        if code == 0 {
            self.st_x = val;
            self.st_seen = true;
            return;
        }
        if code == 1 {
            self.st_y = val;
            self.st_seen = true;
            return;
        }
        let slot = self.slot;
        let f = self.fingers.entry(slot).or_default();
        if code == MT_TRACKING {
            if val == -1 {
                match f.down.take() {
                    None => {}
                    Some(t0) => {
                        let ax = f.ax.or(f.x).unwrap_or(0);
                        let ay = f.ay.or(f.y).unwrap_or(0);
                        let x = f.x.unwrap_or(ax);
                        let y = f.y.unwrap_or(ay);
                        let dx = (x - ax).abs();
                        let dy = (y - ay).abs();
                        let dt = now.duration_since(t0).as_secs_f64();
                        f.ax = None;
                        f.ay = None;
                        if dt < 0.6 && dx < 3000 && dy < 300 {
                            self.tap(x, y, "mt");
                            return;
                        } else {
                            self.log(&format!(
                                "slot {slot} reject dt={dt:.2} dx={dx} dy={dy}"
                            ));
                            return;
                        }
                    }
                }
                f.ax = None;
                f.ay = None;
            } else {
                f.down = Some(now);
                f.ax = None;
                f.ay = None;
                let (fx, fy) = (f.x, f.y);
                self.log(&format!("slot {slot} down id={val} x={fx:?} y={fy:?}"));
            }
        } else if code == MT_POS_X {
            f.x = Some(val);
            if f.ax.is_none() {
                f.ax = Some(val);
            }
        } else if code == MT_POS_Y {
            f.y = Some(val);
            if f.ay.is_none() {
                f.ay = Some(val);
            }
        }
    }
}
