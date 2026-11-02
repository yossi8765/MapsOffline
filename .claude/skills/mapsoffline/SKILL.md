---
name: mapsoffline
description: Working on the MapsOffline desktop app (Tauri 2 + Rust backend + React/MapLibre frontend, offline Israel maps, routing, transit). Use for any change to src/, src-tauri/, data pipeline, or UI; covers architecture, contracts, build/run/test commands, Hebrew/RTL rules and the multi-model delegation policy.
---

# MapsOffline – project skill

## What this is
Offline navigation & maps desktop app for Israel (Windows). Spec: `docs/SPEC.md` (Hebrew, authoritative).
Stack: Tauri 2 · Rust (data pipeline, routing, transit, geocoding, history) · React 19 + TypeScript + Vite + Tailwind v4 · MapLibre GL JS + PMTiles · Zustand.

## Layout
```
src-tauri/src/
  lib.rs            tauri builder, plugin & command registration, AppState
  commands.rs       ALL #[tauri::command] fns (thin: validate → call module → serialize)
  state.rs          AppState { graph: RwLock<Option<Arc<Graph>>>, transit: ..., geocoder, history }
  paths.rs          data dir (%APPDATA%\MapsOffline\data), file names
  data/{download,tiles,satellite,package,manifest}.rs   update pipeline, emits "data-progress" events
                    satellite.rs = Sentinel-2 (EOX) tiles → raster PMTiles v3 written in Rust (Hilbert ids, gzip dirs, resume index, sea mask)
  routing/{graph,profile,astar,alternatives,instructions}.rs
  transit/{model,raptor,journey}.rs
  online/{osrm,motis,polyline}.rs  online providers (short timeouts; errors => offline fallback)
  geocode/mod.rs    SQLite FTS5 index, exact address parsing, partial street names (expand_street_keys),
                    merge_with_stops; categories.rs = Hebrew POI-category lexicon + labels (schema v3: index tokens also carry `norm::spell_fold` forms – doubled י/ו collapsed – and queries are folded too; `similar_street_keys` = Levenshtein fallback for street typos)
  history.rs        SQLite history + favorites
  bin/mapsoffline-data.rs   CLI: build data without the GUI
src/
  lib/api.ts        typed invoke() wrappers – single source of truth for IPC types (mirror of Rust serde structs)
  lib/store.ts      zustand store (route state, ui state, settings)
  lib/i18n.ts       Hebrew (default, RTL) + English strings
  lib/gestures.ts   wheel/trackpad gestures (replaces MapLibre scrollZoom: pinch = fast zoom, wheel = stepped zoom, two-finger scroll = pan|zoom per localStorage pref)
  components/       MapView, Sidebar, RoutePanel, SearchBox, RouteDetails, FavoritesPanel (sections: lines / addresses with home+work first / stops / places), HistoryPanel, DataPanel, SettingsPanel, LayersPanel, PrintSheet
  styles.css        tailwind + theme tokens
public/fonts, public/sprites   offline glyphs/sprites for MapLibre
```

## IPC contract (keep Rust serde names == TS names, camelCase via `#[serde(rename_all = "camelCase")]`)
- `geocode_search(query, near?: [lon,lat], limit) -> GeoResult[]`  `{id, name, kind: 'address'|'street'|'place'|'poi'|'stop'|'coord', precision: 'exact'|'interpolated'|'street'|'city', lon, lat, city?, street?, houseNumber?, category?}`
  `category` is the Hebrew label for POIs ("בית כנסת"), `stop:<code>` for transit stops, the OSM place kind for places.
  Queries: coordinates; exact address (partial street names allowed); category phrase + name ("בית כנסת חניכי הישיבות");
  category alone = nearest of that kind around `near`; stop name / stop code (transit model loads in the background on first use).
- `geocode_reverse(lon, lat) -> GeoResult | null`
- `route(req: RouteRequest) -> RouteResponse` – hybrid: `settings.routingMode` "auto" tries `online/` (OSRM, Transitous) first and falls back to the local engines; resp carries `source: 'online'|'offline'` and `provider`  req `{waypoints: [{lon,lat}], mode: 'car'|'foot'|'bike'|'transit', time: {kind:'now'|'depart'|'arrive', iso?}, avoid: {toll, motorway, unpaved, unpavedLongOnly, ferry, areaA, areaB, hardTurns}, alternatives: number}`
  - `avoid.unpaved` + `avoid.unpavedLongOnly` = penalise only unpaved edges longer than `profile::UNPAVED_LONG_M` (300 m between junctions); `avoid.hardTurns` = extra turn cost for left turns (delta < -45°) and U-turns (|delta| ≥ 160°) at vertices without `v_signal` (traffic lights), outside roundabouts, scaled by the biggest road class involved (`profile::hard_turn_penalty`). Both are offline-only (OSRM has no equivalent; online routes are returned unchanged). Settings defaults for the route panel: `avoidTolls` (true), `avoidAreas` ("ab"), `avoidMotorways`, `avoidFerries`, `unpaved` ("allow"|"avoidLong"|"avoid", default "avoidLong"), `avoidHardTurns`.
  - `avoid.areaA` / `avoid.areaB` = never enter Oslo Area A (incl. Hebron H1 and the Gaza Strip) / Area B. Offline: edges flagged `AREA_A`/`AREA_B` are impassable for car/foot/bike; a waypoint whose nearest road is inside an avoided territory, or an unreachable target, yields the error `NO_ROUTE_AVOID_AREAS`. Online: OSRM cannot avoid polygons, so `route_hybrid` drops online routes whose geometry enters an avoided territory (`Graph::drop_routes_in_avoided_areas`, rings stored in graph.bin) and falls back to the local engine when none is left. Transit (Transitous / local RAPTOR) is **not** filtered by territory.
  - `settings.avoidTolls` (default true) and `settings.avoidAreas` ("none" | "a" | "ab", default "ab") are only the route panel's initial values; the request's `avoid` is what counts.
  resp `{routes: Route[]}`; Route `{id, mode, durationSec, distanceM, geometry: [lon,lat][], legs: Leg[], tags: ('fastest'|'shortest'|'fewest_transfers')[]}`
  Leg `{kind:'walk'|'drive'|'bike'|'transit', durationSec, distanceM, geometry, steps?: Step[], transit?: {routeShortName, routeLongName, agency, headsign, color?, fromStop, toStop, departIso, arriveIso, numStops, stops: {name, lon, lat, timeIso}[]}}`
  Step `{instruction, maneuver: 'depart'|'turn-left'|'turn-right'|'slight-left'|'slight-right'|'sharp-left'|'sharp-right'|'straight'|'uturn'|'roundabout'|'exit-roundabout'|'arrive'|'merge'|'fork', roundaboutExit?, street?, distanceM, durationSec, geometryStart: number}`
- `transit_stops(bbox: [minLon,minLat,maxLon,maxLat], limit?) -> StopInfo[]` `{id, name, code?, lon, lat}` (map layer; NO_TRANSIT_DATA when not installed)
- `history_list(kind: 'search'|'route'|'all', limit) / history_add / history_delete(id) / history_clear`
- `favorites_list / favorites_add(name, icon, lon, lat, address?, kind?, data?) / favorites_update / favorites_delete` – `Favorite {id, name, icon, lon, lat, address?, createdIso, kind: 'address'|'stop'|'place'|'line', data?}`; `favorites_add` returns the existing row for the same spot (same kind, ~10 cm) or the same line (same data) instead of inserting a duplicate. Lines store `{routeShortName, routeLongName?, agency, headsign?, color?}`.
- `transit_agencies() -> string[]`, `transit_lines(agency) -> LineSummary[] {shortName, longName, color?, headsigns}` (favorites line picker); `transit_line_times(shortName, agency) -> LineTimes {directions: [{headsign, fromStop, toStop, durationSec, departures: [secondsFromNow, iso][]}]}` (rest of today's service, both directions, incl. yesterday's after-midnight trips); `transit_stop_board(code?, lon, lat) -> StopBoard {stop, lines: [{shortName, agency, headsign, color?, terminus, totalToday, next: [secondsFromNow, iso][]}], generatedIso}` – schedule-based countdown (no real-time offline); all in `src-tauri/src/transit/board.rs`, CLI `line-times <agency> <line>` / `stop-board <code>`.
- `transit_line(shortName, agency, headsign?) -> LineShape {shortName, longName, agency, headsign, color?, stops: StopInfo[], geometry}` – longest stop pattern of the line (headsign match preferred); drawn by MapView's `line-preview` source for the Favorites panel (`store.linePreview`).
- `data_status() -> DataStatus {dir, components: {tiles, satellite, routing, geocode, transit}: {present, sizeBytes, updatedIso, version}}`; `tiles_path()` / `satellite_path()` → absolute file path or null (frontend opens both via `pmtiles://` + asset protocol; satellite = raster source, `mapStyle.ts` drops the fill layers when `layers.satellite` is on)
- `data_update(components: string[])` – long running; progress via event `data-progress` `{component, phase, done, total, message}`; `data_cancel(component?)` (all, or skip/abort one), `data_pause(paused)` (`DataStatus.paused`). `types::ProgressFn` returns `bool` (false = cancelled → builders `bail!` at the next checkpoint) and blocks while paused; `data::Cancel` carries all/per-component/paused flags + `check()`/`wait_if_paused()`; `cancel.bound_to(&[comps])` gives a handle that is cancelled only when all of its components are (shared PBF download). `data::update` runs every download in parallel (`thread::scope`: tiles, satellite, PBF once for routing+geocode, GTFS) and the builds sequentially as their source arrives; `Reporter` fans progress out to the components a job serves, `Finisher` writes the manifest under a lock
- `data_export(dest, components?: DataComponent[])` / `data_import(src)` → `DataComponent[]` – `.mapsdata` package (zip mirroring the data dir + `package.json`; graph/geocode/transit Deflate, PMTiles stored; empty/missing `components` = all installed; `src-tauri/src/data/package.rs`). Progress via `data-progress`, `data-updated` after import; both reuse the `updating` flag/cancel. CLI: `mapsoffline-data export <file> [--only a,b]` | `import <file>`.
- `data_check()` → `DataCheck {checkedIso, tiles|routing|geocode|transit: {available, remoteVersion?, error?}}` – HEAD/ranged-GET probes only (~2 s, parallel), compares server Last-Modified / Protomaps build key with manifest.json; CLI: `mapsoffline-data check`. Hosts blocked by the NetFree content filter answer HTTP 418 → Hebrew "approve host" message (`download::blocked_message`); tiles fall back to probing `build.protomaps.com/YYYYMMDD.pmtiles` when builds.json is blocked.
- `poi_layer(cats: string[], bbox, limit?) -> GeoResult[]` (raw category id in `category`; frontend `lib/poiCategories.ts` maps ids → ~40 picker categories with optional name regex; `components/CategoryPicker.tsx` = map control above the zoom buttons).
- `terrain_path()`, `elevation_profile(points) -> (number|null)[]` (Terrarium DEM, `data/terrain.rs`; `data/satellite.rs` now exposes the generic `download_raster(dest, version, RasterSpec, client, …)` + `verify(path, placeholders)`; archive header carries the tile type). Style: `raster-dem` source `dem` + `hillshade` layer / `terrain` when `layers.hillshade` / `layers.terrain3d`.
- `locate() -> MyLocation {lon, lat, accuracyM}` – WinRT `Geolocator` (`src/location.rs`, `windows` crate, Windows consent dialog); store `myLocation`, SearchBox row type `me`, map layers `my-location-*`.
- `settings_get / settings_set`

## Installer
NSIS strings for Hebrew come from `src-tauri/nsis/Hebrew.nsh` (`bundle.windows.nsis.customLanguageFiles`): the Tauri CLI's embedded Hebrew file was not emitted (only English.nsh landed in `target/release/nsis/x64`), leaving the reinstall page, the "app is running" box and the desktop-shortcut checkbox blank. Keep every `LangString` key of the bundler's English.nsh in it.

## Commands
```
pnpm install                 # frontend deps
pnpm tauri dev               # run app (hot reload)
pnpm tauri build             # installer in src-tauri/target/release/bundle
cd src-tauri && cargo check  # fast Rust compile check (add --features cli to include the data CLI)
cd src-tauri && cargo test
cd src-tauri && cargo run --bin mapsoffline-data -- build --all   # build data files from CLI
pnpm tsc --noEmit            # typecheck frontend
```
Always `cargo check` after touching Rust and `pnpm tsc --noEmit` after touching TS before declaring done.

## Rules
- Hebrew UI first, `dir="rtl"` on root; every string goes through `lib/i18n.ts`.
- No network calls at runtime except inside `data/` update flow and `online/` routing (which must fail fast and never block offline use).
- Large data (graph, transit) lives in `AppState` behind `RwLock<Option<Arc<_>>>`; load lazily, never block the UI thread; long jobs use `tauri::async_runtime::spawn_blocking` and emit progress events.
- Binary formats: `MAGIC + VERSION` header. graph.bin (v4: v3 + `v_signal` bitset of signalised junction vertices from node tag `highway=traffic_signals`, 9191 vertices; CLI `route --unpaved allow|avoidLong|avoid --hard-turns`, output line shows `unpaved: N m`) is flat little-endian arrays + a persisted 400 m grid index (loads in ~30 ms) + the restricted-territory rings (`routing/areas.rs`: Area A/B boundary relations stitched into closed rings, u32 edge flags with `AREA_A = 1<<15`, `AREA_B = 1<<16`, classification = any sample point >10 m inside a ring); transit.bin is bincode. CLI: `route --avoid-areas a|ab --avoid-toll`, `area lon,lat` prints which territory a point / its nearest roads are in. Bump the version when a layout changes and reject old versions with a clear error. Measure with `cargo run --release --features cli --bin mapsoffline-data -- bench`; see docs/PERF.md and docs/SIZE.md.
- Coordinates everywhere are `[lon, lat]` (GeoJSON order). Distances meters, durations seconds, times ISO-8601 with offset (Asia/Jerusalem).
- Files with Hebrew text: write with the Write tool (bash heredocs on Windows mangle them).

## Multi-model delegation policy (user request: pick the right model per task)
When splitting work with the Agent tool choose the model by task type:
- **opus / fable (fork)**: algorithms and data structures with correctness risk – routing graph build, A*, alternatives, RAPTOR, GTFS model, address parsing; anything that must compile & pass tests first try.
- **sonnet**: UI components, styling, panels, i18n strings, TS wrappers, moderate Rust glue (downloads, manifest, SQLite history), test writing.
- **haiku**: mechanical tasks – boilerplate, config files, renaming, docs formatting, lint fixes, asset downloads, simple scripts.
Always give the agent: the file(s) it owns, the IPC contract above, and "run cargo check / tsc before finishing". Agents own disjoint files; the orchestrator integrates `lib.rs`, `commands.rs`, `Cargo.toml`, `package.json`.
