#!/usr/bin/env python3
"""The Android 4.4 app's data in one file (MapsOffline-android-4.4-data.zip).

The old-phone app (separate repository, Java / Mapsforge) installs it from a file: a phone without
internet gets the file from a computer. It holds the Mapsforge map of Israel and, out of this
week's .mapsdata packages, the routing graph, the address index and the public-transport model:

    map/israel.map  routing/graph.bin  routing/graph.bin.geom  geocode/index.sqlite  transit/transit.bin  android44.json

routing/graph.bin.geom is what the app otherwise computes on the phone after installing (Graph.prepare
in the app: the varint geometry of graph.bin decoded into int32 pairs) – done here, a file unpacked
by hand works as it is and an old phone is spared the work.

    python3 tools/android44-data.py OUT.zip --map israel.map routing.mapsdata geocode.mapsdata transit.mapsdata

Run by tools/ci-data.sh every week (release "data-latest"); also by hand.
"""
import argparse
import datetime
import json
import os
import shutil
import sys
import zipfile

ENTRIES = {"routing/graph.bin", "geocode/index.sqlite", "transit/transit.bin"}


def graph_geom(data: bytes) -> bytes:
    """The app's Graph.prepare: graph.bin's delta + zigzag varint geometry as little-endian int32 pairs."""
    if data[:4] != b"MOGR":
        raise ValueError("not a graph.bin")
    ver = data[4] | data[5] << 8
    if ver < 6:
        raise ValueError(f"graph.bin version {ver} is too old")
    u64 = lambda i: int.from_bytes(data[i:i + 8], "little")
    pos = 8
    # arrays before the geometry: bbox f64, speed u8, proj f64, vlon, vlat i32, signal u8, from, to u32,
    # len f32, speed u8, flags u32, class u8, name u32, bear_s, bear_e u16
    for size in (8, 1, 8, 4, 4, 1, 4, 4, 4, 1, 4, 1, 4, 2, 2):
        pos += 8 + u64(pos) * size
    n_off = u64(pos)
    last_off = int.from_bytes(data[pos + 8 + (n_off - 1) * 4:pos + 8 + n_off * 4], "little") if n_off else 0
    pos += 8 + n_off * 4
    n_bytes = u64(pos)
    pos += 8
    end = pos + n_bytes
    n_values = last_off * 2
    out = bytearray(n_values * 4)
    px = py = 0
    i = pos
    for k in range(n_values):
        z = shift = 0
        while True:
            c = data[i]
            i += 1
            z |= (c & 0x7F) << shift
            if c < 0x80:
                break
            shift += 7
        d = (z >> 1) ^ -(z & 1)
        if k & 1:
            py += d
            v = py
        else:
            px += d
            v = px
        out[k * 4:k * 4 + 4] = (v & 0xFFFFFFFF).to_bytes(4, "little")
    if i != end:
        raise ValueError("graph.bin geometry length mismatch")
    return bytes(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out")
    ap.add_argument("--map", required=True, help="the Mapsforge map of Israel (israel-and-palestine.map)")
    ap.add_argument("mapsdata", nargs="+", help="routing / geocode / transit .mapsdata of the main app")
    a = ap.parse_args()

    tmp = a.out + ".tmp"
    have = []
    with zipfile.ZipFile(tmp, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6, allowZip64=True) as z:
        z.write(a.map, "map/israel.map")
        have.append("map/israel.map")
        for pkg in a.mapsdata:
            with zipfile.ZipFile(pkg) as src:
                for info in src.infolist():
                    if info.filename in ENTRIES and info.filename not in have:
                        zi = zipfile.ZipInfo(info.filename, info.date_time)
                        zi.compress_type = zipfile.ZIP_DEFLATED
                        with src.open(info) as fi, z.open(zi, "w", force_zip64=info.file_size > 0xFFFFFFF0) as fo:
                            shutil.copyfileobj(fi, fo, 1 << 20)
                        have.append(info.filename)
                        if info.filename == "routing/graph.bin":
                            geom = graph_geom(src.read(info))
                            gi = zipfile.ZipInfo("routing/graph.bin.geom", info.date_time)
                            gi.compress_type = zipfile.ZIP_DEFLATED
                            z.writestr(gi, geom)
                            have.append("routing/graph.bin.geom")
        missing = sorted(ENTRIES - set(have))
        z.writestr("android44.json", json.dumps({
            "built": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d"),
            "entries": have,
        }, ensure_ascii=False, indent=1))
    if missing:
        os.remove(tmp)
        print(f"missing in the .mapsdata packages: {', '.join(missing)}", file=sys.stderr)
        return 1
    with zipfile.ZipFile(tmp) as z:
        bad = z.testzip()
        if bad:
            print(f"corrupt entry {bad}", file=sys.stderr)
            return 1
    os.replace(tmp, a.out)
    print(f"{a.out}: {os.path.getsize(a.out) / 1e6:.0f} MB ({', '.join(have)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
