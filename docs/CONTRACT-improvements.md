# IPC contract for the IMPROVEMENTS.md implementation (wave 1)

Authoritative for all six wave-1 streams. TypeScript names below are the serde names (camelCase).
Rust structs use `#[serde(rename_all = "camelCase")]`; optional fields `#[serde(skip_serializing_if = "Option::is_none")]`
and `#[serde(default)]` on input structs so older frontends/backends stay compatible.
The TS side of everything listed here is ALREADY written in `src/lib/api.ts` (section "IMPROVEMENTS contract");
Rust streams implement it, frontend streams consume it. Do not rename anything; if a stream needs an extra field,
add it as optional and list it in its final report.

Errors: every command rejects with `ApiError {code, message, details?}` (wave 0). New codes are listed per stream.

---

## R1 – data, infrastructure, security (`src-tauri/src/{lib,state,paths,history,app_update,online_tiles,map_tiles}.rs`, `data/*` except wiki, `commands/{data,app}.rs`, Cargo.toml, tauri.conf.json, capabilities, tools/, .gitlab-ci.yml)

```ts
// Settings – new fields (all optional on input, defaults in Rust)
interface Settings {
  coordFormat: "dd" | "dms" | "itm" | "utm";          // default "dd"
  uiScale: "system" | 1 | 1.12 | 1.25;                // default "system" (= Windows text scale, clamped 1..1.25)
  accent: "system" | "blue" | ...existing;             // "system" = Windows accent (new default for new installs only)
  customLayers: CustomLayer[];                         // default []
  wantedComponents: DataComponent[] | null;            // null = all (old behaviour); check/update-all only offer these
  teachingTipsSeen: string[];                          // default []
}
interface CustomLayer { id: string; name: string; kind: "xyz" | "wms" | "pmtiles" | "mbtiles"; url: string /* template URL, WMS base URL, or local file path */;
  wmsLayers?: string; opacity: number; visible: boolean; minzoom?: number; maxzoom?: number; attribution?: string }

// commands
system_accent(): { accent: string; light1: string; light2: string; dark1: string } | null   // "#rrggbb"; event "system-accent-changed" with the same payload
system_text_scale(): number                         // UISettings.TextScaleFactor (1.0 .. 2.25)
connectivity(): boolean                             // HEAD to the tiles host, 1.5 s timeout, cached 30 s
logs_dir(): string                                  // (wave 0)
// user layers (imported GPX/KML/GeoJSON and drawings), stored in <data>/userlayers/<id>.geojson + index.json
user_layers_list(): UserLayer[]
user_layer_get(id: string): string                  // GeoJSON FeatureCollection text
user_layer_save(name: string, kind: "import" | "drawing" | "track", geojson: string, color?: string): UserLayer
user_layer_update(id: string, patch: { name?: string; color?: string; visible?: boolean; geojson?: string }): UserLayer
user_layer_delete(id: string): void
interface UserLayer { id: string; name: string; kind: "import" | "drawing" | "track"; color: string; visible: boolean; createdIso: string; features: number; bbox?: [number, number, number, number] }
// favorites – new optional fields + kind "route"
interface Favorite { /* existing */ group?: string; color?: string; note?: string; kind: "address" | "stop" | "place" | "line" | "route" }
favorites_add(..., group?, color?, note?)           // extra optional args, same order appended
favorites_update(id, patch: { name?, icon?, group?, color?, note?, address? })
favorites_groups(): string[]
// minyanim: snapshot first, live later
minyanim_for(lon, lat, poiName?) -> MinyanTable | null   // returns the SNAPSHOT immediately (live: false)
// then, when online, event "minyanim-live" { lon, lat, table: MinyanTable } (same lon/lat as requested)
```
- Custom layers are served by a new scheme `mo-user`: `http://mo-user.localhost/<layerId>/{z}/{x}/{y}` (xyz/wms: online fetch with disk cache `<data>/usercache/<layerId>/`, WMS GetMap EPSG:3857 256px; pmtiles/mbtiles: local file). CSP gets `http://mo-user.localhost` in img-src and connect-src.
- Second map window: capability for label `map` (`core:webview:allow-create-webview-window`, `core:window:*` needed) – frontend opens it with `new WebviewWindow("map", { url: "index.html?window=map" })`.
- `tauri-plugin-window-state` (size, position, maximized).
- New error codes: `DISK_FULL` (details: "needed=<bytes> free=<bytes>"), `INVALID_DATA_DIR`, `INVALID_PATH`, `CHECKSUM_MISMATCH`.

## R2 – routing + navigation (`routing/*`, `nav/*`, `voice.rs`, `geo.rs`, `location.rs`, `commands/{route,nav}.rs`)

```ts
type Mode = "car" | "foot" | "bike" | "transit" | "truck" | "wheelchair";
interface RouteRequest { /* existing */ vehicle?: { heightM?: number; weightT?: number; widthM?: number }; accessible?: boolean /* transit: wheelchair */ }
interface Avoid { /* existing */ points?: [number, number][]   /* avoid the road nearest each point */;
  polygons?: [number, number][][];                               /* closed or open rings, lon/lat */
  hills?: boolean }                                              /* bike/foot: prefer flatter */
interface Step { /* existing */ ref?: string /* "1", "443" */; exit?: string /* junction:ref */; destination?: string /* signposted towards */;
  speedLimitKmh?: number; lanes?: { directions: string[]; active: boolean }[] }
interface NavUpdate { /* existing */ speedKmh?: number; speedLimitKmh?: number; street?: string; accuracyM?: number; approximate: boolean; cameraAheadM?: number; lanes?: Step["lanes"] }
isochrone(lon, lat, mode: "car" | "foot" | "bike" | "wheelchair", minutes: number[]): { minutes: number; rings: [number, number][][] }[]
match_track(points: [number, number][], mode: "car" | "foot" | "bike"): Route        // map-matching an imported track
// track recording (location source = the nav Geolocator), saved through R1's user layers (kind "track")
track_start(): void;  track_stop(save: boolean, name?: string): UserLayer | null
event "track-update" { points: [number, number, number | null][] /* lon, lat, ele */; distanceM: number; durationSec: number; ascentM: number }
```
- Waypoint limit (7) removed when alternatives = 1. `optimize_order` faster, accepts up to 25 stops (heuristic above 12) and optional `stops: { stayMin?: number; windowStartIso?: string; windowEndIso?: string }[]` aligned with waypoints; returns `{ order: number[]; arrivals: string[] }` — NOTE: return type changes from `number[]` to this object.
- Graph v7 (bump VERSION, reject old with a clear error, `NO_ROUTING_DATA` + details "graph version" so the UI offers an update).
- Error codes: `NO_ROUTE_TRUCK` (restriction makes it impossible), `TOO_MANY_POINTS`.

## R3 – transit (`transit/*`, `online/{motis,stride}.rs`, `commands/transit.rs`)

```ts
interface RouteResponse { /* existing */ notice?: { kind: "shabbat" | "holiday" | "no_service" | "late_night"; nextServiceIso?: string; holidayName?: string } }
interface Route { /* existing */ fareBreakdown?: { label: string; ils: number; train: boolean }[] }
interface StopBoard { /* existing */ nextServiceIso?: string /* when today's service is over / Shabbat */ }
interface LineTimes { /* existing */ nextServiceIso?: string }
transit_isochrone(lon, lat, timeIso: string | null, minutes: number[]): { minutes: number; rings: [number, number][][] }[]
transit_vehicles(shortName: string, agency: string): { lon: number; lat: number; bearing?: number; headsign?: string; updatedIso: string }[]   // online (Stride SIRI), [] offline
```
- transit.bin v3 (raw arrays, validate(), END marker), transfers walked on the road network at build time.
- `accessible: true` in RouteRequest filters wheelchair_boarding / wheelchair_accessible when the feed has them.
- Error codes: `NO_SERVICE` (details JSON of the notice), `TRANSIT_DATA_CORRUPT`.

## R4 – geocoding (`geocode/*`, new `coords.rs`, `commands/geo.rs`, `src/lib/poiCategories.ts`)

```ts
interface GeoResult { /* existing */ phone?: string; website?: string; address?: string; brand?: string; wikipedia?: string /* "he:Title" */; wikidata?: string; operator?: string }
wiki_summary(tag: string /* "he:Title" or Qid */): { title: string; extract: string; url: string; lang: string } | null   // offline from geocode.sqlite
geocode_batch(queries: string[], near?: [number, number]): (GeoResult | null)[]                                         // CSV import, best result each
coords_parse(text: string): { lon: number; lat: number; format: "dd" | "dms" | "itm" | "utm" | "plus" } | null
```
- `geocode_search` accepts ITM ("178500 663500"), UTM ("36R 676000 3550000"), DMS, Plus Codes and any valid world lon/lat.
- New POI categories (ids): `spring`, `viewpoint`, `camp_site`, `toilets`, `drinking_water`, `shelter`, `defibrillator`, `playground`, `place_of_worship:muslim`, `place_of_worship:christian`, `cinema`, `theatre`, `charging_station` details via `socket`/`capacity` in `GeoResult.extra`? → use `operator` + `category`.
- Places: `place=square`, named `tourism=attraction` → kind "place", category "landmark".

## F1 – look, UX, accessibility (`src/styles.css` except the [F2] section, `src/components/*` EXCEPT the F2-owned ones, `src/lib/i18n.ts` (F1 owns copy-editing of existing strings), `src/lib/format.ts`, `src/App.tsx`, `src/main.tsx`, `index.html`)
New components: `InfoBar.tsx`, `Tip.tsx` (tooltip), `TeachingTip.tsx`, `CommandPalette.tsx` (Ctrl+K), `ShareDialog.tsx` (QR), `BatchGeocodeDialog.tsx` (CSV), `FirstRunCard.tsx`, `Icon.tsx`.

## F2 – map, performance, map tools (`src/components/MapView.tsx`, `src/map/**`, `src/lib/mapStyle.ts`, `src/lib/contours.ts`, `src/lib/gestures.ts`, `src/lib/nav.ts`, `src/lib/mapImage.ts`, `src/lib/print.ts`, `src/lib/coords.ts` (new), `src/lib/store/map.ts`, and the map-side components `ContextMenu.tsx`, `LayersPopover.tsx`, `LayersPanel.tsx`, `CategoryPicker.tsx`, `NearbyList.tsx`, `MapHoverCard.tsx`, `PrintSheet.tsx`; the `[F2]` section of styles.css)
New store actions (map slice) that F1 may call: `startMeasure(at?: LonLat)`, `startDrawing()`, `showIsochrone(at: LonLat, mode)`, `clearIsochrone()`, `openImport()`, `toggleTrackRecording()`, `setCompare(mode: "off" | "split" | "swipe")`, `openMapWindow()`, `setMapPopover(p: "layers" | "nearby" | "categories" | null)`.
These are declared (as no-op stubs) in `src/lib/store/map.ts` before wave 1 so F1 compiles; F2 implements them.
