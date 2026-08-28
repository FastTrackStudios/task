#!/usr/bin/env python3
"""Generate the example studio's audio — as *songs* the player streams.

Small, musical, and reproducible: each track is a short chord loop with
a plucked-string envelope, rendered to mono 16-bit WAV at 12 kHz — low
enough to keep every file under ~350 KB so the whole set can live in
git, high enough to sound like music rather than a test tone. Rerunning
the script reproduces the same bytes (no randomness).

The layout is the platform's colocated-song convention, which the
global player (`task-player-ui`) streams directly:

    Resources/songs/<slug>/<Title>.wav
    Resources/songs/<slug>/manifest.json   (title, duration, stems)

The album's deliverable items map to these songs by slugified title —
"Track One" → `track-one` — which is how a click on a part becomes a
NowPlaying queue entry.

Run from the repo root:  python3 examples/studio/tools/gen_audio.py
"""

import json
import math
import os
import struct
import wave

RATE = 12_000
HERE = os.path.dirname(os.path.abspath(__file__))
SONGS = os.path.join(HERE, "..", "acme-audio", "Resources", "songs")
VNT_SONGS = os.path.join(HERE, "..", "vnt-video", "Resources", "songs")


def note(freq, dur, amp=0.5):
    """One plucked note: a few harmonics under an exponential decay."""
    n = int(RATE * dur)
    out = []
    for i in range(n):
        t = i / RATE
        env = math.exp(-3.0 * t)
        s = (
            1.00 * math.sin(2 * math.pi * freq * t)
            + 0.45 * math.sin(2 * math.pi * freq * 2 * t)
            + 0.20 * math.sin(2 * math.pi * freq * 3 * t)
        )
        out.append(amp * env * s / 1.65)
    return out


def mix(base, add, at):
    i0 = int(RATE * at)
    for j, v in enumerate(add):
        k = i0 + j
        if k < len(base):
            base[k] += v
    return base


def hz(midi):
    return 440.0 * (2 ** ((midi - 69) / 12))


def render(slug, title, chords, beat, bpm, bass_amp=0.6, songs_dir=None):
    """One song: arpeggiated chords + bass, WAV + manifest."""
    total = len(chords) * 4 * beat + 1.5
    buf = [0.0] * int(RATE * total)
    for bar, chord in enumerate(chords):
        t0 = bar * 4 * beat
        # Bass on the downbeat, an octave under the root.
        mix(buf, note(hz(chord[0] - 12), beat * 3.5, bass_amp), t0)
        # The chord, arpeggiated across the bar.
        for step in range(4):
            n = chord[step % len(chord)]
            mix(buf, note(hz(n), beat * 2.2, 0.42), t0 + step * beat)
    peak = max(abs(v) for v in buf) or 1.0
    frames = b"".join(
        struct.pack("<h", int(max(-1.0, min(1.0, v / peak * 0.9)) * 32767))
        for v in buf
    )
    song_dir = os.path.join(songs_dir or SONGS, slug)
    os.makedirs(song_dir, exist_ok=True)
    wav = os.path.join(song_dir, f"{title}.wav")
    with wave.open(wav, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(frames)
    manifest = {
        "slug": slug,
        "title": title,
        "artist": "ACME Audio",
        "bpm": bpm,
        "duration_sec": round(total, 2),
        # The player picks the reference/original stem as the track.
        "stems": [{"name": "Reference Mix", "file": f"{title}.wav"}],
    }
    with open(os.path.join(song_dir, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    print(f"{wav}  {os.path.getsize(wav)//1024} KB  ({total:.1f}s)")


BEAT = 0.32

# Track One — bright I–V–vi–IV in C.
render(
    "track-one",
    "Track One",
    [[60, 64, 67], [55, 59, 62], [57, 60, 64], [53, 57, 60]] * 2,
    BEAT,
    round(60 / BEAT),
)
# Track Two — minor and slower.
render(
    "track-two",
    "Track Two",
    [[57, 60, 64], [53, 57, 60], [55, 59, 62], [52, 55, 59]] * 2,
    BEAT * 1.25,
    round(60 / (BEAT * 1.25)),
)
# Track Three — suspended, quicker.
render(
    "track-three",
    "Track Three",
    [[62, 67, 69], [60, 65, 67], [58, 63, 65], [60, 65, 67]] * 2,
    BEAT * 0.8,
    round(60 / (BEAT * 0.8)),
)
# The single's master — the album opener a step up, as its own mix.
render(
    "single-master",
    "Single master",
    [[62, 66, 69], [57, 61, 64], [59, 62, 66], [55, 59, 62]] * 2,
    BEAT,
    round(60 / BEAT),
)
# VNT's half of the Shared Project — the live session recording the
# recap cut is synced to. Slower, roomier voicing: a different band.
render(
    "live-session-recording",
    "Live session recording",
    [[55, 62, 66], [53, 60, 64], [57, 64, 67], [50, 57, 62]] * 2,
    BEAT * 1.4,
    round(60 / (BEAT * 1.4)),
    bass_amp=0.7,
    songs_dir=VNT_SONGS,
)
