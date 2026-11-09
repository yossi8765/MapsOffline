#!/usr/bin/env python3
"""The Android 4.4 app's data in one file (MapsOffline-android-4.4-data.zip).

The old-phone app (separate repository, Java / Mapsforge) installs it from a file: a phone without
internet gets the file from a computer. It holds the Mapsforge map of Israel and, out of this
week's .mapsdata packages, the routing graph, the address index and the public-transport model:

    map/israel.map  routing/graph.bin  geocode/index.sqlite  transit/transit.bin  android44.json

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
