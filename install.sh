#!/usr/bin/env bash
# barmarchy — one-command setup.
#
#   curl -fsSL https://raw.githubusercontent.com/ParthivMangukiya/barmarchy/main/install.sh | bash
#   ./install.sh                    # from a checkout
#   ./install.sh --uninstall        # remove service + binary (keeps config)
#
# Installs: deps (via pacman/dnf) → release binary → user systemd unit →
# video/input groups → masks stock tiny-dfr → enables the bar.
set -euo pipefail

UNINSTALL=false
if [[ ${1:-} == "--uninstall" ]]; then UNINSTALL=true; fi

REPO_URL="https://github.com/ParthivMangukiya/barmarchy"
SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ ! -f "$SRC/Cargo.toml" || ! -f "$SRC/src/main.rs" ]]; then
  DEST="$HOME/.local/share/barmarchy"
  if [[ ! -f "$DEST/Cargo.toml" ]]; then
    echo "==> cloning barmarchy to $DEST"
    mkdir -p "$(dirname "$DEST")"
    git clone "$REPO_URL" "$DEST"
  fi
  exec "$DEST/install.sh" "$@"
fi

BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/omarchy-touchbar"
UNIT_DIR="$HOME/.config/systemd/user"
UNIT="$UNIT_DIR/omarchy-touchbar.service"

if $UNINSTALL; then
  echo "==> stopping barmarchy"
  systemctl --user disable --now omarchy-touchbar.service 2>/dev/null || true
  rm -f "$UNIT" "$BIN"
  systemctl --user daemon-reload 2>/dev/null || true
  echo "==> removing udev rule (needs sudo)"
  sudo rm -f /etc/udev/rules.d/99-barmarchy-touchbar.rules 2>/dev/null || true
  sudo udevadm control --reload-rules 2>/dev/null || true
  echo "==> restoring stock tiny-dfr (needs sudo)"
  sudo systemctl unmask tiny-dfr.service 2>/dev/null || true
  sudo systemctl enable --now tiny-dfr.service 2>/dev/null || true
  echo "uninstalled (config kept in ~/.config/omarchy-touchbar/)"
  exit 0
fi

echo "==> barmarchy setup from $SRC"

if [[ $(uname -m) != "aarch64" ]]; then
  echo "warning: barmarchy targets Apple Silicon (aarch64); continuing anyway" >&2
fi

# --- deps ---
install_deps() {
  if command -v pacman >/dev/null; then
    sudo pacman -S --needed --noconfirm base-devel cairo pkg-config git curl jq rust
  elif command -v dnf >/dev/null; then
    sudo dnf install -y gcc pkg-config cairo-devel git curl jq rust cargo
  else
    echo "error: need pacman or dnf to install build deps" >&2
    exit 1
  fi
}
if ! command -v cargo >/dev/null || ! command -v cc >/dev/null; then
  echo "==> installing build dependencies (needs sudo)"
  install_deps
else
  echo "==> build tools present (cargo $(cargo --version | cut -d' ' -f2))"
fi

# --- build ---
echo "==> building release binary"
cargo build --release --manifest-path "$SRC/Cargo.toml"

# --- install binary + unit ---
mkdir -p "$BIN_DIR" "$UNIT_DIR"
install -m755 "$SRC/target/release/omarchy-touchbar" "$BIN"
cat >"$UNIT" <<EOF
[Unit]
Description=Barmarchy Touch Bar daemon (native, replaces tiny-dfr)
After=default.target

[Service]
ExecStart=$BIN
Restart=always
RestartSec=5
# needs DRM device (user must be in video group); Touch Bar input comes from
# the udev rule installed below (video group), input group is a fallback

[Install]
WantedBy=default.target
EOF
echo "==> installed $BIN"

# --- groups (need re-login only if newly added) ---
need_relogin=false
for g in video input; do
  if ! id -nG | tr ' ' '\n' | grep -qx "$g"; then
    echo "==> adding you to group $g (needs sudo)"
    sudo usermod -aG "$g" "$USER"
    need_relogin=true
  fi
done

# --- udev rule (input access tied to the stable video group, survives group resets) ---
if [[ -f "$SRC/udev/99-barmarchy-touchbar.rules" ]]; then
  echo "==> installing udev rule (needs sudo)"
  sudo install -m644 "$SRC/udev/99-barmarchy-touchbar.rules" /etc/udev/rules.d/99-barmarchy-touchbar.rules
  sudo udevadm control --reload-rules
  sudo udevadm trigger --subsystem-match=input --action=change 2>/dev/null || true
fi

# --- replace stock tiny-dfr ---
if systemctl list-unit-files tiny-dfr.service >/dev/null 2>&1; then
  echo "==> masking stock tiny-dfr (needs sudo)"
  sudo systemctl mask --now tiny-dfr.service 2>/dev/null || true
fi

# --- enable ---
systemctl --user daemon-reload
systemctl --user enable --now omarchy-touchbar.service
sleep 2
if systemctl --user is-active --quiet omarchy-touchbar.service; then
  echo "==> barmarchy is running"
  journalctl --user -u omarchy-touchbar.service --since "30 seconds ago" --no-pager 2>/dev/null | tail -3
else
  echo "error: service failed to start — check journalctl --user -u omarchy-touchbar.service" >&2
  exit 1
fi

if $need_relogin; then
  echo "NOTE: you were added to video/input groups — log out and back in, then: systemctl --user restart omarchy-touchbar.service"
fi
echo "done. edit buttons/apps in ~/.config/omarchy-touchbar/config.toml"
