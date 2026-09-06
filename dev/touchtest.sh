#!/usr/bin/env bash
# Repo-side touchtest flow — THIS is what changes throughout development.
# Current: build fresh Rust binary + run it with the service stopped
# (trap-restored). Replaces the old python touchbar_live flow.
# Usage: touchtest [--live SECS] [--probe-touch] [--ui] [--saver SECS]
set -u
REPO="$HOME/Work/omarchy-touchbar"
BIN="$REPO/target/release/omarchy-touchbar"
MODE="live"
LIVE_SECS="120"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --live) LIVE_SECS="$2"; shift 2 ;;
    --probe-touch) MODE="probe"; shift ;;
    --ui) MODE="ui"; shift ;;
    --saver) MODE="saver"; if [[ -n ${2:-} && $2 != -* ]]; then LIVE_SECS="$2"; shift 2; else shift; fi ;;
    *) echo "touchtest: unknown arg $1 (try --live N, --probe-touch, --ui, --saver N)" >&2; exit 1 ;;
  esac
done
cargo build --release --manifest-path "$REPO/Cargo.toml" 2>&1 | tail -1
restore() { systemctl --user start omarchy-touchbar.service >/dev/null 2>&1 || true; }
trap restore EXIT INT TERM
systemctl --user stop omarchy-touchbar.service >/dev/null 2>&1 || true
sudo -n /usr/bin/systemctl stop tiny-dfr.service >/dev/null 2>&1 || true
case "$MODE" in
  live) "$BIN" --live "$LIVE_SECS" ;;
  probe) "$BIN" --probe-touch --live "$LIVE_SECS" ;;
  ui) "$BIN" --ui "$LIVE_SECS" ;;
  saver) "$BIN" --saver "$LIVE_SECS" ;;
esac
