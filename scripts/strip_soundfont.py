#!/usr/bin/env python3
"""Strips a SoundFont down to the settings the MIDI generator reads.

The generator draws one partial per voice and never plays a sample, so it needs
every preset, instrument and zone setting but no audio. This keeps the chosen
presets with the instruments and samples they reach, and replaces each sample's
audio with the same tiny stub of silence. Each sample keeps its root key and
pitch correction, which are part of where the fundamental lands.

    scripts/strip_soundfont.py GeneralUser-GS.sf2 assets/soundfont/piano.sf2

By default it keeps the General MIDI piano family, bank 0 programs 0-7. Pass
`--presets BANK:PATCH,...` for others. The committed file was made from
GeneralUser GS v2.0.3; see assets/soundfont/README.md.
"""

import argparse
import struct
import sys

STUB_SAMPLES = 64  # frames of silence every sample header points into
STUB_LOOP = (8, 56)  # loop start/end inside the stub
GEN_INSTRUMENT = 41
GEN_SAMPLE_ID = 53
# The generators that move a sample's start, end and loop points. They address
# audio the stub no longer has, and a parser rejects a zone that offsets a point
# outside it, so they are dropped. Nothing else is.
SAMPLE_ADDRESS_GENS = {0, 1, 2, 3, 4, 12, 45, 50}

# (chunk id, record size)
PDTA = [
    ("phdr", 38), ("pbag", 4), ("pmod", 10), ("pgen", 4),
    ("inst", 22), ("ibag", 4), ("imod", 10), ("igen", 4), ("shdr", 46),
]


def chunks(data):
    """Yields (id, payload) for the sub-chunks of a RIFF/LIST body."""
    pos = 0
    while pos + 8 <= len(data):
        cid, size = struct.unpack_from("<4sI", data, pos)
        yield cid.decode("ascii"), data[pos + 8 : pos + 8 + size]
        pos += 8 + size + (size & 1)


def chunk(cid, payload):
    pad = b"\0" if len(payload) & 1 else b""
    return struct.pack("<4sI", cid.encode("ascii"), len(payload)) + payload + pad


def records(payload, size):
    return [payload[i : i + size] for i in range(0, len(payload) - size + 1, size)]


def read_pdta(riff):
    body = riff[4:]
    assert riff[:4] == b"sfbk", "not a SoundFont"
    for cid, payload in chunks(body):
        if cid == "LIST" and payload[:4] == b"pdta":
            tables = {name: records(p, dict(PDTA)[name]) for name, p in chunks(payload[4:])}
            return tables
    sys.exit("no pdta list")


def slice_by_index(owners, index_field, i):
    """The children of owner `i`: from its index to the next owner's."""
    lo = struct.unpack_from("<H", owners[i], index_field)[0]
    hi = struct.unpack_from("<H", owners[i + 1], index_field)[0]
    return range(lo, hi)


def strip(source, wanted):
    with open(source, "rb") as f:
        riff = f.read()[8:]
    t = read_pdta(riff)

    def bag_range(owners, offset, i):
        return slice_by_index(owners, offset, i)

    def gen_range(bags, b):
        return range(
            struct.unpack_from("<H", bags[b], 0)[0], struct.unpack_from("<H", bags[b + 1], 0)[0]
        )

    def mod_range(bags, b):
        return range(
            struct.unpack_from("<H", bags[b], 2)[0], struct.unpack_from("<H", bags[b + 1], 2)[0]
        )

    # Presets, in the order asked for.
    phdr = t["phdr"]
    found = {}
    for i, rec in enumerate(phdr[:-1]):
        patch, bank = struct.unpack_from("<HH", rec, 20)
        found[(bank, patch)] = i
    missing = [w for w in wanted if w not in found]
    if missing:
        sys.exit(f"presets not in the font: {missing}")

    out = {name: [] for name, _ in PDTA}
    instrument_ids = {}  # source instrument id -> new id
    sample_ids = {}

    def remap(table, source_id):
        return table.setdefault(source_id, len(table))

    # First pass over the presets decides the instrument numbering.
    for key in wanted:
        i = found[key]
        for b in bag_range(phdr, 24, i):
            for g in gen_range(t["pbag"], b):
                oper, amount = struct.unpack_from("<HH", t["pgen"][g])
                if oper == GEN_INSTRUMENT:
                    remap(instrument_ids, amount)

    def copy_zones(owner_rec, owners, bag_offset, bags, gens, mods, out_bag, out_gen, out_mod, fix):
        """Appends one owner's zones to the output tables."""
        lo = struct.unpack_from("<H", owner_rec[0], bag_offset)[0]
        hi = struct.unpack_from("<H", owner_rec[1], bag_offset)[0]
        for b in range(lo, hi):
            out_bag.append(struct.pack("<HH", len(out_gen), len(out_mod)))
            for g in gen_range(bags, b):
                rec = gens[g]
                oper, amount = struct.unpack("<HH", rec)
                kept = fix(oper, amount, rec)
                if kept is not None:
                    out_gen.append(kept)
            for m in mod_range(bags, b):
                out_mod.append(mods[m])

    def fix_preset_gen(oper, amount, rec):
        if oper == GEN_INSTRUMENT:
            return struct.pack("<HH", oper, instrument_ids[amount])
        return rec

    for key in wanted:
        i = found[key]
        header = bytearray(phdr[i])
        struct.pack_into("<H", header, 24, len(out["pbag"]))
        out["phdr"].append(bytes(header))
        copy_zones(
            (phdr[i], phdr[i + 1]), phdr, 24, t["pbag"], t["pgen"], t["pmod"],
            out["pbag"], out["pgen"], out["pmod"], fix_preset_gen,
        )
    terminal = bytearray(phdr[-1])
    struct.pack_into("<H", terminal, 24, len(out["pbag"]))
    out["phdr"].append(bytes(terminal))
    out["pbag"].append(struct.pack("<HH", len(out["pgen"]), len(out["pmod"])))
    out["pgen"].append(b"\0" * 4)
    out["pmod"].append(b"\0" * 10)

    # Instruments, in their new order.
    inst = t["inst"]
    for source_id in sorted(instrument_ids, key=instrument_ids.get):
        for b in bag_range(inst, 20, source_id):
            for g in gen_range(t["ibag"], b):
                oper, amount = struct.unpack_from("<HH", t["igen"][g])
                if oper == GEN_SAMPLE_ID:
                    remap(sample_ids, amount)

    def fix_instrument_gen(oper, amount, rec):
        if oper in SAMPLE_ADDRESS_GENS:
            return None
        if oper == GEN_SAMPLE_ID:
            return struct.pack("<HH", oper, sample_ids[amount])
        return rec

    for source_id in sorted(instrument_ids, key=instrument_ids.get):
        header = bytearray(inst[source_id])
        struct.pack_into("<H", header, 20, len(out["ibag"]))
        out["inst"].append(bytes(header))
        copy_zones(
            (inst[source_id], inst[source_id + 1]), inst, 20, t["ibag"], t["igen"], t["imod"],
            out["ibag"], out["igen"], out["imod"], fix_instrument_gen,
        )
    terminal = bytearray(inst[-1])
    struct.pack_into("<H", terminal, 20, len(out["ibag"]))
    out["inst"].append(bytes(terminal))
    out["ibag"].append(struct.pack("<HH", len(out["igen"]), len(out["imod"])))
    out["igen"].append(b"\0" * 4)
    out["imod"].append(b"\0" * 10)

    # Samples: the same stub of silence under every header.
    shdr = t["shdr"]
    for source_id in sorted(sample_ids, key=sample_ids.get):
        rec = bytearray(shdr[source_id])
        # name[20] start end startloop endloop rate pitch correction link type
        struct.pack_into("<IIII", rec, 20, 0, STUB_SAMPLES, *STUB_LOOP)
        struct.pack_into("<H", rec, 44 - 2, 0)  # no stereo link
        # A stereo or linked sample keeps its type bits but not its partner.
        out["shdr"].append(bytes(rec))
    out["shdr"].append(b"EOS".ljust(20, b"\0") + b"\0" * 26)

    pdta = b"pdta" + b"".join(chunk(name, b"".join(out[name])) for name, _ in PDTA)
    # 46 zero frames follow each sample by the spec; the stub is one sample.
    smpl = b"\0" * (2 * (STUB_SAMPLES + 46))
    sdta = b"sdta" + chunk("smpl", smpl)
    info = b"INFO" + b"".join([
        chunk("ifil", struct.pack("<HH", 2, 1)),
        chunk("isng", b"EMU8000\0"),
        chunk("INAM", b"GeneralUser GS piano, stripped\0\0"),
        chunk("ICMT", b"Settings only, samples replaced by silence. See README.md.\0\0"),
    ])
    body = b"sfbk" + chunk("LIST", info) + chunk("LIST", sdta) + chunk("LIST", pdta)
    return chunk("RIFF", body)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("source")
    parser.add_argument("dest")
    parser.add_argument(
        "--presets", default=",".join(f"0:{p}" for p in range(8)), help="BANK:PATCH,..."
    )
    args = parser.parse_args()
    wanted = [tuple(int(n) for n in p.split(":")) for p in args.presets.split(",")]
    data = strip(args.source, wanted)
    with open(args.dest, "wb") as f:
        f.write(data)
    print(f"wrote {args.dest}: {len(data)} bytes")


if __name__ == "__main__":
    main()
