//! IPC contract shared between the Rust backend and the React frontend.
//! Every struct here is mirrored 1:1 in `src/lib/api.ts`. Keep `rename_all = "camelCase"`.
//! Coordinates are `[lon, lat]`. Distances in meters, durations in seconds, times ISO-8601 with offset.

use serde::{Deserialize, Serialize};

pub type LonLat = [f64; 2];

// ---------------------------------------------------------------- geocoding

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeoKind {
    Address,
    Street,
    Place,
    Poi,
    Stop,
    Coord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeoPrecision {
    Exact,
    Interpolated,
    Street,
    City,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoResult {
    pub id: String,
    pub name: String,
    pub kind: GeoKind,
    pub precision: GeoPrecision,
    pub lon: f64,
    pub lat: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub house_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Ranking score (higher is better) – informative only.
    #[serde(default)]
    pub score: f64,
}

// ---------------------------------------------------------------- routing

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Car,
    Foot,
    Bike,
    Transit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeKind {
    Now,
    Depart,
    Arrive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeSpec {
    pub kind: TimeKind,
    /// ISO-8601 with offset, required for Depart/Arrive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iso: Option<String>,
}

impl Default for TimeSpec {
    fn default() -> Self {
        Self { kind: TimeKind::Now, iso: None }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Avoid {
    #[serde(default)]
    pub toll: bool,
    #[serde(default)]
    pub motorway: bool,
    #[serde(default)]
    pub unpaved: bool,
    #[serde(default)]
    pub ferry: bool,
    /// Never enter Oslo Area A (incl. Hebron H1 and the Gaza Strip).
    #[serde(default)]
    pub area_a: bool,
    /// Never enter Oslo Area B.
    #[serde(default)]
    pub area_b: bool,
    /// With `unpaved`: only long unpaved stretches (> ~300 m between junctions) are avoided,
    /// short ones (a dirt access road) stay cheap.
    #[serde(default)]
    pub unpaved_long_only: bool,
    /// Avoid difficult junctions: left turns and U-turns at junctions without traffic signals.
    #[serde(default)]
    pub hard_turns: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Waypoint {
    pub lon: f64,
    pub lat: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRequest {
    /// At least 2 waypoints: origin, [via...], destination.
    pub waypoints: Vec<Waypoint>,
    pub mode: Mode,
    #[serde(default)]
    pub time: TimeSpec,
    #[serde(default)]
    pub avoid: Avoid,
    /// Max number of routes to return (>=1).
    #[serde(default = "default_alternatives")]
    pub alternatives: u32,
}

fn default_alternatives() -> u32 {
    3
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Maneuver {
    Depart,
    TurnLeft,
    TurnRight,
    SlightLeft,
    SlightRight,
    SharpLeft,
    SharpRight,
    Straight,
    Uturn,
    Roundabout,
    ExitRoundabout,
    Arrive,
    Merge,
    Fork,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// Human readable Hebrew instruction, e.g. "פנה ימינה לרחוב הרצל".
    pub instruction: String,
    pub maneuver: Maneuver,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roundabout_exit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    pub distance_m: f64,
    pub duration_sec: f64,
    /// Index into the leg geometry where this step starts.
    pub geometry_start: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegKind {
    Walk,
    Drive,
    Bike,
    Transit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitVehicle {
    Bus,
    Rail,
    LightRail,
    Tram,
    Subway,
    Ferry,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitStopTime {
    pub name: String,
    /// Stop code as printed on the sign (GTFS stop_code), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub lon: f64,
    pub lat: f64,
    pub time_iso: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitLegInfo {
    pub vehicle: TransitVehicle,
    pub route_short_name: String,
    pub route_long_name: String,
    pub agency: String,
    pub headsign: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub from_stop: String,
    pub to_stop: String,
    pub depart_iso: String,
    pub arrive_iso: String,
    pub num_stops: u32,
    pub stops: Vec<TransitStopTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Leg {
    pub kind: LegKind,
    pub duration_sec: f64,
    pub distance_m: f64,
    pub geometry: Vec<LonLat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depart_iso: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrive_iso: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transit: Option<TransitLegInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteTag {
    Fastest,
    Shortest,
    FewestTransfers,
    LeastWalking,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub id: String,
    pub mode: Mode,
    pub duration_sec: f64,
    pub distance_m: f64,
    /// Full concatenated geometry of all legs.
    pub geometry: Vec<LonLat>,
    pub legs: Vec<Leg>,
    #[serde(default)]
    pub tags: Vec<RouteTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depart_iso: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrive_iso: Option<String>,
    /// Number of transit legs minus one (transit only).
    #[serde(default)]
    pub transfers: u32,
    /// Total walking meters (transit only).
    #[serde(default)]
    pub walk_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteSource {
    Online,
    #[default]
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteResponse {
    pub routes: Vec<Route>,
    /// Milliseconds spent computing (diagnostics).
    pub compute_ms: u64,
    /// Where the result came from (online provider or the local engines).
    #[serde(default)]
    pub source: RouteSource,
    /// Provider name for online results, e.g. "osrm" / "transitous".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl RouteResponse {
    pub fn offline(routes: Vec<Route>, compute_ms: u64) -> Self {
        Self { routes, compute_ms, source: RouteSource::Offline, provider: None }
    }
}

/// A transit stop for the map layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopInfo {
    pub id: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub lon: f64,
    pub lat: f64,
}

// ---------------------------------------------------------------- history / favorites

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    Search,
    Route,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: i64,
    pub kind: HistoryKind,
    pub created_iso: String,
    /// Free text label shown in the list (query text or "A → B").
    pub label: String,
    /// JSON payload: GeoResult for search, RouteRequest (+ summary) for route.
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub id: i64,
    pub name: String,
    /// lucide icon name, e.g. "home", "briefcase", "star".
    pub icon: String,
    pub lon: f64,
    pub lat: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub created_iso: String,
    /// "address" | "stop" | "place" | "line" (rows from before this field are addresses).
    #[serde(default = "default_fav_kind")]
    pub kind: String,
    /// Kind-specific extras: stop code; for lines routeShortName / agency / headsign / color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

fn default_fav_kind() -> String {
    "address".to_string()
}

/// Result of the in-app update check (see `app_update.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdate {
    pub current: String,
    pub latest: String,
    pub available: bool,
    pub page_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable_url: Option<String>,
    /// Release notes (Markdown as published).
    pub notes: String,
    pub checked_iso: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MyLocation {
    pub lon: f64,
    pub lat: f64,
    pub accuracy_m: f64,
}

/// A line of one agency (favorites picker).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineSummary {
    pub short_name: String,
    pub long_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub headsigns: Vec<String>,
}

/// Departures of one direction of a line for the rest of the service day:
/// `(seconds from now, ISO time)`, sorted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineDirection {
    pub headsign: String,
    pub from_stop: String,
    pub to_stop: String,
    pub duration_sec: u32,
    pub departures: Vec<(i32, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineTimes {
    pub short_name: String,
    pub long_name: String,
    pub agency: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub directions: Vec<LineDirection>,
}

/// One line on a stop's departure board; `next` = `(seconds from now, ISO time)` of the next
/// few departures (schedule-based: there is no real-time data offline).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopLine {
    pub short_name: String,
    pub agency: String,
    pub headsign: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The stop is this line's last stop (vehicles arrive, they do not take passengers on).
    pub terminus: bool,
    pub total_today: u32,
    pub next: Vec<(i32, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopBoard {
    pub stop: StopInfo,
    pub lines: Vec<StopLine>,
    pub generated_iso: String,
    /// 0 = Sunday
    pub weekday: u32,
}

/// One direction of a transit line: its longest stop pattern and shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineShape {
    pub short_name: String,
    pub long_name: String,
    pub agency: String,
    pub headsign: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub stops: Vec<StopInfo>,
    pub geometry: Vec<LonLat>,
}

// ---------------------------------------------------------------- data management

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataComponent {
    Tiles,
    Satellite,
    Terrain,
    Routing,
    Geocode,
    Transit,
}

impl DataComponent {
    pub const ALL: [DataComponent; 6] = [
        DataComponent::Tiles,
        DataComponent::Satellite,
        DataComponent::Terrain,
        DataComponent::Routing,
        DataComponent::Geocode,
        DataComponent::Transit,
    ];
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus {
    pub present: bool,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_iso: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether the component is currently loaded in memory.
    #[serde(default)]
    pub loaded: bool,
}

/// Result of asking the servers whether a newer version of one component exists.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    /// A newer (or missing) version can be installed.
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_version: Option<String>,
    /// Why the check could not be completed (offline, blocked, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataCheck {
    pub checked_iso: String,
    pub tiles: UpdateCheck,
    pub satellite: UpdateCheck,
    pub terrain: UpdateCheck,
    pub routing: UpdateCheck,
    pub geocode: UpdateCheck,
    pub transit: UpdateCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataStatus {
    pub dir: String,
    pub tiles: ComponentStatus,
    pub satellite: ComponentStatus,
    pub terrain: ComponentStatus,
    pub routing: ComponentStatus,
    pub geocode: ComponentStatus,
    pub transit: ComponentStatus,
    pub updating: bool,
    /// The running update is paused (resumes with `data_pause(false)`).
    #[serde(default)]
    pub paused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressPhase {
    Queued,
    Downloading,
    Processing,
    Finalizing,
    Done,
    Failed,
    Cancelled,
}

/// Emitted as tauri event `data-progress`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataProgress {
    pub component: DataComponent,
    pub phase: ProgressPhase,
    pub done: u64,
    pub total: u64,
    pub message: String,
}

// ---------------------------------------------------------------- settings

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "default_lang")]
    pub language: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_units")]
    pub units: String,
    #[serde(default)]
    pub data_dir: Option<String>,
    #[serde(default = "default_true")]
    pub show_transit_layer: bool,
    #[serde(default = "default_true")]
    pub show_poi_layer: bool,
    #[serde(default)]
    pub map_label_language: Option<String>,
    /// "auto" (online first, offline fallback) | "online" | "offline".
    #[serde(default = "default_routing_mode")]
    pub routing_mode: String,
    /// Base URL of an OSRM server family (FOSSGIS layout: {base}/routed-car|bike|foot).
    #[serde(default = "default_osrm_url")]
    pub osrm_url: String,
    /// Base URL of a MOTIS / Transitous API server.
    #[serde(default = "default_transit_api_url")]
    pub transit_api_url: String,
    /// Default for the route panel: avoid toll roads.
    #[serde(default = "default_true")]
    pub avoid_tolls: bool,
    /// Look for a newer program version at startup (once a day).
    #[serde(default = "default_true")]
    pub auto_check_app_updates: bool,
    /// Default for the route panel: "none" | "a" (Area A only) | "ab" (Areas A and B).
    #[serde(default = "default_avoid_areas")]
    pub avoid_areas: String,
    /// Default for the route panel: avoid motorways.
    #[serde(default)]
    pub avoid_motorways: bool,
    /// Default for the route panel: avoid ferries.
    #[serde(default)]
    pub avoid_ferries: bool,
    /// Default for the route panel: "allow" | "avoidLong" | "avoid" (unpaved roads).
    #[serde(default = "default_unpaved")]
    pub unpaved: String,
    /// Default for the route panel: avoid difficult junctions (unsignalised left / U-turns).
    #[serde(default)]
    pub avoid_hard_turns: bool,
}

pub fn default_avoid_areas() -> String {
    "ab".into()
}
pub fn default_unpaved() -> String {
    "avoidLong".into()
}
pub fn default_routing_mode() -> String {
    "auto".into()
}
pub fn default_osrm_url() -> String {
    "https://routing.openstreetmap.de".into()
}
pub fn default_transit_api_url() -> String {
    "https://api.transitous.org".into()
}

fn default_lang() -> String {
    "he".into()
}
fn default_theme() -> String {
    "auto".into()
}
fn default_units() -> String {
    "metric".into()
}
fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: default_lang(),
            theme: default_theme(),
            units: default_units(),
            data_dir: None,
            show_transit_layer: true,
            show_poi_layer: true,
            map_label_language: None,
            routing_mode: default_routing_mode(),
            osrm_url: default_osrm_url(),
            transit_api_url: default_transit_api_url(),
            auto_check_app_updates: true,
            avoid_tolls: true,
            avoid_areas: default_avoid_areas(),
            avoid_motorways: false,
            avoid_ferries: false,
            unpaved: default_unpaved(),
            avoid_hard_turns: false,
        }
    }
}

/// Progress callback used by long running builders (data pipeline).
/// Progress callback. Returns `false` when the job was cancelled: builders stop at their next
/// checkpoint. The callback also blocks while the update is paused.
pub type ProgressFn<'a> = &'a (dyn Fn(ProgressPhase, u64, u64, &str) -> bool + Sync);
