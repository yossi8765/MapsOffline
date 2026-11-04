# MapsOffline – performance report

Measured on the installed Israel data (graph 803 164 vertices / 1 087 713 edges, geocoder
3 909 places / 40 k streets / 167 k addresses / 59 k POIs) on the dev machine (Windows 11),
warm OS file cache, while other agents were compiling in parallel (expect ±10 % noise).

Reproduce:

```
cd src-tauri
cargo build --release --bin mapsoffline-data
target/release/mapsoffline-data bench --iters 9      # release (what users get)
cargo run --bin mapsoffline-data -- bench --iters 1  # dev profile (what `pnpm tauri dev` runs)
```

`bench` is a new subcommand of the data CLI: geocoder open, 10 typical searches, reverse geocode,
graph load, 8 route scenarios (median and max of N runs after one warm-up run), walk legs, and process
working set (RSS). `RUST_LOG=mapsoffline_lib::routing=debug` prints per-search pop counts and phase timings.

## Before / after

Release profile (LTO), median times:

| Metric | Before | After |
|---|---:|---:|
| Graph load (`routing::graph::load`) | 242 ms | **31 ms** |
| Geocoder `open` | 5.3 ms (places loaded eagerly) | **0.8 ms** (lazy places cache) |
| First search after open (includes places cache) | 0.2 ms + 5 ms in open | 4.7 ms |
| Search, median over 10 queries / worst (`"ב"`) | 0.16 / 2.9 ms | 0.15 / 3.3 ms |
| Reverse geocode | 0.23 ms | 0.24 ms |
| Car TLV→JLM, 3 routes | 147 ms | **37 ms** |
| Car TLV→JLM, 1 route | 39 ms | **9.6 ms** |
| Car Eilat→Kiryat Shmona, 3 routes | 181 ms | **61 ms** |
| Car TLV→Haifa, 3 routes | 158 ms | **39 ms** |
| Car, urban TLV, 3 routes | 0.60 ms | 0.49 ms |
| First route after start (buffer allocation) | ~10 ms extra | ~0.5 ms extra |
| Foot, urban TLV, 3 routes | 1.0 ms | 0.9 ms |
| Bike, 5 km, 3 routes | 5.7 ms | 5.2 ms |
| Walk leg 350 m (transit access) | 0.03 ms | 0.03 ms |
| RSS after geocoder + graph | 165 MB | **110 MB** |
| RSS after all route scenarios | 217 MB | **157 MB** |

Dev profile (`pnpm tauri dev`; our crate is unoptimised, dependencies are at opt-level 2):

| Metric | Before | After |
|---|---:|---:|
| Graph load | 6 300 ms | **57 ms** |
| Car TLV→JLM, 3 routes | 1 766 ms | **152 ms** |
| Geocoder open (+ places) | ~1.1 s | 0.9 ms open, places built in background preload |

All routes are identical before and after (same best route cost and the same alternatives for every
scenario above). `tests/routing_real.rs` and `tests/geocode_real.rs` pass on the real extract.

Startup: no command waits on data. `data_preload` now loads the graph (31 ms) and warms the
geocoder (places cache + FTS index pages) on two parallel blocking threads, and transit, when
requested, on a third. Graph and transit have separate load locks, so a long transit load never
delays the first route.

## What changed

### Graph file v2 (`routing/graph.rs`, VERSION 1 → 2)
- **Raw little-endian arrays instead of bincode.** Each array is `u64 count + payload` and is read with
  one `read_exact` directly into a zero-initialised (`calloc`) `Vec` – no per-element decoding, no
  intermediate buffer, ~memcpy speed even in the unoptimised dev build. A trailing `END!` marker plus
  structural validation (array lengths, CSR offsets, index ranges) reject truncated or damaged files
  instead of panicking in a search. Array sizes are bounded by the file size before allocating.
- **No index construction at load.** The rstar R-tree (bulk load of 1 M boxes) is replaced by a
  persisted uniform 400 m grid (CSR `off` + edge ids per cell, built per segment bounding box).
  Projected vertex coordinates (`vpx/vpy`), restriction offsets and the maximum car speed are also
  computed by the builder and stored.
- **Spatial renumbering.** The builder orders vertices along a Hilbert curve and edges by their lower
  endpoint, so searches touch nearby memory (fewer cache misses).
- Street names are one UTF-8 blob + offsets instead of 21 k `String`s.
- Old files are rejected with `graph file version 1 is not supported (expected 2); update the
  routing data`; `AppState::graph()` reports it as `NO_ROUTING_DATA: …` so the UI offers the update.
- File size 78 MB → 95 MB (projected coords + grid are now stored); build time ~5 s.
- v4 (traffic-signal vertices, 2026-11-02): +100 KB bitset, build 6.1 s (node tags are read in the existing coordinate pass), load/route times unchanged. Note: `--unpaved avoid` on desert routes that have no paved alternative explores ~20× more states (16 → 450 ms) because the 5× penalty loosens the A* bound; irrelevant on normal roads.
- v3 (restricted territories, 2026-11-02): edge flags widened to u32 and 223 Area A/B rings stored → 97.4 MB; build ~7 s (one extra relation-only PBF pass ≈0.5 s, edge classification against the rings ≈2 s). Load time unchanged (the ring band index is built in ~1 ms); route times unchanged (the flag test is one AND in `Profile::allowed`).

### Search (`routing/astar.rs`, `profile.rs`, `alternatives.rs`)
- **Road-hierarchy pruning in tiers** for routes longer than 15 km straight-line: all roads within
  `clamp(5 % of distance, 2.5–10 km)` of either endpoint, tertiary roads (car) within
  `clamp(25 %, 10–40 km)`, secondary-and-up everywhere. Pops for TLV→JLM: 130 k → 58 k per search.
  If pruning disconnects the endpoints, the search is retried without pruning (this fallback did not
  exist before).
- **Tighter A\* heuristic**: car bound uses the fastest speed present in the graph (120 km/h)
  instead of a fixed 130 km/h.
- **Packed labels**: stamp, distance and parent of a state are one 12-byte struct (one cache line
  per relaxation instead of three); label buffers come from `alloc_zeroed`, so the first route no
  longer pays a 50 MB memset.
- **Dense penalty array** for alternatives (was an `FxHashMap` lookup on every relaxation), pooled
  and reset only on the touched edges.
- **Allocation-free snapping**: `project_on_edge` walks the edge geometry in place (no `Vec` per
  candidate edge, no haversine per segment); the candidate sort is deterministic.

### Geocoder (`geocode/mod.rs`, `state.rs`)
- `open` only opens SQLite (+ `temp_store=MEMORY`, statement cache 32, 16 MB page cache per
  connection on top of the shared 256 MB mmap). The ~4 k-row places table and the city-population
  map load lazily on first use into a cache shared by all connections; `Geocoder::warm()` does it in
  the startup preload.
- **Connection pool** instead of one `Mutex<Option<Geocoder>>`: concurrent searches (type-ahead plus
  reverse geocode plus a route panel lookup) each check out a connection; no lock is held while a
  query runs. `unload_all` bumps a generation so connections to a replaced index are dropped.

### Commands (`commands.rs`)
- History, favourites, `data_status` and `settings_set` were synchronous commands, which Tauri runs
  on the **main (UI) thread**; they are now `#[tauri::command(async)]`. `data_status` no longer
  locks the geocoder (it could block the UI thread behind a running search).
- All data-touching commands run in `spawn_blocking`; no lock is held across heavy work (the graph
  and transit are `Arc`s cloned out of their `RwLock`).

## Remaining ideas (not done)
- Edge data in one array of structs (from, to, len, flags, speed, class, bearings = 20 B) for the
  search hot loop: probably another 1.5–2× per search. Not needed for the targets.
- Run the up-to-3 penalised alternative searches with a shared upper bound, or switch to via-node
  (plateau) alternatives computed from one search space: ~2× on alternatives.
- Preallocate (touch) the route label buffers in `data_preload` so even the first route is warm.
- A `cargo bench`/criterion harness is not needed: `mapsoffline-data bench` covers the scenarios
  without a new dependency.
- Cold-start graph load is bounded by disk read of 95 MB (~0.1–0.3 s on SSD); memory-mapping the file
  would make even that lazy but needs `memmap2` or unsafe Win32 code.

### Transit (`src/transit/**`)
- Done (transit.bin v3, 2026-11-04): raw little-endian arrays (`transit/binfmt.rs`), the indexes
  `finalize()` used to derive at every load (stop modes, stop grid as CSR, search keys) are built
  once and stored, `END!` trailer, `Transit::validate()` before use. See the numbers below.
- `search_stops` scans all stops with `contains` on every keystroke (fine at ~1 ms); a prefix index
  would only matter if the stop count grows.
- Done: `transit_stops` (map layer) walks the stop grid of the viewport (`Transit::stops_in_bbox`).

### Transit v3 measurements (2026-11-04, release CLI `transit-bench`, 5 runs, median)

Model: the installed GTFS model converted to v3 with `mapsoffline-data transit-convert` (35,164
stops, 8,404 patterns, 314,351 trips); road-network walker = graph.bin g6. Date: the next Tuesday.

| Scenario | Time | Answer |
|---|---|---|
| `transit::load` (v3, 74.8 MB, incl. validate) | 37.8 ms | – |
| TA Azrieli → Jerusalem CBS, depart 08:00 (range RAPTOR + repair + diversification) | 967 ms | 3 routes, best 08:02 → 08:54 |
| TA Azrieli → Jerusalem CBS, arrive by 10:00 | 279 ms | 3 routes, best 09:02 → 09:54 |
| Haifa Merkaz → Beersheba, depart 08:00 | 382 ms | 3 routes, best 08:00 → 10:58 |
| Bnei Brak → Ramat Gan, depart 17:00 | 385 ms | 3 routes, 2 transfers |
| Transit isochrone TA, 15/30/45 min (one-to-all RAPTOR + raster union) | 19 ms | 15 / 55 / 78 rings |
| Stops layer, central Tel Aviv viewport | 0.007 ms | 1,260 stops |

- Loading: v2 needed bincode per-element decoding plus `finalize()` (≈0.3 s in release, seconds in
  dev); v3 is a sequence of bulk reads (38 ms including validation). The file is 1.5 % larger
  (stored indexes and search keys).
- The depart-at plan from Tel Aviv is the slow case (~1 s): the range query iterates every boarding
  in the hour at ~30 access stops, then up to 4 plain runs, 3 retries after the road-walk check and
  one diversification run. With transfers walked on the road network at build time
  (`build_with_graph`, once `routing::walk_times_from` exists) the retries should mostly disappear;
  re-measure then. Next step if still slow: coarsen range shifts to 60 s buckets.

## Frontend recommendations (for the next frontend pass)
1. **Search debounce** (`SearchBox.tsx`, 200 ms): the backend answers in ≤ 4 ms (median 0.15 ms);
   60–80 ms is enough to coalesce keystrokes and feels instant. Keep the `seq` guard.
2. **Startup order** (`App.tsx`): `dataPreload` is only called after `loadSettings` and four other
   awaits. Fire it first (it does not depend on them) so the graph and geocoder warm up in parallel
   with the UI bootstrap.
3. **Map style rebuilds** (`MapView.tsx`): every change of `labelLang`, `poi` or `buildings3d` calls
   `map.setStyle(style, { diff: false })`, which tears down all sources and re-parses visible tiles.
   Toggle POI / 3D buildings with `setLayoutProperty(id, "visibility", …)` and switch label language
   with `setLayoutProperty(id, "text-field", …)`; reserve `setStyle` for theme flavour changes and use
   `diff: true` there.
4. **Route payload**: each route carries its full geometry and each leg repeats it (`legs[].geometry`),
   so a national route with alternatives serialises every coordinate twice. If the UI draws from
   `route.geometry`, a later contract change could drop the leg copy for single-leg routes; rounding
   coordinates to 6 decimals in serialisation would also cut the JSON by ~40 %.
5. **Bundle**: map libraries are already split into their own chunks; additionally lazy-load the
   rarely used panels (`SettingsPanel`, `DataPanel`, `HistoryPanel`) with `React.lazy` so the first
   paint parses less JS.
6. Use narrow zustand selectors in `MapView` so store updates that do not affect the map (search
   text, panel state) do not re-render it.

## Requested Cargo changes
None are required for the results above (no new dependencies). Optional, for the build-config owner:
1. A fast, release-like profile for perf work and real-data tests (the release profile's
   `panic = "abort"` breaks `cargo test --release`; full LTO costs 3–9 min per iteration):
   ```toml
   [profile.perf]
   inherits = "release"
   lto = false
   codegen-units = 16
   incremental = true
   panic = "unwind"
   ```
   Usage: `cargo run --profile perf --bin mapsoffline-data -- bench`,
   `MAPSOFFLINE_PBF=… cargo test --profile perf --test routing_real -- --nocapture`.
   (Until then the same is possible with `CARGO_TARGET_DIR=target/perf CARGO_PROFILE_RELEASE_LTO=off
   CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_INCREMENTAL=true
   CARGO_PROFILE_RELEASE_PANIC=unwind cargo test --release …`.)
2. Optional: `[profile.dev.package.mapsoffline] opt-level = 1` would bring routing under
   `pnpm tauri dev` (national with alternatives ~150 ms; release 37 ms) closer to release speed, at
   the cost of a slower incremental compile of the app crate. Not measured.
3. `rstar` is no longer used by the router (only by `geocode/builder.rs`); `bincode` is now only used
   by transit. Both stay for now.
