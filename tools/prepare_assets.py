# -*- coding: utf-8 -*-
"""
Phirura asset preparation.

Phira ships its note textures as a single hold atlas (hold.png + info.yml
`holdAtlas: [50, 50]`). WP8.1's WinRT ImageBrush has no Viewbox, so cropping an
atlas at runtime is painful. Instead we split the atlas into three PNGs here and
ship them as separate content files.

It also synthesizes a fully self-contained demo song (WAV) plus a matching RPE
chart, so the port is playable without downloading anything.

Usage:  python prepare_assets.py
"""
import os
import struct
import sys
import wave
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
PHIRA = os.path.join(ROOT, "phira-main", "phira-main")
SRC_RESPACK = os.path.join(PHIRA, "assets", "respack")
SRC_ASSETS = os.path.join(PHIRA, "assets")
DST = os.path.join(ROOT, "Phirura", "Phirura", "Phirura.WindowsPhone", "Assets")
DST_GAME = os.path.join(DST, "game")
DST_DEMO = os.path.join(DST, "demo")

# --------------------------------------------------------------------------
# Minimal PNG codec (8-bit RGBA, non-interlaced) - enough for Phira's assets.
# --------------------------------------------------------------------------


def png_read(path):
    data = open(path, "rb").read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a png: " + path
    pos = 8
    idat = []
    w = h = depth = ctype = None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        ctag = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if ctag == b"IHDR":
            w, h, depth, ctype, _comp, _filt, interlace = struct.unpack(">IIBBBBB", body)
            assert depth == 8, "only 8-bit png supported"
            assert interlace == 0, "interlaced png not supported"
        elif ctag == b"IDAT":
            idat.append(body)
        elif ctag == b"IEND":
            break
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[ctype]
    raw = zlib.decompress(b"".join(idat))
    stride = w * channels
    out = bytearray(stride * h)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        ftype = raw[p]
        p += 1
        line = bytearray(raw[p : p + stride])
        p += stride
        if ftype == 1:
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 0xFF
        elif ftype == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ftype == 3:
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                b = prev[i]
                c = prev[i - channels] if i >= channels else 0
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        elif ftype != 0:
            raise ValueError("bad filter %d" % ftype)
        out[y * stride : (y + 1) * stride] = line
        prev = line
    if channels == 4:
        return w, h, bytes(out)
    # widen to RGBA
    rgba = bytearray(w * h * 4)
    for i in range(w * h):
        px = out[i * channels : i * channels + channels]
        if channels == 3:
            rgba[i * 4 : i * 4 + 3] = px
            rgba[i * 4 + 3] = 255
        elif channels == 1:
            rgba[i * 4 : i * 4 + 3] = bytes([px[0], px[0], px[0]])
            rgba[i * 4 + 3] = 255
        elif channels == 2:
            rgba[i * 4 : i * 4 + 3] = bytes([px[0], px[0], px[0]])
            rgba[i * 4 + 3] = px[1]
    return w, h, bytes(rgba)


def png_write(path, w, h, rgba):
    raw = bytearray()
    stride = w * 4
    for y in range(h):
        raw.append(0)
        raw += rgba[y * stride : (y + 1) * stride]

    def chunk(tag, body):
        return (
            struct.pack(">I", len(body))
            + tag
            + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
        )

    out = b"\x89PNG\r\n\x1a\n"
    out += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    out += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    out += chunk(b"IEND", b"")
    open(path, "wb").write(out)


def crop_rows(w, h, rgba, y0, y1):
    stride = w * 4
    return rgba[y0 * stride : y1 * stride]


# --------------------------------------------------------------------------
# WAV synthesis helpers
# --------------------------------------------------------------------------

SR = 44100


def wav_write(path, samples):
    """samples: list of floats in [-1, 1]."""
    data = bytearray()
    for s in samples:
        v = int(max(-1.0, min(1.0, s)) * 32000)
        data += struct.pack("<h", v)
    w = wave.open(path, "wb")
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(SR)
    w.writeframes(bytes(data))
    w.close()


def midi_hz(n):
    return 440.0 * (2.0 ** ((n - 69) / 12.0))


def synth(dur, freqs, env_a, env_d, env_s, env_r, wave_kind="square", duty=0.5):
    """One note: list of frequencies (for slight detune / unison)."""
    n = int(dur * SR)
    out = [0.0] * n
    for f in freqs:
        phase = 0.0
        step = f / SR
        for i in range(n):
            t = i / SR
            if wave_kind == "square":
                v = 1.0 if (phase % 1.0) < duty else -1.0
            elif wave_kind == "saw":
                v = 2.0 * (phase % 1.0) - 1.0
            elif wave_kind == "tri":
                x = phase % 1.0
                v = 4.0 * abs(x - 0.5) - 1.0
            else:
                v = __import__("math").sin(2 * __import__("math").pi * phase)
            # ADSR
            if t < env_a:
                e = t / env_a if env_a > 0 else 1.0
            elif t < env_a + env_d:
                e = 1.0 - (1.0 - env_s) * (t - env_a) / env_d
            else:
                e = env_s
            rel = dur - t
            if rel < env_r:
                e *= rel / env_r
            out[i] += v * e
            phase += step
    return out


def mix_into(dst, src, at, gain):
    n = min(len(src), len(dst) - at)
    for i in range(n):
        dst[at + i] += src[i] * gain


def noise_hit(dur, decay, lowpass=0.0):
    import random

    random.seed(12345)
    n = int(dur * SR)
    out = [0.0] * n
    prev = 0.0
    for i in range(n):
        x = random.uniform(-1.0, 1.0)
        if lowpass > 0:
            prev = prev * lowpass + x * (1.0 - lowpass)
            x = prev
        out[i] = x * ((1.0 - i / n) ** decay)
    return out


def kick(dur=0.18):
    n = int(dur * SR)
    out = [0.0] * n
    phase = 0.0
    for i in range(n):
        t = i / SR
        f = 140.0 * (0.35 ** (t / dur)) + 45.0
        phase += f / SR
        out[i] = __import__("math").sin(2 * __import__("math").pi * phase) * ((1.0 - t / dur) ** 1.5)
    return out


def main():
    os.makedirs(DST_GAME, exist_ok=True)
    os.makedirs(DST_DEMO, exist_ok=True)

    # ---- 1. split the hold atlas -----------------------------------------
    w, h, rgba = png_read(os.path.join(SRC_RESPACK, "hold.png"))
    tail_rows, head_rows = 50, 50
    assert w == 1000 and h == 1100, "unexpected hold.png size %dx%d" % (w, h)
    png_write(os.path.join(DST_GAME, "hold_tail.png"), w, tail_rows, crop_rows(w, h, rgba, 0, tail_rows))
    png_write(os.path.join(DST_GAME, "hold_head.png"), w, head_rows, crop_rows(w, h, rgba, h - head_rows, h))
    png_write(
        os.path.join(DST_GAME, "hold_body.png"),
        w,
        h - tail_rows - head_rows,
        crop_rows(w, h, rgba, tail_rows, h - head_rows),
    )
    print("hold atlas split: head %dx%d body %dx%d tail %dx%d" % (w, head_rows, w, h - tail_rows - head_rows, w, tail_rows))

    # ---- 2. copy the plain note textures ---------------------------------
    for name in ("click.png", "drag.png", "flick.png", "hit_fx.png"):
        src = os.path.join(SRC_RESPACK, name)
        open(os.path.join(DST_GAME, name), "wb").write(open(src, "rb").read())
        print("copied", name)

    # ---- 3. fonts / misc --------------------------------------------------
    for name in ("phigros.ttf", "bold.ttf"):
        src = os.path.join(SRC_ASSETS, name)
        if os.path.exists(src):
            open(os.path.join(DST_GAME, name), "wb").write(open(src, "rb").read())
            print("copied", name)

    # ---- 4. hit sound effects (WP8.1 cannot decode .ogg) ------------------
    wav_write(os.path.join(DST_GAME, "click.wav"), synth(0.09, [1600.0], 0.001, 0.02, 0.0, 0.05, "tri"))
    wav_write(os.path.join(DST_GAME, "drag.wav"), synth(0.06, [900.0], 0.001, 0.01, 0.0, 0.03, "tri"))
    wav_write(os.path.join(DST_GAME, "flick.wav"), synth(0.11, [2400.0], 0.001, 0.02, 0.0, 0.06, "saw"))
    print("generated hit sfx")

    # ---- 5. demo song -----------------------------------------------------
    bpm = 120.0
    beat = 60.0 / bpm
    bars = 16
    total = bars * 4 * beat
    n = int(total * SR)
    song = [0.0] * n

    # Am - F - C - G, one bar each, repeated
    prog = [
        (57, [57, 60, 64]),  # Am
        (53, [53, 57, 60]),  # F
        (48, [48, 52, 55]),  # C
        (55, [55, 59, 62]),  # G
    ]
    # a simple pentatonic melody, one entry per half beat
    scale = [69, 72, 76, 79, 81, 79, 76, 72]
    melody = []
    for b in range(bars):
        base = scale[b % 4] + (0 if (b // 4) % 2 == 0 else -3)
        for k in range(8):
            melody.append(None if (b + k) % 7 == 3 else base + scale[(k * 3 + b) % 8] - 69)

    for b in range(bars):
        bar_at = int(b * 4 * beat * SR)
        root, chord = prog[b % 4]

        # bass: root on beats 1 and 3
        for k in (0, 2):
            at = bar_at + int(k * beat * SR)
            mix_into(song, synth(beat * 1.4, [midi_hz(root - 12)], 0.005, 0.08, 0.45, 0.25, "saw"), at, 0.22)

        # arpeggio on eighths
        for k in range(8):
            at = bar_at + int(k * beat / 2 * SR)
            f = midi_hz(chord[k % 3] + 12)
            mix_into(song, synth(beat * 0.42, [f], 0.002, 0.05, 0.15, 0.12, "square", 0.25), at, 0.10)

        # melody on the lead
        for k in range(8):
            note = melody[b * 8 + k]
            if note is None:
                continue
            at = bar_at + int(k * beat / 2 * SR)
            mix_into(
                song,
                synth(beat * 0.46, [midi_hz(note), midi_hz(note) * 1.003], 0.004, 0.06, 0.35, 0.18, "square", 0.5),
                at,
                0.13,
            )

        # drums
        for k in (0, 2):
            mix_into(song, kick(), bar_at + int(k * beat * SR), 0.55)
        for k in range(8):
            if k % 2 == 1:
                mix_into(song, noise_hit(0.05, 3.0, 0.6), bar_at + int(k * beat / 2 * SR), 0.10)

    peak = max(abs(v) for v in song) or 1.0
    song = [v / peak * 0.85 for v in song]
    wav_write(os.path.join(DST_DEMO, "demo.wav"), song)
    print("demo song: %.2fs" % total)

    # ---- 6. demo RPE chart ------------------------------------------------
    # Beat grid: note times are [i, n, d] triples meaning i + n/d beats.
    lines = []
    notes = []
    half = 0.5
    for b in range(bars):
        for k in range(8):
            beat_in_bar = k * 0.5
            abs_beat = b * 4 + beat_in_bar
            # leave the very first bar empty so the player can settle in
            if b == 0:
                continue
            pattern = (b + k) % 8
            if pattern in (0, 1):
                kind = 1  # click
            elif pattern == 2:
                kind = 4  # drag
            elif pattern == 5:
                kind = 3  # flick
            else:
                kind = 1
            above = 1 if (b + k) % 2 == 0 else 0
            x = [(-0.62 + 0.31 * ((b + k) % 5)), -0.62 + 0.31 * ((b * 3 + k) % 5)][0]
            i = int(abs_beat)
            frac = abs_beat - i
            n_, d_ = (int(round(frac * 4)), 4) if frac else (0, 1)
            end_i = i
            end_n, end_d = n_, d_
            if kind == 1 and (b + k) % 16 == 7:
                kind = 2  # hold
                end_beat = abs_beat + 1.0
                end_i = int(end_beat)
                ef = end_beat - end_i
                end_n, end_d = (int(round(ef * 4)), 4) if ef else (0, 1)
            notes.append(
                {
                    "type": kind,
                    "above": above,
                    "startTime": [i, n_, d_],
                    "endTime": [end_i, end_n, end_d],
                    "positionX": round(x * 675.0, 3),
                    "yOffset": 0.0,
                    "alpha": 255,
                    "hitsound": None,
                    "size": 1.0,
                    "speed": 1.0,
                    "isFake": 0,
                    "visibleTime": 999.0,
                }
            )

    lines.append(
        {
            "Name": "line",
            "Texture": "line.png",
            "father": -1,
            "rotateWithFather": False,
            "eventLayers": [
                {
                    "alphaEvents": [
                        {
                            "easingLeft": 0.0,
                            "easingRight": 1.0,
                            "bezier": 0,
                            "bezierPoints": [0.0, 0.0, 0.0, 0.0],
                            "easingType": 1,
                            "start": 255.0,
                            "end": 255.0,
                            "startTime": [0, 0, 1],
                            "endTime": [0, 0, 1],
                        }
                    ],
                    # RPE REQUIRES speedEvents: the line's height is the integral of the
                    # speed, and a note's travel distance is (note height - line height).
                    # With no speed events every note height stays 0, so the whole chart
                    # collapses onto the judge line and nothing appears to move.
                    #
                    # Phira scales these values by SPEED_RATIO = 10/45/0.83175, so
                    # 1/SPEED_RATIO here means an internal speed of exactly 1 chart-unit
                    # per second, which gives each note ~1s of visible travel time.
                    "speedEvents": [
                        {
                            "easingLeft": 0.0,
                            "easingRight": 1.0,
                            "bezier": 0,
                            "bezierPoints": [0.0, 0.0, 0.0, 0.0],
                            "easingType": 1,
                            "start": 1.0 / (10.0 / 45.0 / 0.83175),
                            "end": 1.0 / (10.0 / 45.0 / 0.83175),
                            "startTime": [0, 0, 1],
                            "endTime": [bars * 4, 0, 1],
                        }
                    ],
                }
            ],
            "extended": None,
            "notes": notes,
            "isCover": 0,
            "zOrder": 0,
            "attachUI": None,
            "posControl": [],
            "sizeControl": [],
            "alphaControl": [],
            "yControl": [],
        }
    )

    chart = {
        "META": {"offset": 0, "RPEVersion": 170},
        "BPMList": [{"bpm": bpm, "startTime": [0, 0, 1]}],
        "judgeLineList": lines,
    }

    import json

    open(os.path.join(DST_DEMO, "chart.json"), "w", encoding="utf-8").write(
        json.dumps(chart, ensure_ascii=False, indent=1)
    )
    print("demo chart: %d notes, %d lines" % (len(notes), len(lines)))
    print("done ->", DST)


if __name__ == "__main__":
    main()
