//! Hyprland IPC over ~/.socket sockets.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Resolve the Hyprland instance signature: env first, otherwise pick
/// the newest /run/user/$UID/hypr/<sig> dir with a live socket
/// (systemd services don't inherit the session env).
fn signature() -> Option<String> {
    if let Ok(sig) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        if !sig.is_empty() {
            return Some(sig);
        }
    }
    let uid = unsafe { libc::getuid() };
    let dir = format!("/run/user/{uid}/hypr");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for e in entries.flatten() {
        let sig = e.file_name().to_string_lossy().to_string();
        if !e.path().join(".socket.sock").exists() {
            continue;
        }
        let mtime = e.metadata().and_then(|m| m.modified()).ok();
        let mtime = mtime.unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if best.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            best = Some((mtime, sig));
        }
    }
    best.map(|(_, s)| s)
}

fn sock_path(name: &str) -> Option<String> {
    let uid = unsafe { libc::getuid() };
    let sig = signature()?;
    Some(format!("/run/user/{uid}/hypr/{sig}/{name}"))
}

/// Send a command to .socket.sock, return raw reply.
pub fn hypr_cmd(cmd: &str) -> String {
    let Some(path) = sock_path(".socket.sock") else {
        return String::new();
    };
    let Ok(mut s) = UnixStream::connect(path) else {
        return String::new();
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = s.write_all(cmd.as_bytes());
    let mut out = Vec::new();
    let mut buf = [0u8; 65536];
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if n < buf.len() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn active_workspace() -> i32 {
    let rep = hypr_cmd("j/activeworkspace");
    serde_json::from_str::<serde_json::Value>(&rep)
        .ok()
        .and_then(|v| v.get("id").and_then(|i| i.as_i64()))
        .unwrap_or(1) as i32
}

#[derive(Debug, Clone, Default)]
pub struct WsInfo {
    pub id: i32,
    #[allow(dead_code)]
    pub windows: i32,
}

pub fn workspaces() -> Vec<WsInfo> {
    let rep = hypr_cmd("j/workspaces");
    let mut out = vec![];
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&rep) {
        if let Some(arr) = v.as_array() {
            for w in arr {
                out.push(WsInfo {
                    id: w.get("id").and_then(|i| i.as_i64()).unwrap_or(1) as i32,
                    windows: w
                        .get("windows")
                        .and_then(|i| i.as_i64())
                        .unwrap_or(0) as i32,
                });
            }
        }
    }
    if out.is_empty() {
        out.push(WsInfo { id: 1, windows: 1 });
    }
    out
}

#[derive(Debug, Clone)]
pub struct Client {
    pub ws_id: i32,
    pub class: String,
    pub title: String,
}

pub fn clients() -> Vec<Client> {
    let rep = hypr_cmd("j/clients");
    let mut out = vec![];
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&rep) {
        if let Some(arr) = v.as_array() {
            for c in arr {
                let ws = c
                    .get("workspace")
                    .and_then(|w| w.get("id"))
                    .and_then(|i| i.as_i64())
                    .unwrap_or(0) as i32;
                let class = c
                    .get("class")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                let title = c
                    .get("title")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(Client { ws_id: ws, class, title });
            }
        }
    }
    out
}

pub fn focus_workspace(n: i32) -> String {
    hypr_cmd(&format!("dispatch hl.dsp.focus({{ workspace = \"{n}\" }})"))
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Non-blocking event socket (.socket2.sock). Returns the stream.
pub fn event_stream() -> Option<UnixStream> {
    let path = sock_path(".socket2.sock")?;
    let s = UnixStream::connect(path).ok()?;
    s.set_nonblocking(true).ok()?;
    Some(s)
}
