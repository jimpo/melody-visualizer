#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy"]
# ///
"""Builds the eval SoundFonts: one piano cut out of each source bank, with a
volume envelope written into the SoundFont from the MIDI generator's numbers.

    uv run eval/soundfont/build.py [--config chain.toml] [--out DIR] [--sources DIR]

The envelope comes from the `[spectrum_generator.envelope]` table of the
plugin's own `Config` (what `eval --dump-config` prints), the same numbers the
MIDI generator reads, so the two cannot drift apart. Without `--config` the
defaults are taken from `cargo run --bin eval -- --dump-config`.

Sources are downloaded to `--sources` (default target/soundfont-sources),
checked against the sha256 in manifest.toml, and cut down to one preset with:

* each sample replaced by a stationary loop of the same harmonic content: the
  recorded loop is analysed, and a whole number of periods of its harmonics is
  resynthesised at exactly the key's pitch. A recorded loop is not periodic, so
  it beats and glitches at the wrap, and a recorded decay would multiply the
  envelope; the loop has neither, so the envelope alone shapes the sound;
* every preset-level generator, modulator, filter, LFO, modulation envelope,
  reverb/chorus send and key-scaling generator removed;
* the volume envelope set to the config's values, in timecents and centibels.

The output is deterministic: the same source and config give the same bytes.
"""

import argparse
import hashlib
import math
import os
import struct
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.request

import numpy as np
from pathlib import Path

import sf2

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent  # the repository
FLOOR_TIMECENTS = -12000  # the SoundFont minimum: about a millisecond
EXTRA_SAMPLE_POINTS = 46  # points the format requires after each sample
LOOP_SAMPLES = 4096  # about how long a loop is, in samples

# Generators that would colour the sound or its level beyond the envelope.
STRIPPED = {
	sf2.GEN_START_ADDRS_OFFSET, sf2.GEN_END_ADDRS_OFFSET,
	sf2.GEN_START_ADDRS_COARSE_OFFSET, sf2.GEN_END_ADDRS_COARSE_OFFSET,
	5, 6, 7, 8, 9, 10, 11, 13,  # LFO and mod envelope routings, filter
	15, 16,  # chorus and reverb sends
	17,  # pan: every voice plays centred
	*range(21, 33),  # LFO and modulation envelope timing
	sf2.GEN_DELAY_VOL_ENV, sf2.GEN_ATTACK_VOL_ENV, sf2.GEN_HOLD_VOL_ENV,
	sf2.GEN_DECAY_VOL_ENV, sf2.GEN_SUSTAIN_VOL_ENV, sf2.GEN_RELEASE_VOL_ENV,
	sf2.GEN_KEYTOVOLENVHOLD, sf2.GEN_KEYTOVOLENVDECAY,
	46, 47,  # forced key and velocity
	51, 52,  # tuning offsets: every key plays at its own pitch
	57,  # exclusive class
}


def timecents(seconds):
	return FLOOR_TIMECENTS if seconds <= 0 else max(FLOOR_TIMECENTS, round(1200 * math.log2(seconds)))


def envelope_generators(env):
	"""The volume envelope as SoundFont generators (operator, signed amount)."""
	return [
		(sf2.GEN_DELAY_VOL_ENV, timecents(env["delay"])),
		(sf2.GEN_ATTACK_VOL_ENV, timecents(env["attack"])),
		(sf2.GEN_HOLD_VOL_ENV, timecents(env["hold"])),
		(sf2.GEN_DECAY_VOL_ENV, timecents(env["decay"])),
		(sf2.GEN_SUSTAIN_VOL_ENV, round(env["sustain"] * 10)),
		(sf2.GEN_RELEASE_VOL_ENV, timecents(env["release"])),
		(sf2.GEN_KEYTOVOLENVHOLD, round(env["key_to_hold"])),
		(sf2.GEN_KEYTOVOLENVDECAY, round(env["key_to_decay"])),
	]


def load_envelope(config_path):
	if config_path:
		text = Path(config_path).read_text()
	else:
		text = subprocess.run(
			["cargo", "run", "-q", "--release", "--bin", "eval", "--", "--dump-config"],
			cwd=ROOT, check=True, capture_output=True, text=True,
		).stdout
	return tomllib.loads(text)["spectrum_generator"]["envelope"]


def sha256(path):
	h = hashlib.sha256()
	with open(path, "rb") as f:
		for block in iter(lambda: f.read(1 << 20), b""):
			h.update(block)
	return h.hexdigest()


def fetch(source, directory):
	"""Downloads and unpacks a source's SF2, verifying both checksums."""
	directory.mkdir(parents=True, exist_ok=True)
	sf2_path = directory / source["file"]
	if sf2_path.exists() and sha256(sf2_path) == source["sha256"]:
		return sf2_path
	url = source["url"]
	archive = directory / (source["id"] + ".download")
	if not archive.exists() or sha256(archive) != source["download_sha256"]:
		print(f"downloading {url}", file=sys.stderr)
		urllib.request.urlretrieve(url, archive)
	if sha256(archive) != source["download_sha256"]:
		raise SystemExit(f"{url}: download checksum differs from the manifest")
	kind = source.get("archive", "none")
	if kind == "deb":
		unpack_deb(archive, source["member"], sf2_path)
	elif kind == "none":
		archive.replace(sf2_path)
	else:
		raise SystemExit(f"unknown archive kind {kind}")
	if sha256(sf2_path) != source["sha256"]:
		raise SystemExit(f"{sf2_path}: checksum differs from the manifest")
	return sf2_path


def unpack_deb(deb, member, dest):
	"""Pulls one file out of a .deb (an `ar` archive holding data.tar.*)."""
	data = deb.read_bytes()
	pos = 8
	while pos < len(data):
		name = data[pos : pos + 16].decode().strip()
		size = int(data[pos + 48 : pos + 58])
		body = pos + 60
		if name.startswith("data.tar"):
			with tempfile.TemporaryDirectory() as tmp:
				part = Path(tmp) / name
				part.write_bytes(data[body : body + size])
				with tarfile.open(part) as tar:
					f = tar.extractfile(member)
					dest.write_bytes(f.read())
			return
		pos = body + size + (size & 1)
	raise SystemExit(f"{deb}: no data.tar")


def find_preset(font, bank, program):
	for p in font.presets:
		if p.bank == bank and p.program == program:
			return p
	raise SystemExit(f"no preset {bank}:{program}")


def text(string):
	"""A zero-terminated string padded to an even length, as FluidSynth insists."""
	raw = string.encode("latin-1", "replace") + b"\0"
	return raw + b"\0" * (len(raw) & 1)


def estimate_f0(x, rate, nominal):
	"""The fundamental of a roughly periodic signal near `nominal` Hz: the
	frequency within a semitone whose first harmonics carry the most power."""
	x = x[:32768] - x[:32768].mean()
	n = np.arange(len(x))
	w = np.hanning(len(x))
	best = None
	for cents in np.arange(-100, 100.01, 2.0):
		f = nominal * 2 ** (cents / 1200)
		power = sum(abs((w * x * np.exp(-2j * np.pi * h * f / rate * n)).sum()) ** 2 for h in range(1, 7))
		if best is None or power > best[0]:
			best = (power, cents)
	centre = best[1]
	best = None
	for cents in np.arange(centre - 2, centre + 2.001, 0.1):
		f = nominal * 2 ** (cents / 1200)
		power = sum(abs((w * x * np.exp(-2j * np.pi * h * f / rate * n)).sum()) ** 2 for h in range(1, 7))
		if best is None or power > best[0]:
			best = (power, f)
	return best[1]


def loopify(font, sample, root_key):
	"""A stationary loop for `sample`: (int16 samples, loop length, rate).

	The loop is `k` periods of the recorded loop's own harmonics, at the phases
	and amplitudes it measured, and the rate is chosen so that it plays at
	exactly `root_key`'s pitch."""
	rate = sample.rate
	pcm = np.frombuffer(font.smpl, "<i2", count=sample.loop_end - sample.loop_start, offset=sample.loop_start * 2)
	x = pcm.astype(float)
	root_freq = 440 * 2 ** ((root_key - 69) / 12)
	# The sample's own pitch is the root's less its correction.
	nominal = root_freq * 2 ** (-sample.correction / 1200)
	f0 = estimate_f0(x, rate, nominal)
	period = rate / f0
	if len(x) < 4 * period:
		raise SystemExit(f"sample {sample.name}: loop shorter than four periods")
	window = x[:65536] - x[:65536].mean()
	n = np.arange(len(window))
	w = np.hanning(len(window))
	harmonics = int(0.45 * rate / f0)
	coeffs = [
		2 * (w * window * np.exp(-2j * np.pi * h * f0 / rate * n)).sum() / w.sum()
		for h in range(1, harmonics + 1)
	]
	periods = max(1, int(LOOP_SAMPLES // period))
	length = round(periods * period)
	new_rate = round(root_freq * length / periods)
	t = np.arange(length)
	y = np.zeros(length)
	for h, c in enumerate(coeffs, start=1):
		y += (c * np.exp(2j * np.pi * h * periods * t / length)).real
	# Keep the recorded loop's loudness.
	y *= np.sqrt((window**2).mean() / (y**2).mean())
	peak = np.abs(y).max()
	if peak > 32767:
		y *= 32767 / peak
	return np.round(y).astype("<i2"), length, new_rate


def cut(font, envelope, name, comment):
	"""One instrument out of the font's grand piano: its first layer, with the
	samples trimmed to their loops and the envelope replaced."""
	preset = find_preset(font, 0, 0)
	layer = next(z for z in preset.zones if z.get(sf2.GEN_INSTRUMENT) is not None)
	source = font.instruments[layer.get(sf2.GEN_INSTRUMENT)]

	samples, smpl = [], bytearray()
	index_of = {}
	roots = {}
	for z in source.zones:
		old = z.get(sf2.GEN_SAMPLE_ID)
		if old is not None:
			override = z.get(sf2.GEN_OVERRIDING_ROOT_KEY)
			s = font.samples[old]
			root = sf2.signed(override) if override is not None and sf2.signed(override) >= 0 else s.pitch
			roots.setdefault(old, root if root < 128 else 60)

	def sample_index(old):
		if old in index_of:
			return index_of[old]
		s = font.samples[old]
		if s.kind & 0x8000:
			raise SystemExit("ROM sample")
		if s.loop_end <= s.loop_start:
			raise SystemExit(f"sample {s.name} has no loop")
		pcm, length, rate = loopify(font, s, roots[old])
		start = len(smpl) // 2
		smpl.extend(pcm.tobytes())
		# Points past the end for the interpolator: the loop's start again.
		smpl.extend(np.resize(pcm, EXTRA_SAMPLE_POINTS).tobytes())
		index_of[old] = len(samples)
		samples.append(sf2.Sample(s.name, start, start + length, start, start + length, rate, roots[old], 0, 0, 1))
		return index_of[old]

	zones = []
	for z in source.zones:
		old = z.get(sf2.GEN_SAMPLE_ID)
		# A stereo pair's two recordings sum with a phase that a fraction of a
		# cent of detune walks around, so only the left one is kept.
		if old is not None and font.samples[old].kind & 2:
			continue
		gens = [(o, a) for o, a in z.gens if o not in STRIPPED]
		zone = sf2.Zone(gens, [])  # no modulators: the SoundFont defaults apply
		zone.remove(sf2.GEN_SAMPLE_ID)
		zone.remove(sf2.GEN_STARTLOOP_ADDRS_OFFSET)
		zone.remove(sf2.GEN_ENDLOOP_ADDRS_OFFSET)
		zone.remove(sf2.GEN_STARTLOOP_ADDRS_COARSE_OFFSET)
		zone.remove(sf2.GEN_ENDLOOP_ADDRS_COARSE_OFFSET)
		zone.remove(sf2.GEN_SAMPLE_MODES)
		if old is None:
			zone.gens = zone.gens + [(o, a & 0xFFFF) for o, a in envelope]
			zone.set(sf2.GEN_SAMPLE_MODES, 1)
		else:
			zone.set(sf2.GEN_SAMPLE_MODES, 1)
			zone.gens.append((sf2.GEN_SAMPLE_ID, sample_index(old)))
		zones.append(zone)
	# The global zone, if the source had none, carries the envelope.
	if zones and zones[0].get(sf2.GEN_SAMPLE_ID) is not None:
		zones.insert(0, sf2.Zone([(o, a & 0xFFFF) for o, a in envelope], []))
	# Sample links are dropped (the L/R pairs play as two mono voices).

	info = {
		"ifil": struct.pack("<HH", 2, 1),
		"isng": b"EMU8000\0",
		"INAM": text(name),
		"ICMT": text(comment),
	}
	inst = sf2.Instrument(name[:19], zones)
	preset_out = sf2.Preset(name[:19], 0, 0, [sf2.Zone([(sf2.GEN_INSTRUMENT, 0)], [])])
	return sf2.SoundFont(info, bytes(smpl), None, [preset_out], [inst], samples)


def main():
	ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
	ap.add_argument("--config")
	ap.add_argument("--out", default=str(HERE))
	ap.add_argument("--sources", default=None)
	ap.add_argument("--only", help="build just this source id")
	args = ap.parse_args()

	out = Path(args.out)
	out.mkdir(parents=True, exist_ok=True)
	sources_dir = Path(args.sources) if args.sources else ROOT / "target/soundfont-sources"
	manifest = tomllib.loads((HERE / "manifest.toml").read_text())
	envelope = envelope_generators(load_envelope(args.config))
	for source in manifest["source"]:
		if args.only and source["id"] != args.only:
			continue
		font = sf2.read(fetch(source, sources_dir))
		comment = f"Eval piano cut from {source['name']} ({source['license']}), {source['url']}"
		built = cut(font, envelope, f"Eval {source['id']}", comment)
		path = out / f"{source['id']}.sf2"
		sf2.write(built, path)
		digest = sha256(path)
		print(f"{path} {digest}")
		if digest != source["derived_sha256"]:
			print(
				f"  differs from the manifest's derived_sha256 ({source['derived_sha256']}): "
				"expected if the envelope or the build changed; update the manifest",
				file=sys.stderr,
			)


if __name__ == "__main__":
	main()
