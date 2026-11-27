#!/usr/bin/env python3
"""Renders a MIDI file to a WAV through FluidSynth. All eval audio comes from
this tool.

    render.py --soundfont piano.sf2 [--rate 44100] [--tail 3] in.mid out.wav

Reverb and chorus are off, so no note gets a tail the reference does not know
about. The sample rate is fixed (44100 unless asked otherwise), the synth runs
on one core with a fixed gain, and the output is 16-bit PCM, so the same
inputs give the same bytes from run to run. FluidSynth is found through
$FLUIDSYNTH, then the PATH. $FLUIDSYNTH_LIB_DIR is added to the loader path for
builds that are not installed.
"""

import argparse
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import wave
from pathlib import Path

SAMPLE_RATE = 44100
GAIN = 0.5


def fluidsynth():
	path = os.environ.get("FLUIDSYNTH") or shutil.which("fluidsynth")
	if not path:
		raise SystemExit("fluidsynth not found: install it or set $FLUIDSYNTH")
	return path


def render(midi, wav, soundfont, rate=SAMPLE_RATE):
	env = dict(os.environ)
	if lib := os.environ.get("FLUIDSYNTH_LIB_DIR"):
		env["LD_LIBRARY_PATH"] = lib + ":" + env.get("LD_LIBRARY_PATH", "")
	with tempfile.TemporaryDirectory() as tmp:
		raw = Path(tmp) / "out.raw"
		command = [
			fluidsynth(), "-ni", "-q",
			"-r", str(rate), "-g", str(GAIN),
			"-R", "0", "-C", "0",  # reverb and chorus off
			"-o", "synth.reverb.active=0",
			"-o", "synth.chorus.active=0",
			"-o", "synth.cpu-cores=1",
			"-o", "synth.polyphony=256",
			"-o", "synth.midi-bank-select=gm",
			# Raw PCM, which every build can write; the WAV header is added
			# below, since a FluidSynth without libsndfile cannot write one.
			"-T", "raw", "-O", "s16", "-E", "little",
			"-F", str(raw), str(soundfont), str(midi),
		]
		subprocess.run(command, check=True, env=env, stdout=subprocess.DEVNULL)
		pcm = raw.read_bytes()
	with wave.open(str(wav), "wb") as out:
		out.setnchannels(2)
		out.setsampwidth(2)
		out.setframerate(rate)
		out.writeframes(pcm)


def vlq(n):
	out = [n & 0x7F]
	n >>= 7
	while n:
		out.append((n & 0x7F) | 0x80)
		n >>= 7
	return bytes(reversed(out))


def note_midi(notes, tail=3.0, ppq=480, tempo=500_000):
	"""A Standard MIDI File of (key, velocity, start, end) notes in seconds, on
	channel 0 with the piano, followed by `tail` seconds of silence so the
	renderer runs on through the releases."""
	ticks = lambda s: round(s * 1e6 / tempo * ppq)
	events = [(0, 0, bytes([0xC0, 0]))]
	for key, velocity, start, end in notes:
		events.append((ticks(start), 2, bytes([0x90, key, velocity])))
		events.append((ticks(end), 1, bytes([0x80, key, 0])))
	last = max(end for *_, end in notes) + tail
	events.sort(key=lambda e: (e[0], e[1]))
	track, now = bytearray(), 0
	track += vlq(0) + b"\xff\x51\x03" + tempo.to_bytes(3, "big")
	for tick, _, data in events:
		track += vlq(tick - now) + data
		now = tick
	track += vlq(max(0, ticks(last) - now)) + b"\xff\x2f\x00"
	return b"MThd" + struct.pack(">IHHH", 6, 0, 1, ppq) + b"MTrk" + struct.pack(">I", len(track)) + bytes(track)


def main():
	ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
	ap.add_argument("--soundfont", required=True)
	ap.add_argument("--rate", type=int, default=SAMPLE_RATE)
	ap.add_argument("midi")
	ap.add_argument("wav")
	args = ap.parse_args()
	render(args.midi, args.wav, args.soundfont, args.rate)


if __name__ == "__main__":
	main()
