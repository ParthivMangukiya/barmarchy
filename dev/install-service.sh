#!/usr/bin/env bash
# Install the always-on Rust touchbar service (replaces tiny-dfr).
# Usage: ./dev/install-service.sh
set -euo pipefail
REPO="$HOME/Work/omarchy-touchbar"
UNIT_DIR="$HOME/.config/systemd/user"
echo "==> building release"
cargo build --release --manifest-path "$REPO/Cargo.toml"
echo "==> installing user unit"
mkdir -p "$UNIT_DIR"
cp "$REPO/dev/omarchy-touchbar.service" "$UNIT_DIR/omarchy-touchbar.service"
systemctl --user daemon-reload
systemctl --user enable --now omarchy-touchbar.service
echo "==> disabling stock tiny-dfr (may prompt for sudo once)"
sudo systemctl disable --now tiny-dfr.service
echo "==> status"
systemctl --user status omarchy-touchbar.service --no-pager | head -12
echo "touchbar service installed. Edit buttons in ~/.config/omarchy-touchbar/config.toml,"
echo "then: systemctl --user restart omarchy-touchbar"
