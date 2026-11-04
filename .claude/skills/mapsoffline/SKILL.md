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
  commands/         ALL #[tauri::command] fns (thin: validate → call module → serialize), one file per
                    stream: geo.rs (R4 geocode), route.rs + nav.rs (R2 routing/nav), transit.rs (R3),
                    data.rs + app.rs (R1 data/infra); register in lib.rs under the stream's `// [Rn]` marker
  error.rs          AppError {code, message, details?} = what every command returns as Err (R<T>);
                    code from `CODE: …` prefixes or the cause chain (NETWORK, BLOCKED_HOST, DISK_FULL,
                    PERMISSION, INTERNAL_PANIC, INTERNAL); guard()/guard_async() turn panics into errors
  logging.rs        log file <data root>\logs\mapsoffline.log (2 MB × 5), panic hook; command logs_dir()
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
  lib/api.ts        typed invoke() wrappers – single source of truth for IPC types (mirror of Rust serde structs);
                    errors: commands reject with ApiError {code, message, details?}; toApiError / errorCode / errorText
  lib/errors.ts     describeError(e, t) / errorMessage: code -> i18n key `err_<CODE>` (unknown/INTERNAL -> errGeneric + text)
  lib/store.ts      re-export of lib/store/ (keeps `import { useStore } from "../lib/store"`)
  lib/store/        zustand slices: ui, settings (DEFAULT_SETTINGS, serialised writes), map (layers, tiles URLs,
                    my location, popovers, map-tool stubs for F2), route, data, saved (favorites+history), appUpdate
  lib/transitSort.ts, links.ts (map/geo links), sun.ts (resolveTheme, isNightInIsrael), prefs.ts (typed localStorage `mapsoffline.*`)
  lib/useTauriEvent.ts  race-free backend event subscription (hook + subscribe/subscribeWith)
  lib/useDialog.ts  useDialog (focus in, Tab trap, Escape, focus restore) + useMenuKeys (role=menu arrows/Home/End)
  lib/i18n.ts       Hebrew (default, RTL) + English strings
  lib/gestures.ts   wheel/trackpad gestures (replaces MapLibre scrollZoom: pinch = fast zoom, wheel = stepped zoom, two-finger scroll = pan|zoom per localStorage pref)
  map/              MapView internals: useMapInstance (create map, controls, view), useCamera, useHoverCard,
                    useViewportFetch (POI / stops layers), popups, colors (CSS tokens), icons (bus, POI, shields),
                    locale, overlays/{routes,transitStops,pois,myLocation,linePreview,waypoints} (add + sync each)
  components/       MapView (composition only), Sidebar, RoutePanel, SearchBox, RouteDetails, FavoritesPanel (sections: lines / addresses with home+work first / stops / places), HistoryPanel, DataPanel, SettingsPanel, LayersPanel, PrintSheet
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
- OSM source (routing + geocode) is refreshed incrementally: `data/osmdiff.rs` reads the replication sequence from the PBF header, fetches Geofabrik's daily `.osc.gz` diffs (≤ 120 missing days, full re-download every 180 days or on any failure), merges them and rewrites the PBF with its own writer (dense nodes / ways / relations, zlib blocks). Version string `OSM YYYY-MM-DD #seq` on both paths; `data::check` compares it with `state.txt`. CLI: `mapsoffline-data osm-update`.
- `data_export(dest, components?: DataComponent[])` / `data_import(src)` → `DataComponent[]` – `.mapsdata` package (zip mirroring the data dir + `package.json`; graph/geocode/transit Deflate, PMTiles stored; empty/missing `components` = all installed; `src-tauri/src/data/package.rs`). Progress via `data-progress`, `data-updated` after import; both reuse the `updating` flag/cancel. CLI: `mapsoffline-data export <file> [--only a,b]` | `import <file>`.
- `data_download_package(dest) -> number` – downloads the full ready-made package of the weekly CI build (`mapsdata-all/latest/MapsOffline-data.mapsdata`, ~420 MB) through `downloads/` (resumable, `data_cancel()` stops it) and moves it to `dest`; event `package-download` {phase, done, total, message}; DataPanel button at the top of "העברה למחשב ללא אינטרנט", then "install" (= `data_import`) / "show in folder".
- `data_check()` → `DataCheck {checkedIso, tiles|routing|geocode|transit: {available, remoteVersion?, error?}}` – HEAD/ranged-GET probes only (~2 s, parallel), compares server Last-Modified / Protomaps build key with manifest.json; CLI: `mapsoffline-data check`. Hosts blocked by the NetFree content filter answer HTTP 418 → Hebrew "approve host" message (`download::blocked_message`); tiles fall back to probing `build.protomaps.com/YYYYMMDD.pmtiles` when builds.json is blocked.
- `poi_layer(cats: string[], bbox, limit?) -> GeoResult[]` (raw category id in `category`; frontend `lib/poiCategories.ts` maps ids → ~40 picker categories with optional name regex; `components/CategoryPicker.tsx` = map control above the zoom buttons).
- `terrain_path()`, `elevation_profile(points) -> (number|null)[]` (Terrarium DEM, `data/terrain.rs`; `data/satellite.rs` now exposes the generic `download_raster(dest, version, RasterSpec, client, …)` + `verify(path, placeholders)`; archive header carries the tile type). Style: `raster-dem` source `dem` + `hillshade` layer / `terrain` when `layers.hillshade` / `layers.terrain3d`.
- `locate() -> MyLocation {lon, lat, accuracyM}` – WinRT `Geolocator` (`src/location.rs`, `windows` crate, Windows consent dialog); store `myLocation`, SearchBox row type `me`, map layers `my-location-*`.
- Other countries (`data/regions.rs`, `map_tiles.rs`, UI `components/RegionsSection.tsx` in DataPanel): `regions_catalog(refresh?) -> RegionEntry[] {id, parent, name, iso, bbox, builtin}` (Geofabrik index-v1.json cached in `regions/catalog.json`, weekly), `regions_installed() -> InstalledRegion[] {id, name, iso, bbox, map, search: {present, sizeBytes, version}, updatedIso, busy}`, `region_size(id) -> {pbfBytes}`, `region_install(id, parts: ('map'|'search')[])` (event `region-progress` {id, part, phase, done, total, message}; map = `pmtiles extract --region=region.geojson` → `regions/<id>/map.pmtiles`; search = Geofabrik PBF → `geocode::build_region` → `regions/<id>/geocode.sqlite`, PBF deleted), `region_cancel(id)`, `region_delete(id)`, `map_tiles_available() -> bool`. When a country is installed the frontend's `tilesUrl` becomes `http://mo-map.localhost/{z}/{x}/{y}.mvt` (scheme `mo-map`, first archive with the tile wins: Israel then countries; busy countries are hidden so their files can be replaced; `AppState::refresh_map_files`). `geocode_search` / `geocode_reverse` / `poi_layer` query Israel + every ready country (`AppState::with_region_geocoder`), the area around `near` first. CLI: `mapsoffline-data region list|install|delete|search`. Routing/transit stay Israel-only offline.
- Satellite source (`settings.satelliteSource`): `govmap` (default; Survey of Israel orthophoto, `data/orthophoto.rs`: ITM cache `cdn.govmap.gov.il/B0BZ1ORTO23/Lnn/Rhex/Chex.jpg` reprojected to WebMercator; zoom ≤ 9 from Sentinel-2; quality low/medium/high/max = z14/15/16/17; requests paced 8–12/s because the CDN's AWS WAF challenges bursts — never bypass it) or `sentinel` (EOX, z12–14). Manifest version `govmap-B0BZ1ORTO23 zNN`.
- `settings_get / settings_set`

## Added in the 2026-11 review (stages 1-4)
- **UI shell**: rail of 4 labelled groups – search (tabs search/route), saved (favorites/history), data, settings. Layers are a map popover (`LayersPopover.tsx`, `store.layersOpen/toggleLayers`, map button has `data-layers-toggle`). Place card in SearchPanel (round actions, stop departures, city categories, opening hours, minyanim for synagogues via `MinyanimCard.tsx`). Hover card `MapHoverCard.tsx`. Shortcuts by `e.code` (work in the Hebrew layout), `ShortcutsDialog.tsx`, `NearbyList.tsx`. Running text is JUSTIFIED (user rule); UI font Segoe UI; radii 8/4; opaque surfaces; map colours from `--map-*` tokens (`mapColors()`).
- **Links**: single instance + deep link (`tauri-plugin-single-instance`, `tauri-plugin-deep-link`, schemes `mapsoffline`, `geo`); a second start emits `open-link`; `take_pending_link()` for the first; `store.openLink(url)` parses `mapsoffline://map=z/lat/lon&q=` and RFC 5870 `geo:`.
- **Security**: asset scope = active data folder only (`allow_asset_dir`), CSP in tauri.conf.json, region ids validated, update installer re-hashed against the signed list before launch, `save_text_file` only gpx/kml/txt/json.
- **MapLibre 6.15** (worker via `setWorkerUrl`, `setMissingStyleImageResolver` for road shields, built-in RTL, no rtl-text plugin); Hebrew labels via `font-faces` (Noto Sans Hebrew TTF), other scripts PBF glyphs; Israeli road shields; contours (`lib/contours.ts`, maplibre-contour on the DEM); "topo" theme; trails GeoJSON layer; `lib/mapImage.ts` print image.
- **Geocoder** (build revision 3): street synonyms (data.gov.il), FTS5 trigram fuzzy fallback, query grammar (category + city / near / open / kosher), `nearHouseNumber`, `interpretation`, opening hours (`opening-hours` crate: `openingHours/openNow/closesAt/opensAt`), POI `minzoom` thinning – `poi_layer(cats, bbox, limit, zoom?)`; road numbers ("כביש 6").
- **Geocoder revision 4** (2026-11 improvements, R4): `pois.info` / `places.info` JSON (`geocode/info.rs` PoiInfo: phone, website, address, brand, operator, wikipedia, wikidata, extra) → `GeoExtra` fields of the same names (+ `extraInfo` for charging details); unnamed POIs of `NAMELESS_OK` kinds (spring, viewpoint, toilets, drinking_water, shelter, defibrillator, playground, camp_site, charging_station, atm; `emergency` key added); landmarks = places kind `landmark` (place=square, named tourism=attraction, named highway=pedestrian+area=yes; pop = their town's) – relations are still not read, so e.g. Tel Aviv's Rabin Square is only a POI; alt/official/old/short names, brand, operator and phone digits in the FTS index; phone queries (`info::phone_key`); English town names in addresses (`Places.en_names`); OSM `addr:interpolation` ways (`addresses.src` = 1 → Interpolated); energy-ministry fuel register (data.gov.il, brand/address/phone; fills missing details of OSM stations); Wikipedia summaries fetched at build (`geocode/wiki.rs`, batch API, ≤5 req/s; NetFree blocks he.wikipedia locally – 418 stops the fetch, CI gets them) into `wiki` / `wiki_qid`. Caps flags keep older files working. Coordinates: `src-tauri/src/coords.rs` (DD both orders worldwide, DMS incl. Hebrew צ/ד/מ/מע, ITM, UTM, Plus Codes) – commands `coords_parse`, `wiki_summary`, `geocode_batch` (max 5000). Golden real-data test: `cargo test --release --features cli --lib geocode::golden -- --ignored --nocapture` (CARGO_PROFILE_RELEASE_LTO=off keeps it light). CLI: `search --index <file>`, `wiki <tag>`.
- **Routing**: graph.bin v6 (delta+varint geometry; v4/v5 still read), turn_time vs turn_cost (ETA without preference penalties), `Route.avoidViolations`, A*-vs-Dijkstra random test. Austria measured 0.4 s for 705 km → no CCH.
- **Transit**: transit.bin v2 (dep stored only when ≠ arr), range RAPTOR merged with plain runs, lines told apart by long name (`transit_line(_times)(…, longName?)`), fares (`Route.fareEstimateIls/fareNote`, transit/fares.rs), Stride delays `transit_delays(legs)` (slow, online, setting `transitReliability`).
- **Navigation**: `nav/` state machine + `voice.rs` (WinRT, voice "Microsoft Asaf"); commands `nav_start/nav_simulate/nav_set_route/nav_set_voice/nav_stop/speak/voices_list`; events `nav-update/nav-error/nav-ended`; `store.navigation`, `startNavigation(routeId, simulate?)`; `NavigationView.tsx`; setting `navVoice`.
- **Data**: terrain from Mapterhorn (512 px WebP Terrarium, `israel-dem-mapterhorn.pmtiles`; old file used until updated, `terrain_file_in_use`); components `trails` (GeoJSON, `trails_path`) and `minyanim` (Nedarim Plus weekly, `minyanim_for`, `minyanim_next`; unless `routingMode == "offline"` both ask Nedarim Plus live first – `table_live` / `next_live`, 8 s timeout, `MinyanTable.live` – and fall back to the snapshot); CI re-encodes satellite to WebP q80 (`webp-satellite`); operator logos in public/operators (index.json + CREDITS.md).
- **Size/quality**: `.cargo/config.toml` (chrono-tz filter, SQLite flags), release RUSTFLAGS path remap, `tools/check.ps1` (tsc + clippy -D warnings + tests) run by release.ps1. React Compiler on.

## Program updates & releases
- `app_update_check / app_update_download / app_update_install` (`src-tauri/src/app_update.rs`, ported from the RAF design): the GitLab release list is the only source; a release counts only when its `SHA256SUMS` (first line `# version X.Y.Z`, must equal the tag and be newer than the running version) is signed by the Ed25519 key whose public half is `PUBLIC_KEY_HEX`; the installer is downloaded to `%LOCALAPPDATA%\MapsOffline\update`, SHA-256-checked against the signed list, started, and the app exits. Progress event `app-update-progress`. Startup check on every start (5 s in) when `settings.autoCheckAppUpdates`.
- Publish with `.\tools\release.ps1` (`-NoBuild`, `-Replace` to overwrite an existing tag/release, `-SignOnly`): builds, packs `MapsOffline-portable.zip`, writes + signs `SHA256SUMS`, appends the checksum table to `release-notes.md`, tags, and creates the GitLab release with the stable asset names `MapsOffline-setup.exe` / `MapsOffline-portable.zip` (permalink/latest). Private key: `%USERPROFILE%\.mapsoffline-signing\mapsoffline-release-ed25519.pem` – never in the repo.
- `optimize_order(req, fixedEnd) -> number[]` (`routing/order.rs`, Held–Karp over local travel times; origin fixed, destination fixed when asked; car/foot/bike only).

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
pnpm test --run              # frontend unit tests (vitest; src/**/*.test.ts[x], jsdom per file)
```
Always `cargo check` after touching Rust and `pnpm tsc --noEmit` after touching TS before declaring done.

## Rules
- Hebrew UI first, `dir="rtl"` on root; every string goes through `lib/i18n.ts`. Parallel streams add keys only under
  their own `// ===== [F1|F2|R1..R4] =====` marker at the end of each dictionary (same markers at the end of api.ts).
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

## R1 (2026-11 improvements): data / infra / security
- New modules: `system.rs` (Windows accent + text scale via WinRT `UISettings`, polled every 5 s → event `system-accent-changed`; `connectivity()` 1.5 s HEAD cached 30 s; bounded worker pools `local_pool` 8 / `net_pool` 16 that serve every `mo-*` scheme), `user_layers.rs` (`<data>/userlayers/<id>.geojson` + `index.json`, any GeoJSON normalised to a FeatureCollection, 64 MB cap), `user_tiles.rs` (scheme `mo-user`: `http://mo-user.localhost/<layerId>/{z}/{x}/{y}` for `Settings.customLayers`; xyz/wms online with disk cache `<data>/usercache/<id>/` 30 days, pmtiles via `map_tiles`, mbtiles via rusqlite TMS rows), `data/disk.rs` (`ensure_free` → `DISK_FULL: needed=… free=…`), `data/components.rs` (one table: key, folder, file, package compression, format suffix).
- Commands: `system_accent`, `system_text_scale`, `connectivity`, `user_layers_list/get/save/update/delete`, `custom_layer_clear_cache`, `favorites_patch` (absent = unchanged, null = cleared), `favorites_groups`, `favorites_add(…, group?, color?, note?)`, `package_info`, `app_update_cancel` (program downloads have their own flag `app_updating` / `app_cancel`; event `app-update-progress` has no `component` any more).
- `minyanim_for` answers from the snapshot at once and emits `minyanim-live` {lon, lat, table} when the live timetable arrives.
- Settings: `coordFormat`, `uiScale` ("system" | number), `customLayers`, `wantedComponents` (null = all; update checks / "update all" only offer these), `teachingTipsSeen`; new installs default to `accent: "system"`. Damaged settings.json / manifest.json are kept as `*.json.bad`. `settings_set` rejects an unusable data folder (`INVALID_DATA_DIR`).
- Security: webview-supplied save paths (`save_text_file` gpx/kml/txt/json/geojson/csv, `data_export` / `data_download_package` .mapsdata) must be absolute and outside the data, program and Windows folders (`INVALID_PATH`); `mo-tiles` forwards only single byte ranges ≤ 64 MB; package import accepts only each component's own files, caps entry size / ratio and checks free space; `region_dir` maps every character outside `[A-Za-z0-9_-]` to `_`; Nominatim ≤ 1 request/s with a memo.
- Integrity: Geofabrik PBF checked against `<url>.md5`; ready-made package files against the registry's `file_sha256` (`CHECKSUM_MISMATCH`). Package import records each component's version right after its file is in place; tiles sidecar written via tmp + rename.
- Performance: updates unload only the components they replaced (`AppState::unload`); govmap imagery starts next to the package downloads; package downloads 3 in parallel; one shared `http_client()`; region metas cached (10 s / generation); archive verification cached per (file, size, mtime); decoded DEM tiles LRU (48); orthophoto source cache 2,048 encoded + 192 decoded tiles; history payloads > 16 KB lose their geometries; history WAL checkpointed on exit; one `download::retry` for extract loops; `map_tiles::Archive` is the one PMTiles reader.
- Windows: `tauri-plugin-window-state`; capability covers windows `main` and `map` (second map window) incl. `core:webview:allow-create-webview-window` and `core:window:allow-set-theme`.
- CI: `test` job (clippy -D warnings, cargo test, cargo audit) on push / MR; `tools/ci-test-real.sh` after the weekly data build. CLI: `minyan <lon,lat> [--name] [--live]`, `prebuilt`.
