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
    /// Optional details (opening hours, nearest house number, how the query was read), flattened
    /// into the same JSON object; `None` for most results.
    #[serde(flatten, default)]
    pub extra: Option<Box<GeoExtra>>,
}

/// Optional `GeoResult` fields (camelCase in JSON, each omitted when absent).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoExtra {
    /// Raw OSM `opening_hours` of a point of interest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opening_hours: Option<String>,
    /// Open right now (Asia/Jerusalem); absent when the hours are unknown or could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_now: Option<bool>,
    /// "HH:MM" today when it closes (open now) …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closes_at: Option<String>,
    /// … or the next opening ("HH:MM", or "Mon HH:MM"-style ISO date-time when not today).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opens_at: Option<String>,
    /// Kosher (OSM `diet:kosher` yes/only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kosher: Option<bool>,
    /// The asked house number does not exist: the result is placed at this existing number
    /// (same side of the street when possible). UI: "מיקום משוער (ליד מס׳ N)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near_house_number: Option<String>,
    /// How the query was understood when it was not taken literally (keyboard layout fixed,
    /// typo corrected, category + place): set on the first result only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpretation: Option<String>,
    /// Hebrew name of a POI's category ("בית כנסת") where `category` carries the raw id (map layer,
    /// nearby list).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_label: Option<String>,
}

impl GeoExtra {
    pub fn is_empty(&self) -> bool {
        *self == GeoExtra::default()
    }
}

// ---------------------------------------------------------------- routing

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Car,
    Foot,
    Bike,
    Transit,
    /// Heavy goods vehicle: car network minus hgv=no, with the request's `vehicle` dimensions
    /// checked against maxheight / maxweight / maxwidth (local engine only).
    Truck,
    /// Walking for wheelchair users: no steps, no wheelchair=no, steep / unpaved ways avoided
    /// (local engine only).
    Wheelchair,
}

impl Mode {
    /// car-like modes (vehicle network, turn costs, restrictions)
    pub fn is_vehicle(self) -> bool {
        matches!(self, Mode::Car | Mode::Truck)
    }
    /// pedestrian-like modes (foot network; transit access legs)
    pub fn is_pedestrian(self) -> bool {
        matches!(self, Mode::Foot | Mode::Wheelchair | Mode::Transit)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Car => "car",
            Mode::Foot => "foot",
            Mode::Bike => "bike",
            Mode::Transit => "transit",
            Mode::Truck => "truck",
            Mode::Wheelchair => "wheelchair",
        }
    }
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
    /// Avoid the road nearest each point ("avoid this road" on a route step), local engine only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<LonLat>,
    /// Areas drawn by the user (lon/lat rings, closed or open): never entered, local engine only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub polygons: Vec<Vec<LonLat>>,
    /// bike / foot / wheelchair: prefer flatter ways (needs the DEM sampled into graph v7).
    #[serde(default)]
    pub hills: bool,
}

impl Avoid {
    /// Options only the local engine can honour (online routes ignore them).
    pub fn needs_local(&self) -> bool {
        !self.points.is_empty() || !self.polygons.is_empty() || self.hills
    }
}

/// Truck dimensions checked against maxheight / maxweight / maxwidth.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleDims {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_t: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_m: Option<f64>,
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
    /// Truck dimensions (mode "truck").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<VehicleDims>,
    /// Transit: wheelchair-accessible trips and stops only.
    #[serde(default)]
    pub accessible: bool,
}

fn default_alternatives() -> u32 {
    3
}

impl Default for RouteRequest {
    fn default() -> Self {
        Self { waypoints: Vec::new(), mode: Mode::Car, time: TimeSpec::default(), avoid: Avoid::default(), alternatives: default_alternatives(), vehicle: None, accessible: false }
    }
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
    /// Road number of the street of this step ("1", "443").
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub road_ref: Option<String>,
    /// Motorway exit number (junction:ref) taken at this maneuver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<String>,
    /// Signposted destination of the way taken ("ירושלים").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    /// Posted speed limit at the start of this step (km/h), when tagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_limit_kmh: Option<u32>,
    /// Lanes on the approach to this maneuver, left to right.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<Lane>,
}

/// One lane before a maneuver (OSM turn:lanes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lane {
    /// turn:lanes values of this lane: "left", "through", "right", "slight_right", "none", ...
    pub directions: Vec<String>,
    /// this lane leads along the route
    pub active: bool,
}

impl Step {
    /// A step with only the basic fields set (the R2 extras empty).
    pub fn basic(instruction: String, maneuver: Maneuver, roundabout_exit: Option<u32>, street: Option<String>, distance_m: f64, duration_sec: f64, geometry_start: usize) -> Self {
        Step { instruction, maneuver, roundabout_exit, street, distance_m, duration_sec, geometry_start, road_ref: None, exit: None, destination: None, speed_limit_kmh: None, lanes: Vec::new() }
    }
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
    /// Typical delay of this line at the boarding stop (minutes, median of recent rides), from
    /// Open Bus Stride when online and `settings.transitReliability`; None otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_p50_min: Option<f64>,
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
    /// Estimated Rav-Kav "ערך צבור" fare in shekels (transit only; `transit::fares`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fare_estimate_ils: Option<f64>,
    /// How the fare was made up, e.g. "55 + 490 · 14.5 ₪ (כולל מעבר)" (transit only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fare_note: Option<String>,
    /// Avoid options the route could not honour ("toll", "motorway", "unpaved", "ferry",
    /// "areaA", "areaB"); local engine only, empty for online routes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub avoid_violations: Vec<String>,
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
    World,
    Satellite,
    Terrain,
    Routing,
    Geocode,
    Transit,
    /// Marked hiking / cycling trails (GeoJSON from the OSM PBF)
    Trails,
    /// Synagogue prayer times (Nedarim Plus)
    Minyanim,
}

impl DataComponent {
    pub const ALL: [DataComponent; 9] = [
        DataComponent::Tiles,
        DataComponent::World,
        DataComponent::Satellite,
        DataComponent::Terrain,
        DataComponent::Routing,
        DataComponent::Geocode,
        DataComponent::Transit,
        DataComponent::Trails,
        DataComponent::Minyanim,
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
    pub world: UpdateCheck,
    pub satellite: UpdateCheck,
    pub terrain: UpdateCheck,
    pub routing: UpdateCheck,
    pub geocode: UpdateCheck,
    pub transit: UpdateCheck,
    #[serde(default)]
    pub trails: UpdateCheck,
    #[serde(default)]
    pub minyanim: UpdateCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataStatus {
    pub dir: String,
    pub tiles: ComponentStatus,
    pub world: ComponentStatus,
    pub satellite: ComponentStatus,
    pub terrain: ComponentStatus,
    pub routing: ComponentStatus,
    pub geocode: ComponentStatus,
    pub transit: ComponentStatus,
    #[serde(default)]
    pub trails: ComponentStatus,
    #[serde(default)]
    pub minyanim: ComponentStatus,
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
    /// Imagery detail. Sentinel-2: "low" z12, "medium" z13, "high"/"max" z14 (the sensor's 10 m).
    /// govmap orthophoto: "low" z14, "medium" z15, "high" z16, "max" z17.
    #[serde(default = "default_sat_quality")]
    pub satellite_quality: String,
    /// Imagery source: "govmap" (Survey of Israel aerial orthophoto, ~0.13 m) | "sentinel" (Sentinel-2, 10 m).
    #[serde(default = "default_sat_source")]
    pub satellite_source: String,
    /// Open the map where it was last left.
    #[serde(default = "default_true")]
    pub remember_map_view: bool,
    /// Accent colour of the light theme: "blue" | "green" | "purple" | "orange" | "teal" | "rose" | "slate".
    #[serde(default = "default_accent")]
    pub accent: String,
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
    #[serde(default = "default_true")]
    pub avoid_hard_turns: bool,
    /// Take ready-made components from the project's weekly GitLab build when possible.
    #[serde(default = "default_true")]
    pub use_prebuilt_data: bool,
    /// Show how late a line usually is at the boarding stop (Open Bus Stride, online only).
    #[serde(default = "default_true")]
    pub transit_reliability: bool,
    /// Live navigation speaks the instructions (Windows Hebrew voice when installed).
    #[serde(default = "default_true")]
    pub nav_voice: bool,
    /// Order of public-transport results: "fastest" | "fewestTransfers" | "leastWalking".
    #[serde(default = "default_transit_pref")]
    pub transit_preference: String,
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
fn default_accent() -> String {
    "blue".into()
}

fn default_sat_quality() -> String {
    "high".into()
}

fn default_transit_pref() -> String {
    "fastest".into()
}

fn default_sat_source() -> String {
    "govmap".into()
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
            satellite_quality: "high".into(),
            satellite_source: default_sat_source(),
            remember_map_view: true,
            accent: "blue".into(),
            avoid_tolls: true,
            avoid_areas: default_avoid_areas(),
            avoid_motorways: false,
            avoid_ferries: false,
            unpaved: default_unpaved(),
            avoid_hard_turns: true,
            transit_preference: default_transit_pref(),
            use_prebuilt_data: true,
            nav_voice: true,
            transit_reliability: true,
        }
    }
}

/// Progress callback used by long running builders (data pipeline).
/// Progress callback. Returns `false` when the job was cancelled: builders stop at their next
/// checkpoint. The callback also blocks while the update is paused.
pub type ProgressFn<'a> = &'a (dyn Fn(ProgressPhase, u64, u64, &str) -> bool + Sync);

// ---------------------------------------------------------------- other countries (regions)

/// One downloadable region of the Geofabrik catalogue.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionEntry {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    /// ISO 3166-1 alpha-2 when the region is exactly one country
    pub iso: Option<String>,
    pub bbox: [f64; 4],
    /// Israel: covered by the built-in components
    pub builtin: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionPartStatus {
    pub present: bool,
    pub size_bytes: u64,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledRegion {
    pub id: String,
    pub name: String,
    pub iso: Option<String>,
    pub bbox: [f64; 4],
    pub map: RegionPartStatus,
    pub search: RegionPartStatus,
    pub routing: RegionPartStatus,
    pub updated_iso: Option<String>,
    pub busy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegionPart {
    Map,
    Search,
    Routing,
}

/// Emitted as tauri event `region-progress`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionProgress {
    pub id: String,
    pub part: RegionPart,
    pub phase: ProgressPhase,
    pub done: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionSize {
    pub pbf_bytes: Option<u64>,
}

/// A place found by name (any language) and the downloadable regions around it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionLocate {
    /// the place's name as the search service gives it ("ניו יורק")
    pub name: String,
    /// the rest of its address ("ארצות הברית")
    pub detail: String,
    pub lon: f64,
    pub lat: f64,
    /// region ids that contain the place, smallest first (state → country → continent)
    pub regions: Vec<String>,
}

// ===== [R1] data / infra / security =====

// ===== [R2] routing + navigation =====

/// One reachability band of an isochrone: lon/lat rings (outer rings counter-clockwise, holes
/// clockwise; each hole follows its outer ring).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsochroneBand {
    pub minutes: f64,
    pub rings: Vec<Vec<LonLat>>,
}

/// Per-stop options of a multi-stop trip (aligned with the request's waypoints).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TripStop {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stay_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_start_iso: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_end_iso: Option<String>,
}

/// Visiting order (indices into the waypoints) and the arrival time at each of them, in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TripPlan {
    pub order: Vec<usize>,
    pub arrivals: Vec<String>,
}

/// Progress of a track recording (event "track-update").
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackUpdate {
    /// lon, lat, elevation (m, when the DEM is installed)
    pub points: Vec<(f64, f64, Option<f32>)>,
    pub distance_m: f64,
    pub duration_sec: f64,
    pub ascent_m: f64,
}

/// A saved user layer (same shape as R1's `UserLayer`; the track recorder returns it).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackLayer {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub color: String,
    pub visible: bool,
    pub created_iso: String,
    pub features: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
}

// ===== [R3] transit =====

// ===== [R4] geocode =====
