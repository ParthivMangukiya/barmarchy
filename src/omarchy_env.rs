//! Omarchy environment for spawned helpers.
//!
//! Systemd user services don't source `~/.bashrc`, so the daemon starts
//! without `OMARCHY_PATH` (normally set by
//! `/usr/share/omarchy/default/bash/env-bootstrap`). Current omarchy helpers
//! require it — e.g. `omarchy-theme-set` resolves system themes via
//! `$OMARCHY_PATH/themes` and exits `Theme ... does not exist` without it,
//! which is why taps on the theme picker silently did nothing.
//!
//! `ensure()` mirrors the bootstrap rule (`/etc/omarchy.conf` wins for
//! dev-link, else the packaged default). Call once at startup: every child
//! spawned afterwards inherits the fixed environment.

use std::process::Child;

const DEFAULT_PATH: &str = "/usr/share/omarchy";

/// Parse `OMARCHY_PATH=<value>` out of `/etc/omarchy.conf` (a bash snippet
/// written by `omarchy-dev-link`). Returns None when absent/unparseable.
fn from_omarchy_conf() -> Option<String> {
    let text = std::fs::read_to_string("/etc/omarchy.conf").ok()?;
    for line in text.lines() {
        let line = line.trim().trim_start_matches("export ").trim();
        if let Some(rest) = line.strip_prefix("OMARCHY_PATH=") {
            let v = rest
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .trim_end_matches(';')
                .trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub fn omarchy_path() -> String {
    resolve(
        std::env::var("OMARCHY_PATH").ok(),
        std::path::Path::new("/etc/omarchy.conf").exists(),
        from_omarchy_conf(),
    )
}

/// Pure rule, unit-tested: ambient env wins, then dev-link conf, then default.
fn resolve(env: Option<String>, conf_exists: bool, conf_val: Option<String>) -> String {
    if let Some(v) = env {
        if !v.trim().is_empty() {
            return v;
        }
    }
    if conf_exists {
        return conf_val.unwrap_or_else(|| DEFAULT_PATH.into());
    }
    DEFAULT_PATH.into()
}

/// Fix the daemon's own environment in place; children inherit it.
pub fn ensure() {
    let base = omarchy_path();
    // SAFETY: single-threaded at startup (called first in main, before any
    // threads spawn), so no concurrent env access to race with.
    unsafe {
        std::env::set_var("OMARCHY_PATH", &base);
    }
    // Dev-link mode keeps helpers in $OMARCHY_PATH/bin instead of /usr/bin;
    // mirror env-bootstrap and prepend it when missing.
    if base != DEFAULT_PATH {
        let bin = format!("{}/bin", base.trim_end_matches('/'));
        let path = std::env::var("PATH").unwrap_or_default();
        if !path.split(':').any(|p| p == bin) {
            unsafe {
                std::env::set_var(
                    "PATH",
                    if path.is_empty() {
                        bin
                    } else {
                        format!("{bin}:{path}")
                    },
                );
            }
        }
    }
}

/// Park a fire-and-forget child on a reaper thread so it can't linger as a
/// zombie (the daemon never joins these handles itself).
pub fn detach(child: Child) {
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_default_without_conf() {
        assert_eq!(
            resolve(None, false, None),
            "/usr/share/omarchy",
        );
    }

    #[test]
    fn ambient_env_wins_over_everything() {
        assert_eq!(
            resolve(Some("/dev/omarchy".into()), true, Some("/other".into())),
            "/dev/omarchy",
        );
    }

    #[test]
    fn empty_env_falls_through_to_conf() {
        assert_eq!(
            resolve(Some("  ".into()), true, Some("/dev/omarchy".into())),
            "/dev/omarchy",
        );
    }

    #[test]
    fn conf_without_value_falls_back_to_default() {
        assert_eq!(resolve(None, true, None), "/usr/share/omarchy");
    }
}
