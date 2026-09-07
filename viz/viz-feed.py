#!/usr/bin/env python3
"""Touch-bar visualizer feed: capture the default sink monitor, FFT it
into 10 log-spaced bands, and publish normalized levels for the bar.

The bar itself never captures audio — it only reads this file
(`~/.cache/omarchy-touchbar/deck/levels.json`, fresh = <2s old).
This helper owns all audio work:

  - idle (nothing MPRIS-Playing): sleep, remove levels.json so the bar
    hides the visualizer promptly instead of waiting out the stale window
  - playing: `pw-record` the default sink's `.monitor`, one 1024-pt Hann
    FFT per 100ms tick, log bands 60Hz..14kHz, auto-gain + attack/decay
    smoothing, atomic JSON write at ~10Hz

Stdlib only (no numpy/cava). ~5% of one core while playing, ~0 idle.
"""
import json
import math
import os
import struct
import subprocess
import sys
import tempfile
import time

RATE = 48000
N = 1024  # fft size (21ms window @48k)
TICK = 0.1  # seconds per frame (~10Hz writes)
BANDS = 10
FMIN = 60.0
FMAX = 14000.0
ATTACK = 0.7  # rise fast (bounce up)
RELEASE = 0.25  # fall slow (settle down)
GAIN_DECAY = 0.995  # auto-gain peak falloff per tick
GAIN_FLOOR = 50.0  # ignore noise below this magnitude


def deck_path():
    home = os.environ.get("HOME", "/root")
    d = os.path.join(home, ".cache", "omarchy-touchbar", "deck")
    os.makedirs(d, exist_ok=True)
    return os.path.join(d, "levels.json")


def run_out(args):
    try:
        out = subprocess.run(
            args, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5
        )
        if out.returncode == 0:
            return out.stdout.decode(errors="replace").strip()
    except Exception:
        pass
    return ""


_mpris_cache = (None, 0.0)


def mpris_playing():
    """(is_playing, identity) with a 1s cache — busctl forks are slow."""
    global _mpris_cache
    now = time.monotonic()
    if now - _mpris_cache[1] < 1.0:
        return _mpris_cache[0]
    playing, ident = False, "music"
    buses = [
        line.split()[0]
        for line in run_out(["busctl", "--user", "list"]).splitlines()
        if "org.mpris.MediaPlayer2." in line
    ]
    for bus in sorted(buses):
        st = run_out(
            [
                "busctl", "--user", "get-property", bus,
                "/org/mpris/MediaPlayer2",
                "org.mpris.MediaPlayer2.Player", "PlaybackStatus",
            ]
        )
        if '"Playing"' in st:
            playing = True
            # busctl prints typed values: s "Cliamp"
            name = run_out(
                [
                    "busctl", "--user", "get-property", bus,
                    "/org/mpris/MediaPlayer2",
                    "org.mpris.MediaPlayer2", "Identity",
                ]
            ).strip()
            if name.startswith("s "):
                name = name[2:].strip()
            name = name.strip('"')
            if name:
                ident = name[:16]
            break
    _mpris_cache = ((playing, ident), now)
    return playing, ident


def monitor_source():
    """Default sink's monitor (what you hear), verified to exist."""
    sink = run_out(["pactl", "get-default-sink"])
    if not sink:
        return None
    mon = sink + ".monitor"
    short = run_out(["pactl", "list", "short", "sources"])
    if mon in short:
        return mon
    # fallback: first running monitor
    for line in short.splitlines():
        if ".monitor" in line and "RUNNING" in line:
            return line.split()[1]
    return None


def hann(n):
    return [0.5 - 0.5 * math.cos(2 * math.pi * i / (n - 1)) for i in range(n)]


def fft(x):
    """In-place iterative radix-2 FFT (n must be a power of two)."""
    n = len(x)
    j = 0
    for i in range(1, n):
        bit = n >> 1
        while j & bit:
            j ^= bit
            bit >>= 1
        j ^= bit
        if i < j:
            x[i], x[j] = x[j], x[i]
    length = 2
    while length <= n:
        ang = -2.0 * math.pi / length
        wlen = complex(math.cos(ang), math.sin(ang))
        for i in range(0, n, length):
            w = 1.0 + 0.0j
            half = length // 2
            for k in range(half):
                u = x[i + k]
                v = x[i + k + half] * w
                x[i + k] = u + v
                x[i + k + half] = u - v
                w *= wlen
        length <<= 1
    return x


def band_edges(nbands, fmin, fmax, rate, n):
    lo = math.log(fmin)
    hi = math.log(fmax)
    edges = [int(math.exp(lo + (hi - lo) * i / nbands) * n / rate) for i in range(nbands + 1)]
    edges[0] = max(edges[0], 1)  # skip DC
    return edges


class Analyzer:
    def __init__(self):
        self.win = hann(N)
        self.edges = band_edges(BANDS, FMIN, FMAX, RATE, N)
        self.smooth = [0.0] * BANDS
        self.gmax = GAIN_FLOOR

    def analyze(self, samples):
        """1024+ int16 samples -> 10 smoothed 0..1 levels."""
        buf = [samples[i] * self.win[i] for i in range(N)]
        spec = fft([complex(v, 0.0) for v in buf])
        mags = [abs(c) / (N / 2) for c in spec[: N // 2]]
        raw = []
        for b in range(BANDS):
            lo, hi = self.edges[b], max(self.edges[b + 1], self.edges[b] + 1)
            raw.append(sum(mags[lo:hi]) / (hi - lo))
        peak = max(raw)
        self.gmax = max(peak, self.gmax * GAIN_DECAY, GAIN_FLOOR)
        out = []
        for b in range(BANDS):
            target = math.sqrt(min(raw[b] / self.gmax, 1.0))
            s = self.smooth[b]
            s += (target - s) * (ATTACK if target > s else RELEASE)
            self.smooth[b] = s
            out.append(round(max(0.0, min(s, 1.0)), 3))
        return out


def read_exact(pipe, size):
    buf = bytearray()
    while len(buf) < size:
        chunk = pipe.read(size - len(buf))
        if not chunk:
            return None  # EOF: capture died (sink moved?)
        buf += chunk
    return bytes(buf)


def publish(path, levels, label):
    body = json.dumps({"levels": levels, "label": label})
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".levels")
    try:
        with os.fdopen(fd, "w") as f:
            f.write(body)
        os.replace(tmp, path)
    except Exception:
        try:
            os.unlink(tmp)
        except OSError:
            pass


def capture_loop(mon, analyzer, path, label):
    """Run pw-record until it dies; returns when capture ends."""
    proc = subprocess.Popen(
        [
            "pw-record", "--target", mon,
            "--format", "s16", "--rate", str(RATE), "--channels", "1",
            "-",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    try:
        want = int(RATE * TICK) * 2  # bytes per 100ms tick
        while True:
            playing, _ = mpris_playing()
            if not playing:
                return  # back to idle; caller unlinks the file
            data = read_exact(proc.stdout, want)
            if data is None:
                return  # sink moved or stream died; caller restarts
            ns = len(data) // 2
            samples = struct.unpack("<%dh" % ns, data)
            frame = list(samples[-N:]) if ns >= N else [0] * (N - ns) + list(samples)
            levels = analyzer.analyze(frame)
            publish(path, levels, label)
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=2)
        except Exception:
            try:
                proc.kill()
            except Exception:
                pass


def main():
    path = deck_path()
    analyzer = Analyzer()
    print("viz-feed: publishing to", path, file=sys.stderr)
    while True:
        playing, label = mpris_playing()
        if not playing:
            try:
                os.unlink(path)
            except OSError:
                pass
            time.sleep(1.0)
            continue
        mon = monitor_source()
        if not mon:
            time.sleep(1.0)
            continue
        print(f"viz-feed: capturing {mon} ({label})", file=sys.stderr)
        capture_loop(mon, analyzer, path, label)
        print("viz-feed: capture stopped", file=sys.stderr)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
