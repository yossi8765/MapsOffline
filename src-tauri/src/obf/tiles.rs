//! OBF map section → Protomaps-schema (v4) vector tiles → `map.pmtiles`.
//!
//! The output is drawn by the app's normal style (`@protomaps/basemaps` `layers("protomaps", …)`),
//! so every feature carries the source-layer / `kind` / `kind_detail` / `min_zoom` / name fields
//! that style filters on (see [`classify`] for the tag → layer table).
//!
//! Pipeline (memory stays bounded by the spill buckets, not by the file size):
//!  1. for each OBF zoom band (once): objects are read on a reader thread in batches, every
//!     batch is classified / simplified / clipped per output zoom in parallel (rayon), and the
//!     encoded features are spilled as `(tile id, layer, sort, props, MVT geometry)` records into
//!     N bucket files keyed by tile id; coastlines are kept aside;
//!  2. per output zoom: land (`earth`) and sea (`water` kind `ocean`) polygons for every tile
//!     that has data: coastline pieces are clipped per tile and closed along the tile border
//!     (OSM: land on the left), tiles without coastline take their state from a neighbour edge
//!     (flood fill), the band's ocean / land boxes, or default to land;
//!  3. each bucket is loaded, its records grouped by tile, encoded as MVT, gzipped and handed
//!     to the PMTiles writer (parallel over buckets).
//!
//! Only tiles with data are written (objects with output features, coastline, sea next to the
//! coast, tiles fully inside an ocean box): an archive served together with other countries
//! never hides their maps behind filler tiles.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::sync_channel;

use anyhow::{anyhow, bail, Context};
use parking_lot::Mutex;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use super::map::{MapLevel, MapObject, MapSource, OceanBox};
use super::mvt::{self, Feature, Value, EXTENT, GEOM_LINE, GEOM_POINT, GEOM_POLYGON};
use super::pmtiles_out::{self, tile_id, tile_zxy};
use crate::types::{ProgressFn, ProgressPhase};

const E: f64 = EXTENT as f64;

/// Source layers in the order they are written into a tile.
pub const LAYERS: [&str; 9] = ["earth", "landcover", "landuse", "water", "roads", "buildings", "pois", "places", "boundaries"];
const L_EARTH: u8 = 0;
const L_LANDCOVER: u8 = 1;
const L_LANDUSE: u8 = 2;
const L_WATER: u8 = 3;
const L_ROADS: u8 = 4;
const L_BUILDINGS: u8 = 5;
const L_POIS: u8 = 6;
const L_PLACES: u8 = 7;
const L_BOUNDARIES: u8 = 8;

/// Objects handed to the workers at once.
const BATCH: usize = 4096;
/// Untouched tiles at most this many tiles from a coastline tile may become sea tiles.
const SEA_RADIUS: i64 = 2;
/// At most this many "ocean box only" tiles per zoom (all identical, stored once).
const MAX_OCEAN_TILES_PER_ZOOM: usize = 4_000_000;

#[derive(Debug, Clone)]
pub struct TileOptions {
    pub min_zoom: u8,
    /// highest zoom written (the map overzooms beyond it); 15 like the other country maps
    pub max_zoom: u8,
    pub name: String,
    pub attribution: String,
    /// where the spill files go (default: next to `out`)
    pub tmp_dir: Option<PathBuf>,
    /// number of spill buckets (0 = 1024)
    pub buckets: usize,
    /// clip buffer around each tile in extent units (4096 per tile; 0 = 64, i.e. 4 px of 256)
    pub buffer: u32,
}

impl Default for TileOptions {
    fn default() -> Self {
        Self {
            min_zoom: 0,
            max_zoom: 15,
            name: "OsmAnd map".into(),
            attribution: "<a href=\"https://www.openstreetmap.org/copyright\">© OpenStreetMap</a>".into(),
            tmp_dir: None,
            buckets: 0,
            buffer: 0,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TileSummary {
    /// tiles written per zoom, index = zoom (0..=max_zoom)
    pub tiles_per_zoom: Vec<u64>,
    pub tiles: u64,
    pub tile_entries: u64,
    pub tile_contents: u64,
    pub bytes: u64,
    /// map objects read (all bands)
    pub objects: u64,
    /// encoded feature records (one per feature per tile)
    pub features: u64,
    /// [min_lon, min_lat, max_lon, max_lat] of the objects that produced features
    pub bbox: [f64; 4],
    /// tiles whose land / sea came from coastline pieces
    pub coast_tiles: u64,
    /// coastline tiles where open coastline ends had to be extended to the tile border
    pub coast_extended_tiles: u64,
    /// sea tiles without any object (ocean boxes, or next to the coast)
    pub ocean_only_tiles: u64,
}

// ================================================================ tags

struct Tags<'a>(&'a [(String, String)]);

impl<'a> Tags<'a> {
    fn get(&self, k: &str) -> Option<&'a str> {
        self.0.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty())
    }
    fn is(&self, k: &str, v: &str) -> bool {
        self.get(k) == Some(v)
    }
    fn has(&self, k: &str) -> bool {
        self.get(k).is_some()
    }
    fn yes(&self, k: &str) -> bool {
        matches!(self.get(k), Some(v) if v != "no" && v != "false" && v != "0")
    }
}

const NAME_LANGS: [&str; 7] = ["he", "en", "ar", "ru", "fr", "de", "es"];

/// Unicode script of the first letter (None = Latin, which Protomaps leaves out).
fn script_of(s: &str) -> Option<&'static str> {
    let c = s.chars().find(|c| c.is_alphabetic())? as u32;
    Some(match c {
        0..=0x024F | 0x1E00..=0x1EFF => return None,
        0x0370..=0x03FF => "Greek",
        0x0400..=0x052F => "Cyrillic",
        0x0530..=0x058F => "Armenian",
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => "Hebrew",
        0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => "Arabic",
        0x0900..=0x097F => "Devanagari",
        0x0980..=0x09FF => "Bengali",
        0x0B80..=0x0BFF => "Tamil",
        0x0E00..=0x0E7F => "Thai",
        0x10A0..=0x10FF => "Georgian",
        0x1200..=0x139F => "Ethiopic",
        0x3040..=0x309F => "Hiragana",
        0x30A0..=0x30FF => "Katakana",
        0xAC00..=0xD7AF | 0x1100..=0x11FF => "Hangul",
        0x3400..=0x4DBF | 0x4E00..=0x9FFF => "Han",
        _ => return None,
    })
}

fn name_props(t: &Tags, props: &mut Vec<(String, Value)>) {
    let name = t.get("name").or_else(|| t.get("name:en")).or_else(|| NAME_LANGS.iter().find_map(|l| t.get(&format!("name:{l}"))));
    if let Some(n) = name {
        props.push(("name".into(), n.into()));
        if let Some(s) = script_of(n) {
            props.push(("script".into(), s.into()));
        }
    }
    for l in NAME_LANGS {
        let k = format!("name:{l}");
        if let Some(v) = t.get(&k) {
            props.push((k, v.into()));
        }
    }
}

fn has_name(t: &Tags) -> bool {
    t.has("name") || NAME_LANGS.iter().any(|l| t.has(&format!("name:{l}")))
}

/// Tags that make a closed way an area.
fn is_area_tags(t: &Tags) -> bool {
    if t.is("area", "yes") {
        return true;
    }
    if t.is("area", "no") {
        return false;
    }
    t.0.iter().any(|(k, v)| match k.as_str() {
        "building" => v != "no",
        "building:part" | "landuse" | "leisure" | "amenity" | "shop" | "tourism" | "military" | "water" => true,
        "place" => matches!(v.as_str(), "island" | "islet" | "square"),
        "natural" => !matches!(v.as_str(), "coastline" | "cliff" | "tree_row" | "ridge" | "arete" | "valley" | "earth_bank" | "gully" | "tree" | "peak" | "saddle" | "volcano" | "spring"),
        "waterway" => matches!(v.as_str(), "riverbank" | "dock" | "boatyard"),
        "aeroway" => matches!(v.as_str(), "aerodrome" | "apron" | "helipad" | "terminal"),
        "boundary" => matches!(v.as_str(), "national_park" | "protected_area"),
        _ => false,
    })
}

// ================================================================ classification

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    Point,
    Line,
    Polygon,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Emit {
    /// the object's own geometry
    Native,
    /// one label point (polygon → inside point)
    Label,
    /// a polygon's rings as lines
    Outline,
}

#[derive(Debug, Clone)]
struct Out {
    layer: u8,
    emit: Emit,
    min_zoom: u8,
    max_zoom: u8,
    /// smallest polygon kept, in px² of a 256 px tile at the output zoom
    min_area: f64,
    props: Vec<(String, Value)>,
    sort: u32,
}

fn out(layer: u8, emit: Emit, min_zoom: u8, props: Vec<(String, Value)>) -> Out {
    Out { layer, emit, min_zoom, max_zoom: 30, min_area: 0.0, props, sort: 0 }
}

fn kv(k: &str, v: impl Into<Value>) -> (String, Value) {
    (k.to_string(), v.into())
}

/// Smallest zoom at which a polygon of `a0` px² at zoom 0 (256 px tiles) covers `px` px².
fn area_zoom(a0: f64, px: f64) -> u8 {
    if a0 <= 0.0 {
        return 30;
    }
    ((px / a0).log(4.0).ceil()).clamp(0.0, 30.0) as u8
}

/// kind, min zoom, is_link
fn road(hw: &str, t: &Tags) -> Option<(&'static str, u8, bool)> {
    Some(match hw {
        "motorway" => ("highway", 3, false),
        "motorway_link" => ("highway", 11, true),
        "trunk" => ("major_road", 6, false),
        "trunk_link" => ("major_road", 11, true),
        "primary" => ("major_road", 7, false),
        "primary_link" => ("major_road", 11, true),
        "secondary" => ("major_road", 9, false),
        "secondary_link" => ("major_road", 13, true),
        "tertiary" => ("major_road", 10, false),
        "tertiary_link" => ("major_road", 13, true),
        "residential" | "unclassified" | "road" | "living_street" => ("minor_road", 12, false),
        "service" => ("minor_road", if matches!(t.get("service"), Some("parking_aisle" | "driveway" | "drive-through")) { 14 } else { 13 }, false),
        "pedestrian" => ("path", 12, false),
        "track" | "path" | "cycleway" => ("path", 13, false),
        "footway" | "bridleway" | "steps" => ("path", 14, false),
        "corridor" => ("path", 15, false),
        "busway" | "bus_guideway" | "raceway" => ("other", 13, false),
        _ => return None,
    })
}

fn poi_zoom(kind: &str) -> u8 {
    match kind {
        "station" => 12,
        "peak" | "volcano" | "zoo" | "theme_park" | "hospital" | "ferry_terminal" | "marina" | "nature_reserve" => 13,
        "park" | "university" | "museum" | "stadium" | "golf_course" | "beach" | "bus_station" | "castle" | "mall" | "forest" => 14,
        "college" | "school" | "library" | "townhall" | "attraction" | "theatre" | "viewpoint" | "supermarket" | "fuel" | "hotel" | "camp_site" | "garden" | "cinema" | "courthouse" | "embassy" => 15,
        "restaurant" | "cafe" | "place_of_worship" | "police" | "fire_station" | "post_office" | "pharmacy" | "parking" | "hostel" | "monument" | "clinic" | "convenience" | "archaeological_site" | "ruins" => 16,
        "fast_food" | "bar" | "pub" | "bank" | "toilets" | "drinking_water" | "bus_stop" | "kindergarten" | "guest_house" | "artwork" | "memorial" | "spring" => 17,
        "bench" | "atm" => 18,
        _ => 16,
    }
}

const POI_SKIP: [&str; 14] = [
    "parking_space", "parking_entrance", "waste_basket", "waste_disposal", "vending_machine", "bicycle_parking", "telephone", "post_box", "grit_bin", "hunting_stand", "recycling", "yes", "motorcycle_parking", "loading_dock",
];
const POI_NAMELESS: [&str; 5] = ["toilets", "drinking_water", "bench", "spring", "atm"];

/// (kind, min zoom) of a point of interest.
fn poi_kind<'a>(t: &Tags<'a>) -> Option<(&'a str, u8)> {
    if let Some(v) = t.get("railway") {
        match v {
            "station" => return Some(("station", 12)),
            "halt" => return Some(("station", 14)),
            _ => {}
        }
    }
    if t.is("aeroway", "aerodrome") {
        return Some(("aerodrome", if t.has("iata") { 11 } else { 13 }));
    }
    if t.is("highway", "bus_stop") {
        return Some(("bus_stop", 17));
    }
    for key in ["amenity", "shop", "tourism", "leisure", "historic"] {
        if let Some(v) = t.get(key) {
            if POI_SKIP.contains(&v) || (key == "tourism" && v == "information") {
                continue;
            }
            let kind = if key == "shop" && v == "mall" { "mall" } else { v };
            let z = if key == "shop" && !matches!(v, "supermarket" | "convenience" | "mall") { 17 } else { poi_zoom(kind) };
            return Some((kind, z));
        }
    }
    if let Some(v @ ("peak" | "volcano" | "spring" | "cave_entrance" | "beach")) = t.get("natural") {
        return Some((v, poi_zoom(v)));
    }
    if let Some(v @ ("lighthouse" | "windmill" | "tower")) = t.get("man_made") {
        return Some((v, 16));
    }
    if t.is("landuse", "forest") || t.is("natural", "wood") {
        return Some(("forest", 14));
    }
    if t.is("boundary", "national_park") {
        return Some(("national_park", 12));
    }
    None
}

/// `kind` of the landuse layer.
fn landuse_kind<'a>(t: &Tags<'a>) -> Option<&'a str> {
    if let Some(v @ ("aerodrome" | "runway" | "taxiway")) = t.get("aeroway") {
        return Some(v);
    }
    if let Some(v) = t.get("amenity") {
        match v {
            "hospital" | "school" | "university" | "college" | "kindergarten" | "parking" => return Some(v),
            "grave_yard" => return Some("cemetery"),
            _ => {}
        }
    }
    if let Some(v @ ("park" | "garden" | "golf_course" | "nature_reserve" | "playground" | "pitch" | "stadium" | "sports_centre" | "dog_park" | "marina")) = t.get("leisure") {
        return Some(v);
    }
    if let Some(v @ ("zoo" | "theme_park" | "camp_site")) = t.get("tourism") {
        return Some(v);
    }
    if let Some(v) = t.get("landuse") {
        match v {
            "forest" | "cemetery" | "military" | "residential" | "commercial" | "retail" | "industrial" | "railway" | "farmland" | "farmyard" | "meadow" | "grass" | "orchard"
            | "vineyard" | "allotments" | "village_green" | "recreation_ground" | "quarry" | "brownfield" | "greenfield" | "construction" | "landfill" | "garages" | "religious" => return Some(v),
            "reservoir" | "basin" => {}
            _ => {}
        }
    }
    if let Some(v) = t.get("military") {
        return Some(match v {
            "naval_base" => "naval_base",
            "airfield" => "airfield",
            _ => "military",
        });
    }
    if let Some(v @ ("wood" | "scrub" | "heath" | "grassland" | "beach" | "sand" | "bare_rock" | "scree" | "glacier" | "wetland")) = t.get("natural") {
        return Some(v);
    }
    if let Some(v @ ("national_park" | "protected_area")) = t.get("boundary") {
        return Some(v);
    }
    if t.is("highway", "pedestrian") || t.is("place", "square") {
        return Some("pedestrian");
    }
    if t.is("man_made", "pier") {
        return Some("pier");
    }
    if t.is("waterway", "dam") {
        return Some("dam");
    }
    None
}

/// Low-zoom `landcover` kind for a landuse kind.
fn landcover_kind(landuse: &str) -> Option<&'static str> {
    Some(match landuse {
        "forest" | "wood" | "nature_reserve" | "national_park" => "forest",
        "scrub" | "heath" => "scrub",
        "grassland" | "grass" | "meadow" => "grassland",
        "farmland" | "orchard" | "vineyard" | "farmyard" => "farmland",
        "sand" | "bare_rock" | "scree" | "beach" => "barren",
        "glacier" => "glacier",
        "residential" | "commercial" | "industrial" | "retail" => "urban_area",
        _ => return None,
    })
}

/// (kind, kind_detail) of a water polygon.
fn water_kind<'a>(t: &Tags<'a>) -> Option<(&'static str, Option<&'a str>)> {
    let detail = t.get("water");
    if t.is("leisure", "swimming_pool") {
        return Some(("swimming_pool", None));
    }
    if t.is("natural", "bay") {
        return Some(("bay", None));
    }
    let watery = t.is("natural", "water") || t.has("water") || matches!(t.get("waterway"), Some("riverbank" | "dock")) || matches!(t.get("landuse"), Some("reservoir" | "basin"));
    if !watery {
        return None;
    }
    let kind = match detail {
        Some("lake" | "oxbow" | "lagoon" | "reservoir" | "pond") => "lake",
        _ if t.is("landuse", "reservoir") => "lake",
        _ => "water",
    };
    let detail = detail.or_else(|| if t.is("waterway", "riverbank") { Some("river") } else { t.get("landuse").filter(|v| *v == "reservoir" || *v == "basin") });
    Some((kind, detail))
}

fn parse_num(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let end = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(s.len());
    s[..end].parse::<f64>().ok().filter(|v| v.is_finite())
}

const POP_BREAKS: [i64; 17] = [200, 1_000, 2_000, 5_000, 10_000, 20_000, 50_000, 100_000, 200_000, 500_000, 1_000_000, 5_000_000, 10_000_000, 20_000_000, 50_000_000, 100_000_000, 1_000_000_000];

fn population_rank(pop: i64) -> i64 {
    POP_BREAKS.iter().filter(|&&b| pop >= b).count() as i64
}

/// Place labels (`places` layer, islands in `earth`, seas in `water`).
fn place_out(t: &Tags, pl: &str, shape: Shape, a0: f64) -> Option<Out> {
    let emit = if shape == Shape::Point { Emit::Native } else { Emit::Label };
    let mut props = Vec::new();
    let pop_tag = t.get("population").and_then(parse_num).map(|p| p as i64);
    let (layer, kind, min_zoom, default_pop) = match pl {
        "country" => (L_PLACES, "country", 3, 1_000_000),
        "state" | "province" => (L_PLACES, "region", 4, 100_000),
        "city" => (L_PLACES, "locality", 0, 50_000),
        "town" => (L_PLACES, "locality", 0, 10_000),
        "village" => (L_PLACES, "locality", 0, 1_000),
        "hamlet" => (L_PLACES, "locality", 13, 100),
        "isolated_dwelling" | "farm" => (L_PLACES, "locality", 14, 10),
        "suburb" | "quarter" | "borough" => (L_PLACES, "macrohood", 11, 0),
        "neighbourhood" => (L_PLACES, "neighbourhood", 13, 0),
        "city_block" => (L_PLACES, "neighbourhood", 15, 0),
        "island" | "islet" => {
            let z = if shape == Shape::Polygon { area_zoom(a0, 2000.0).clamp(5, 15) } else if pl == "island" { 11 } else { 14 };
            props.push(kv("kind", "island"));
            props.push(kv("min_zoom", z as i64));
            name_props(t, &mut props);
            return Some(Out { sort: u32::from(z), ..out(L_EARTH, emit, z, props) });
        }
        "sea" | "ocean" => {
            props.push(kv("kind", pl));
            props.push(kv("min_zoom", 3i64));
            name_props(t, &mut props);
            return Some(out(L_WATER, emit, 3, props));
        }
        _ => return None,
    };
    let pop = pop_tag.unwrap_or(default_pop);
    let rank = population_rank(pop);
    let capital = matches!(t.get("capital"), Some("yes" | "2")) || t.is("admin_level", "2");
    let min_zoom = match pl {
        "city" => {
            let z = match pop {
                p if p >= 5_000_000 => 3,
                p if p >= 1_000_000 => 4,
                p if p >= 500_000 => 5,
                p if p >= 200_000 => 6,
                p if p >= 50_000 => 7,
                _ => 8,
            };
            if capital {
                z.min(4)
            } else {
                z
            }
        }
        "town" => {
            if pop >= 50_000 {
                8
            } else {
                9
            }
        }
        "village" => {
            if pop >= 5_000 {
                10
            } else {
                11
            }
        }
        _ => min_zoom,
    };
    props.push(kv("kind", kind));
    props.push(kv("kind_detail", pl));
    props.push(kv("min_zoom", i64::from(min_zoom)));
    if pop > 0 {
        props.push(kv("population", pop));
    }
    props.push(kv("population_rank", rank));
    props.push(kv("sort_key", i64::from(min_zoom) * 20 + (17 - rank)));
    if capital {
        props.push(kv("capital", "yes"));
    }
    if let Some(r) = t.get("ref") {
        props.push(kv("ref", r));
    }
    name_props(t, &mut props);
    Some(Out { sort: u32::from(min_zoom) * 20 + (17 - rank) as u32, ..out(layer, emit, min_zoom, props) })
}

/// Tag → schema mapping. `a0` = polygon area in px² at zoom 0 (256 px tiles).
fn classify(t: &Tags, shape: Shape, a0: f64) -> Vec<Out> {
    let mut outs = Vec::new();
    let area_sort = u32::MAX - (a0 * 4f64.powi(15)).clamp(0.0, f64::from(u32::MAX - 1)) as u32;

    // ---- boundaries (nothing else comes from these objects)
    if t.is("boundary", "administrative") {
        if shape != Shape::Point {
            if let Some(l @ (2 | 4)) = t.get("admin_level").and_then(|v| v.parse::<i64>().ok()) {
                let props = vec![kv("kind", if l == 2 { "country" } else { "region" }), kv("kind_detail", l), kv("min_zoom", if l == 2 { 0i64 } else { 4 })];
                let emit = if shape == Shape::Polygon { Emit::Outline } else { Emit::Native };
                outs.push(out(L_BOUNDARIES, emit, if l == 2 { 0 } else { 4 }, props));
            }
        }
        return outs;
    }

    // ---- lines: roads, rail, waterways, piers, ferries
    if shape == Shape::Line {
        let bridge = t.yes("bridge");
        let tunnel = t.yes("tunnel") && !t.is("tunnel", "building_passage");
        let structure = |props: &mut Vec<(String, Value)>| {
            if bridge {
                props.push(kv("is_bridge", true));
            }
            if tunnel {
                props.push(kv("is_tunnel", true));
            }
        };
        if let Some(hw) = t.get("highway") {
            if let Some((kind, mz, link)) = road(hw, t) {
                let mut props = vec![kv("kind", kind), kv("kind_detail", hw), kv("min_zoom", i64::from(mz))];
                if link {
                    props.push(kv("is_link", true));
                }
                structure(&mut props);
                if matches!(t.get("oneway"), Some("yes" | "true" | "1")) {
                    props.push(kv("oneway", "yes"));
                }
                if let Some(r) = t.get("ref") {
                    props.push(kv("ref", r));
                    let first = r.split(';').next().unwrap_or(r).trim();
                    if !first.is_empty() && matches!(kind, "highway" | "major_road") {
                        props.push(kv("shield_text", first));
                    }
                }
                name_props(t, &mut props);
                outs.push(Out { sort: 30 - u32::from(mz), ..out(L_ROADS, Emit::Native, mz, props) });
            }
        }
        if let Some(rw) = t.get("railway") {
            let mz = match rw {
                "rail" if t.has("service") => 14,
                "rail" => 9,
                "light_rail" | "subway" | "tram" => 13,
                "narrow_gauge" | "monorail" | "funicular" | "preserved" => 14,
                _ => 0,
            };
            if mz > 0 {
                let mut props = vec![kv("kind", "rail"), kv("kind_detail", rw), kv("min_zoom", i64::from(mz))];
                structure(&mut props);
                name_props(t, &mut props);
                outs.push(Out { sort: 30 - u32::from(mz), ..out(L_ROADS, Emit::Native, mz, props) });
            }
        }
        if let Some(v @ ("runway" | "taxiway")) = t.get("aeroway") {
            let mz = if v == "runway" { 9 } else { 13 };
            outs.push(out(L_ROADS, Emit::Native, mz, vec![kv("kind", "aeroway"), kv("kind_detail", v), kv("min_zoom", i64::from(mz))]));
        }
        if t.is("man_made", "pier") {
            outs.push(out(L_ROADS, Emit::Native, 13, vec![kv("kind", "other"), kv("kind_detail", "pier"), kv("min_zoom", 13i64)]));
        }
        if t.is("route", "ferry") {
            let mut props = vec![kv("kind", "ferry"), kv("min_zoom", 10i64)];
            name_props(t, &mut props);
            outs.push(out(L_ROADS, Emit::Native, 10, props));
        }
        if let Some(ww) = t.get("waterway") {
            let (kind, mz) = match ww {
                "river" => ("river", 9),
                "canal" => ("canal", 11),
                "stream" => ("stream", 13),
                "ditch" => ("ditch", 14),
                "drain" => ("drain", 14),
                _ => ("", 0),
            };
            if !kind.is_empty() {
                let mut props = vec![kv("kind", kind), kv("min_zoom", i64::from(mz))];
                if tunnel {
                    props.push(kv("is_tunnel", true));
                }
                name_props(t, &mut props);
                outs.push(out(L_WATER, Emit::Native, mz, props));
            }
        }
    }

    // ---- areas
    if shape == Shape::Polygon {
        if let Some((kind, detail)) = water_kind(t) {
            let mut props = vec![kv("kind", kind)];
            if let Some(d) = detail {
                props.push(kv("kind_detail", d));
            }
            let mz = if kind == "swimming_pool" { 14 } else { 0 };
            outs.push(Out { min_area: 1.0, sort: area_sort, ..out(L_WATER, Emit::Native, mz, props) });
            if has_name(t) && kind != "swimming_pool" {
                let lz = area_zoom(a0, 3000.0).clamp(6, 15);
                let mut props = vec![kv("kind", if kind == "bay" { "bay" } else { kind }), kv("min_zoom", i64::from(lz))];
                name_props(t, &mut props);
                outs.push(Out { sort: u32::from(lz), ..out(L_WATER, Emit::Label, lz, props) });
            }
        }
        if let Some(kind) = landuse_kind(t) {
            outs.push(Out { min_area: 4.0, sort: area_sort, ..out(L_LANDUSE, Emit::Native, 6, vec![kv("kind", kind)]) });
            if let Some(lc) = landcover_kind(kind) {
                outs.push(Out { min_area: 16.0, max_zoom: 7, sort: area_sort, ..out(L_LANDCOVER, Emit::Native, 0, vec![kv("kind", lc)]) });
            }
        }
        let building = t.get("building").filter(|v| *v != "no");
        if building.is_some() || t.has("building:part") {
            let kind = if building.is_some() { "building" } else { "building_part" };
            let mut props = vec![kv("kind", kind)];
            let height = t.get("height").and_then(parse_num).or_else(|| t.get("building:levels").and_then(parse_num).map(|l| l * 3.0 + 2.0));
            if let Some(h) = height {
                props.push(kv("height", Value::Float((h * 10.0).round() / 10.0)));
            }
            let min_h = t.get("min_height").and_then(parse_num).or_else(|| t.get("building:min_level").and_then(parse_num).map(|l| l * 3.0));
            if let Some(h) = min_h {
                props.push(kv("min_height", Value::Float((h * 10.0).round() / 10.0)));
            }
            outs.push(Out { min_area: 2.0, ..out(L_BUILDINGS, Emit::Native, 12, props) });
        }
    }

    // ---- labels: places, points of interest, addresses, bays
    if let Some(pl) = t.get("place") {
        if has_name(t) {
            if let Some(o) = place_out(t, pl, shape, a0) {
                outs.push(o);
            }
        }
    } else if let Some((kind, table_z)) = poi_kind(t) {
        let named = has_name(t);
        let ok = match shape {
            Shape::Point => named || POI_NAMELESS.contains(&kind),
            Shape::Polygon => named,
            Shape::Line => false,
        };
        if ok {
            let mz = if shape == Shape::Polygon { table_z.min(area_zoom(a0, 2500.0).max(10)) } else { table_z };
            let mut props = vec![kv("kind", kind), kv("min_zoom", i64::from(mz))];
            name_props(t, &mut props);
            let emit = if shape == Shape::Point { Emit::Native } else { Emit::Label };
            outs.push(Out { sort: u32::from(mz), ..out(L_POIS, emit, mz, props) });
        }
    }
    if let Some(v @ ("bay" | "strait")) = t.get("natural") {
        if shape != Shape::Polygon && has_name(t) {
            let mut props = vec![kv("kind", v), kv("min_zoom", 9i64)];
            name_props(t, &mut props);
            outs.push(out(L_WATER, if shape == Shape::Point { Emit::Native } else { Emit::Label }, 9, props));
        }
    }
    if let Some(hn) = t.get("addr:housenumber") {
        if shape != Shape::Line {
            let emit = if shape == Shape::Point { Emit::Native } else { Emit::Label };
            outs.push(out(L_BUILDINGS, emit, 18, vec![kv("kind", "address"), kv("addr_housenumber", hn), kv("min_zoom", 18i64)]));
        }
    }
    outs
}

// ================================================================ geometry

type P = (f64, f64);

#[derive(Debug, Clone)]
enum Geom {
    Points(Vec<P>),
    Lines(Vec<Vec<P>>),
    /// polygons: exterior first, then holes; rings open (no repeated closing point)
    Polys(Vec<Vec<Vec<P>>>),
}

impl Geom {
    fn is_empty(&self) -> bool {
        match self {
            Geom::Points(v) => v.is_empty(),
            Geom::Lines(v) => v.is_empty(),
            Geom::Polys(v) => v.is_empty(),
        }
    }

    fn bounds(&self) -> [f64; 4] {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        let mut add = |p: &P| {
            b[0] = b[0].min(p.0);
            b[1] = b[1].min(p.1);
            b[2] = b[2].max(p.0);
            b[3] = b[3].max(p.1);
        };
        match self {
            Geom::Points(v) => v.iter().for_each(&mut add),
            Geom::Lines(v) => v.iter().flatten().for_each(&mut add),
            Geom::Polys(v) => v.iter().flatten().flatten().for_each(&mut add),
        }
        b
    }
}

#[inline]
fn ax(p: P, axis: usize) -> f64 {
    if axis == 0 {
        p.0
    } else {
        p.1
    }
}

#[inline]
fn set_ax(p: &mut P, axis: usize, v: f64) {
    if axis == 0 {
        p.0 = v
    } else {
        p.1 = v
    }
}

#[inline]
fn lerp(a: P, b: P, t: f64) -> P {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Clip a polyline to the slab `k1 <= coord[axis] <= k2`. Points created on the slab border
/// get exactly `k1` / `k2` on that axis.
fn clip_line_axis(line: &[P], axis: usize, k1: f64, k2: f64, out: &mut Vec<Vec<P>>) {
    let mut cur: Vec<P> = Vec::new();
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (ak, bk) = (ax(a, axis), ax(b, axis));
        let (t0, t1) = if ak == bk {
            if ak < k1 || ak > k2 {
                (1.0, 0.0)
            } else {
                (0.0, 1.0)
            }
        } else {
            let ta = (k1 - ak) / (bk - ak);
            let tb = (k2 - ak) / (bk - ak);
            (ta.min(tb).max(0.0), ta.max(tb).min(1.0))
        };
        if t0 > t1 {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        }
        let mut p0 = if t0 > 0.0 { lerp(a, b, t0) } else { a };
        if t0 > 0.0 {
            set_ax(&mut p0, axis, if ak < k1 { k1 } else { k2 });
        }
        let mut p1 = if t1 < 1.0 { lerp(a, b, t1) } else { b };
        if t1 < 1.0 {
            set_ax(&mut p1, axis, if bk > k2 { k2 } else { k1 });
        }
        if cur.is_empty() || t0 > 0.0 {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            cur.push(p0);
        }
        cur.push(p1);
        if t1 < 1.0 {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
}

/// Sutherland–Hodgman clip of an open ring to one half plane.
fn clip_ring_half(ring: &[P], axis: usize, k: f64, keep_ge: bool) -> Vec<P> {
    let n = ring.len();
    let mut out = Vec::with_capacity(n + 4);
    if n == 0 {
        return out;
    }
    let inside = |p: P| if keep_ge { ax(p, axis) >= k } else { ax(p, axis) <= k };
    let cross = |a: P, b: P| {
        let (ak, bk) = (ax(a, axis), ax(b, axis));
        let mut p = lerp(a, b, (k - ak) / (bk - ak));
        set_ax(&mut p, axis, k);
        p
    };
    for i in 0..n {
        let cur = ring[i];
        let prev = ring[(i + n - 1) % n];
        let (ci, pi) = (inside(cur), inside(prev));
        if ci {
            if !pi {
                out.push(cross(prev, cur));
            }
            out.push(cur);
        } else if pi {
            out.push(cross(prev, cur));
        }
    }
    out
}

fn clip_axis(g: &Geom, axis: usize, k1: f64, k2: f64) -> Geom {
    let b = g.bounds();
    let (lo, hi) = if axis == 0 { (b[0], b[2]) } else { (b[1], b[3]) };
    if lo >= k1 && hi <= k2 {
        return g.clone();
    }
    if hi < k1 || lo > k2 {
        return match g {
            Geom::Points(_) => Geom::Points(Vec::new()),
            Geom::Lines(_) => Geom::Lines(Vec::new()),
            Geom::Polys(_) => Geom::Polys(Vec::new()),
        };
    }
    match g {
        Geom::Points(v) => Geom::Points(v.iter().copied().filter(|p| ax(*p, axis) >= k1 && ax(*p, axis) <= k2).collect()),
        Geom::Lines(v) => {
            let mut out = Vec::new();
            for l in v {
                clip_line_axis(l, axis, k1, k2, &mut out);
            }
            Geom::Lines(out)
        }
        Geom::Polys(v) => {
            let mut out = Vec::new();
            for poly in v {
                let mut rings = Vec::with_capacity(poly.len());
                for (i, r) in poly.iter().enumerate() {
                    let c = clip_ring_half(&clip_ring_half(r, axis, k1, true), axis, k2, false);
                    if c.len() >= 3 {
                        rings.push(c);
                    } else if i == 0 {
                        break;
                    }
                }
                if !rings.is_empty() {
                    out.push(rings);
                }
            }
            Geom::Polys(out)
        }
    }
}

fn clip_rect(g: &Geom, x0: f64, y0: f64, x1: f64, y1: f64) -> Geom {
    clip_axis(&clip_axis(g, 0, x0, x1), 1, y0, y1)
}

/// Clip `g` (world pixel coordinates of one zoom) into every tile of the range, splitting the
/// range in halves so each level clips the geometry once (O(n log tiles)).
fn split_tiles(g: Geom, tx0: u32, tx1: u32, ty0: u32, ty1: u32, buf: f64, emit: &mut dyn FnMut(u32, u32, Geom)) {
    if g.is_empty() {
        return;
    }
    if tx0 == tx1 && ty0 == ty1 {
        let (x0, y0) = (f64::from(tx0) * E - buf, f64::from(ty0) * E - buf);
        let c = clip_rect(&g, x0, y0, x0 + E + 2.0 * buf, y0 + E + 2.0 * buf);
        if !c.is_empty() {
            emit(tx0, ty0, c);
        }
        return;
    }
    if tx1 - tx0 >= ty1 - ty0 {
        let m = tx0 + (tx1 - tx0) / 2;
        let a = clip_axis(&g, 0, f64::from(tx0) * E - buf, f64::from(m + 1) * E + buf);
        let b = clip_axis(&g, 0, f64::from(m + 1) * E - buf, f64::from(tx1 + 1) * E + buf);
        drop(g);
        split_tiles(a, tx0, m, ty0, ty1, buf, emit);
        split_tiles(b, m + 1, tx1, ty0, ty1, buf, emit);
    } else {
        let m = ty0 + (ty1 - ty0) / 2;
        let a = clip_axis(&g, 1, f64::from(ty0) * E - buf, f64::from(m + 1) * E + buf);
        let b = clip_axis(&g, 1, f64::from(m + 1) * E - buf, f64::from(ty1 + 1) * E + buf);
        drop(g);
        split_tiles(a, tx0, tx1, ty0, m, buf, emit);
        split_tiles(b, tx0, tx1, m + 1, ty1, buf, emit);
    }
}

fn seg_dist2(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qy) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    qx * qx + qy * qy
}

/// Douglas–Peucker (iterative); keeps the end points.
fn simplify(pts: &[P], tol2: f64) -> Vec<P> {
    let n = pts.len();
    if n <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((s, e)) = stack.pop() {
        if e <= s + 1 {
            continue;
        }
        let (mut best, mut bi) = (-1.0, s);
        for i in s + 1..e {
            let d = seg_dist2(pts[i], pts[s], pts[e]);
            if d > best {
                best = d;
                bi = i;
            }
        }
        if best > tol2 {
            keep[bi] = true;
            stack.push((s, bi));
            stack.push((bi, e));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// Simplify a ring given open; result open.
fn simplify_ring(ring: &[P], tol2: f64) -> Vec<P> {
    if ring.len() <= 4 {
        return ring.to_vec();
    }
    let mut closed = ring.to_vec();
    closed.push(ring[0]);
    let mut s = simplify(&closed, tol2);
    s.pop();
    s
}

fn ring_area(r: &[P]) -> f64 {
    let n = r.len();
    let mut s = 0.0;
    for i in 0..n {
        let (a, b) = (r[i], r[(i + 1) % n]);
        s += a.0 * b.1 - b.0 * a.1;
    }
    s / 2.0
}

fn point_in_ring(p: P, r: &[P]) -> bool {
    let n = r.len();
    let mut inside = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (r[i], r[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A point inside a ring: midpoint of the widest inside run of the horizontal line through the
/// middle of the bounding box (centroid-like, but always inside for concave shapes).
fn label_point(r: &[P]) -> Option<P> {
    if r.is_empty() {
        return None;
    }
    let (mut y0, mut y1) = (f64::MAX, f64::MIN);
    for p in r {
        y0 = y0.min(p.1);
        y1 = y1.max(p.1);
    }
    for frac in [0.5, 0.4, 0.6, 0.3, 0.7] {
        let y = y0 + (y1 - y0) * frac;
        let n = r.len();
        let mut xs = Vec::new();
        for i in 0..n {
            let (a, b) = (r[i], r[(i + 1) % n]);
            if (a.1 > y) != (b.1 > y) {
                xs.push(a.0 + (y - a.1) * (b.0 - a.0) / (b.1 - a.1));
            }
        }
        xs.sort_by(f64::total_cmp);
        if let Some(best) = xs.as_chunks::<2>().0.iter().max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0]))) {
            return Some(((best[0] + best[1]) / 2.0, y));
        }
    }
    Some(r[0])
}

fn round_pt(p: P, ox: f64, oy: f64) -> (i32, i32) {
    ((p.0 - ox).round() as i32, (p.1 - oy).round() as i32)
}

/// Tile-local MVT geometry of a clipped geometry; also whether it reaches into the tile proper.
fn encode_local(g: &Geom, ox: f64, oy: f64) -> (u8, Vec<u32>, bool) {
    let e = EXTENT as i32;
    let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    let mut add = |q: (i32, i32)| {
        b[0] = b[0].min(q.0);
        b[1] = b[1].min(q.1);
        b[2] = b[2].max(q.0);
        b[3] = b[3].max(q.1);
        q
    };
    let (gt, geom) = match g {
        Geom::Points(v) => {
            let pts: Vec<(i32, i32)> = v.iter().map(|p| add(round_pt(*p, ox, oy))).collect();
            let touched = pts.iter().any(|&(x, y)| x >= 0 && x < e && y >= 0 && y < e);
            return (GEOM_POINT, mvt::encode_points(&pts), touched);
        }
        Geom::Lines(v) => {
            let lines: Vec<Vec<(i32, i32)>> = v.iter().map(|l| l.iter().map(|p| add(round_pt(*p, ox, oy))).collect()).collect();
            (GEOM_LINE, mvt::encode_lines(&lines))
        }
        Geom::Polys(v) => {
            let polys: Vec<Vec<Vec<(i32, i32)>>> = v.iter().map(|poly| poly.iter().map(|r| r.iter().map(|p| add(round_pt(*p, ox, oy))).collect()).collect()).collect();
            (GEOM_POLYGON, mvt::encode_polygons(&polys))
        }
    };
    let touched = !geom.is_empty() && b[2] >= 0 && b[0] < e && b[3] >= 0 && b[1] < e;
    (gt, geom, touched)
}

// ================================================================ records

fn encode_props(props: &[(String, Value)]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(props.len() as u16).to_le_bytes());
    for (k, v) in props {
        let kb = &k.as_bytes()[..k.len().min(255)];
        b.push(kb.len() as u8);
        b.extend_from_slice(kb);
        match v {
            Value::Str(s) => {
                b.push(0);
                b.extend_from_slice(&(s.len() as u32).to_le_bytes());
                b.extend_from_slice(s.as_bytes());
            }
            Value::Int(i) => {
                b.push(1);
                b.extend_from_slice(&i.to_le_bytes());
            }
            Value::Float(f) => {
                b.push(2);
                b.extend_from_slice(&f.to_le_bytes());
            }
            Value::Bool(x) => {
                b.push(3);
                b.push(u8::from(*x));
            }
        }
    }
    b
}

fn take<'a>(b: &'a [u8], p: &mut usize, n: usize) -> anyhow::Result<&'a [u8]> {
    let s = b.get(*p..*p + n).ok_or_else(|| anyhow!("truncated spill record"))?;
    *p += n;
    Ok(s)
}

fn decode_props(b: &[u8]) -> anyhow::Result<Vec<(String, Value)>> {
    let mut p = 0;
    let n = u16::from_le_bytes(take(b, &mut p, 2)?.try_into()?) as usize;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let kl = take(b, &mut p, 1)?[0] as usize;
        let k = String::from_utf8_lossy(take(b, &mut p, kl)?).into_owned();
        let val = match take(b, &mut p, 1)?[0] {
            0 => {
                let l = u32::from_le_bytes(take(b, &mut p, 4)?.try_into()?) as usize;
                Value::Str(String::from_utf8_lossy(take(b, &mut p, l)?).into_owned())
            }
            1 => Value::Int(i64::from_le_bytes(take(b, &mut p, 8)?.try_into()?)),
            2 => Value::Float(f64::from_le_bytes(take(b, &mut p, 8)?.try_into()?)),
            _ => Value::Bool(take(b, &mut p, 1)?[0] != 0),
        };
        v.push((k, val));
    }
    Ok(v)
}

/// Record: [tile u64][len u32] then payload [layer u8][gtype u8][sort u32][props_len u32][props][geometry varints].
fn push_rec(blob: &mut Vec<u8>, tile: u64, layer: u8, gtype: u8, sort: u32, props: &[u8], geom: &[u32]) {
    blob.extend_from_slice(&tile.to_le_bytes());
    let len_pos = blob.len();
    blob.extend_from_slice(&[0; 4]);
    let start = blob.len();
    blob.push(layer);
    blob.push(gtype);
    blob.extend_from_slice(&sort.to_le_bytes());
    blob.extend_from_slice(&(props.len() as u32).to_le_bytes());
    blob.extend_from_slice(props);
    for &g in geom {
        mvt::put_varint(blob, u64::from(g));
    }
    let len = (blob.len() - start) as u32;
    blob[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());
}

struct RecRef<'a> {
    tile: u64,
    layer: u8,
    sort: u32,
    seq: u32,
    payload: &'a [u8],
}

fn parse_feature(payload: &[u8]) -> anyhow::Result<Feature> {
    let mut p = 0;
    let head = take(payload, &mut p, 10)?;
    let gtype = head[1];
    let pl = u32::from_le_bytes(head[6..10].try_into()?) as usize;
    let props = decode_props(take(payload, &mut p, pl)?)?;
    let mut geometry = Vec::new();
    while p < payload.len() {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = take(payload, &mut p, 1)?[0];
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 35 {
                bail!("bad varint in spill record");
            }
        }
        geometry.push(v as u32);
    }
    Ok(Feature { geom_type: gtype, geometry, props })
}

struct Spill {
    paths: Vec<PathBuf>,
    files: Vec<BufWriter<File>>,
    records: u64,
}

fn bucket_of(tile: u64, n: usize) -> usize {
    ((tile.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) % n as u64) as usize
}

impl Spill {
    fn new(dir: &Path, n: usize) -> anyhow::Result<Self> {
        let mut paths = Vec::with_capacity(n);
        let mut files = Vec::with_capacity(n);
        for i in 0..n {
            let p = dir.join(format!("b{i:04}.spill"));
            files.push(BufWriter::with_capacity(32 << 10, File::create(&p).with_context(|| format!("create {}", p.display()))?));
            paths.push(p);
        }
        Ok(Self { paths, files, records: 0 })
    }

    fn write_blob(&mut self, blob: &[u8]) -> anyhow::Result<()> {
        let n = self.files.len();
        let mut p = 0;
        while p + 12 <= blob.len() {
            let tile = u64::from_le_bytes(blob[p..p + 8].try_into()?);
            let len = u32::from_le_bytes(blob[p + 8..p + 12].try_into()?) as usize;
            let end = p + 12 + len;
            self.files[bucket_of(tile, n)].write_all(&blob[p..end])?;
            self.records += 1;
            p = end;
        }
        Ok(())
    }

    fn finish(self) -> anyhow::Result<Vec<PathBuf>> {
        for mut f in self.files {
            f.flush()?;
        }
        Ok(self.paths)
    }
}

/// Removes the temp directory however `build` ends.
struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ================================================================ objects → records

struct Ctx {
    min_zoom: u8,
    max_zoom: u8,
    buffer: f64,
}

#[derive(Default)]
struct ObjResult {
    blob: Vec<u8>,
    touched: Vec<u64>,
    coast: Option<Vec<(u32, u32)>>,
    /// bbox in 31-units of an object that produced features
    bbox: Option<[u32; 4]>,
}

fn tol2(z: u8, max_zoom: u8) -> f64 {
    let t = if z >= max_zoom { 0.5 } else { 2.0 };
    t * t
}

fn tile_range(b: [f64; 4], z: u8, buf: f64) -> (u32, u32, u32, u32) {
    let n = (1u64 << z) as f64;
    let f = |v: f64| ((v / E).floor()).clamp(0.0, n - 1.0) as u32;
    (f(b[0] - buf), f(b[2] + buf), f(b[1] - buf), f(b[3] + buf))
}

fn process_object(obj: &MapObject, zooms: &[u8], ctx: &Ctx) -> ObjResult {
    let mut res = ObjResult::default();
    if obj.coords.is_empty() {
        return res;
    }
    let t = Tags(&obj.tags);
    if t.is("natural", "coastline") {
        if obj.coords.len() >= 2 {
            res.coast = Some(obj.coords.clone());
        }
        return res;
    }
    let closed = obj.coords.len() >= 4 && obj.coords.first() == obj.coords.last();
    let shape = if obj.coords.len() == 1 {
        Shape::Point
    } else if (obj.area && obj.coords.len() >= 3) || (closed && is_area_tags(&t)) {
        Shape::Polygon
    } else {
        Shape::Line
    };
    let outer31: Vec<P> = obj.coords.iter().map(|&(x, y)| (f64::from(x), f64::from(y))).collect();
    let mut outer31_open = outer31.clone();
    if shape == Shape::Polygon && outer31_open.len() > 1 && outer31_open.first() == outer31_open.last() {
        outer31_open.pop();
    }
    let a31 = if shape == Shape::Polygon { ring_area(&outer31_open).abs() } else { 0.0 };
    let a0 = a31 / 2f64.powi(46);
    let outs = classify(&t, shape, a0);
    if outs.is_empty() {
        return res;
    }
    let label31: Option<P> = if outs.iter().any(|o| o.emit == Emit::Label) {
        match (obj.label, shape) {
            (Some((x, y)), _) => Some((f64::from(x), f64::from(y))),
            (None, Shape::Polygon) => label_point(&outer31_open),
            (None, _) => Some(outer31[outer31.len() / 2]),
        }
    } else {
        None
    };
    let inner31: Vec<Vec<P>> = obj
        .inner
        .iter()
        .map(|r| {
            let mut v: Vec<P> = r.iter().map(|&(x, y)| (f64::from(x), f64::from(y))).collect();
            if v.len() > 1 && v.first() == v.last() {
                v.pop();
            }
            v
        })
        .filter(|r| r.len() >= 3)
        .collect();
    let props_bytes: Vec<Vec<u8>> = outs.iter().map(|o| encode_props(&o.props)).collect();
    let mut any = false;

    for &z in zooms {
        let scale = 2f64.powi(i32::from(z) - 19);
        let tol = tol2(z, ctx.max_zoom);
        let at_max = z >= ctx.max_zoom;
        // lazily built per zoom
        let mut line_z: Option<Vec<P>> = None;
        let mut rings_z: Option<(Vec<P>, Vec<Vec<P>>)> = None;
        for (oi, o) in outs.iter().enumerate() {
            if z < o.min_zoom.min(ctx.max_zoom).max(ctx.min_zoom) || z > o.max_zoom {
                continue;
            }
            let geom = match (o.emit, shape) {
                (Emit::Label, _) => match label31 {
                    Some(p) => Geom::Points(vec![(p.0 * scale, p.1 * scale)]),
                    None => continue,
                },
                (Emit::Native, Shape::Point) => Geom::Points(vec![(outer31[0].0 * scale, outer31[0].1 * scale)]),
                (Emit::Native, Shape::Line) => {
                    let l = line_z.get_or_insert_with(|| simplify(&outer31.iter().map(|p| (p.0 * scale, p.1 * scale)).collect::<Vec<_>>(), tol));
                    Geom::Lines(vec![l.clone()])
                }
                (Emit::Outline, _) | (Emit::Native, Shape::Polygon) => {
                    let min_units = if at_max { (o.min_area * 256.0).min(4.0) } else { o.min_area * 256.0 };
                    if o.emit == Emit::Native && a31 * scale * scale < min_units {
                        continue;
                    }
                    let (outer, holes) = rings_z.get_or_insert_with(|| {
                        let sc = |r: &[P]| simplify_ring(&r.iter().map(|p| (p.0 * scale, p.1 * scale)).collect::<Vec<_>>(), tol);
                        (sc(&outer31_open), inner31.iter().map(|r| sc(r)).collect())
                    });
                    if outer.len() < 3 {
                        continue;
                    }
                    if o.emit == Emit::Outline {
                        let mut lines = Vec::with_capacity(1 + holes.len());
                        for r in std::iter::once(&*outer).chain(holes.iter()) {
                            let mut l = r.clone();
                            l.push(r[0]);
                            lines.push(l);
                        }
                        Geom::Lines(lines)
                    } else {
                        let mut poly = vec![outer.clone()];
                        poly.extend(holes.iter().filter(|h| h.len() >= 3 && ring_area(h).abs() >= min_units / 4.0).cloned());
                        Geom::Polys(vec![poly])
                    }
                }
            };
            let b = geom.bounds();
            if !(b[0].is_finite() && b[1].is_finite() && b[2].is_finite() && b[3].is_finite()) {
                continue;
            }
            let (tx0, tx1, ty0, ty1) = tile_range(b, z, ctx.buffer);
            let props = &props_bytes[oi];
            split_tiles(geom, tx0, tx1, ty0, ty1, ctx.buffer, &mut |tx, ty, g| {
                let (gt, enc, touched) = encode_local(&g, f64::from(tx) * E, f64::from(ty) * E);
                if enc.is_empty() {
                    return;
                }
                let id = tile_id(z, tx, ty);
                push_rec(&mut res.blob, id, o.layer, gt, o.sort, props, &enc);
                if touched {
                    res.touched.push(id);
                    any = true;
                }
            });
        }
    }
    if any {
        let mut b = [u32::MAX, u32::MAX, 0, 0];
        for &(x, y) in &obj.coords {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        res.bbox = Some(b);
    }
    res
}

// ================================================================ coastline → land / sea

/// Join coastline ways that continue each other (exact shared end points).
fn merge_lines(lines: Vec<Vec<(u32, u32)>>) -> Vec<Vec<(u32, u32)>> {
    let n = lines.len();
    let mut by_start: FxHashMap<(u32, u32), Vec<usize>> = FxHashMap::default();
    let mut by_end: FxHashMap<(u32, u32), Vec<usize>> = FxHashMap::default();
    for (i, l) in lines.iter().enumerate() {
        by_start.entry(l[0]).or_default().push(i);
        by_end.entry(*l.last().unwrap()).or_default().push(i);
    }
    let mut used = vec![false; n];
    let mut out = Vec::new();
    for i in 0..n {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut cur = lines[i].clone();
        loop {
            if cur.len() > 2 && cur.first() == cur.last() {
                break;
            }
            let e = *cur.last().unwrap();
            let Some(&j) = by_start.get(&e).and_then(|v| v.iter().find(|&&j| !used[j])) else { break };
            used[j] = true;
            cur.extend_from_slice(&lines[j][1..]);
        }
        loop {
            if cur.len() > 2 && cur.first() == cur.last() {
                break;
            }
            let s = cur[0];
            let Some(&k) = by_end.get(&s).and_then(|v| v.iter().find(|&&k| !used[k])) else { break };
            used[k] = true;
            let mut pre = lines[k][..lines[k].len() - 1].to_vec();
            pre.extend_from_slice(&cur);
            cur = pre;
        }
        out.push(cur);
    }
    out
}

enum CoastTile {
    /// coastline crosses the tile border: polygons are known
    Crossing { land: Vec<Vec<Vec<P>>>, sea: Vec<Vec<Vec<P>>>, extended: bool },
    /// only closed rings inside the tile (islands / lagoons); the surroundings come from elsewhere
    Closed(Vec<Vec<P>>),
}

impl CoastTile {
    fn land_at(&self, p: P) -> Option<bool> {
        match self {
            CoastTile::Crossing { land, .. } => Some(land.iter().any(|poly| poly.iter().filter(|r| point_in_ring(p, r)).count() % 2 == 1)),
            CoastTile::Closed(_) => None,
        }
    }
}

/// Tile perimeter position (clockwise on screen from the top-left corner).
fn perim_t(p: P, lo: f64, hi: f64) -> f64 {
    let l = hi - lo;
    if p.1 == lo && p.0 < hi {
        p.0 - lo
    } else if p.0 == hi && p.1 < hi {
        l + (p.1 - lo)
    } else if p.1 == hi && p.0 > lo {
        2.0 * l + (hi - p.0)
    } else {
        3.0 * l + (hi - p.1)
    }
}

/// Close open pieces (both ends on the border, the area to keep on their RIGHT on screen) into
/// rings by walking clockwise along the border from each exit to the next entry.
fn walk_border(pieces: &[Vec<P>], lo: f64, hi: f64) -> Vec<Vec<P>> {
    let n = pieces.len();
    let l = hi - lo;
    let per = 4.0 * l;
    let corners = [(0.0, (lo, lo)), (l, (hi, lo)), (2.0 * l, (hi, hi)), (3.0 * l, (lo, hi))];
    let starts: Vec<f64> = pieces.iter().map(|p| perim_t(p[0], lo, hi)).collect();
    let ends: Vec<f64> = pieces.iter().map(|p| perim_t(*p.last().unwrap(), lo, hi)).collect();
    let mut used = vec![false; n];
    let mut rings = Vec::new();
    for s0 in 0..n {
        if used[s0] {
            continue;
        }
        let mut ring: Vec<P> = Vec::new();
        let mut cur = s0;
        for _ in 0..=n {
            used[cur] = true;
            ring.extend_from_slice(&pieces[cur]);
            let te = ends[cur];
            let mut best = s0;
            let mut bd = f64::MAX;
            for j in 0..n {
                if used[j] && j != s0 {
                    continue;
                }
                let d = (starts[j] - te).rem_euclid(per);
                if d < bd {
                    bd = d;
                    best = j;
                }
            }
            let mut cs: Vec<(f64, P)> = corners.iter().map(|&(ct, cp)| ((ct - te).rem_euclid(per), cp)).filter(|&(d, _)| d > 0.0 && d < bd).collect();
            cs.sort_by(|a, b| a.0.total_cmp(&b.0));
            ring.extend(cs.into_iter().map(|(_, p)| p));
            if best == s0 {
                break;
            }
            cur = best;
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    rings
}

/// Positive rings (clockwise on screen) become exteriors; negative ones holes of the smallest
/// exterior holding them.
fn group_rings(rings: Vec<Vec<P>>) -> Vec<Vec<Vec<P>>> {
    let mut outers: Vec<(f64, Vec<Vec<P>>)> = Vec::new();
    let mut holes = Vec::new();
    for r in rings {
        let a = ring_area(&r);
        if a > 0.0 {
            outers.push((a, vec![r]));
        } else if a < 0.0 {
            holes.push(r);
        }
    }
    for h in holes {
        let p = h[0];
        let best = outers.iter_mut().filter(|(_, o)| point_in_ring(p, &o[0])).min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, o)) = best {
            o.push(h);
        }
    }
    outers.into_iter().map(|(_, o)| o).collect()
}

/// Extend an open end that stops inside the tile along its last segment to the border.
fn extend_to_border(p: P, q: P, lo: f64, hi: f64) -> P {
    let (mut dx, mut dy) = (p.0 - q.0, p.1 - q.1);
    if dx == 0.0 && dy == 0.0 {
        dx = 1.0;
        dy = 0.0;
    }
    let mut s = f64::MAX;
    if dx > 0.0 {
        s = s.min((hi - p.0) / dx);
    } else if dx < 0.0 {
        s = s.min((lo - p.0) / dx);
    }
    if dy > 0.0 {
        s = s.min((hi - p.1) / dy);
    } else if dy < 0.0 {
        s = s.min((lo - p.1) / dy);
    }
    let mut r = (p.0 + dx * s, p.1 + dy * s);
    // snap onto the border
    let snap = |v: f64| if (v - lo).abs() < 1e-6 { lo } else if (v - hi).abs() < 1e-6 { hi } else { v.clamp(lo, hi) };
    r = (snap(r.0), snap(r.1));
    if r.0 != lo && r.0 != hi && r.1 != lo && r.1 != hi {
        // numerical trouble: go to the nearest border
        let d = [(r.0 - lo).abs(), (hi - r.0).abs(), (r.1 - lo).abs(), (hi - r.1).abs()];
        let i = (0..4).min_by(|&a, &b| d[a].total_cmp(&d[b])).unwrap_or(0);
        match i {
            0 => r.0 = lo,
            1 => r.0 = hi,
            2 => r.1 = lo,
            _ => r.1 = hi,
        }
    }
    r
}

/// Land / sea of one tile from its coastline pieces (tile-local coordinates, square [lo, hi]²).
fn assemble_coast(mut pieces: Vec<Vec<P>>, lo: f64, hi: f64) -> Option<CoastTile> {
    let on_b = |p: P| p.0 == lo || p.0 == hi || p.1 == lo || p.1 == hi;
    for p in pieces.iter_mut() {
        p.dedup();
    }
    pieces.retain(|p| p.len() >= 2);
    // join pieces that continue each other inside the tile
    let key = |p: P| (p.0.to_bits(), p.1.to_bits());
    let mut alive = vec![true; pieces.len()];
    let mut by_start: FxHashMap<(u64, u64), Vec<usize>> = FxHashMap::default();
    for (i, p) in pieces.iter().enumerate() {
        if !on_b(p[0]) {
            by_start.entry(key(p[0])).or_default().push(i);
        }
    }
    for i in 0..pieces.len() {
        if !alive[i] {
            continue;
        }
        let mut guard = 0;
        loop {
            guard += 1;
            let end = *pieces[i].last().unwrap();
            if on_b(end) || pieces[i].first() == pieces[i].last() || guard > pieces.len() {
                break;
            }
            let Some(&j) = by_start.get(&key(end)).and_then(|v| v.iter().find(|&&j| j != i && alive[j])) else { break };
            alive[j] = false;
            let tail = std::mem::take(&mut pieces[j]);
            pieces[i].extend_from_slice(&tail[1..]);
        }
    }
    let mut closed = Vec::new();
    let mut open = Vec::new();
    let mut extended = false;
    for (i, mut p) in pieces.into_iter().enumerate() {
        if !alive[i] || p.len() < 2 {
            continue;
        }
        if p.len() >= 4 && p.first() == p.last() && !on_b(p[0]) {
            p.pop();
            closed.push(p);
            continue;
        }
        if !on_b(p[0]) {
            let s = extend_to_border(p[0], p[1], lo, hi);
            p.insert(0, s);
            extended = true;
        }
        let n = p.len();
        if !on_b(p[n - 1]) {
            let e = extend_to_border(p[n - 1], p[n - 2], lo, hi);
            p.push(e);
            extended = true;
        }
        open.push(p);
    }
    if open.is_empty() {
        return (!closed.is_empty()).then_some(CoastTile::Closed(closed));
    }
    // OSM coastlines have the land on their LEFT: reversed, the land is on the right.
    let land_pieces: Vec<Vec<P>> = open.iter().map(|p| p.iter().rev().copied().collect()).collect();
    let mut land_rings = walk_border(&land_pieces, lo, hi);
    let mut sea_rings = walk_border(&open, lo, hi);
    for r in &closed {
        land_rings.push(r.iter().rev().copied().collect());
        sea_rings.push(r.clone());
    }
    Some(CoastTile::Crossing { land: group_rings(land_rings), sea: group_rings(sea_rings), extended })
}

fn square(lo: f64, hi: f64) -> Vec<P> {
    vec![(lo, lo), (hi, lo), (hi, hi), (lo, hi)]
}

fn encode_polys_local(polys: &[Vec<Vec<P>>]) -> Vec<u32> {
    let p: Vec<Vec<Vec<(i32, i32)>>> = polys.iter().map(|poly| poly.iter().map(|r| r.iter().map(|q| round_pt(*q, 0.0, 0.0)).collect()).collect()).collect();
    mvt::encode_polygons(&p)
}

struct EarthOut {
    blob: Vec<u8>,
    /// tiles of this zoom that are written (have records)
    tiles: Vec<(u32, u32)>,
    /// sea tiles without records (shared ocean content)
    ocean_only: Vec<(u32, u32)>,
    coast_tiles: u64,
    extended: u64,
}

/// Tiles fully inside one ocean (false) / land (true) box of the band.
fn box_states(z: u8, boxes: &[OceanBox], cap: usize) -> FxHashMap<(u32, u32), bool> {
    let mut m: FxHashMap<(u32, u32), bool> = FxHashMap::default();
    let sh = 31 - u32::from(z.min(31));
    let size = 1u64 << sh;
    let n = 1u64 << z;
    for b in boxes {
        let [l, t, r, btm] = b.bbox31.map(u64::from);
        if r <= l || btm <= t {
            continue;
        }
        let x0 = l.div_ceil(size);
        let x1 = (r / size).min(n);
        let y0 = t.div_ceil(size);
        let y1 = (btm / size).min(n);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        if m.len() as u64 + (x1 - x0) * (y1 - y0) > cap as u64 {
            log::warn!("obf tiles: z{z} ocean/land boxes cover more than {cap} tiles; the rest is skipped");
            break;
        }
        for x in x0..x1 {
            for y in y0..y1 {
                let e = m.entry((x as u32, y as u32)).or_insert(!b.ocean);
                if b.ocean {
                    *e = false; // ocean wins over a conflicting land flag
                }
            }
        }
    }
    m
}

fn neighbours(x: u32, y: u32, z: u8) -> impl Iterator<Item = (usize, (u32, u32))> {
    let n = 1i64 << z;
    let (x, y) = (i64::from(x), i64::from(y));
    // 0 top, 1 right, 2 bottom, 3 left
    [(0, x, y - 1), (1, x + 1, y), (2, x, y + 1), (3, x - 1, y)]
        .into_iter()
        .filter(move |&(_, a, b)| a >= 0 && b >= 0 && a < n && b < n)
        .map(|(d, a, b)| (d, (a as u32, b as u32)))
}

/// Land / sea records of zoom `z` for every tile that has data.
fn earth_zoom(z: u8, coasts: &[Vec<(u32, u32)>], boxes: &[OceanBox], touched: &FxHashSet<(u32, u32)>, ctx: &Ctx) -> EarthOut {
    let buf = ctx.buffer;
    let (lo, hi) = (-buf, E + buf);
    let scale = 2f64.powi(i32::from(z) - 19);
    let tol = tol2(z, ctx.max_zoom);

    // coastline pieces per tile (tile-local coordinates)
    let pieces: Vec<((u32, u32), Vec<P>)> = coasts
        .par_iter()
        .flat_map_iter(|c| {
            let line = simplify(&c.iter().map(|&(x, y)| (f64::from(x) * scale, f64::from(y) * scale)).collect::<Vec<_>>(), tol);
            let mut v = Vec::new();
            if line.len() < 2 {
                return v;
            }
            let g = Geom::Lines(vec![line]);
            let (tx0, tx1, ty0, ty1) = tile_range(g.bounds(), z, buf);
            split_tiles(g, tx0, tx1, ty0, ty1, buf, &mut |tx, ty, g| {
                let (ox, oy) = (f64::from(tx) * E, f64::from(ty) * E);
                if let Geom::Lines(ls) = g {
                    for l in ls {
                        v.push(((tx, ty), l.into_iter().map(|p| (p.0 - ox, p.1 - oy)).collect()));
                    }
                }
            });
            v
        })
        .collect();
    let mut per_tile: FxHashMap<(u32, u32), Vec<Vec<P>>> = FxHashMap::default();
    for (k, p) in pieces {
        per_tile.entry(k).or_default().push(p);
    }
    let coast: FxHashMap<(u32, u32), CoastTile> = per_tile.into_par_iter().filter_map(|(k, ps)| assemble_coast(ps, lo, hi).map(|c| (k, c))).collect();
    let crossing = |k: &(u32, u32)| matches!(coast.get(k), Some(CoastTile::Crossing { .. }));

    // tiles to decide: data tiles, coastline tiles and a small sea margin around the coast
    let mut nodes: FxHashSet<(u32, u32)> = touched.clone();
    nodes.extend(coast.keys().copied());
    let n = 1i64 << z;
    let mut margin: FxHashSet<(u32, u32)> = FxHashSet::default();
    for (&(x, y), c) in &coast {
        if !matches!(c, CoastTile::Crossing { .. }) {
            continue;
        }
        for dx in -SEA_RADIUS..=SEA_RADIUS {
            for dy in -SEA_RADIUS..=SEA_RADIUS {
                let (a, b) = (i64::from(x) + dx, i64::from(y) + dy);
                if a >= 0 && b >= 0 && a < n && b < n && !nodes.contains(&(a as u32, b as u32)) {
                    margin.insert((a as u32, b as u32));
                }
            }
        }
    }
    nodes.extend(margin.iter().copied());

    // flood fill: edges of coastline tiles first, then ocean / land boxes, then land
    let boxed = box_states(z, boxes, MAX_OCEAN_TILES_PER_ZOOM);
    let mut state: FxHashMap<(u32, u32), bool> = FxHashMap::default();
    let mut q: VecDeque<(u32, u32)> = VecDeque::new();
    let mids = [(E / 2.0, 0.0), (E, E / 2.0), (E / 2.0, E), (0.0, E / 2.0)];
    for (&(x, y), c) in &coast {
        for (d, nb) in neighbours(x, y, z) {
            if !nodes.contains(&nb) || crossing(&nb) || state.contains_key(&nb) {
                continue;
            }
            if let Some(s) = c.land_at(mids[d]) {
                state.insert(nb, s);
                q.push_back(nb);
            }
        }
    }
    let flood = |state: &mut FxHashMap<(u32, u32), bool>, q: &mut VecDeque<(u32, u32)>| {
        while let Some(t) = q.pop_front() {
            let s = state[&t];
            for (_, nb) in neighbours(t.0, t.1, z) {
                if nodes.contains(&nb) && !crossing(&nb) && !state.contains_key(&nb) {
                    state.insert(nb, s);
                    q.push_back(nb);
                }
            }
        }
    };
    flood(&mut state, &mut q);
    for k in &nodes {
        if !state.contains_key(k) && !crossing(k) {
            if let Some(&s) = boxed.get(k) {
                state.insert(*k, s);
                q.push_back(*k);
            }
        }
    }
    flood(&mut state, &mut q);

    let full = vec![vec![square(lo, hi)]];
    let earth_props = encode_props(&[kv("kind", "earth")]);
    let ocean_props = encode_props(&[kv("kind", "ocean")]);
    let mut out = EarthOut { blob: Vec::new(), tiles: Vec::new(), ocean_only: Vec::new(), coast_tiles: 0, extended: 0 };
    let mut list: Vec<(u32, u32)> = nodes.iter().copied().collect();
    list.sort_unstable();
    for k in list {
        let is_margin = margin.contains(&k);
        let land = state.get(&k).copied().unwrap_or(true);
        let (land_polys, sea_polys): (Vec<Vec<Vec<P>>>, Vec<Vec<Vec<P>>>) = match coast.get(&k) {
            Some(CoastTile::Crossing { land, sea, extended }) => {
                out.coast_tiles += 1;
                out.extended += u64::from(*extended);
                (land.clone(), sea.clone())
            }
            Some(CoastTile::Closed(rings)) => {
                // rings run with the land on their left (counter-clockwise on screen around islands)
                let islands: Vec<Vec<P>> = rings.iter().filter(|r| ring_area(r) < 0.0).map(|r| r.iter().rev().copied().collect()).collect();
                let lagoons: Vec<Vec<P>> = rings.iter().filter(|r| ring_area(r) > 0.0).cloned().collect();
                if land {
                    let mut e = vec![square(lo, hi)];
                    e.extend(lagoons.iter().map(|r| r.iter().rev().copied().collect::<Vec<P>>()));
                    (vec![e], lagoons.into_iter().map(|r| vec![r]).collect())
                } else {
                    let mut s = vec![square(lo, hi)];
                    s.extend(islands.iter().map(|r| r.iter().rev().copied().collect::<Vec<P>>()));
                    (islands.into_iter().map(|r| vec![r]).collect(), vec![s])
                }
            }
            None if land => (full.clone(), Vec::new()),
            None => (Vec::new(), full.clone()),
        };
        if is_margin && !coast.contains_key(&k) {
            // margin tiles are written only as plain sea
            if !land {
                out.ocean_only.push(k);
            }
            continue;
        }
        let id = tile_id(z, k.0, k.1);
        let mut wrote = false;
        let g = encode_polys_local(&land_polys);
        if !g.is_empty() {
            push_rec(&mut out.blob, id, L_EARTH, GEOM_POLYGON, 0, &earth_props, &g);
            wrote = true;
        }
        let g = encode_polys_local(&sea_polys);
        if !g.is_empty() {
            push_rec(&mut out.blob, id, L_WATER, GEOM_POLYGON, 0, &ocean_props, &g);
            wrote = true;
        }
        if wrote {
            out.tiles.push(k);
        }
    }
    // ocean boxes: sea tiles without data
    for (&k, &land) in &boxed {
        if !land && !nodes.contains(&k) {
            out.ocean_only.push(k);
        }
    }
    out
}

// ================================================================ build

fn band_for(levels: &[MapLevel], z: u8) -> usize {
    let containing = (0..levels.len()).filter(|&i| levels[i].min_zoom <= z && z <= levels[i].max_zoom).min_by_key(|&i| levels[i].max_zoom);
    if let Some(i) = containing {
        return i;
    }
    let above = (0..levels.len()).filter(|&i| levels[i].min_zoom > z).min_by_key(|&i| levels[i].min_zoom);
    above.unwrap_or_else(|| (0..levels.len()).max_by_key(|&i| levels[i].max_zoom).unwrap_or(0))
}

fn lon_of(x31: f64) -> f64 {
    x31 / 2f64.powi(31) * 360.0 - 180.0
}

fn lat_of(y31: f64) -> f64 {
    let n = std::f64::consts::PI * (1.0 - 2.0 * y31 / 2f64.powi(31));
    n.sinh().atan().to_degrees()
}

fn layer_fields() -> Vec<(String, Vec<(String, String)>)> {
    let s = |v: &[&str]| v.iter().map(|k| (k.to_string(), "String".to_string())).collect::<Vec<_>>();
    let names = ["name", "name:he", "name:en", "script"];
    let with = |base: &[&str], nums: &[&str], bools: &[&str]| {
        let mut v = s(base);
        v.extend(s(&names));
        v.extend(nums.iter().map(|k| (k.to_string(), "Number".to_string())));
        v.extend(bools.iter().map(|k| (k.to_string(), "Boolean".to_string())));
        v
    };
    LAYERS
        .iter()
        .map(|&l| {
            let f = match l {
                "earth" => with(&["kind"], &["min_zoom"], &[]),
                "landcover" | "landuse" => s(&["kind"]),
                "water" => with(&["kind", "kind_detail"], &["min_zoom"], &["is_tunnel"]),
                "roads" => with(&["kind", "kind_detail", "ref", "shield_text", "oneway"], &["min_zoom"], &["is_bridge", "is_tunnel", "is_link"]),
                "buildings" => with(&["kind", "addr_housenumber"], &["height", "min_height", "min_zoom"], &[]),
                "pois" => with(&["kind"], &["min_zoom"], &[]),
                "places" => with(&["kind", "kind_detail", "capital", "ref"], &["min_zoom", "population", "population_rank", "sort_key"], &[]),
                _ => {
                    let mut v = s(&["kind"]);
                    v.push(("kind_detail".into(), "Number".into()));
                    v.push(("min_zoom".into(), "Number".into()));
                    v
                }
            };
            (l.to_string(), f)
        })
        .collect()
}

/// Build `out` (a PMTiles v3 vector archive) from the map section of `src`.
pub fn build(src: &dyn MapSource, out: &Path, opts: TileOptions, progress: ProgressFn) -> anyhow::Result<TileSummary> {
    let levels = src.levels();
    if levels.is_empty() {
        bail!("the OBF file has no map section");
    }
    let max_zoom = opts.max_zoom.min(20);
    let min_zoom = opts.min_zoom.min(max_zoom);
    let ctx = Ctx { min_zoom, max_zoom, buffer: f64::from(if opts.buffer == 0 { 64 } else { opts.buffer }) };
    let nb = if opts.buckets == 0 { 1024 } else { opts.buckets };
    let parent = out.parent().unwrap_or(Path::new("."));
    let tmp = TmpDir(opts.tmp_dir.clone().unwrap_or_else(|| parent.to_path_buf()).join(format!(".obf-tiles-{}-{}", std::process::id(), out.file_stem().and_then(|s| s.to_str()).unwrap_or("map"))));
    let _ = std::fs::remove_dir_all(&tmp.0);
    std::fs::create_dir_all(&tmp.0).with_context(|| format!("create {}", tmp.0.display()))?;
    let mut spill = Spill::new(&tmp.0, nb)?;

    let zoom_band: Vec<usize> = (0..=max_zoom).map(|z| band_for(&levels, z)).collect();
    let mut coasts: Vec<Vec<Vec<(u32, u32)>>> = vec![Vec::new(); levels.len()];
    let mut boxes: Vec<Vec<OceanBox>> = vec![Vec::new(); levels.len()];
    let mut touched: FxHashSet<u64> = FxHashSet::default();
    let mut bbox31 = [u32::MAX, u32::MAX, 0u32, 0u32];
    let mut objects = 0u64;
    let report = |done: u64, msg: &str| -> anyhow::Result<()> {
        if !progress(ProgressPhase::Processing, done, 1000, msg) {
            bail!("cancelled");
        }
        Ok(())
    };

    // ---- 1. objects → spilled records, band by band
    let bands_used: Vec<usize> = {
        let mut v: Vec<usize> = zoom_band[min_zoom as usize..].to_vec();
        v.sort_unstable();
        v.dedup();
        v
    };
    for (bi, &band) in bands_used.iter().enumerate() {
        let zooms: Vec<u8> = (min_zoom..=max_zoom).filter(|&z| zoom_band[z as usize] == band).collect();
        boxes[band] = src.ocean_boxes(band)?;
        let lv = levels[band];
        let base = 600 * bi as u64 / bands_used.len() as u64;
        report(base, &format!("קריאת אובייקטי מפה (רמה {}–{})", lv.min_zoom, lv.max_zoom))?;
        let stop = AtomicBool::new(false);
        let mut cancelled = false;
        let mut err: Option<anyhow::Error> = None;
        std::thread::scope(|s| -> anyhow::Result<()> {
            let (tx, rx) = sync_channel::<Vec<MapObject>>(2);
            let stop_r = &stop;
            let reader = s.spawn(move || {
                let mut batch: Vec<MapObject> = Vec::with_capacity(BATCH);
                let r = src.for_each_object(band, &mut |o| {
                    if stop_r.load(Ordering::Relaxed) {
                        return;
                    }
                    batch.push(o);
                    if batch.len() >= BATCH && tx.send(std::mem::replace(&mut batch, Vec::with_capacity(BATCH))).is_err() {
                        stop_r.store(true, Ordering::Relaxed);
                    }
                });
                if !batch.is_empty() && !stop_r.load(Ordering::Relaxed) {
                    let _ = tx.send(batch);
                }
                r
            });
            for batch in rx.iter() {
                objects += batch.len() as u64;
                let results: Vec<ObjResult> = batch.par_iter().map(|o| process_object(o, &zooms, &ctx)).collect();
                drop(batch);
                for r in results {
                    if let Err(e) = spill.write_blob(&r.blob) {
                        err = Some(e);
                        break;
                    }
                    touched.extend(r.touched);
                    if let Some(c) = r.coast {
                        coasts[band].push(c);
                    }
                    if let Some(b) = r.bbox {
                        bbox31 = [bbox31[0].min(b[0]), bbox31[1].min(b[1]), bbox31[2].max(b[2]), bbox31[3].max(b[3])];
                    }
                }
                if err.is_some() {
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
                if !progress(ProgressPhase::Processing, base, 1000, &format!("עיבוד אובייקטי מפה: {objects}")) {
                    cancelled = true;
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
            }
            drop(rx);
            reader.join().map_err(|_| anyhow!("OBF reader thread panicked"))??;
            Ok(())
        })?;
        if let Some(e) = err {
            return Err(e);
        }
        if cancelled {
            bail!("cancelled");
        }
        coasts[band] = merge_lines(std::mem::take(&mut coasts[band]));
    }

    // ---- 2. earth / ocean per zoom
    let mut by_zoom: Vec<FxHashSet<(u32, u32)>> = vec![FxHashSet::default(); max_zoom as usize + 1];
    for &id in &touched {
        let (z, x, y) = tile_zxy(id);
        if let Some(s) = by_zoom.get_mut(z as usize) {
            s.insert((x, y));
        }
    }
    drop(touched);
    let mut output: FxHashSet<u64> = FxHashSet::default();
    let mut ocean_only: Vec<(u8, u32, u32)> = Vec::new();
    let (mut coast_tiles, mut extended) = (0u64, 0u64);
    for z in min_zoom..=max_zoom {
        report(600 + 100 * u64::from(z - min_zoom) / u64::from(max_zoom - min_zoom + 1), &format!("יבשה וים, זום {z}"))?;
        let band = zoom_band[z as usize];
        let eo = earth_zoom(z, &coasts[band], &boxes[band], &by_zoom[z as usize], &ctx);
        spill.write_blob(&eo.blob)?;
        coast_tiles += eo.coast_tiles;
        extended += eo.extended;
        output.extend(eo.tiles.iter().map(|&(x, y)| tile_id(z, x, y)));
        ocean_only.extend(eo.ocean_only.iter().map(|&(x, y)| (z, x, y)));
    }
    drop(coasts);
    drop(by_zoom);
    let features = spill.records;
    let paths = spill.finish()?;

    // ---- 3. assemble tiles
    let data_tmp = tmp.0.join("tiles.data.tmp");
    let writer = Mutex::new(pmtiles_out::Writer::create(&data_tmp)?);
    let per_zoom: Vec<AtomicU64> = (0..=max_zoom).map(|_| AtomicU64::new(0)).collect();
    let done = AtomicU64::new(0);
    let cancel = AtomicBool::new(false);
    paths.par_iter().try_for_each(|path| -> anyhow::Result<()> {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let data = std::fs::read(path)?;
        let _ = std::fs::remove_file(path);
        let mut recs: Vec<RecRef> = Vec::new();
        let mut p = 0;
        while p + 12 <= data.len() {
            let tile = u64::from_le_bytes(data[p..p + 8].try_into()?);
            let len = u32::from_le_bytes(data[p + 8..p + 12].try_into()?) as usize;
            let payload = data.get(p + 12..p + 12 + len).ok_or_else(|| anyhow!("truncated spill file"))?;
            p += 12 + len;
            if payload.len() < 10 || !output.contains(&tile) {
                continue;
            }
            let sort = u32::from_le_bytes(payload[2..6].try_into()?);
            recs.push(RecRef { tile, layer: payload[0], sort, seq: recs.len() as u32, payload });
        }
        recs.sort_unstable_by_key(|r| (r.tile, r.layer, r.sort, r.seq));
        let mut tiles_out: Vec<(u8, u32, u32, Vec<u8>)> = Vec::new();
        for group in recs.chunk_by(|a, b| a.tile == b.tile) {
            let mut layers: Vec<Vec<Feature>> = vec![Vec::new(); LAYERS.len()];
            for r in group {
                if let Some(l) = layers.get_mut(r.layer as usize) {
                    l.push(parse_feature(r.payload)?);
                }
            }
            let named: Vec<(&str, &[Feature])> = LAYERS.iter().zip(&layers).map(|(n, f)| (*n, f.as_slice())).collect();
            let raw = mvt::encode_tile(&named);
            if raw.is_empty() {
                continue;
            }
            let (z, x, y) = tile_zxy(group[0].tile);
            tiles_out.push((z, x, y, pmtiles_out::gzip(&raw)?));
        }
        drop(recs);
        drop(data);
        {
            let mut w = writer.lock();
            for (z, x, y, b) in &tiles_out {
                w.add(*z, *x, *y, b)?;
                per_zoom[*z as usize].fetch_add(1, Ordering::Relaxed);
            }
        }
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        if !progress(ProgressPhase::Processing, 700 + 280 * d / paths.len() as u64, 1000, "בניית אריחי מפה") {
            cancel.store(true, Ordering::Relaxed);
            bail!("cancelled");
        }
        Ok(())
    })?;
    let mut writer = writer.into_inner();
    if !ocean_only.is_empty() {
        let full = vec![vec![square(-ctx.buffer, E + ctx.buffer)]];
        let f = Feature { geom_type: GEOM_POLYGON, geometry: encode_polys_local(&full), props: vec![kv("kind", "ocean")] };
        let feats = [f];
        let gz = pmtiles_out::gzip(&mvt::encode_tile(&[("water", &feats)]))?;
        ocean_only.sort_unstable();
        ocean_only.dedup();
        for &(z, x, y) in &ocean_only {
            if output.contains(&tile_id(z, x, y)) {
                continue;
            }
            writer.add(z, x, y, &gz)?;
            per_zoom[z as usize].fetch_add(1, Ordering::Relaxed);
        }
    }
    if writer.tiles() == 0 {
        bail!("the OBF map section produced no tiles");
    }

    // ---- 4. archive
    report(985, "כתיבת קובץ המפה")?;
    let bbox = if bbox31[0] <= bbox31[2] {
        [lon_of(f64::from(bbox31[0])), lat_of(f64::from(bbox31[3])), lon_of(f64::from(bbox31[2])), lat_of(f64::from(bbox31[1]))]
    } else {
        let b = levels.iter().fold([u32::MAX, u32::MAX, 0, 0], |a, l| [a[0].min(l.bbox31[0]), a[1].min(l.bbox31[1]), a[2].max(l.bbox31[2]), a[3].max(l.bbox31[3])]);
        [lon_of(f64::from(b[0])), lat_of(f64::from(b[3])), lon_of(f64::from(b[2])), lat_of(f64::from(b[1]))]
    };
    let meta = pmtiles_out::VectorMeta {
        name: opts.name.clone(),
        description: "OsmAnd map converted to the Protomaps basemap schema (v4)".into(),
        attribution: opts.attribution.clone(),
        bbox,
        min_zoom,
        max_zoom,
        layers: layer_fields(),
    };
    let part = out.with_extension("pmtiles.part");
    let stats = writer.finish(&part, &meta)?;
    std::fs::rename(&part, out).with_context(|| format!("rename to {}", out.display()))?;
    progress(ProgressPhase::Processing, 1000, 1000, "");
    Ok(TileSummary {
        tiles_per_zoom: per_zoom.iter().map(|a| a.load(Ordering::Relaxed)).collect(),
        tiles: stats.addressed_tiles,
        tile_entries: stats.tile_entries,
        tile_contents: stats.tile_contents,
        bytes: stats.bytes,
        objects,
        features,
        bbox,
        coast_tiles,
        coast_extended_tiles: extended,
        ocean_only_tiles: ocean_only.len() as u64,
    })
}

// ================================================================ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obf::mvt::{decode_tile, Decoded};

    fn x31(lon: f64) -> u32 {
        ((lon + 180.0) / 360.0 * 2f64.powi(31)) as u32
    }
    fn y31(lat: f64) -> u32 {
        let r = lat.to_radians();
        ((1.0 - r.tan().asinh() / std::f64::consts::PI) / 2.0 * 2f64.powi(31)) as u32
    }
    fn pt(lon: f64, lat: f64) -> (u32, u32) {
        (x31(lon), y31(lat))
    }
    fn tags(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn clip_line_and_polygon() {
        let mut out = Vec::new();
        clip_line_axis(&[(-10.0, 5.0), (10.0, 5.0), (30.0, 5.0)], 0, 0.0, 20.0, &mut out);
        assert_eq!(out, vec![vec![(0.0, 5.0), (10.0, 5.0), (20.0, 5.0)]]);
        // leaves and comes back: two pieces
        let mut out = Vec::new();
        clip_line_axis(&[(5.0, 0.0), (30.0, 0.0), (30.0, 10.0), (5.0, 10.0)], 0, 0.0, 20.0, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], vec![(5.0, 0.0), (20.0, 0.0)]);
        assert_eq!(out[1], vec![(20.0, 10.0), (5.0, 10.0)]);
        // square half outside
        let g = Geom::Polys(vec![vec![vec![(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)]]]);
        let c = clip_rect(&g, 0.0, 0.0, 100.0, 100.0);
        let Geom::Polys(p) = c else { panic!() };
        assert!((ring_area(&p[0][0]).abs() - 100.0).abs() < 1e-9);
        // fully outside
        assert!(clip_rect(&g, 50.0, 50.0, 60.0, 60.0).is_empty());
    }

    #[test]
    fn split_into_tiles() {
        // a line across 3 tiles horizontally at zoom-local world pixels
        let g = Geom::Lines(vec![vec![(100.0, 100.0), (3.0 * E - 100.0, 100.0)]]);
        let mut got = Vec::new();
        split_tiles(g, 0, 2, 0, 0, 64.0, &mut |x, y, g| got.push((x, y, g)));
        assert_eq!(got.len(), 3);
        for (x, _, g) in &got {
            let b = g.bounds();
            assert!(b[0] >= f64::from(*x) * E - 64.0 && b[2] <= f64::from(*x + 1) * E + 64.0);
        }
        // a polygon covering 2x2 tiles appears in all four
        let g = Geom::Polys(vec![vec![vec![(10.0, 10.0), (2.0 * E - 10.0, 10.0), (2.0 * E - 10.0, 2.0 * E - 10.0), (10.0, 2.0 * E - 10.0)]]]);
        let mut n = 0;
        split_tiles(g, 0, 1, 0, 1, 64.0, &mut |_, _, _| n += 1);
        assert_eq!(n, 4);
    }

    #[test]
    fn coast_land_left() {
        // coastline heading west through the middle: land on its left = south = bottom half
        let (lo, hi) = (-64.0, E + 64.0);
        let m = E / 2.0;
        let c = assemble_coast(vec![vec![(hi, m), (lo, m)]], lo, hi).unwrap();
        let CoastTile::Crossing { land, sea, .. } = &c else { panic!() };
        assert_eq!(land.len(), 1);
        assert_eq!(sea.len(), 1);
        assert!(!c.land_at((m, 10.0)).unwrap());
        assert!(c.land_at((m, E - 10.0)).unwrap());
        assert!(ring_area(&land[0][0]) > 0.0 && ring_area(&sea[0][0]) > 0.0);
        // dangling end inside the tile is extended along its direction
        let c = assemble_coast(vec![vec![(hi, m), (m, m)]], lo, hi).unwrap();
        assert!(matches!(c, CoastTile::Crossing { extended: true, .. }));
        assert!(c.land_at((100.0, E - 100.0)).unwrap());
        // an island (counter-clockwise on screen) alone is a Closed tile
        let island = vec![(100.0, 100.0), (100.0, 200.0), (200.0, 200.0), (200.0, 100.0), (100.0, 100.0)];
        assert!(matches!(assemble_coast(vec![island], lo, hi), Some(CoastTile::Closed(_))));
    }

    #[test]
    fn classification_table() {
        let c = |t: &[(&str, &str)], s: Shape| classify(&Tags(&tags(t)), s, 1.0);
        let r = c(&[("highway", "primary"), ("ref", "40;41"), ("name", "דרך"), ("bridge", "yes")], Shape::Line);
        assert_eq!(r[0].layer, L_ROADS);
        assert!(r[0].props.contains(&kv("kind", "major_road")));
        assert!(r[0].props.contains(&kv("shield_text", "40")));
        assert!(r[0].props.contains(&kv("is_bridge", true)));
        assert!(r[0].props.contains(&kv("script", "Hebrew")));
        let r = c(&[("building", "yes"), ("building:levels", "3"), ("addr:housenumber", "12")], Shape::Polygon);
        assert!(r.iter().any(|o| o.layer == L_BUILDINGS && o.props.contains(&kv("kind", "building"))));
        assert!(r.iter().any(|o| o.layer == L_BUILDINGS && o.props.contains(&kv("addr_housenumber", "12"))));
        let r = c(&[("landuse", "forest"), ("name", "Kibira")], Shape::Polygon);
        assert!(r.iter().any(|o| o.layer == L_LANDUSE && o.props.contains(&kv("kind", "forest"))));
        assert!(r.iter().any(|o| o.layer == L_LANDCOVER && o.props.contains(&kv("kind", "forest"))));
        assert!(r.iter().any(|o| o.layer == L_POIS && o.emit == Emit::Label));
        let r = c(&[("boundary", "administrative"), ("admin_level", "2")], Shape::Line);
        assert_eq!(r.len(), 1);
        assert!(r[0].props.contains(&kv("kind_detail", 2i64)));
        let r = c(&[("amenity", "cafe"), ("name", "Café")], Shape::Point);
        assert!(r[0].props.contains(&kv("kind", "cafe")) && r[0].props.contains(&kv("min_zoom", 16i64)));
        assert!(c(&[("amenity", "cafe")], Shape::Point).is_empty());
        let r = c(&[("place", "city"), ("name", "Bujumbura"), ("population", "1,000,000"), ("capital", "yes")], Shape::Point);
        assert!(r[0].props.contains(&kv("kind", "locality")) && r[0].props.contains(&kv("capital", "yes")));
        assert!(c(&[("natural", "water"), ("water", "lake")], Shape::Polygon)[0].props.contains(&kv("kind", "lake")));
    }

    // ---------------------------------------------------------------- synthetic source

    struct Synth {
        objects: Vec<MapObject>,
        boxes: Vec<OceanBox>,
    }

    impl MapSource for Synth {
        fn levels(&self) -> Vec<MapLevel> {
            vec![MapLevel { min_zoom: 9, max_zoom: 22, bbox31: [x31(33.8), y31(32.2), x31(34.2), y31(31.8)] }]
        }
        fn for_each_object(&self, _level: usize, f: &mut dyn FnMut(MapObject)) -> anyhow::Result<()> {
            for o in &self.objects {
                f(o.clone());
            }
            Ok(())
        }
        fn ocean_boxes(&self, _level: usize) -> anyhow::Result<Vec<OceanBox>> {
            Ok(self.boxes.clone())
        }
    }

    fn obj(t: &[(&str, &str)], coords: Vec<(u32, u32)>, area: bool) -> MapObject {
        MapObject { id: 1, tags: tags(t), coords, inner: vec![], area, label: None }
    }

    fn tile_at(path: &Path, z: u8, lon: f64, lat: f64) -> Option<Vec<(String, u32, Vec<Decoded>)>> {
        let (x, y) = (x31(lon) >> (31 - z), y31(lat) >> (31 - z));
        let raw = crate::map_tiles::read_tile_uncached(path, z, x, y).unwrap()?; // the reader un-gzips
        Some(decode_tile(&raw).unwrap())
    }

    fn kinds(t: &[(String, u32, Vec<Decoded>)], layer: &str) -> Vec<String> {
        t.iter()
            .filter(|(n, _, _)| n == layer)
            .flat_map(|(_, _, fs)| fs.iter())
            .filter_map(|f| f.props.iter().find(|(k, _)| k == "kind").and_then(|(_, v)| if let Value::Str(s) = v { Some(s.clone()) } else { None }))
            .collect()
    }

    #[test]
    fn synthetic_source_end_to_end() {
        let lake = vec![pt(34.0, 32.045), pt(34.01, 32.045), pt(34.01, 32.055), pt(34.0, 32.055), pt(34.0, 32.045)];
        let src = Synth {
            objects: vec![
                // coastline heading east (land on its left = north), drawn as two ways that meet
                obj(&[("natural", "coastline")], vec![pt(34.0, 32.0), pt(34.15, 32.0)], false),
                obj(&[("natural", "coastline")], vec![pt(33.85, 32.0), pt(34.0, 32.0)], false),
                obj(&[("highway", "primary"), ("name", "Main road"), ("ref", "4")], vec![pt(33.95, 32.02), pt(34.05, 32.02)], false),
                obj(&[("highway", "residential"), ("name", "רחוב")], vec![pt(34.0, 32.03), pt(34.003, 32.031)], false),
                obj(&[("natural", "water"), ("water", "lake"), ("name", "Lake")], lake, true),
                obj(&[("place", "city"), ("name", "Synth City"), ("population", "250000")], vec![pt(34.02, 32.03)], false),
                obj(&[("amenity", "cafe"), ("name", "Cafe")], vec![pt(34.001, 32.031)], false),
            ],
            boxes: vec![OceanBox { bbox31: [x31(33.9), y31(31.97), x31(34.1), y31(31.85)], ocean: true }],
        };
        let dir = std::env::temp_dir().join(format!("mo-obf-tiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("map.pmtiles");
        let prog = |_: ProgressPhase, _: u64, _: u64, _: &str| true;
        let opts = TileOptions { max_zoom: 14, buckets: 8, tmp_dir: Some(dir.clone()), ..Default::default() };
        let s = build(&src, &out, opts, &prog).unwrap();
        assert_eq!(s.tiles_per_zoom.len(), 15);
        assert!(s.tiles_per_zoom.iter().all(|&n| n > 0), "{s:?}");
        assert!(s.coast_tiles > 0 && s.ocean_only_tiles > 0, "{s:?}");
        assert!(s.bbox[0] > 33.8 && s.bbox[2] < 34.2 && s.bbox[1] > 31.9 && s.bbox[3] < 32.1, "{:?}", s.bbox);

        // land tile with the road, lake, city and cafe
        let t = tile_at(&out, 14, 34.0, 32.03).expect("land tile");
        assert_eq!(kinds(&t, "earth"), vec!["earth"]);
        assert!(!kinds(&t, "water").contains(&"ocean".to_string()), "{:?}", kinds(&t, "water"));
        assert!(kinds(&t, "roads").contains(&"minor_road".to_string()));
        assert!(kinds(&t, "pois").contains(&"cafe".to_string()));
        let t = tile_at(&out, 14, 34.02, 32.03).expect("city tile");
        assert!(kinds(&t, "places").contains(&"locality".to_string()));
        let t = tile_at(&out, 14, 34.005, 32.05).expect("lake tile");
        assert!(kinds(&t, "water").contains(&"lake".to_string()));
        let t = tile_at(&out, 11, 34.0, 32.02).expect("z11 road tile");
        assert!(kinds(&t, "roads").contains(&"major_road".to_string()));
        assert!(!kinds(&t, "roads").contains(&"minor_road".to_string()), "minor roads start at z12: {:?}", kinds(&t, "roads"));
        // the coast: both land and sea in the same tile
        let t = tile_at(&out, 14, 34.0, 32.0).expect("coast tile");
        assert_eq!(kinds(&t, "earth"), vec!["earth"]);
        assert_eq!(kinds(&t, "water"), vec!["ocean"]);
        let mean_y = |layer: &str| {
            let f = &t.iter().find(|(n, _, _)| n == layer).unwrap().2[0];
            let pts: Vec<_> = f.parts.iter().flatten().collect();
            pts.iter().map(|p| f64::from(p.1)).sum::<f64>() / pts.len() as f64
        };
        assert!(mean_y("earth") < mean_y("water"), "land must be north of the sea");
        // open sea from the ocean box: water only
        let t = tile_at(&out, 14, 34.0, 31.9).expect("ocean tile");
        assert!(kinds(&t, "earth").is_empty());
        assert_eq!(kinds(&t, "water"), vec!["ocean"]);
        // far away: no tile at all (no filler outside the data)
        assert!(tile_at(&out, 14, 34.5, 32.5).is_none());
        assert!(tile_at(&out, 10, 35.5, 33.5).is_none());
        // low zoom: one tile with land and sea
        let t = tile_at(&out, 5, 34.0, 32.0).expect("z5 tile");
        assert!(!kinds(&t, "earth").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn landlocked_gets_full_earth() {
        let src = Synth {
            objects: vec![obj(&[("highway", "motorway")], vec![pt(29.3, -3.4), pt(29.4, -3.3)], false), obj(&[("natural", "water"), ("name", "Tanganyika")], vec![pt(29.2, -3.6), pt(29.3, -3.6), pt(29.3, -3.7), pt(29.2, -3.6)], true)],
            boxes: vec![],
        };
        let dir = std::env::temp_dir().join(format!("mo-obf-tiles-ll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("map.pmtiles");
        let prog = |_: ProgressPhase, _: u64, _: u64, _: &str| true;
        let s = build(&src, &out, TileOptions { max_zoom: 12, buckets: 4, tmp_dir: Some(dir.clone()), ..Default::default() }, &prog).unwrap();
        assert_eq!(s.coast_tiles, 0);
        assert_eq!(s.ocean_only_tiles, 0);
        let t = tile_at(&out, 12, 29.35, -3.35).expect("road tile");
        assert_eq!(kinds(&t, "earth"), vec!["earth"]);
        assert!(kinds(&t, "roads").contains(&"highway".to_string()));
        let t = tile_at(&out, 10, 29.25, -3.62).expect("lake tile");
        assert!(kinds(&t, "water").contains(&"water".to_string()));
        assert!(kinds(&t, "water").iter().all(|k| k != "ocean"));
        // cancellation
        let no = |_: ProgressPhase, _: u64, _: u64, _: &str| false;
        assert!(build(&src, &dir.join("c.pmtiles"), TileOptions { max_zoom: 8, buckets: 2, tmp_dir: Some(dir.clone()), ..Default::default() }, &no).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `cargo test --release --features cli --lib obf::tiles::tests::synthetic_scale -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn synthetic_scale() {
        let mut objects = Vec::new();
        // 300k buildings and a road grid over ~0.6 x 0.6 degrees, one huge forest polygon
        for i in 0..550u32 {
            for j in 0..550u32 {
                let (lon, lat) = (29.0 + f64::from(i) * 0.001, -3.0 - f64::from(j) * 0.001);
                let d = 0.0002;
                objects.push(obj(&[("building", "yes")], vec![pt(lon, lat), pt(lon + d, lat), pt(lon + d, lat - d), pt(lon, lat - d), pt(lon, lat)], true));
            }
            let lon = 29.0 + f64::from(i) * 0.001;
            objects.push(obj(&[("highway", "residential"), ("name", "Street")], (0..550).map(|j| pt(lon + 0.0005, -3.0 - f64::from(j) * 0.001)).collect(), false));
        }
        let ring: Vec<(u32, u32)> = (0..=50_000).map(|k| {
            let a = f64::from(k) / 50_000.0 * std::f64::consts::TAU;
            let r = 0.3 + 0.01 * (a * 37.0).sin();
            pt(29.3 + r * a.cos(), -3.3 + r * a.sin())
        }).collect();
        objects.push(obj(&[("landuse", "forest"), ("name", "Forest")], ring, true));
        let src = Synth { objects, boxes: vec![] };
        let dir = std::env::temp_dir().join(format!("mo-obf-tiles-scale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = std::time::Instant::now();
        let prog = |_: ProgressPhase, _: u64, _: u64, _: &str| true;
        let s = build(&src, &dir.join("map.pmtiles"), TileOptions { tmp_dir: Some(dir.clone()), ..Default::default() }, &prog).unwrap();
        println!("{:?}: {s:?}", t.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
