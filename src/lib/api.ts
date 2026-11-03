// Typed IPC wrappers. Types mirror src-tauri/src/types.rs 1:1 (camelCase).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type LonLat = [number, number];

// ------------------------------------------------------------- geocoding
export type GeoKind = "address" | "street" | "place" | "poi" | "stop" | "coord";
export type GeoPrecision = "exact" | "interpolated" | "street" | "city";

export interface GeoResult {
  id: string;
  name: string;
  kind: GeoKind;
  precision: GeoPrecision;
  lon: number;
  lat: number;
  city?: string;
  street?: string;
  houseNumber?: string;
  category?: string;
  score?: number;
}

// ------------------------------------------------------------- routing
export type Mode = "car" | "foot" | "bike" | "transit";
export type TimeKind = "now" | "depart" | "arrive";

export interface TimeSpec {
  kind: TimeKind;
  iso?: string;
}

export interface Avoid {
  toll: boolean;
  motorway: boolean;
  unpaved: boolean;
  ferry: boolean;
  /** Palestinian Authority Area A (and H1 / Gaza) – forbidden for the route. */
  areaA: boolean;
  /** Area B. */
  areaB: boolean;
  /** With `unpaved`: only long unpaved stretches are avoided (short ones are allowed). */
  unpavedLongOnly: boolean;
  /** Avoid difficult junctions (left turns / U-turns without traffic lights). */
  hardTurns: boolean;
}

export type AvoidAreas = "none" | "a" | "ab";
export type UnpavedMode = "allow" | "avoidLong" | "avoid";

export interface Waypoint {
  lon: number;
  lat: number;
  name?: string;
}

export interface RouteRequest {
  waypoints: Waypoint[];
  mode: Mode;
  time: TimeSpec;
  avoid: Avoid;
  alternatives: number;
}

export type Maneuver =
  | "depart"
  | "turn-left"
  | "turn-right"
  | "slight-left"
  | "slight-right"
  | "sharp-left"
  | "sharp-right"
  | "straight"
  | "uturn"
  | "roundabout"
  | "exit-roundabout"
  | "arrive"
  | "merge"
  | "fork";

export interface Step {
  instruction: string;
  maneuver: Maneuver;
  roundaboutExit?: number;
  street?: string;
  distanceM: number;
  durationSec: number;
  geometryStart: number;
}

export type LegKind = "walk" | "drive" | "bike" | "transit";
export type TransitVehicle = "bus" | "rail" | "light_rail" | "tram" | "subway" | "ferry" | "other";

export interface TransitStopTime {
  name: string;
  /** Stop code as printed on the sign, when known. */
  code?: string;
  lon: number;
  lat: number;
  timeIso: string;
}

export interface TransitLegInfo {
  vehicle: TransitVehicle;
  routeShortName: string;
  routeLongName: string;
  agency: string;
  headsign: string;
  color?: string;
  fromStop: string;
  toStop: string;
  departIso: string;
  arriveIso: string;
  numStops: number;
  stops: TransitStopTime[];
}

export interface Leg {
  kind: LegKind;
  durationSec: number;
  distanceM: number;
  geometry: LonLat[];
  departIso?: string;
  arriveIso?: string;
  steps?: Step[];
  transit?: TransitLegInfo;
}

export type RouteTag = "fastest" | "shortest" | "fewest_transfers" | "least_walking";

export interface Route {
  id: string;
  mode: Mode;
  durationSec: number;
  distanceM: number;
  geometry: LonLat[];
  legs: Leg[];
  tags: RouteTag[];
  departIso?: string;
  arriveIso?: string;
  transfers: number;
  walkM: number;
}

export type RouteSource = "online" | "offline";

export interface RouteResponse {
  routes: Route[];
  computeMs: number;
  source: RouteSource;
  provider?: string;
}

export interface StopInfo {
  id: number;
  name: string;
  code?: string;
  lon: number;
  lat: number;
}

// ------------------------------------------------------------- history / favorites
export type HistoryKind = "search" | "route";

export interface HistoryItem {
  id: number;
  kind: HistoryKind;
  createdIso: string;
  label: string;
  payload: unknown;
}

export type FavoriteKind = "address" | "stop" | "place" | "line";

export interface Favorite {
  id: number;
  name: string;
  icon: string;
  lon: number;
  lat: number;
  address?: string;
  createdIso: string;
  kind: FavoriteKind;
  /** stop: `{code}`; line: `{routeShortName, routeLongName, agency, headsign, color}` */
  data?: Record<string, unknown>;
}

/** Extras stored with a favorite transit line. */
export interface LineRef {
  routeShortName: string;
  routeLongName?: string;
  agency: string;
  headsign?: string;
  color?: string;
}

/** In-app update check result (signed GitLab release vs the running version). */
export interface AppUpdate {
  current: string;
  latest: string;
  available: boolean;
  pageUrl: string;
  setupUrl?: string;
  portableUrl?: string;
  notes: string;
  checkedIso: string;
  error?: string;
}

export interface MyLocation {
  lon: number;
  lat: number;
  accuracyM: number;
}

/** A line of one agency (favorites picker). */
export interface LineSummary {
  shortName: string;
  longName: string;
  color?: string;
  headsigns: string[];
}

/** `[secondsFromNow, iso]` of a scheduled departure. */
export type Departure = [number, string];

export interface LineDirection {
  headsign: string;
  fromStop: string;
  toStop: string;
  durationSec: number;
  departures: Departure[];
}

export interface LineTimes {
  shortName: string;
  longName: string;
  agency: string;
  color?: string;
  directions: LineDirection[];
}

export interface StopLine {
  shortName: string;
  agency: string;
  headsign: string;
  color?: string;
  terminus: boolean;
  totalToday: number;
  next: Departure[];
}

export interface StopBoard {
  stop: StopInfo;
  lines: StopLine[];
  generatedIso: string;
  weekday: number;
}

/** One direction of a transit line: longest stop pattern + shape. */
export interface LineShape {
  shortName: string;
  longName: string;
  agency: string;
  headsign: string;
  color?: string;
  stops: StopInfo[];
  geometry: LonLat[];
}

// ------------------------------------------------------------- data
export type DataComponent = "tiles" | "world" | "satellite" | "terrain" | "routing" | "geocode" | "transit";
export const DATA_COMPONENTS: DataComponent[] = ["tiles", "world", "satellite", "terrain", "routing", "geocode", "transit"];

export interface ComponentStatus {
  present: boolean;
  sizeBytes: number;
  updatedIso?: string;
  version?: string;
  loaded: boolean;
}

export interface UpdateCheck {
  available: boolean;
  remoteVersion?: string;
  error?: string;
}

export interface DataCheck {
  checkedIso: string;
  tiles: UpdateCheck;
  world: UpdateCheck;
  satellite: UpdateCheck;
  terrain: UpdateCheck;
  routing: UpdateCheck;
  geocode: UpdateCheck;
  transit: UpdateCheck;
}

export interface DataStatus {
  dir: string;
  /** The running update is paused. */
  paused: boolean;
  tiles: ComponentStatus;
  world: ComponentStatus;
  satellite: ComponentStatus;
  terrain: ComponentStatus;
  routing: ComponentStatus;
  geocode: ComponentStatus;
  transit: ComponentStatus;
  updating: boolean;
}

export type ProgressPhase = "queued" | "downloading" | "processing" | "finalizing" | "done" | "failed" | "cancelled";

export interface DataProgress {
  component: DataComponent;
  phase: ProgressPhase;
  done: number;
  total: number;
  message: string;
}

// ------------------------------------------------------------- settings
export interface Settings {
  language: string;
  theme: string;
  units: string;
  dataDir?: string | null;
  showTransitLayer: boolean;
  showPoiLayer: boolean;
  mapLabelLanguage?: string | null;
  /** "auto" (online first, offline fallback) | "online" | "offline" */
  routingMode: "auto" | "online" | "offline";
  osrmUrl: string;
  transitApiUrl: string;
  /** Look for a newer program version at startup (once a day). */
  autoCheckAppUpdates: boolean;
  /** Satellite imagery detail: low (zoom 12), medium (13), high (14 = native 10 m). */
  satelliteQuality: "low" | "medium" | "high" | "max";
  /** Imagery source: Survey of Israel orthophoto (govmap, ~0.13 m) or Sentinel-2 (10 m). */
  satelliteSource: "govmap" | "sentinel";
  /** Open the map where it was last left. */
  rememberMapView: boolean;
  /** Accent colour of the light theme. */
  accent: "blue" | "green" | "purple" | "orange" | "teal" | "rose" | "slate" | "pastel";
  /** Default for the route panel: avoid toll roads. */
  avoidTolls: boolean;
  /** Default for the route panel: which PA areas are forbidden. */
  avoidAreas: AvoidAreas;
  avoidMotorways: boolean;
  avoidFerries: boolean;
  unpaved: UnpavedMode;
  avoidHardTurns: boolean;
  /** Order of public-transport results. */
  transitPreference: TransitPreference;
}

export interface AppInfo {
  version: string;
  bbox: [number, number, number, number];
}

// ------------------------------------------------------------- error helpers
export const ERROR_CODES = [
  // longer codes first: errorCode() matches by substring
  "NO_ROUTING_DATA_REGION",
  "NO_ROUTING_DATA",
  "NO_TRANSIT_DATA",
  "NO_GEOCODE_DATA",
  "UPDATE_RUNNING",
  "NEED_TWO_WAYPOINTS", "LOCATION_DENIED", "LOCATION_UNAVAILABLE", "NO_TERRAIN_DATA"] as const;
export type ErrorCode = (typeof ERROR_CODES)[number];

export function errorCode(e: unknown): ErrorCode | null {
  const s = errorText(e);
  for (const c of ERROR_CODES) if (s.includes(c)) return c;
  return null;
}

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

// ------------------------------------------------------------- commands

// ---------------------------------------------------------------- countries / regions (world maps)

/** One downloadable region of the Geofabrik catalogue (continent, country or sub-region). */
export interface RegionEntry {
  /** Geofabrik id, e.g. "france", "us/california" style ids are plain ("california"). */
  id: string;
  /** Parent id (continent / country); null for continents. */
  parent: string | null;
  /** English name from the catalogue. */
  name: string;
  /** ISO 3166-1 alpha-2 when the region is a country ("FR"); use Intl.DisplayNames for the Hebrew name. */
  iso: string | null;
  /** [minLon, minLat, maxLon, maxLat] */
  bbox: [number, number, number, number];
  /** Israel itself: covered by the built-in components, not installable here. */
  builtin: boolean;
}

export interface RegionPartStatus {
  present: boolean;
  sizeBytes: number;
  version: string | null;
}

export interface InstalledRegion {
  id: string;
  name: string;
  iso: string | null;
  bbox: [number, number, number, number];
  map: RegionPartStatus;
  search: RegionPartStatus;
  /** offline routing graph (car / foot / bike) */
  routing: RegionPartStatus;
  updatedIso: string | null;
  /** an install / update of this region is running */
  busy: boolean;
}

export type TransitPreference = "fastest" | "fewestTransfers" | "leastWalking";

export type RegionPart = "map" | "search" | "routing";

/** Emitted as tauri event `region-progress`. */
export interface RegionProgress {
  id: string;
  part: RegionPart;
  phase: ProgressPhase;
  done: number;
  total: number;
  message: string;
}

/** A place found by name and the regions that contain it (smallest first). */
export interface RegionLocate {
  name: string;
  detail: string;
  lon: number;
  lat: number;
  regions: string[];
}

/** Download size of the region's OSM extract (search source); the map is roughly 1.5× this. */
export interface RegionSize {
  pbfBytes: number | null;
}

export const api = {
  geocodeSearch: (query: string, near?: LonLat, limit?: number) =>
    invoke<GeoResult[]>("geocode_search", { query, near, limit }),
  geocodeReverse: (lon: number, lat: number) => invoke<GeoResult | null>("geocode_reverse", { lon, lat }),
  route: (req: RouteRequest) => invoke<RouteResponse>("route", { req }),
  transitStops: (bbox: [number, number, number, number], limit?: number) =>
    invoke<StopInfo[]>("transit_stops", { bbox, limit }),

  historyList: (kind?: HistoryKind, limit?: number, filter?: string) =>
    invoke<HistoryItem[]>("history_list", { kind, limit, filter }),
  historyAdd: (kind: HistoryKind, label: string, payload: unknown) =>
    invoke<HistoryItem>("history_add", { kind, label, payload }),
  historyDelete: (id: number) => invoke<void>("history_delete", { id }),
  historyClear: (kind?: HistoryKind) => invoke<void>("history_clear", { kind }),

  favoritesList: () => invoke<Favorite[]>("favorites_list"),
  /** Adds a favorite; returns the existing one when the same place / line is already saved. */
  favoritesAdd: (name: string, icon: string, lon: number, lat: number, address?: string, kind: FavoriteKind = "address", data?: Record<string, unknown>) =>
    invoke<Favorite>("favorites_add", { name, icon, lon, lat, address, kind, data: data ?? null }),
  transitLine: (shortName: string, agency: string, headsign?: string) => invoke<LineShape>("transit_line", { shortName, agency, headsign: headsign ?? null }),
  /** POIs of the given geocoder categories inside the viewport; `category` is the raw id. */
  poiLayer: (cats: string[], bbox: [number, number, number, number], limit?: number) => invoke<GeoResult[]>("poi_layer", { cats, bbox, limit: limit ?? null }),
  transitAgencies: () => invoke<string[]>("transit_agencies"),
  transitLines: (agency: string) => invoke<LineSummary[]>("transit_lines", { agency }),
  /** Departures of a line from now to the end of today's service, both directions. */
  transitLineTimes: (shortName: string, agency: string, headsign?: string) => invoke<LineTimes>("transit_line_times", { shortName, agency, headsign: headsign ?? null }),
  /** Departure board of a stop (by code, else the nearest served stop within 40 m). */
  transitStopBoard: (code: string | undefined, lon: number, lat: number) => invoke<StopBoard>("transit_stop_board", { code: code ?? null, lon, lat }),
  favoritesUpdate: (id: number, name: string, icon: string) => invoke<void>("favorites_update", { id, name, icon }),
  favoritesDelete: (id: number) => invoke<void>("favorites_delete", { id }),

  /** Geofabrik region catalogue (cached on disk, refreshed weekly or with `refresh`). */
  regionsCatalog: (refresh = false) => invoke<RegionEntry[]>("regions_catalog", { refresh }),
  /** True when countries besides Israel are installed: use the merged tile source. */
  mapTilesAvailable: () => invoke<boolean>("map_tiles_available"),
  /** Write a text file chosen in the save dialog. */
  saveTextFile: (path: string, contents: string) => invoke<void>("save_text_file", { path, contents }),
  /** City / country by name (any language) → the downloadable regions around it. Needs internet. */
  regionsLocate: (query: string) => invoke<RegionLocate[]>("regions_locate", { query }),
  regionsInstalled: () => invoke<InstalledRegion[]>("regions_installed"),
  regionSize: (id: string) => invoke<RegionSize>("region_size", { id }),
  /** Install or update a region (long running; progress on `region-progress`). Resolves when done. */
  regionInstall: (id: string, parts: RegionPart[]) => invoke<void>("region_install", { id, parts }),
  regionCancel: (id: string) => invoke<void>("region_cancel", { id }),
  regionDelete: (id: string) => invoke<void>("region_delete", { id }),
  onRegionProgress: (cb: (p: RegionProgress) => void) => listen<RegionProgress>("region-progress", (e) => cb(e.payload)),
  dataStatus: () => invoke<DataStatus>("data_status"),
  tilesPath: () => invoke<string | null>("tiles_path"),
  /** Newest Protomaps build key for the online map (no local map installed). */
  tilesOnlineKey: () => invoke<string>("tiles_online_key"),
  satellitePath: () => invoke<string | null>("satellite_path"),
  worldPath: () => invoke<string | null>("world_path"),
  terrainPath: () => invoke<string | null>("terrain_path"),
  /** Current position via the Windows location service; LOCATION_DENIED / LOCATION_UNAVAILABLE. */
  locate: () => invoke<MyLocation>("locate"),
  appUpdateCheck: () => invoke<AppUpdate>("app_update_check"),
  /** Downloads + verifies the newest signed installer; resolves to its path. Progress on `app-update-progress`. */
  appUpdateDownload: () => invoke<string>("app_update_download"),
  /** Starts the verified installer and quits the program. */
  appUpdateInstall: (path: string) => invoke<void>("app_update_install", { path }),
  onAppUpdateProgress: (cb: (p: DataProgress) => void) => listen<DataProgress>("app-update-progress", (e) => cb(e.payload)),
  /** Shortest visiting order of the request's waypoints (indices); origin fixed, destination fixed when `fixedEnd`. */
  optimizeOrder: (req: RouteRequest, fixedEnd: boolean) => invoke<number[]>("optimize_order", { req, fixedEnd }),
  /** Elevation (m) at each point from the installed DEM; null outside it. NO_TERRAIN_DATA when not installed. */
  elevationProfile: (points: LonLat[]) => invoke<(number | null)[]>("elevation_profile", { points }),
  dataUpdate: (components: DataComponent[]) => invoke<void>("data_update", { components }),
  /** Cancel everything, or one component (skipped if queued, aborted if running). */
  dataCancel: (component?: DataComponent) => invoke<void>("data_cancel", { component: component ?? null }),
  dataPause: (paused: boolean) => invoke<void>("data_pause", { paused }),
  dataCheck: () => invoke<DataCheck>("data_check"),
  /** Pack all installed data into one .mapsdata file (for a machine without internet). */
  /** Pack the installed components (all when `components` is omitted) into a .mapsdata file. */
  dataExport: (dest: string, components?: DataComponent[]) => invoke<DataComponent[]>("data_export", { dest, components: components ?? null }),
  /** Install a .mapsdata file into the data directory. */
  dataImport: (src: string) => invoke<DataComponent[]>("data_import", { src }),
  dataPreload: (transit: boolean) => invoke<void>("data_preload", { transit }),

  settingsGet: () => invoke<Settings>("settings_get"),
  settingsSet: (settings: Settings) => invoke<Settings>("settings_set", { settings }),
  appInfo: () => invoke<AppInfo>("app_info"),

  onDataProgress: (cb: (p: DataProgress) => void): Promise<UnlistenFn> =>
    listen<DataProgress>("data-progress", (e) => cb(e.payload)),
  onDataUpdated: (cb: () => void): Promise<UnlistenFn> => listen("data-updated", () => cb()),
};
