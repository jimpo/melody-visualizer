"""A small SoundFont 2 reader and writer: just enough to cut one preset out of a
bank and rewrite its generators. Standard library only.

Generators are kept as (operator, amount) pairs, where the amount is the raw
16 bits as an unsigned int (a range generator packs low and high bytes).
"""

import struct
from dataclasses import dataclass, field

# Generator operators used here (SoundFont 2.04 spec, section 8.1.3).
GEN_START_ADDRS_OFFSET = 0
GEN_END_ADDRS_OFFSET = 1
GEN_STARTLOOP_ADDRS_OFFSET = 2
GEN_ENDLOOP_ADDRS_OFFSET = 3
GEN_START_ADDRS_COARSE_OFFSET = 4
GEN_END_ADDRS_COARSE_OFFSET = 12
GEN_STARTLOOP_ADDRS_COARSE_OFFSET = 45
GEN_ENDLOOP_ADDRS_COARSE_OFFSET = 50
GEN_INITIAL_ATTENUATION = 48
GEN_DELAY_VOL_ENV = 33
GEN_ATTACK_VOL_ENV = 34
GEN_HOLD_VOL_ENV = 35
GEN_DECAY_VOL_ENV = 36
GEN_SUSTAIN_VOL_ENV = 37
GEN_RELEASE_VOL_ENV = 38
GEN_KEYTOVOLENVHOLD = 39
GEN_KEYTOVOLENVDECAY = 40
GEN_INSTRUMENT = 41
GEN_KEY_RANGE = 43
GEN_VEL_RANGE = 44
GEN_SAMPLE_ID = 53
GEN_SAMPLE_MODES = 54
GEN_OVERRIDING_ROOT_KEY = 58
GEN_SCALE_TUNE = 56

GEN_NAMES = {
	0: "startAddrsOffset", 1: "endAddrsOffset", 2: "startloopAddrsOffset",
	3: "endloopAddrsOffset", 4: "startAddrsCoarse", 5: "modLfoToPitch",
	6: "vibLfoToPitch", 7: "modEnvToPitch", 8: "initialFilterFc",
	9: "initialFilterQ", 10: "modLfoToFilterFc", 11: "modEnvToFilterFc",
	12: "endAddrsCoarse", 13: "modLfoToVolume", 15: "chorusEffectsSend",
	16: "reverbEffectsSend", 17: "pan", 21: "delayModLFO", 22: "freqModLFO",
	23: "delayVibLFO", 24: "freqVibLFO", 25: "delayModEnv", 26: "attackModEnv",
	27: "holdModEnv", 28: "decayModEnv", 29: "sustainModEnv", 30: "releaseModEnv",
	31: "keynumToModEnvHold", 32: "keynumToModEnvDecay", 33: "delayVolEnv",
	34: "attackVolEnv", 35: "holdVolEnv", 36: "decayVolEnv", 37: "sustainVolEnv",
	38: "releaseVolEnv", 39: "keynumToVolEnvHold", 40: "keynumToVolEnvDecay",
	41: "instrument", 43: "keyRange", 44: "velRange", 45: "startloopCoarse",
	46: "keynum", 47: "velocity", 48: "initialAttenuation", 50: "endloopCoarse",
	51: "coarseTune", 52: "fineTune", 53: "sampleID", 54: "sampleModes",
	56: "scaleTuning", 57: "exclusiveClass", 58: "overridingRootKey",
}


def signed(amount):
	return amount - 0x10000 if amount >= 0x8000 else amount


@dataclass
class Zone:
	gens: list = field(default_factory=list)  # [(oper, amount u16)]
	mods: list = field(default_factory=list)  # [raw 10-byte records]

	def get(self, oper):
		for o, a in self.gens:
			if o == oper:
				return a
		return None

	def set(self, oper, amount):
		amount &= 0xFFFF
		for i, (o, _) in enumerate(self.gens):
			if o == oper:
				self.gens[i] = (oper, amount)
				return
		# Instrument and sampleID must stay last; ranges first.
		last = self.gens[-1:] if self.gens and self.gens[-1][0] in (GEN_INSTRUMENT, GEN_SAMPLE_ID) else []
		head = self.gens[: len(self.gens) - len(last)]
		self.gens = head + [(oper, amount)] + last

	def remove(self, oper):
		self.gens = [(o, a) for o, a in self.gens if o != oper]


@dataclass
class Preset:
	name: str
	program: int
	bank: int
	zones: list


@dataclass
class Instrument:
	name: str
	zones: list


@dataclass
class Sample:
	name: str
	start: int
	end: int
	loop_start: int
	loop_end: int
	rate: int
	pitch: int
	correction: int
	link: int
	kind: int


@dataclass
class SoundFont:
	info: dict  # fourcc -> bytes
	smpl: bytes
	sm24: bytes | None
	presets: list
	instruments: list
	samples: list


def _chunks(data, pos, end):
	while pos < end:
		tag = data[pos : pos + 4]
		(size,) = struct.unpack_from("<I", data, pos + 4)
		yield tag, pos + 8, size
		pos += 8 + size + (size & 1)


def _name(raw):
	return raw.split(b"\0", 1)[0].decode("latin-1")


def _zones(bag, gen, mod, first, last):
	zones = []
	for b in range(first, last):
		g0, m0 = bag[b]
		g1, m1 = bag[b + 1]
		zones.append(Zone([tuple(x) for x in gen[g0:g1]], list(mod[m0:m1])))
	return zones


def read(path):
	with open(path, "rb") as f:
		data = f.read()
	if data[:4] != b"RIFF" or data[8:12] != b"sfbk":
		raise ValueError("not a SoundFont 2 file")
	info, smpl, sm24, pdta = {}, b"", None, {}
	for tag, pos, size in _chunks(data, 12, len(data)):
		if tag != b"LIST":
			continue
		kind = data[pos : pos + 4]
		for sub, spos, ssize in _chunks(data, pos + 4, pos + size):
			body = data[spos : spos + ssize]
			if kind == b"INFO":
				info[sub.decode()] = body
			elif kind == b"sdta":
				if sub == b"smpl":
					smpl = body
				elif sub == b"sm24":
					sm24 = body
			elif kind == b"pdta":
				pdta[sub.decode()] = body

	def records(key, fmt):
		size = struct.calcsize(fmt)
		body = pdta[key]
		return [struct.unpack_from(fmt, body, i) for i in range(0, len(body) - size + 1, size)]

	phdr = records("phdr", "<20sHHHIII")
	pbag = records("pbag", "<HH")
	pgen = records("pgen", "<HH")
	pmod = [pdta["pmod"][i : i + 10] for i in range(0, len(pdta["pmod"]), 10)]
	inst = records("inst", "<20sH")
	ibag = records("ibag", "<HH")
	igen = records("igen", "<HH")
	imod = [pdta["imod"][i : i + 10] for i in range(0, len(pdta["imod"]), 10)]
	shdr = records("shdr", "<20sIIIIIBbHH")

	presets = []
	for i in range(len(phdr) - 1):
		name, program, bank, bag0, *_ = phdr[i]
		bag1 = phdr[i + 1][3]
		presets.append(Preset(_name(name), program, bank, _zones(pbag, pgen, pmod, bag0, bag1)))
	instruments = []
	for i in range(len(inst) - 1):
		name, bag0 = inst[i]
		instruments.append(Instrument(_name(name), _zones(ibag, igen, imod, bag0, inst[i + 1][1])))
	samples = [Sample(_name(s[0]), *s[1:]) for s in shdr[:-1]]
	return SoundFont(info, smpl, sm24, presets, instruments, samples)


def _chunk(tag, body):
	pad = b"\0" if len(body) & 1 else b""
	return tag + struct.pack("<I", len(body)) + body + pad


def _list(kind, *chunks):
	body = kind + b"".join(chunks)
	return _chunk(b"LIST", body)


def _padname(name):
	return name.encode("latin-1")[:19].ljust(20, b"\0")


def write(sf, path):
	def build(zones_per, terminal_name, packers):
		"""Packs headers, bags, gens and mods for presets or instruments."""
		raise NotImplementedError

	pbag, pgen, pmod, phdr = [], [], [], []
	for p in sf.presets:
		phdr.append(struct.pack("<20sHHHIII", _padname(p.name), p.program, p.bank, len(pbag), 0, 0, 0))
		for z in p.zones:
			pbag.append(struct.pack("<HH", len(pgen), len(pmod) // 10))
			for o, a in z.gens:
				pgen.append(struct.pack("<HH", o, a))
			pmod.extend(z.mods)
	phdr.append(struct.pack("<20sHHHIII", _padname("EOP"), 0, 0, len(pbag), 0, 0, 0))
	pbag.append(struct.pack("<HH", len(pgen), len(pmod)))
	pgen.append(struct.pack("<HH", 0, 0))
	pmod.append(b"\0" * 10)

	ibag, igen, imod, inst = [], [], [], []
	for i in sf.instruments:
		inst.append(struct.pack("<20sH", _padname(i.name), len(ibag)))
		for z in i.zones:
			ibag.append(struct.pack("<HH", len(igen), len(imod)))
			for o, a in z.gens:
				igen.append(struct.pack("<HH", o, a))
			imod.extend(z.mods)
	inst.append(struct.pack("<20sH", _padname("EOI"), len(ibag)))
	ibag.append(struct.pack("<HH", len(igen), len(imod)))
	igen.append(struct.pack("<HH", 0, 0))
	imod.append(b"\0" * 10)

	shdr = [
		struct.pack("<20sIIIIIBbHH", _padname(s.name), s.start, s.end, s.loop_start, s.loop_end, s.rate, s.pitch, s.correction, s.link, s.kind)
		for s in sf.samples
	]
	shdr.append(struct.pack("<20sIIIIIBbHH", _padname("EOS"), 0, 0, 0, 0, 0, 0, 0, 0, 0))

	# pmod/imod records are 10 bytes; the terminal one is all zero.
	info = b"".join(_chunk(k.encode(), v) for k, v in sf.info.items())
	sdta = _chunk(b"smpl", sf.smpl) + (_chunk(b"sm24", sf.sm24) if sf.sm24 else b"")
	pdta = b"".join(
		_chunk(tag, b"".join(recs))
		for tag, recs in [
			(b"phdr", phdr), (b"pbag", pbag), (b"pmod", pmod), (b"pgen", pgen),
			(b"inst", inst), (b"ibag", ibag), (b"imod", imod), (b"igen", igen), (b"shdr", shdr),
		]
	)
	body = b"sfbk" + _list(b"INFO", info) + _list(b"sdta", sdta) + _list(b"pdta", pdta)
	with open(path, "wb") as f:
		f.write(b"RIFF" + struct.pack("<I", len(body)) + body)
