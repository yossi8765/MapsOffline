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
    // ---- R4: more POI details (OSM tags, data.gov.il registers)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website: Option<String>,
    /// Street address of a POI ("הרצל 12, תל אביב-יפו").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// OSM `wikipedia` tag ("he:Title").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wikipedia: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wikidata: Option<String>,
    /// One line of kind-specific details (charging stations: "4 עמדות · Type 2, CCS").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_info: Option<String>,
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
    /// As few traffic lights as possible: every signalised junction passed costs extra in the
    /// search (not in the ETA). Local engine only (OSRM knows nothing of it).
    #[serde(default)]
    pub traffic_lights: bool,
    /// Avoid the road nearest each point ("avoid this road" on a route step), local engine only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<LonLat>,
    /// Areas drawn by the user (lon/lat rings, closed or open): never entered, local engine only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub polygons: Vec<Vec<LonLat>>,
    /// bike / foot / wheelchair: prefer flatter ways (needs the DEM sampled into graph v7).
    #[serde(default)]
    pub hills: bool,
    /// Route for kohanim: 0 = off, 1 = basic, 2 = Tahara of Kohanim standard, 3 = strict
    /// (data::kohanim); the route command turns it into `polygons`. Local engine only.
    #[serde(default)]
    pub kohanim: u8,
}

impl Avoid {
    /// Options only the local engine can honour (online routes ignore them).
    pub fn needs_local(&self) -> bool {
        !self.points.is_empty() || !self.polygons.is_empty() || self.hills || self.kohanim > 0 || self.traffic_lights
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
    /// Transit: wheelchair-accessible trips and stops only (when the feed says so).
    #[serde(default)]
    pub accessible: bool,
    #[serde(default)]
    pub avoid: Avoid,
    /// Max number of routes to return (>=1).
    #[serde(default = "default_alternatives")]
    pub alternatives: u32,
    /// Truck dimensions (mode "truck").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<VehicleDims>,
    /// Transit: trains (Israel Railways and other heavy rail) as usual, required, or never.
    #[serde(default)]
    pub rail: RailPref,
}

/// Transit: what to do with trains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RailPref {
    /// The planner's best routes, with or without trains.
    #[default]
    Any,
    /// Only routes with at least one train ride, when there is such a route.
    Prefer,
    /// No train rides.
    Avoid,
}

impl Route {
    /// Has at least one train ride (heavy rail; light rail / tram is not a train here).
    pub fn has_rail(&self) -> bool {
        self.legs.iter().any(|l| l.transit.as_ref().is_some_and(|t| t.vehicle == TransitVehicle::Rail))
    }
}

fn default_alternatives() -> u32 {
    3
}

impl Default for RouteRequest {
    fn default() -> Self {
        Self { waypoints: Vec::new(), mode: Mode::Car, time: TimeSpec::default(), avoid: Avoid::default(), alternatives: default_alternatives(), vehicle: None, accessible: false, rail: RailPref::Any }
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
    /// Fare per ticket (transit only; `transit::fares`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fare_breakdown: Vec<FareTicket>,
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
    /// Transit: why there is no (or little) service at the requested time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<ServiceNotice>,
}

impl RouteResponse {
    pub fn offline(routes: Vec<Route>, compute_ms: u64) -> Self {
        Self { routes, compute_ms, source: RouteSource::Offline, provider: None, notice: None }
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
    /// Kind-specific extras: stop code; for lines routeShortName / agency / headsign / color;
    /// for routes the RouteRequest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Folder / list the favorite belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// "#rrggbb"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    /// No departures left today: the first one after the break (Shabbat, night).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_service_iso: Option<String>,
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
    /// No departures left today: the first one after the break (Shabbat, night).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_service_iso: Option<String>,
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
    /// Components being updated right now (several jobs may run side by side).
    #[serde(default)]
    pub active: Vec<DataComponent>,
    /// Components waiting for a running job that shares their source (the OSM extract).
    #[serde(default)]
    pub queued: Vec<DataComponent>,
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
    /// Default for the route panel: as few traffic lights as possible.
    #[serde(default)]
    pub avoid_traffic_lights: bool,
    /// Take ready-made components from the project's weekly GitLab build when possible.
    #[serde(default = "default_true")]
    pub use_prebuilt_data: bool,
    /// Where maps, search and routing come from, for Israel and every other country:
    /// "standard" (Protomaps + Geofabrik OpenStreetMap) | "osmand" (OsmAnd .obf files).
    #[serde(default = "default_map_source")]
    pub map_source: String,
    /// Route for kohanim by default (route panel): 0 off, 1 basic, 2 association standard, 3 strict.
    #[serde(default)]
    pub kohanim_level: u8,
    /// Show how late a line usually is at the boarding stop (Open Bus Stride, online only).
    #[serde(default = "default_true")]
    pub transit_reliability: bool,
    /// Live navigation speaks the instructions (Windows Hebrew voice when installed).
    #[serde(default = "default_true")]
    pub nav_voice: bool,
    /// The voice gets hidden niqqud on the fixed instruction words (`nav::speech`).
    #[serde(default = "default_true")]
    pub nav_voice_niqqud: bool,
    /// Navigation shows the speedometer (current speed next to the posted limit).
    #[serde(default = "default_true")]
    pub nav_speedometer: bool,
    /// Warn (red speedometer + a short tone) when driving above the posted limit.
    #[serde(default = "default_true")]
    pub nav_speed_alert: bool,
    /// Simulated drive speed in km/h; 0 = as the road allows (posted limits); None = by mode.
    #[serde(default)]
    pub nav_sim_speed_kmh: Option<f64>,
    /// Simulated walk speed in km/h (1–10); None = 5.
    #[serde(default)]
    pub nav_sim_walk_kmh: Option<f64>,
    /// How the vehicle is drawn while navigating by car: "arrow" | "car" | "suv" | "van" |
    /// "truck" | "motorbike" | "bus".
    #[serde(default = "default_nav_vehicle")]
    pub nav_vehicle: String,
    /// Body colour of the navigation vehicle: "accent" (the colour scheme's) | "white" | "silver" |
    /// "black" | "gray" | "blue" | "red" | "yellow".
    #[serde(default = "default_nav_vehicle_color")]
    pub nav_vehicle_color: String,
    /// Order of public-transport results: "fastest" | "fewestTransfers" | "leastWalking".
    #[serde(default = "default_transit_pref")]
    pub transit_preference: String,
    /// Trains in public-transport routes: "any" | "prefer" | "avoid" (route panel's initial value).
    #[serde(default = "default_transit_rail")]
    pub transit_rail: String,
    /// How coordinates are shown and copied: "dd" | "dms" | "itm" | "utm".
    #[serde(default = "default_coord_format")]
    pub coord_format: String,
    /// Colour scheme of the whole interface (panel, rail, cards, controls and the accent):
    /// "white" (plain, the accent colour applies) | "sky" | "mint" | "lavender" | "peach" | "rose" | "sand" | "aqua".
    #[serde(default = "default_color_scheme")]
    pub color_scheme: String,
    /// UI scale: "system" (Windows text scale, clamped 1..1.25) or 1 / 1.12 / 1.25.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: serde_json::Value,
    /// User tile layers (XYZ / WMS online, PMTiles / MBTiles files), served by `mo-user`.
    #[serde(default)]
    pub custom_layers: Vec<CustomLayer>,
    /// Areas with sharp aerial imagery downloaded on top of the country archive
    /// (`satellite/areas/<id>.pmtiles`, see `data/sat_areas.rs`).
    #[serde(default)]
    pub satellite_areas: Vec<SatelliteArea>,
    /// Components the user wants; `None` = all (update checks only offer these).
    #[serde(default)]
    pub wanted_components: Option<Vec<DataComponent>>,
    /// Teaching tips already shown (ids).
    #[serde(default)]
    pub teaching_tips_seen: Vec<String>,
}

pub fn default_nav_vehicle() -> String {
    "arrow".into()
}

pub fn default_nav_vehicle_color() -> String {
    "accent".into()
}

pub fn default_color_scheme() -> String {
    "white".into()
}

pub fn default_coord_format() -> String {
    "dd".into()
}
pub fn default_ui_scale() -> serde_json::Value {
    serde_json::Value::String("system".into())
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

fn default_transit_rail() -> String {
    "any".into()
}

fn default_transit_pref() -> String {
    "fastest".into()
}

fn default_sat_source() -> String {
    "govmap".into()
}
fn default_map_source() -> String {
    "standard".into()
}

impl Settings {
    /// Maps, search and routing come from OsmAnd files (Israel and the other countries).
    pub fn osmand_source(&self) -> bool {
        self.map_source == "osmand"
    }
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
            accent: "system".into(),
            avoid_tolls: true,
            avoid_areas: default_avoid_areas(),
            avoid_motorways: false,
            avoid_ferries: false,
            unpaved: default_unpaved(),
            avoid_hard_turns: true,
            avoid_traffic_lights: false,
            transit_preference: default_transit_pref(),
            transit_rail: default_transit_rail(),
            use_prebuilt_data: true,
            map_source: default_map_source(),
            kohanim_level: 0,
            nav_voice: true,
            nav_voice_niqqud: true,
            nav_speedometer: true,
            nav_speed_alert: true,
            nav_sim_speed_kmh: None,
            nav_sim_walk_kmh: None,
            nav_vehicle: default_nav_vehicle(),
            nav_vehicle_color: default_nav_vehicle_color(),
            transit_reliability: true,
            coord_format: default_coord_format(),
            color_scheme: default_color_scheme(),
            ui_scale: default_ui_scale(),
            custom_layers: Vec::new(),
            satellite_areas: Vec::new(),
            wanted_components: None,
            teaching_tips_seen: Vec::new(),
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
    /// None = downloaded here (Geofabrik / Protomaps), "osmand" = imported OsmAnd file
    pub source: Option<String>,
}

/// What to do when a new country covers the same area as one already installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// delete the installed one(s) once the new one is ready
    #[default]
    Replace,
    /// keep both; the smaller (more specific) area is used first, then the newer
    KeepBoth,
    /// do not install the new one
    Skip,
}

/// One entry of OsmAnd's map list (download.osmand.net).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObfCatalogEntry {
    /// file name on the server, e.g. "Burundi_africa_2.obf.zip"
    pub file: String,
    /// readable name from the file name ("Us california san-diego")
    pub name: String,
    /// continent key from the file name: africa, asia, europe, northamerica, …
    pub continent: String,
    /// first word of the name (groups the parts of a split country: "Germany", "Us")
    pub country: String,
    /// download size (zip) and unpacked size in bytes
    pub size_bytes: u64,
    pub content_bytes: u64,
    /// server date "DD.MM.YYYY"
    pub date: String,
    /// region id the import would use ("osmand/burundi_africa")
    pub region_id: String,
}

/// An installed country that covers the same area as a candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionConflict {
    pub id: String,
    pub name: String,
    pub source: Option<String>,
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

/// A user tile layer (Settings.customLayers).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomLayer {
    pub id: String,
    pub name: String,
    /// "xyz" | "wms" | "pmtiles" | "mbtiles"
    pub kind: String,
    /// XYZ template, WMS base URL, or a local file path.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wms_layers: Option<String>,
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minzoom: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxzoom: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
}

fn default_opacity() -> f64 {
    1.0
}

/// An imported file, a drawing or a recorded track (`<data>/userlayers/<id>.geojson`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserLayer {
    pub id: String,
    pub name: String,
    /// "import" | "drawing" | "track"
    pub kind: String,
    pub color: String,
    pub visible: bool,
    pub created_iso: String,
    pub features: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
}

/// Changes to a user layer; absent fields stay.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserLayerPatch {
    pub name: Option<String>,
    pub color: Option<String>,
    pub visible: Option<bool>,
    pub geojson: Option<String>,
}

/// Changes to a favorite. Absent keys stay; `null` clears the optional ones.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FavoritePatch {
    pub name: Option<String>,
    pub icon: Option<String>,
    #[serde(deserialize_with = "patch_field")]
    pub group: Option<Option<String>>,
    #[serde(deserialize_with = "patch_field")]
    pub color: Option<Option<String>>,
    #[serde(deserialize_with = "patch_field")]
    pub note: Option<Option<String>>,
    #[serde(deserialize_with = "patch_field")]
    pub address: Option<Option<String>>,
    pub data: Option<serde_json::Value>,
}

/// `Some(None)` for an explicit null, `Some(Some(v))` for a value; a missing key stays `None`
/// through `#[serde(default)]`.
fn patch_field<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Ok(Some(Option::<String>::deserialize(d)?))
}

/// Windows accent colours ("#rrggbb"), event `system-accent-changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemAccent {
    pub accent: String,
    pub light1: String,
    pub light2: String,
    pub dark1: String,
}

/// Size and version of the combined ready-made data package (first-run choice).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageInfo {
    pub size_bytes: u64,
    pub version: String,
}

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


/// The track recorder returns the saved user layer.
pub type TrackLayer = UserLayer;

// ===== [R3] transit =====

/// One ticket of a transit fare estimate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FareTicket {
    /// Lines covered by the ticket ("1 + 18", the agency for unnamed trains).
    pub label: String,
    pub ils: f64,
    pub train: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Shabbat,
    Holiday,
    NoService,
    LateNight,
}

/// Why transit service is missing (or starts much later) at the requested time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceNotice {
    pub kind: NoticeKind,
    /// First departure from the origin's stops after the break.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_service_iso: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holiday_name: Option<String>,
}

/// A transit vehicle position (online, Open Bus Stride SIRI archive).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitVehiclePos {
    pub lon: f64,
    pub lat: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headsign: Option<String>,
    pub updated_iso: String,
}


// ===== [R4] geocode =====

/// Offline Wikipedia summary of a place (`wiki_summary`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiSummary {
    pub title: String,
    /// plain-text introduction, shortened to a few sentences
    pub extract: String,
    pub url: String,
    /// "he" | "en"
    pub lang: String,
}

/// One area of sharp aerial imagery (Settings → satellite → sharp areas).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SatelliteArea {
    pub id: String,
    pub name: String,
    /// west, south, east, north
    pub bbox: [f64; 4],
    /// deepest zoom: 16 sharp · 17 very sharp · 18 sharpest
    pub zoom: u8,
}

/// Installed state of a sharp-imagery area.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SatelliteAreaStatus {
    pub id: String,
    pub present: bool,
    /// a download was started and can be resumed
    pub partial: bool,
    pub size_bytes: u64,
    pub zoom: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_iso: Option<String>,
    /// downloading now
    pub busy: bool,
}

/// Size and time of a sharp-imagery area before it is downloaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SatelliteAreaEstimate {
    pub tiles: u64,
    pub bytes: u64,
    /// at the govmap pacing (8–12 requests / s)
    pub minutes: f64,
    pub km2: f64,
    /// within the limits (inside the orthophoto coverage, not too large)
    pub ok: bool,
}

/// Event `satellite-area-progress`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SatelliteAreaProgress {
    pub id: String,
    pub phase: ProgressPhase,
    pub done: u64,
    pub total: u64,
    pub message: String,
}
