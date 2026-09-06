#!/usr/bin/env bash
# Hardware PoC — RUN THIS IN YOUR OWN TERMINAL (needs sudo).
# Proves we can modeset the M1 Touch Bar (connector 39, 60x2008)
# while tiny-dfr is stopped, then restores tiny-dfr.
set -u

echo "== touchbar hardware PoC =="
echo "[1/4] tiny-dfr status:"
systemctl status tiny-dfr --no-pager | head -n 5

echo "[2/4] stopping tiny-dfr (ESC/F-keys will freeze briefly)..."
sudo systemctl stop tiny-dfr
sleep 1

echo "[3/4] modeset test pattern for 6s (you should see color bars)..."
sudo timeout 6 modetest -a -s 39:60x2008 || \
  sudo timeout 6 modetest -s 39:60x2008
echo "      ...pattern done (exit $?)"

echo "[4/4] restarting tiny-dfr..."
sudo systemctl start tiny-dfr
sleep 1
systemctl status tiny-dfr --no-pager | head -n 5
echo "If the Touch Bar is back to F-keys/media, the PoC PASSED."
