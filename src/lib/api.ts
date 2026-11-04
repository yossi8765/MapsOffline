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
  // ---- optional details (Rust GeoExtra, flattened)
  /** Raw OSM opening_hours of a point of interest. */
  openingHours?: string;
  /** Open right now (Israel time); absent when the hours are unknown or unreadable. */
  openNow?: boolean;
  /** While open: closing time "HH:MM". */
  closesAt?: string;
  /** While closed: next opening, "HH:MM" today or "<day> HH:MM" ("א׳ 08:00") later. */
  opensAt?: string;
  /** Kosher (OSM diet:kosher). */
  kosher?: boolean;
  /** The asked house number does not exist; placed at this existing number ("מיקום משוער (ליד מס׳ N)"). */
  nearHouseNumber?: string;
  /** How the query was read when not literally (first result only): "מציג תוצאות עבור: X". */
  interpretation?: string;
  /** Hebrew name of a POI category ("בית כנסת") when `category` is the raw id (map layer, nearby) */
  categoryLabel?: string;
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
  /** Typical delay of this line at the boarding stop in minutes (median of recent rides; Open Bus Stride, online only). */
  delayP50Min?: number;
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
  /** Estimated Rav-Kav "ערך צבור" fare in ₪ (transit routes; estimate, no passes/discounts). */
  fareEstimateIls?: number;
  /** How the fare is made up, e.g. "55 + 490 · 14.5 ₪ (כולל מעבר)"; "; " between tickets. */
  fareNote?: string;
  /** Avoid options the local route could not honour: "toll" | "motorway" | "unpaved" | "ferry" | "areaA" | "areaB". */
  avoidViolations?: string[];
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
export type DataComponent = "tiles" | "world" | "satellite" | "terrain" | "routing" | "geocode" | "transit" | "trails" | "minyanim";
export const DATA_COMPONENTS: DataComponent[] = ["tiles", "world", "satellite", "terrain", "routing", "geocode", "transit", "trails", "minyanim"];

/** One prayer of a synagogue's weekly timetable (Nedarim Plus). */
export interface Minyan {
  prayer: string;
  /** "HH:MM" */
  time: string;
  /** day letters Sunday..Shabbat, e.g. "אבגדהו", "ש" */
  days: string;
  hall?: string;
  note?: string;
}
/** minyanim_for: the timetable of the synagogue at a point. Show `source` as credit. */
export interface MinyanTable {
  mosadId: string;
  name?: string | null;
  address?: string | null;
  lon: number;
  lat: number;
  distanceM: number;
  minyanim: Minyan[];
  message?: string | null;
  source: string;
  /** YYYY-MM-DD of the snapshot, or "YYYY-MM-DD HH:MM" when live */
  fetched: string;
  /** fetched from Nedarim Plus just now (online) instead of the weekly snapshot */
  live?: boolean;
}
/** minyanim_next: a prayer starting soon nearby. */
export interface NextMinyan {
  mosadId: string;
  name?: string | null;
  address?: string | null;
  lon: number;
  lat: number;
  distanceM: number;
  prayer: string;
  time: string;
  inMinutes: number;
  hall?: string | null;
  note?: string | null;
}

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
  trails: UpdateCheck;
  minyanim: UpdateCheck;
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
  trails: ComponentStatus;
  minyanim: ComponentStatus;
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
  /** Look for a newer program version at every start. */
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
  /** Take ready-made components from the weekly GitLab build when possible. */
  usePrebuiltData: boolean;
  /** Show typical line delays from Open Bus Stride (online only). */
  transitReliability: boolean;
  /** live navigation speaks the instructions */
  navVoice: boolean;
}

/** One live-navigation fix (event "nav-update"). Distances metres, times seconds. */
export interface NavUpdate {
  routeId: string;
  lon: number;
  lat: number;
  snappedLon: number;
  snappedLat: number;
  /** route bearing at the snapped point, degrees from north (heading-up camera) */
  bearing: number;
  stepIndex: number;
  maneuver: Maneuver;
  distToManeuverM: number;
  instruction: string;
  street?: string | null;
  nextInstruction?: string | null;
  nextManeuver?: Maneuver | null;
  remainingM: number;
  remainingSec: number;
  etaIso: string;
  /** leg the vehicle is on: waypoints after it are still ahead */
  legIndex: number;
  offRoute: boolean;
  /** once per deviation: compute a new route from here */
  rerouteNeeded: boolean;
  arrived: boolean;
  speech?: string | null;
}

export interface AppInfo {
  version: string;
  bbox: [number, number, number, number];
}

// ------------------------------------------------------------- error helpers
/** What a failed command rejects with. Current Rust commands reject with this object
 *  (`AppError` in src-tauri); older ones reject with a string such as "NO_TRANSIT_DATA: …",
 *  which `toApiError` turns into the same shape. */
export interface ApiError {
  /** UPPER_SNAKE code ("NO_TRANSIT_DATA", "NETWORK", "INTERNAL", …) */
  code: string;
  /** developer text (English, may be long) */
  message: string;
  /** extra detail, e.g. the blocked host, the path, the anyhow cause chain */
  details?: string;
}

/** Codes the UI knows by name (others still work through `code`). */
export const ERROR_CODES = [
  "NO_ROUTING_DATA_REGION",
  "NO_ROUTING_DATA",
  "NO_TRANSIT_DATA",
  "NO_GEOCODE_DATA",
  "NO_TERRAIN_DATA",
  "NO_MINYANIM_DATA",
  "NO_ROUTE_AVOID_AREAS",
  "ONLINE_FAILED",
  "UPDATE_RUNNING",
  "NEED_TWO_WAYPOINTS",
  "LOCATION_DENIED",
  "LOCATION_UNAVAILABLE",
  "SAVE_FAILED",
  "NOT_IMPLEMENTED",
  "NETWORK",
  "DISK_FULL",
  "PERMISSION",
  "BLOCKED_HOST",
  "INTERNAL",
  "INTERNAL_PANIC",
  "UNKNOWN_REGION",
  "TOO_MANY_WAYPOINTS",
  "TOO_MANY_POINTS",
  "ORDER_NOT_FOR_TRANSIT",
  "NO_ROAD_NEAR_WAYPOINT",
  "NO_ROUTE_TRUCK",
  "NO_TRANSIT_SERVICE_ON_DATE",
  "NO_SERVICE",
  "LINE_NOT_FOUND",
  "TRANSIT_DATA_CORRUPT",
  "NAV_NOT_RUNNING",
  "REGION_BUSY",
  "UPDATE_FILE_INVALID",
  "INVALID_DATA_DIR",
  "INVALID_PATH",
  "CHECKSUM_MISMATCH",
] as const;
export type ErrorCode = (typeof ERROR_CODES)[number];

const LEADING_CODE = /^([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+|[A-Z]{4,})(?=$|[\s:\-–])(?:\s*[:\-–]\s*)?([\s\S]*)$/;
const SINGLE_WORD_CODES = new Set<string>(ERROR_CODES.filter((c) => !c.includes("_")));

/** Any rejection (the new object, a legacy string, an Error) as an ApiError. */
export function toApiError(e: unknown): ApiError {
  if (e && typeof e === "object" && !(e instanceof Error) && typeof (e as { code?: unknown }).code === "string") {
    const o = e as { code: string; message?: unknown; details?: unknown };
    return {
      code: o.code,
      message: typeof o.message === "string" ? o.message : String(o.message ?? o.code),
      ...(o.details != null && o.details !== "" ? { details: String(o.details) } : {}),
    };
  }
  const text = errorText(e);
  const m = LEADING_CODE.exec(text.trim());
  // a one-word code only when it is a known one ("OSRM request failed" is not a code)
  if (m && (m[1].includes("_") || SINGLE_WORD_CODES.has(m[1]))) return { code: m[1], message: text, ...(m[2] ? { details: m[2].trim() } : {}) };
  // legacy strings that carry a code somewhere inside
  for (const c of ERROR_CODES) if (c.length > 6 && text.includes(c)) return { code: c, message: text };
  if (/not implemented/i.test(text)) return { code: "NOT_IMPLEMENTED", message: text };
  return { code: "INTERNAL", message: text };
}

/** The error's code, or null for an unclassified failure. */
export function errorCode(e: unknown): string | null {
  const c = toApiError(e).code;
  return c === "INTERNAL" && !(e && typeof e === "object" && (e as { code?: unknown }).code === "INTERNAL") ? null : c;
}

/** Developer text of any rejection (for logs and "details"). */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e) {
    const o = e as { message: unknown; details?: unknown };
    return o.details ? `${String(o.message)} (${String(o.details)})` : String(o.message);
  }
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

/** fastest = earliest arrival ("arrive by": latest departure); shortest = the shortest trip. */
export type TransitPreference = "fastest" | "shortest" | "fewestTransfers" | "leastWalking";

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
  // longName ("Eilat <-> Eilat") tells apart lines of one agency that share a number
  transitLine: (shortName: string, agency: string, headsign?: string, longName?: string) => invoke<LineShape>("transit_line", { shortName, agency, headsign: headsign ?? null, longName: longName ?? null }),
  /** POIs of the given geocoder categories inside the viewport; `category` is the raw id.
   *  With `zoom` (the map zoom), only the POIs the build-time thinning shows at that zoom. */
  poiLayer: (cats: string[], bbox: [number, number, number, number], limit?: number, zoom?: number) =>
    invoke<GeoResult[]>("poi_layer", { cats, bbox, limit: limit ?? null, zoom: zoom ?? null }),
  transitAgencies: () => invoke<string[]>("transit_agencies"),
  transitLines: (agency: string) => invoke<LineSummary[]>("transit_lines", { agency }),
  /** Departures of a line from now to the end of today's service, both directions. */
  transitLineTimes: (shortName: string, agency: string, headsign?: string, longName?: string) => invoke<LineTimes>("transit_line_times", { shortName, agency, headsign: headsign ?? null, longName: longName ?? null }),
  /** Departure board of a stop (by code, else the nearest served stop within 40 m). */
  transitStopBoard: (code: string | undefined, lon: number, lat: number) => invoke<StopBoard>("transit_stop_board", { code: code ?? null, lon, lat }),
  /** Typical delay (min) per boarding from Open Bus Stride; slow (10-15 s), online only – call after showing routes,
   *  one entry per transit leg: {shortName, agency, lon, lat (boarding stop), departIso}. Credit: "נתוני דיוק: Open Bus Stride, הסדנא לידע ציבורי". */
  transitDelays: (legs: { shortName: string; agency: string; lon: number; lat: number; departIso: string }[]) =>
    invoke<(number | null)[]>("transit_delays", { legs }),
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
  /** Marked trails GeoJSON (load with convertFileSrc as a geojson source), null if not installed. */
  trailsPath: () => invoke<string | null>("trails_path"),
  /** Weekly prayer timetable of the synagogue at a point; null when none matches. Rejects with NO_MINYANIM_DATA when not installed. */
  minyanimFor: (lon: number, lat: number, poiName?: string) => invoke<MinyanTable | null>("minyanim_for", { lon, lat, poiName: poiName ?? null }),
  /** Prayers starting within `withinMin` minutes (default 180) inside `radiusM` metres, soonest first. */
  minyanimNext: (lon: number, lat: number, radiusM: number, withinMin?: number) => invoke<NextMinyan[]>("minyanim_next", { lon, lat, radiusM, withinMin: withinMin ?? null }),
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
  /** Download the full ready-made data package (weekly GitLab build) to `dest`; returns its size. Progress: event `package-download`. */
  dataDownloadPackage: (dest: string) => invoke<number>("data_download_package", { dest }),
  dataPreload: (transit: boolean) => invoke<void>("data_preload", { transit }),

  settingsGet: () => invoke<Settings>("settings_get"),
  settingsSet: (settings: Settings) => invoke<Settings>("settings_set", { settings }),
  appInfo: () => invoke<AppInfo>("app_info"),

  // live navigation (src-tauri/src/nav): events "nav-update" | "nav-error" | "nav-ended"
  navStart: (route: Route) => invoke<void>("nav_start", { route }),
  navSimulate: (route: Route, speedKmh?: number) => invoke<void>("nav_simulate", { route, speedKmh: speedKmh ?? null }),
  navSetRoute: (route: Route) => invoke<void>("nav_set_route", { route }),
  navSetVoice: (on: boolean) => invoke<void>("nav_set_voice", { on }),
  navStop: () => invoke<void>("nav_stop"),
  speak: (text: string) => invoke<void>("speak", { text }),
  /** [displayName, language][], the voice used first */
  voicesList: () => invoke<[string, string][]>("voices_list"),
  onNavUpdate: (cb: (u: NavUpdate) => void): Promise<UnlistenFn> => listen<NavUpdate>("nav-update", (e) => cb(e.payload)),
  onNavError: (cb: (msg: string) => void): Promise<UnlistenFn> => listen<string>("nav-error", (e) => cb(e.payload)),
  onNavEnded: (cb: () => void): Promise<UnlistenFn> => listen("nav-ended", () => cb()),

  onDataProgress: (cb: (p: DataProgress) => void): Promise<UnlistenFn> =>
    listen<DataProgress>("data-progress", (e) => cb(e.payload)),
  onDataUpdated: (cb: () => void): Promise<UnlistenFn> => listen("data-updated", () => cb()),
};

// ===== [F1] =====

// ===== [F2] =====

// ===== [R1] =====

// ===== [R2] =====

// ===== [R3] =====

// ===== [R4] =====
