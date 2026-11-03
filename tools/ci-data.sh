#!/usr/bin/env bash
# Weekly ready-made data for MapsOffline, run by the scheduled GitLab pipeline (.gitlab-ci.yml).
#
# Builds every data component with the data CLI on Linux, exports one .mapsdata per component and
# uploads them with the data directory's manifest.json as generic package mapsdata/<date>. The app
# (src-tauri/src/data/prebuilt.rs) takes components from the newest package when they are recent,
# compatible and newer than what it has installed. Satellite = Sentinel-2 at the lowest detail
# (the govmap orthophoto is personal-use only and must not be published). Old packages are pruned.
set -uo pipefail

ROOT="${CI_PROJECT_DIR:-$(pwd)}"
D="$ROOT/ci-data"
OUT="$ROOT/ci-out"
# dated packages kept (per-component files); override with a CI variable
KEEP="${KEEP:-2}"
# one file with every component at a fixed address, for sharing a link that never changes
ALL_PKG="mapsdata-all"
ALL_FILE="MapsOffline-data.mapsdata"
mkdir -p "$D/downloads" "$OUT"

# The Tauri crate embeds the frontend at compile time; the CLI never uses it, a stub is enough.
mkdir -p "$ROOT/dist"
[ -f "$ROOT/dist/index.html" ] || echo '<!doctype html><title>MapsOffline</title>' > "$ROOT/dist/index.html"

echo "== building the data tool"
cd "$ROOT/src-tauri"
# No fat LTO here: the data tool only needs to be fast enough, the shared runner's time is limited.
cargo build --release --features cli --bin mapsoffline-data \
  --config 'profile.release.lto=false' --config 'profile.release.codegen-units=16' --config 'profile.release.opt-level=2' \
  || exit 1
BIN="$ROOT/src-tauri/target/release/mapsoffline-data"
cd "$ROOT"

# pmtiles CLI (Linux build) where the data tool looks for it
if [ ! -x "$D/downloads/pmtiles.exe" ]; then
  url=$(curl -fsSL https://api.github.com/repos/protomaps/go-pmtiles/releases/latest \
    | jq -r '.assets[] | select(.name | test("Linux_x86_64\\.tar\\.gz$")) | .browser_download_url' | head -1)
  curl -fsSL "$url" | tar -xz -C "$D/downloads" pmtiles && mv "$D/downloads/pmtiles" "$D/downloads/pmtiles.exe"
fi

echo '{"satelliteSource":"sentinel","satelliteQuality":"low","usePrebuiltData":false}' > "$D/settings.json"

echo "== building the components"
"$BIN" --dir "$D" build --tiles --world --terrain --routing --geocode --transit || echo "WARNING: some components failed"
"$BIN" --dir "$D" build --satellite || echo "WARNING: satellite failed"
"$BIN" --dir "$D" status || true

echo "== packing"
built=0
for c in tiles world satellite terrain routing geocode transit; do
  if "$BIN" --dir "$D" export "$OUT/$c.mapsdata" --only "$c"; then
    built=$((built + 1))
  else
    echo "skip $c (not built)"
    rm -f "$OUT/$c.mapsdata"
  fi
done
[ "$built" -gt 0 ] || { echo "nothing was built"; exit 1; }
cp "$D/manifest.json" "$OUT/manifest.json"
mkdir -p "$OUT/all"
"$BIN" --dir "$D" export "$OUT/all/$ALL_FILE" || echo "WARNING: combined package failed"
ls -la "$OUT" "$OUT/all"

if [ -z "${CI_JOB_TOKEN:-}" ]; then
  echo "not in CI: packages left in $OUT"
  exit 0
fi

VERSION=$(date -u +%Y-%m-%d-%H%M)
API="$CI_API_V4_URL/projects/$CI_PROJECT_ID/packages"
echo "== uploading mapsdata/$VERSION"
# manifest.json last: the app reads it to decide, so it must only appear once the files are there
for f in "$OUT"/*.mapsdata "$OUT/manifest.json"; do
  [ -f "$f" ] || continue
  curl -fsS --retry 3 -H "JOB-TOKEN: $CI_JOB_TOKEN" --upload-file "$f" "$API/generic/mapsdata/$VERSION/$(basename "$f")" \
    || { echo "upload of $f failed"; exit 1; }
  echo
done

if [ -f "$OUT/all/$ALL_FILE" ]; then
  echo "== uploading $ALL_PKG/latest/$ALL_FILE (fixed link)"
  curl -fsS --retry 3 -H "JOB-TOKEN: $CI_JOB_TOKEN" --upload-file "$OUT/all/$ALL_FILE" "$API/generic/$ALL_PKG/latest/$ALL_FILE"     || { echo "upload of the combined package failed"; exit 1; }
  echo
  # the link serves the newest upload; remove the previous ones so they do not pile up
  pid=$(curl -fsS -H "JOB-TOKEN: $CI_JOB_TOKEN" "$API?package_name=$ALL_PKG&package_type=generic" | jq -r '.[] | select(.version=="latest") | .id' | head -1)
  if [ -n "$pid" ]; then
    curl -fsS -H "JOB-TOKEN: $CI_JOB_TOKEN" "$API/$pid/package_files?per_page=100"       | jq -r 'sort_by(.created_at) | .[:-1][] | .id' | while read -r fid; do
          curl -fsS -X DELETE -H "JOB-TOKEN: $CI_JOB_TOKEN" "$API/$pid/package_files/$fid" && echo "deleted old combined file $fid" || echo "could not delete old combined file $fid"
        done
  fi
fi

echo "== pruning old packages (keeping $KEEP)"
curl -fsS -H "JOB-TOKEN: $CI_JOB_TOKEN" "$API?package_name=mapsdata&order_by=created_at&sort=desc&per_page=50" \
  | jq -r "[.[] | select(.name==\"mapsdata\")] | .[$KEEP:][] | .id" | while read -r id; do
      curl -fsS -X DELETE -H "JOB-TOKEN: $CI_JOB_TOKEN" "$API/$id" && echo "deleted package $id" || echo "could not delete package $id"
    done
echo "done"
