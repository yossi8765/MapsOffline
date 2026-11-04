#!/usr/bin/env bash
# Tests that need the real data, run after tools/ci-data.sh built it (GitLab `data-package` job).
# They reuse the PBF the data build downloaded and the graph it wrote; the GTFS zip is deleted
# after the build, so the transit test runs on the built transit.bin when the test supports it.
set -uo pipefail

ROOT="${CI_PROJECT_DIR:-$(pwd)}"
D="$ROOT/ci-data"
PBF="$D/downloads/israel-and-palestine-latest.osm.pbf"
cd "$ROOT/src-tauri"

if [ ! -f "$PBF" ]; then
  echo "no PBF in $D/downloads; skipping the real-data tests"
  exit 0
fi

export MAPSOFFLINE_PBF="$PBF"
export MAPSOFFLINE_REUSE_GRAPH=1
export MAPSOFFLINE_TRANSIT_BIN="$D/transit/transit.bin"
export MAPSOFFLINE_DATA_DIR="$D"

# same optimisation settings as the data tool build in ci-data.sh, so the dependencies are reused
cargo test --release --features cli \
  --config 'profile.release.lto=false' --config 'profile.release.codegen-units=16' --config 'profile.release.opt-level=2' \
  --test routing_real --test geocode_real --test transit_real -- --nocapture
