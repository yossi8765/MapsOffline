//! Compact routing graph built from an OSM PBF extract.
//!
//! Vertices are OSM nodes shared by ≥2 routable ways (or way endpoints). Edges are the way
//! segments between vertices with their full intermediate geometry. Each edge is stored once;
//! a *state* in the search is a directed edge `s = edge * 2 + dir` (`dir 0` = from→to).
//! Adjacency is a CSR array of incident edge ids per vertex. Turn restrictions via a node are
//! stored per via vertex; restrictions via one or more ways ("via-way") as edge sequences.
//!
//! File format (v3): `MOGR` + u32 version, then a fixed sequence of raw little-endian arrays
//! (`u64 element count` + payload), then `END!`. Everything the router needs at runtime –
//! projected vertex coordinates and the uniform-grid spatial index – is precomputed by the
//! builder, so loading is a handful of `read_exact` calls straight into the final vectors
//! (no per-element decoding, no index construction).

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use osmpbf::{Element, ElementReader, RelMemberType};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::geo::{bearing_deg, haversine_m};
use crate::routing::areas::{self, Areas};
use crate::types::{LonLat, ProgressFn, ProgressPhase};

pub const MAGIC: &[u8; 4] = b"MOGR";
/// v7: per-direction speeds and posted limits, road refs / exits / destinations / turn lanes
/// (sparse), truck limits (sparse), elevation gain per edge (when the DEM was installed at build
/// time), speed cameras, via-way turn restrictions and vehicle masks on restrictions.
/// v6: geometry as zig-zag varint deltas, projected coordinates and restriction offsets
/// recomputed at load. v6 files are still read (the new fields stay empty).
pub const VERSION: u32 = 7;
/// Oldest graph.bin layout `load` still reads.
pub const MIN_READ_VERSION: u32 = 6;

/// Can this build read a graph.bin of format `ver` (load, package import)?
pub fn readable_version(ver: u32) -> bool {
    (MIN_READ_VERSION..=VERSION).contains(&ver)
}
/// Revision of the build rules (access, costs) for the same file format: bumping it makes the
/// update check offer a rebuild of graph.bin (see `data::build_tag`).
pub const BUILD_REV: u32 = 1;
const END_MAGIC: &[u8; 4] = b"END!";

pub const NO_NAME: u32 = u32::MAX;
pub const NONE: u32 = u32::MAX;

/// Edge flags.
pub mod flags {
    pub const CAR_FWD: u32 = 1 << 0;
    pub const CAR_BWD: u32 = 1 << 1;
    pub const FOOT: u32 = 1 << 2;
    pub const BIKE_FWD: u32 = 1 << 3;
    pub const BIKE_BWD: u32 = 1 << 4;
    pub const TOLL: u32 = 1 << 5;
    /// motorway / trunk (incl. links)
    pub const MOTORWAY: u32 = 1 << 6;
    pub const UNPAVED: u32 = 1 << 7;
    pub const FERRY: u32 = 1 << 8;
    pub const ROUNDABOUT: u32 = 1 << 9;
    pub const LINK: u32 = 1 << 10;
    /// dedicated cycling infrastructure (cycleway or cycleway=* lanes/tracks)
    pub const CYCLEWAY: u32 = 1 << 11;
    /// footway / pedestrian / path / sidewalk-ish
    pub const FOOTWAY: u32 = 1 << 12;
    pub const STEPS: u32 = 1 << 13;
    /// has a sidewalk tag (foot friendly on bigger roads)
    pub const SIDEWALK: u32 = 1 << 14;
    /// inside Oslo Area A (or Hebron H1 / the Gaza Strip) – see `routing::areas`
    pub const AREA_A: u32 = 1 << 15;
    /// inside Oslo Area B
    pub const AREA_B: u32 = 1 << 16;
    /// car access only to reach a destination on it (access / motor_vehicle = destination)
    pub const DESTINATION: u32 = 1 << 17;
    /// closed to heavy goods vehicles (hgv=no)
    pub const HGV_NO: u32 = 1 << 18;
    /// not usable in a wheelchair (wheelchair=no, steep incline, bad smoothness)
    pub const WHEELCHAIR_NO: u32 = 1 << 19;
    /// explicitly wheelchair friendly (wheelchair=yes / designated, ramp:wheelchair=yes)
    pub const WHEELCHAIR_YES: u32 = 1 << 20;
}

/// Vehicle bits of a turn restriction.
pub mod rmask {
    pub const CAR: u8 = 1;
    pub const TRUCK: u8 = 2;
    pub const BIKE: u8 = 4;
    pub const ALL: u8 = CAR | TRUCK | BIKE;
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Class {
    Motorway = 0,
    Trunk = 1,
    Primary = 2,
    Secondary = 3,
    Tertiary = 4,
    Unclassified = 5,
    Residential = 6,
    Service = 7,
    LivingStreet = 8,
    Track = 9,
    Pedestrian = 10,
    Footway = 11,
    Path = 12,
    Cycleway = 13,
    Steps = 14,
    Road = 15,
    Other = 16,
}

impl Class {
    pub fn from_u8(v: u8) -> Class {
        use Class::*;
        match v {
            0 => Motorway,
            1 => Trunk,
            2 => Primary,
            3 => Secondary,
            4 => Tertiary,
            5 => Unclassified,
            6 => Residential,
            7 => Service,
            8 => LivingStreet,
            9 => Track,
            10 => Pedestrian,
            11 => Footway,
            12 => Path,
            13 => Cycleway,
            14 => Steps,
            15 => Road,
            _ => Other,
        }
    }

    /// (class, is_link) from an OSM `highway` value, None if not routable at all.
    fn parse(highway: &str) -> Option<(Class, bool)> {
        use Class::*;
        Some(match highway {
            "motorway" => (Motorway, false),
            "motorway_link" => (Motorway, true),
            "trunk" => (Trunk, false),
            "trunk_link" => (Trunk, true),
            "primary" => (Primary, false),
            "primary_link" => (Primary, true),
            "secondary" => (Secondary, false),
            "secondary_link" => (Secondary, true),
            "tertiary" => (Tertiary, false),
            "tertiary_link" => (Tertiary, true),
            "unclassified" => (Unclassified, false),
            "residential" => (Residential, false),
            "service" => (Service, false),
            "living_street" => (LivingStreet, false),
            "track" => (Track, false),
            "pedestrian" => (Pedestrian, false),
            "footway" => (Footway, false),
            "path" => (Path, false),
            "cycleway" => (Cycleway, false),
            "steps" => (Steps, false),
            "road" => (Road, false),
            "bridleway" => (Path, false),
            "corridor" => (Footway, false),
            _ => return None,
        })
    }

    pub fn default_car_speed(self) -> u8 {
        use Class::*;
        match self {
            Motorway => 110,
            Trunk => 90,
            Primary => 80,
            Secondary => 70,
            Tertiary => 60,
            Unclassified => 50,
            Residential => 40,
            Service => 20,
            LivingStreet => 15,
            Track => 20,
            Road => 40,
            _ => 0,
        }
    }

    pub fn is_major(self) -> bool {
        (self as u8) <= Class::Tertiary as u8
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Restriction {
    pub via: u32,
    pub from_edge: u32,
    pub to_edge: u32,
    pub only: bool,
    /// vehicles it applies to ([`rmask`])
    pub mask: u8,
}

impl Restriction {
    /// stored byte: bit 0 only, bit 1 bike, bit 2 car, bit 3 truck
    fn to_byte(self) -> u8 {
        self.only as u8 | ((self.mask & rmask::BIKE != 0) as u8) << 1 | ((self.mask & rmask::CAR != 0) as u8) << 2 | ((self.mask & rmask::TRUCK != 0) as u8) << 3
    }
    fn from_byte(b: u8, ver: u32) -> (bool, u8) {
        let mut m = 0;
        if b & 2 != 0 {
            m |= rmask::BIKE;
        }
        if ver < 7 || b & 4 != 0 {
            m |= rmask::CAR;
        }
        if ver < 7 || b & 8 != 0 {
            m |= rmask::TRUCK;
        }
        (b & 1 != 0, m)
    }
}

/// A turn restriction via one or more ways: `edges` = from, via…, to.
#[derive(Clone, Debug, PartialEq)]
pub struct MultiRestriction {
    pub edges: Vec<u32>,
    pub only: bool,
    pub mask: u8,
}

/// Sparse per-edge text extras (indices into the name table, [`NO_NAME`] when absent).
#[derive(Default, Clone, Debug)]
pub struct EdgeExt {
    /// sorted edge ids
    pub edge: Vec<u32>,
    /// road number (`ref`)
    pub road_ref: Vec<u32>,
    /// motorway exit number (`junction:ref`)
    pub junction: Vec<u32>,
    pub dest_fwd: Vec<u32>,
    pub dest_bwd: Vec<u32>,
    /// raw `turn:lanes` per direction
    pub lanes_fwd: Vec<u32>,
    pub lanes_bwd: Vec<u32>,
}

/// Sparse truck limits: height / width in decimetres, weight in half tonnes (0 = none).
#[derive(Default, Clone, Debug)]
pub struct Limits {
    pub edge: Vec<u32>,
    pub height_dm: Vec<u8>,
    pub weight_ht: Vec<u8>,
    pub width_dm: Vec<u8>,
}

/// Local equirectangular projection (meters) centred on Israel – used for the spatial index,
/// the A* heuristic and the pruning radius.
pub const LON0: f64 = 35.0;
pub const LAT0: f64 = 31.5;
pub const KX: f64 = 111_320.0 * 0.852_64; // cos(31.5°)
pub const KY: f64 = 110_574.0;

#[inline]
pub fn proj(lon: f64, lat: f64) -> [f32; 2] {
    [((lon - LON0) * KX) as f32, ((lat - LAT0) * KY) as f32]
}

#[inline]
pub fn unproj(x: f32, y: f32) -> LonLat {
    [x as f64 / KX + LON0, y as f64 / KY + LAT0]
}

/// The graph's own equirectangular projection, centred on its bbox (graph v5; v4 files and
/// tests use the Israel one). `hscale` (≤ 1) keeps the A* heuristic admissible where the true
/// east-west scale is smaller than at the centre latitude (big countries far from the equator).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proj {
    pub lon0: f64,
    pub lat0: f64,
    pub kx: f64,
    pub hscale: f64,
}

impl Default for Proj {
    fn default() -> Self {
        Proj { lon0: LON0, lat0: LAT0, kx: KX, hscale: 1.0 }
    }
}

impl Proj {
    pub fn for_bbox(b: [f64; 4]) -> Self {
        if b == [0.0; 4] || (b[0] >= 34.0 && b[2] <= 36.5 && b[1] >= 29.0 && b[3] <= 34.0) {
            return Proj::default(); // Israel (and unit tests): the historical constants
        }
        let lat0 = (b[1] + b[3]) / 2.0;
        let c0 = lat0.to_radians().cos().max(0.05);
        let far = b[1].abs().max(b[3].abs()).min(89.0);
        Proj { lon0: (b[0] + b[2]) / 2.0, lat0, kx: 111_320.0 * c0, hscale: (far.to_radians().cos() / c0).clamp(0.05, 1.0) }
    }
    #[inline]
    pub fn fwd(&self, lon: f64, lat: f64) -> [f32; 2] {
        [((lon - self.lon0) * self.kx) as f32, ((lat - self.lat0) * KY) as f32]
    }
    #[inline]
    pub fn inv(&self, x: f32, y: f32) -> LonLat {
        [x as f64 / self.kx + self.lon0, y as f64 / KY + self.lat0]
    }
}

/// Uniform grid over the projected plane; each cell lists the edges whose geometry (segment
/// bounding boxes) touches it. Built once by the builder and stored in the graph file.
#[derive(Default)]
pub struct Grid {
    pub x0: f32,
    pub y0: f32,
    pub cell: f32,
    pub nx: u32,
    pub ny: u32,
    /// CSR offsets into `items`, len nx*ny+1
    pub off: Vec<u32>,
    /// edge ids (sorted & unique per cell)
    pub items: Vec<u32>,
}

impl Grid {
    pub const CELL_M: f32 = 400.0;

    #[inline]
    fn cx(&self, x: f32) -> u32 {
        (((x - self.x0) / self.cell).floor().max(0.0) as u32).min(self.nx.saturating_sub(1))
    }
    #[inline]
    fn cy(&self, y: f32) -> u32 {
        (((y - self.y0) / self.cell).floor().max(0.0) as u32).min(self.ny.saturating_sub(1))
    }

    /// Edges whose geometry may intersect the box (superset; unique, ascending).
    pub fn query(&self, min: [f32; 2], max: [f32; 2], out: &mut Vec<u32>) {
        out.clear();
        if self.nx == 0 || self.ny == 0 {
            return;
        }
        let (gx1, gy1) = (self.x0 + self.nx as f32 * self.cell, self.y0 + self.ny as f32 * self.cell);
        if max[0] < self.x0 || max[1] < self.y0 || min[0] > gx1 || min[1] > gy1 {
            return;
        }
        let (x0, x1, y0, y1) = (self.cx(min[0]), self.cx(max[0]), self.cy(min[1]), self.cy(max[1]));
        for y in y0..=y1 {
            let row = (y * self.nx) as usize;
            for x in x0..=x1 {
                let c = row + x as usize;
                out.extend_from_slice(&self.items[self.off[c] as usize..self.off[c + 1] as usize]);
            }
        }
        if (x1 > x0) || (y1 > y0) {
            out.sort_unstable();
            out.dedup();
        }
    }

    /// Build the grid for a graph (needs `vpx/vpy` and `geom`).
    pub fn build(g: &Graph) -> Grid {
        let cell = Self::CELL_M;
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for i in 0..g.vpx.len() {
            minx = minx.min(g.vpx[i]);
            maxx = maxx.max(g.vpx[i]);
            miny = miny.min(g.vpy[i]);
            maxy = maxy.max(g.vpy[i]);
        }
        let n_pts = g.geom.len() / 2;
        for i in 0..n_pts {
            let p = g.proj(g.geom[2 * i] as f64 * 1e-7, g.geom[2 * i + 1] as f64 * 1e-7);
            minx = minx.min(p[0]);
            maxx = maxx.max(p[0]);
            miny = miny.min(p[1]);
            maxy = maxy.max(p[1]);
        }
        if !minx.is_finite() {
            return Grid { cell, off: vec![0], ..Default::default() };
        }
        // big countries: grow the cell so the CSR offsets stay small (≤ ~4M cells)
        let area = ((maxx - minx).max(1.0) as f64) * ((maxy - miny).max(1.0) as f64);
        let cell = cell.max((area / 4.0e6).sqrt() as f32);
        let mut grid = Grid {
            x0: minx,
            y0: miny,
            cell,
            nx: ((maxx - minx) / cell).floor() as u32 + 1,
            ny: ((maxy - miny) / cell).floor() as u32 + 1,
            off: Vec::new(),
            items: Vec::new(),
        };
        // (cell << 32 | edge) pairs
        let mut pairs: Vec<u64> = Vec::with_capacity(g.num_edges() * 2);
        let mut cells: Vec<u32> = Vec::new();
        let mut pts: Vec<[f32; 2]> = Vec::new();
        for e in 0..g.num_edges() {
            pts.clear();
            pts.push(g.vproj(g.e_from[e]));
            let (a, b) = (g.e_geom_off[e] as usize, g.e_geom_off[e + 1] as usize);
            for i in a..b {
                pts.push(g.proj(g.geom[2 * i] as f64 * 1e-7, g.geom[2 * i + 1] as f64 * 1e-7));
            }
            pts.push(g.vproj(g.e_to[e]));
            cells.clear();
            for w in pts.windows(2) {
                let (x0, x1) = (grid.cx(w[0][0].min(w[1][0])), grid.cx(w[0][0].max(w[1][0])));
                let (y0, y1) = (grid.cy(w[0][1].min(w[1][1])), grid.cy(w[0][1].max(w[1][1])));
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        cells.push(y * grid.nx + x);
                    }
                }
            }
            cells.sort_unstable();
            cells.dedup();
            pairs.extend(cells.iter().map(|&c| ((c as u64) << 32) | e as u64));
        }
        pairs.sort_unstable();
        let n_cells = (grid.nx * grid.ny) as usize;
        let mut off = vec![0u32; n_cells + 1];
        for &p in &pairs {
            off[(p >> 32) as usize + 1] += 1;
        }
        for i in 0..n_cells {
            off[i + 1] += off[i];
        }
        grid.off = off;
        grid.items = pairs.iter().map(|&p| p as u32).collect();
        grid
    }
}

/// Result of snapping a point to the network.
#[derive(Clone, Copy, Debug)]
pub struct Snap {
    pub edge: u32,
    /// fraction along the edge in forward (from→to) direction, 0..=1
    pub frac: f32,
    pub point: LonLat,
    pub dist_m: f64,
}

#[derive(Default)]
pub struct Graph {
    pub bbox: [f64; 4],
    pub proj0: Proj,
    // vertices (1e-7 degrees)
    pub vlon: Vec<i32>,
    pub vlat: Vec<i32>,
    /// projected vertex coordinates (meters, see [`proj`])
    pub vpx: Vec<f32>,
    pub vpy: Vec<f32>,
    /// bitset, (V+7)/8 bytes: vertex is a junction with traffic signals (`highway=traffic_signals`)
    pub v_signal: Vec<u8>,
    // edges
    pub e_from: Vec<u32>,
    pub e_to: Vec<u32>,
    pub e_len: Vec<f32>,
    pub e_speed: Vec<u8>,
    pub e_flags: Vec<u32>,
    pub e_class: Vec<u8>,
    pub e_name: Vec<u32>,
    /// bearing (deg) of the first segment leaving `from`, and of the last segment arriving at `to`
    pub e_bear_start: Vec<u16>,
    pub e_bear_end: Vec<u16>,
    /// offsets (in points) into `geom` of the intermediate points of each edge; len E+1
    pub e_geom_off: Vec<u32>,
    /// interleaved lon,lat (1e-7 deg) intermediate points
    pub geom: Vec<i32>,
    /// all street names concatenated (UTF-8); name `i` is `name_buf[name_off[i]..name_off[i+1]]`
    pub name_buf: String,
    pub name_off: Vec<u32>,
    /// CSR adjacency: incident edges of each vertex
    pub adj_off: Vec<u32>,
    pub adj: Vec<u32>,
    /// sorted by `via`
    pub restrictions: Vec<Restriction>,
    /// first restriction index per via vertex (len V+1)
    pub restr_off: Vec<u32>,
    /// spatial index for snapping
    pub grid: Grid,
    /// highest car speed (km/h) of any car-accessible edge – bounds the A* heuristic
    pub max_car_kmh: u8,
    /// restricted territories (rings) – used to check online routes
    pub areas: Areas,
    // ---- v7 (empty when read from v6)
    /// car speed against the edge direction (km/h); empty = same as `e_speed`
    pub e_speed_bwd: Vec<u8>,
    /// posted speed limit per direction (km/h, 0 = not tagged)
    pub e_limit_fwd: Vec<u8>,
    pub e_limit_bwd: Vec<u8>,
    /// elevation gain / loss along the edge in its forward direction (decimetres); empty = no DEM
    pub e_rise: Vec<u16>,
    pub e_fall: Vec<u16>,
    pub ext: EdgeExt,
    pub limits: Limits,
    /// fixed speed cameras, interleaved lon,lat (1e-7 deg)
    pub cameras: Vec<i32>,
    /// via-way restrictions
    pub multi: Vec<MultiRestriction>,
    /// derived: multi restrictions by their last via edge / first via edge
    pub multi_by_last: FxHashMap<u32, smallvec::SmallVec<[u32; 2]>>,
    pub multi_by_first: FxHashMap<u32, smallvec::SmallVec<[u32; 2]>>,
}

// ---------------------------------------------------------------- accessors

impl Graph {
    #[inline]
    pub fn num_vertices(&self) -> usize {
        self.vlon.len()
    }
    #[inline]
    pub fn num_edges(&self) -> usize {
        self.e_from.len()
    }
    #[inline]
    pub fn num_states(&self) -> usize {
        self.e_from.len() * 2
    }
    #[inline]
    pub fn vertex(&self, v: u32) -> LonLat {
        [self.vlon[v as usize] as f64 * 1e-7, self.vlat[v as usize] as f64 * 1e-7]
    }
    #[inline]
    pub fn tail(&self, s: u32) -> u32 {
        let e = (s >> 1) as usize;
        if s & 1 == 0 { self.e_from[e] } else { self.e_to[e] }
    }
    #[inline]
    pub fn head(&self, s: u32) -> u32 {
        let e = (s >> 1) as usize;
        if s & 1 == 0 { self.e_to[e] } else { self.e_from[e] }
    }
    #[inline]
    pub fn edge_of(s: u32) -> u32 {
        s >> 1
    }
    #[inline]
    pub fn dir_of(s: u32) -> u32 {
        s & 1
    }
    #[inline]
    pub fn state(e: u32, dir: u32) -> u32 {
        e * 2 + dir
    }
    #[inline]
    pub fn reverse(s: u32) -> u32 {
        s ^ 1
    }
    /// Bearing when leaving the tail of state `s`.
    #[inline]
    pub fn bearing_out(&self, s: u32) -> u16 {
        let e = (s >> 1) as usize;
        if s & 1 == 0 { self.e_bear_start[e] } else { (self.e_bear_end[e] + 180) % 360 }
    }
    /// Bearing when arriving at the head of state `s`.
    #[inline]
    pub fn bearing_in(&self, s: u32) -> u16 {
        let e = (s >> 1) as usize;
        if s & 1 == 0 { self.e_bear_end[e] } else { (self.e_bear_start[e] + 180) % 360 }
    }
    #[inline]
    pub fn incident(&self, v: u32) -> &[u32] {
        &self.adj[self.adj_off[v as usize] as usize..self.adj_off[v as usize + 1] as usize]
    }
    #[inline]
    pub fn degree(&self, v: u32) -> usize {
        self.incident(v).len()
    }
    /// Is vertex `v` a signalised junction?
    #[inline]
    pub fn has_signal(&self, v: u32) -> bool {
        self.v_signal.get((v / 8) as usize).is_some_and(|b| b & (1 << (v % 8)) != 0)
    }
    pub fn count_signals(&self) -> usize {
        self.v_signal.iter().map(|b| b.count_ones() as usize).sum()
    }
    /// Metres of `geometry` that run on unpaved roads (each segment is attributed to the road
    /// nearest to its midpoint; diagnostics only).
    pub fn unpaved_length_m(&self, geometry: &[LonLat]) -> f64 {
        let mut total = 0.0;
        for w in geometry.windows(2) {
            let (a, b) = (w[0], w[1]);
            let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            let Some(s) = self.snap(mid, &|_| true) else { continue };
            if self.e_flags[s.edge as usize] & flags::UNPAVED != 0 {
                let kx = 111_320.0 * mid[1].to_radians().cos();
                let (dx, dy) = ((b[0] - a[0]) * kx, (b[1] - a[1]) * 110_574.0);
                total += (dx * dx + dy * dy).sqrt();
            }
        }
        total
    }
    /// State leaving vertex `v` along edge `e` (e must be incident to v).
    #[inline]
    pub fn state_from(&self, v: u32, e: u32) -> u32 {
        if self.e_from[e as usize] == v { e * 2 } else { e * 2 + 1 }
    }
    pub fn restrictions_at(&self, via: u32) -> &[Restriction] {
        if self.restr_off.is_empty() {
            return &[];
        }
        &self.restrictions[self.restr_off[via as usize] as usize..self.restr_off[via as usize + 1] as usize]
    }
    pub fn name(&self, e: u32) -> Option<&str> {
        let n = self.e_name[e as usize];
        if n == NO_NAME {
            return None;
        }
        let (a, b) = (self.name_off[n as usize] as usize, self.name_off[n as usize + 1] as usize);
        Some(&self.name_buf[a..b])
    }
    #[inline]
    pub fn class(&self, e: u32) -> Class {
        Class::from_u8(self.e_class[e as usize])
    }
    #[inline]
    pub fn flags(&self, e: u32) -> u32 {
        self.e_flags[e as usize]
    }
    /// Car speed (km/h) along directed state `s`.
    #[inline]
    pub fn speed_s(&self, s: u32) -> u8 {
        let e = (s >> 1) as usize;
        if s & 1 == 1 && !self.e_speed_bwd.is_empty() { self.e_speed_bwd[e] } else { self.e_speed[e] }
    }
    /// Posted speed limit along state `s`, when tagged.
    #[inline]
    pub fn limit_s(&self, s: u32) -> Option<u8> {
        let e = (s >> 1) as usize;
        let v = if s & 1 == 0 { self.e_limit_fwd.get(e) } else { self.e_limit_bwd.get(e) };
        v.copied().filter(|&l| l > 0)
    }
    /// Metres climbed / descended along state `s` (0 without elevation data).
    #[inline]
    pub fn climb_s(&self, s: u32) -> (f32, f32) {
        let e = (s >> 1) as usize;
        if self.e_rise.is_empty() {
            return (0.0, 0.0);
        }
        let (r, f) = (self.e_rise[e] as f32 * 0.1, self.e_fall[e] as f32 * 0.1);
        if s & 1 == 0 { (r, f) } else { (f, r) }
    }
    pub fn has_elevation(&self) -> bool {
        !self.e_rise.is_empty()
    }
    /// Name-table string by index.
    pub fn name_at(&self, n: u32) -> Option<&str> {
        if n == NO_NAME || n as usize + 1 >= self.name_off.len() {
            return None;
        }
        let (a, b) = (self.name_off[n as usize] as usize, self.name_off[n as usize + 1] as usize);
        Some(&self.name_buf[a..b])
    }
    fn ext_index(&self, e: u32) -> Option<usize> {
        self.ext.edge.binary_search(&e).ok()
    }
    /// Road number of edge `e` ("1", "443").
    pub fn road_ref(&self, e: u32) -> Option<&str> {
        self.ext_index(e).and_then(|i| self.name_at(self.ext.road_ref[i]))
    }
    /// Exit number (junction:ref) of a link edge.
    pub fn junction_ref(&self, e: u32) -> Option<&str> {
        self.ext_index(e).and_then(|i| self.name_at(self.ext.junction[i]))
    }
    /// Signposted destination along state `s`.
    pub fn destination_s(&self, s: u32) -> Option<&str> {
        let i = self.ext_index(s >> 1)?;
        self.name_at(if s & 1 == 0 { self.ext.dest_fwd[i] } else { self.ext.dest_bwd[i] })
    }
    /// Raw turn:lanes along state `s`.
    pub fn lanes_s(&self, s: u32) -> Option<&str> {
        let i = self.ext_index(s >> 1)?;
        self.name_at(if s & 1 == 0 { self.ext.lanes_fwd[i] } else { self.ext.lanes_bwd[i] })
    }
    /// (height m, weight t, width m) limits of edge `e`, 0 = none.
    pub fn limits_of(&self, e: u32) -> Option<(f32, f32, f32)> {
        let i = self.limits.edge.binary_search(&e).ok()?;
        Some((self.limits.height_dm[i] as f32 * 0.1, self.limits.weight_ht[i] as f32 * 0.5, self.limits.width_dm[i] as f32 * 0.1))
    }
    /// Speed camera positions.
    pub fn camera_points(&self) -> impl Iterator<Item = LonLat> + '_ {
        self.cameras.as_chunks::<2>().0.iter().map(|c| [c[0] as f64 * 1e-7, c[1] as f64 * 1e-7])
    }
    /// Rebuild the lookup maps of the via-way restrictions.
    pub fn index_multi(&mut self) {
        self.multi_by_last.clear();
        self.multi_by_first.clear();
        for (i, m) in self.multi.iter().enumerate() {
            if m.edges.len() < 3 {
                continue;
            }
            self.multi_by_last.entry(m.edges[m.edges.len() - 2]).or_default().push(i as u32);
            self.multi_by_first.entry(m.edges[1]).or_default().push(i as u32);
        }
    }
    /// Project with this graph's projection (see [`Proj`]).
    #[inline]
    pub fn proj(&self, lon: f64, lat: f64) -> [f32; 2] {
        self.proj0.fwd(lon, lat)
    }
    #[inline]
    pub fn unproj(&self, x: f32, y: f32) -> LonLat {
        self.proj0.inv(x, y)
    }
    #[inline]
    pub fn vproj(&self, v: u32) -> [f32; 2] {
        [self.vpx[v as usize], self.vpy[v as usize]]
    }

    /// Full polyline of the edge in forward direction (from, intermediates…, to).
    pub fn edge_points(&self, e: u32) -> Vec<LonLat> {
        let ei = e as usize;
        let (a, b) = (self.e_geom_off[ei] as usize, self.e_geom_off[ei + 1] as usize);
        let mut v = Vec::with_capacity(b - a + 2);
        v.push(self.vertex(self.e_from[ei]));
        for i in a..b {
            v.push([self.geom[2 * i] as f64 * 1e-7, self.geom[2 * i + 1] as f64 * 1e-7]);
        }
        v.push(self.vertex(self.e_to[ei]));
        v
    }

    /// Polyline of the directed state (tail → head).
    pub fn state_points(&self, s: u32) -> Vec<LonLat> {
        let mut p = self.edge_points(s >> 1);
        if s & 1 == 1 {
            p.reverse();
        }
        p
    }

    /// Snap a point to the nearest edge accepted by `allowed`.
    pub fn snap(&self, p: LonLat, allowed: &dyn Fn(u32) -> bool) -> Option<Snap> {
        self.snap_candidates(p, allowed, 1).into_iter().next()
    }

    /// Up to `k` snap candidates ordered by (slightly class-weighted) distance. Service roads and
    /// tracks are de-prioritised so that a point next to a street is not snapped to a parking lot.
    pub fn snap_candidates(&self, p: LonLat, allowed: &dyn Fn(u32) -> bool, k: usize) -> Vec<Snap> {
        let q = self.proj(p[0], p[1]);
        let mut cands: Vec<(f32, Snap)> = Vec::new();
        let mut edges: Vec<u32> = Vec::new();
        for &radius in &[150.0f32, 600.0, 2500.0, 8000.0] {
            self.grid.query([q[0] - radius, q[1] - radius], [q[0] + radius, q[1] + radius], &mut edges);
            cands.clear();
            for &e in &edges {
                if !allowed(e) {
                    continue;
                }
                let (d2, frac, pt) = self.project_on_edge(e, q);
                let dist = (d2 as f64).sqrt();
                let w = match self.class(e) {
                    Class::Service | Class::Track | Class::Road => 1.5,
                    Class::Steps | Class::Path => 1.3,
                    _ => 1.0,
                };
                cands.push((dist as f32 * w + 1.0, Snap { edge: e, frac, point: self.unproj(pt[0], pt[1]), dist_m: dist }));
            }
            if !cands.is_empty() {
                cands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.edge.cmp(&b.1.edge)));
                if cands[0].1.dist_m as f32 <= radius || radius >= 8000.0 {
                    cands.dedup_by_key(|c| c.1.edge);
                    return cands.into_iter().take(k).map(|c| c.1).collect();
                }
            }
        }
        Vec::new()
    }

    /// Point `i` of edge `e` in forward order (0 = from vertex, last = to vertex), projected.
    #[inline]
    fn edge_pt_proj(&self, e: usize, a: usize, b: usize, i: usize) -> [f32; 2] {
        if i == 0 {
            self.vproj(self.e_from[e])
        } else if i == b - a + 1 {
            self.vproj(self.e_to[e])
        } else {
            let j = a + i - 1;
            self.proj(self.geom[2 * j] as f64 * 1e-7, self.geom[2 * j + 1] as f64 * 1e-7)
        }
    }

    /// Squared distance (m²), fraction along edge (0..1, forward), closest projected point.
    fn project_on_edge(&self, e: u32, q: [f32; 2]) -> (f32, f32, [f32; 2]) {
        let ei = e as usize;
        let (a, b) = (self.e_geom_off[ei] as usize, self.e_geom_off[ei + 1] as usize);
        let n = b - a + 2;
        // Fractions are measured on projected segment lengths, normalised by their sum (the
        // projection is near-conformal over an edge, so this equals the geodesic fraction).
        let mut total = 0.0f32;
        let mut prev = self.edge_pt_proj(ei, a, b, 0);
        for i in 1..n {
            let cur = self.edge_pt_proj(ei, a, b, i);
            total += d2(prev, cur).sqrt();
            prev = cur;
        }
        let total = total.max(1e-3);
        let mut best = (f32::INFINITY, 0.0f32, q);
        let mut acc = 0.0f32;
        let mut prev = self.edge_pt_proj(ei, a, b, 0);
        for i in 1..n {
            let cur = self.edge_pt_proj(ei, a, b, i);
            let (dx, dy) = (cur[0] - prev[0], cur[1] - prev[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((q[0] - prev[0]) * dx + (q[1] - prev[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let px = prev[0] + t * dx;
            let py = prev[1] + t * dy;
            let dd = (q[0] - px) * (q[0] - px) + (q[1] - py) * (q[1] - py);
            if dd < best.0 {
                let frac = (acc + t * l2.sqrt()) / total;
                best = (dd, frac.clamp(0.0, 1.0), [px, py]);
            }
            acc += l2.sqrt();
            prev = cur;
        }
        best
    }

    /// CSR adjacency (incident edges per vertex, in edge order; self loops appear twice).
    fn build_adjacency(&mut self) {
        let nv = self.vlon.len();
        let ne = self.e_from.len();
        let mut off = vec![0u32; nv + 1];
        for e in 0..ne {
            off[self.e_from[e] as usize + 1] += 1;
            off[self.e_to[e] as usize + 1] += 1;
        }
        for i in 0..nv {
            off[i + 1] += off[i];
        }
        let mut fill = off.clone();
        let mut adj = vec![0u32; off[nv] as usize];
        for e in 0..ne {
            let (a, b) = (self.e_from[e] as usize, self.e_to[e] as usize);
            adj[fill[a] as usize] = e as u32;
            fill[a] += 1;
            adj[fill[b] as usize] = e as u32;
            fill[b] += 1;
        }
        self.adj_off = off;
        self.adj = adj;
    }

    /// Renumber vertices along a Hilbert curve and edges by their (new) lower endpoint, so that
    /// nearby parts of the network are nearby in memory (fewer cache misses during searches).
    fn reorder_spatial(&mut self) {
        let nv = self.num_vertices();
        let ne = self.num_edges();
        if nv == 0 {
            return;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for i in 0..nv {
            x0 = x0.min(self.vlon[i]);
            x1 = x1.max(self.vlon[i]);
            y0 = y0.min(self.vlat[i]);
            y1 = y1.max(self.vlat[i]);
        }
        let span = ((x1 as i64 - x0 as i64).max(y1 as i64 - y0 as i64) + 1) as f64;
        let scale = 65535.0 / span;
        let key = |i: usize| -> u64 {
            let x = ((self.vlon[i] as i64 - x0 as i64) as f64 * scale) as u32;
            let y = ((self.vlat[i] as i64 - y0 as i64) as f64 * scale) as u32;
            hilbert_d(x.min(65535), y.min(65535))
        };
        let mut vorder: Vec<u32> = (0..nv as u32).collect();
        let keys: Vec<u64> = (0..nv).map(key).collect();
        vorder.sort_by_key(|&v| (keys[v as usize], v));
        let mut vnew = vec![0u32; nv];
        for (n, &o) in vorder.iter().enumerate() {
            vnew[o as usize] = n as u32;
        }
        self.vlon = vorder.iter().map(|&o| self.vlon[o as usize]).collect();
        self.vlat = vorder.iter().map(|&o| self.vlat[o as usize]).collect();
        let mut sig = vec![0u8; nv.div_ceil(8)];
        for (n, &o) in vorder.iter().enumerate() {
            if self.has_signal(o) {
                sig[n / 8] |= 1 << (n % 8);
            }
        }
        self.v_signal = sig;
        for v in self.e_from.iter_mut().chain(self.e_to.iter_mut()) {
            *v = vnew[*v as usize];
        }

        let mut eorder: Vec<u32> = (0..ne as u32).collect();
        eorder.sort_by_key(|&e| {
            let (a, b) = (self.e_from[e as usize], self.e_to[e as usize]);
            (a.min(b), a.max(b), e)
        });
        let mut enew = vec![0u32; ne];
        for (n, &o) in eorder.iter().enumerate() {
            enew[o as usize] = n as u32;
        }
        fn perm<T: Copy>(v: &[T], order: &[u32]) -> Vec<T> {
            order.iter().map(|&o| v[o as usize]).collect()
        }
        self.e_from = perm(&self.e_from, &eorder);
        self.e_to = perm(&self.e_to, &eorder);
        self.e_len = perm(&self.e_len, &eorder);
        self.e_speed = perm(&self.e_speed, &eorder);
        self.e_flags = perm(&self.e_flags, &eorder);
        self.e_class = perm(&self.e_class, &eorder);
        self.e_name = perm(&self.e_name, &eorder);
        self.e_bear_start = perm(&self.e_bear_start, &eorder);
        self.e_bear_end = perm(&self.e_bear_end, &eorder);
        if !self.e_speed_bwd.is_empty() {
            self.e_speed_bwd = perm(&self.e_speed_bwd, &eorder);
        }
        if !self.e_limit_fwd.is_empty() {
            self.e_limit_fwd = perm(&self.e_limit_fwd, &eorder);
            self.e_limit_bwd = perm(&self.e_limit_bwd, &eorder);
        }
        if !self.e_rise.is_empty() {
            self.e_rise = perm(&self.e_rise, &eorder);
            self.e_fall = perm(&self.e_fall, &eorder);
        }
        {
            let x = std::mem::take(&mut self.ext);
            let mut idx: Vec<usize> = (0..x.edge.len()).collect();
            idx.sort_by_key(|&i| enew[x.edge[i] as usize]);
            self.ext = EdgeExt {
                edge: idx.iter().map(|&i| enew[x.edge[i] as usize]).collect(),
                road_ref: idx.iter().map(|&i| x.road_ref[i]).collect(),
                junction: idx.iter().map(|&i| x.junction[i]).collect(),
                dest_fwd: idx.iter().map(|&i| x.dest_fwd[i]).collect(),
                dest_bwd: idx.iter().map(|&i| x.dest_bwd[i]).collect(),
                lanes_fwd: idx.iter().map(|&i| x.lanes_fwd[i]).collect(),
                lanes_bwd: idx.iter().map(|&i| x.lanes_bwd[i]).collect(),
            };
            let l = std::mem::take(&mut self.limits);
            let mut idx: Vec<usize> = (0..l.edge.len()).collect();
            idx.sort_by_key(|&i| enew[l.edge[i] as usize]);
            self.limits = Limits {
                edge: idx.iter().map(|&i| enew[l.edge[i] as usize]).collect(),
                height_dm: idx.iter().map(|&i| l.height_dm[i]).collect(),
                weight_ht: idx.iter().map(|&i| l.weight_ht[i]).collect(),
                width_dm: idx.iter().map(|&i| l.width_dm[i]).collect(),
            };
            for m in &mut self.multi {
                for e in &mut m.edges {
                    *e = enew[*e as usize];
                }
            }
        }
        let mut geom = Vec::with_capacity(self.geom.len());
        let mut goff = Vec::with_capacity(ne + 1);
        goff.push(0u32);
        for &o in &eorder {
            let (a, b) = (self.e_geom_off[o as usize] as usize, self.e_geom_off[o as usize + 1] as usize);
            geom.extend_from_slice(&self.geom[2 * a..2 * b]);
            goff.push((geom.len() / 2) as u32);
        }
        self.geom = geom;
        self.e_geom_off = goff;
        for r in &mut self.restrictions {
            r.via = vnew[r.via as usize];
            r.from_edge = enew[r.from_edge as usize];
            r.to_edge = enew[r.to_edge as usize];
        }
        self.restrictions.sort_by_key(|r| (r.via, r.from_edge, r.to_edge));
        self.build_adjacency();
    }

    /// Spatial renumbering plus derived runtime fields (restriction offsets, grid, projected
    /// coords, speed bound). Called by the builder before saving; all of it is stored in the file.
    pub fn finish_build(&mut self) {
        let nv8 = self.num_vertices().div_ceil(8);
        if self.v_signal.len() != nv8 {
            self.v_signal.resize(nv8, 0);
        }
        self.reorder_spatial();
        self.proj0 = Proj::for_bbox(self.bbox);
        let n = self.num_vertices();
        self.vpx = Vec::with_capacity(n);
        self.vpy = Vec::with_capacity(n);
        for i in 0..n {
            let p = self.proj(self.vlon[i] as f64 * 1e-7, self.vlat[i] as f64 * 1e-7);
            self.vpx.push(p[0]);
            self.vpy.push(p[1]);
        }
        self.restr_off = restriction_offsets(&self.restrictions, n);
        self.max_car_kmh = (0..self.num_edges())
            .filter(|&e| self.e_flags[e] & (flags::CAR_FWD | flags::CAR_BWD) != 0)
            .map(|e| self.e_speed[e].max(self.e_speed_bwd.get(e).copied().unwrap_or(0)))
            .max()
            .unwrap_or(130)
            .max(5);
        self.grid = Grid::build(self);
        self.index_multi();
    }
}

#[inline]
fn d2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy
}

/// Position of (x, y) on a 2^16 × 2^16 Hilbert curve.
fn hilbert_d(mut x: u32, mut y: u32) -> u64 {
    let n: u32 = 1 << 16;
    let mut d: u64 = 0;
    let mut s = n / 2;
    while s > 0 {
        let rx = (x & s > 0) as u32;
        let ry = (y & s > 0) as u32;
        d += s as u64 * s as u64 * ((3 * rx) ^ ry) as u64;
        if ry == 0 {
            if rx == 1 {
                x = n - 1 - x;
                y = n - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    d
}

fn restriction_offsets(restrictions: &[Restriction], n: usize) -> Vec<u32> {
    let mut off = vec![0u32; n + 1];
    for r in restrictions {
        off[r.via as usize + 1] += 1;
    }
    for i in 0..n {
        off[i + 1] += off[i];
    }
    off
}

// ---------------------------------------------------------------- save / load

/// Plain-old-data element types stored as raw little-endian arrays.
///
/// # Safety
/// Implementors must be `Copy` types without padding for which every bit pattern is valid.
unsafe trait Pod: Copy + Default {}
unsafe impl Pod for u8 {}
unsafe impl Pod for u16 {}
unsafe impl Pod for u32 {}
unsafe impl Pod for i32 {}
unsafe impl Pod for f32 {}
unsafe impl Pod for f64 {}

#[cfg(not(target_endian = "little"))]
compile_error!("graph file I/O assumes a little-endian target");

fn as_bytes<T: Pod>(v: &[T]) -> &[u8] {
    // SAFETY: T is Pod (no padding), so its memory is a valid initialised byte slice.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn as_bytes_mut<T: Pod>(v: &mut [T]) -> &mut [u8] {
    // SAFETY: T is Pod: every byte pattern written through this view is a valid T.
    unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut u8, std::mem::size_of_val(v)) }
}

fn put<T: Pod, W: Write>(w: &mut W, v: &[T]) -> std::io::Result<()> {
    w.write_all(&(v.len() as u64).to_le_bytes())?;
    w.write_all(as_bytes(v))
}

/// Read one array; `max_bytes` bounds the allocation so a damaged header cannot request more
/// memory than the file could possibly hold.
fn get<T: Pod, R: Read>(r: &mut R, max_bytes: u64) -> anyhow::Result<Vec<T>> {
    let mut n = [0u8; 8];
    r.read_exact(&mut n)?;
    let n = u64::from_le_bytes(n);
    if n.saturating_mul(std::mem::size_of::<T>() as u64) > max_bytes {
        anyhow::bail!("corrupt graph file (array of {n} elements)");
    }
    // zero-initialised allocation (calloc) + one read straight into the final vector
    let mut v = vec![T::default(); n as usize];
    r.read_exact(as_bytes_mut(&mut v))?;
    Ok(v)
}

/// Zig-zag varint deltas of the fixed-point geometry (consecutive points of an edge are a few
/// metres apart, so most deltas take 1–2 bytes instead of 4).
fn encode_geom(geom: &[i32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(geom.len() * 2);
    let (mut px, mut py) = (0i64, 0i64);
    for c in geom.as_chunks::<2>().0 {
        for (v, prev) in [(c[0] as i64, &mut px), (c[1] as i64, &mut py)] {
            let d = v - *prev;
            *prev = v;
            let mut z = ((d << 1) ^ (d >> 63)) as u64;
            loop {
                let b = (z & 0x7f) as u8;
                z >>= 7;
                if z == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
        }
    }
    out
}

fn decode_geom(buf: &[u8], n_values: usize) -> anyhow::Result<Vec<i32>> {
    let mut out = Vec::with_capacity(n_values);
    let (mut i, mut prev) = (0usize, [0i64; 2]);
    while out.len() < n_values {
        let (mut z, mut shift) = (0u64, 0u32);
        loop {
            let b = *buf.get(i).ok_or_else(|| anyhow::anyhow!("corrupt graph file (geometry)"))?;
            i += 1;
            z |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            anyhow::ensure!(shift < 64, "corrupt graph file (geometry)");
        }
        let d = ((z >> 1) as i64) ^ -((z & 1) as i64);
        let k = out.len() & 1;
        prev[k] += d;
        out.push(i32::try_from(prev[k]).map_err(|_| anyhow::anyhow!("corrupt graph file (geometry)"))?);
    }
    anyhow::ensure!(i == buf.len(), "corrupt graph file (geometry length)");
    Ok(out)
}

pub fn save(g: &Graph, out: &Path) -> anyhow::Result<()> {
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(out)?);
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    put(&mut w, &g.bbox)?;
    put(&mut w, &[g.max_car_kmh])?;
    put(&mut w, &[g.proj0.lon0, g.proj0.lat0, g.proj0.kx, g.proj0.hscale])?;
    put(&mut w, &g.vlon)?;
    put(&mut w, &g.vlat)?;
    put(&mut w, &g.v_signal)?;
    put(&mut w, &g.e_from)?;
    put(&mut w, &g.e_to)?;
    put(&mut w, &g.e_len)?;
    put(&mut w, &g.e_speed)?;
    put(&mut w, &g.e_flags)?;
    put(&mut w, &g.e_class)?;
    put(&mut w, &g.e_name)?;
    put(&mut w, &g.e_bear_start)?;
    put(&mut w, &g.e_bear_end)?;
    put(&mut w, &g.e_geom_off)?;
    put(&mut w, &encode_geom(&g.geom))?;
    put(&mut w, g.name_buf.as_bytes())?;
    put(&mut w, &g.name_off)?;
    put(&mut w, &g.adj_off)?;
    put(&mut w, &g.adj)?;
    let rv: Vec<u32> = g.restrictions.iter().map(|r| r.via).collect();
    let rf: Vec<u32> = g.restrictions.iter().map(|r| r.from_edge).collect();
    let rt: Vec<u32> = g.restrictions.iter().map(|r| r.to_edge).collect();
    let rb: Vec<u8> = g.restrictions.iter().map(|r| r.to_byte()).collect();
    put(&mut w, &rv)?;
    put(&mut w, &rf)?;
    put(&mut w, &rt)?;
    put(&mut w, &rb)?;
    put(&mut w, &[g.grid.x0, g.grid.y0, g.grid.cell])?;
    put(&mut w, &[g.grid.nx, g.grid.ny])?;
    put(&mut w, &g.grid.off)?;
    put(&mut w, &g.grid.items)?;
    put(&mut w, &g.areas.kind)?;
    put(&mut w, &g.areas.off)?;
    put(&mut w, &g.areas.pts)?;
    // ---- v7
    put(&mut w, &g.e_speed_bwd)?;
    put(&mut w, &g.e_limit_fwd)?;
    put(&mut w, &g.e_limit_bwd)?;
    put(&mut w, &g.e_rise)?;
    put(&mut w, &g.e_fall)?;
    put(&mut w, &g.ext.edge)?;
    put(&mut w, &g.ext.road_ref)?;
    put(&mut w, &g.ext.junction)?;
    put(&mut w, &g.ext.dest_fwd)?;
    put(&mut w, &g.ext.dest_bwd)?;
    put(&mut w, &g.ext.lanes_fwd)?;
    put(&mut w, &g.ext.lanes_bwd)?;
    put(&mut w, &g.limits.edge)?;
    put(&mut w, &g.limits.height_dm)?;
    put(&mut w, &g.limits.weight_ht)?;
    put(&mut w, &g.limits.width_dm)?;
    put(&mut w, &g.cameras)?;
    let mut mr_off: Vec<u32> = vec![0];
    let mut mr_edges: Vec<u32> = Vec::new();
    for m in &g.multi {
        mr_edges.extend_from_slice(&m.edges);
        mr_off.push(mr_edges.len() as u32);
    }
    let mr_flags: Vec<u8> = g.multi.iter().map(|m| Restriction { via: 0, from_edge: 0, to_edge: 0, only: m.only, mask: m.mask }.to_byte()).collect();
    put(&mut w, &mr_off)?;
    put(&mut w, &mr_edges)?;
    put(&mut w, &mr_flags)?;
    w.write_all(END_MAGIC)?;
    w.flush()?;
    Ok(())
}

pub fn load(path: &Path) -> anyhow::Result<Graph> {
    let t0 = std::time::Instant::now();
    let file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut r = BufReader::with_capacity(1 << 16, file);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        anyhow::bail!("not a MapsOffline graph file");
    }
    let mut ver = [0u8; 4];
    r.read_exact(&mut ver)?;
    let ver = u32::from_le_bytes(ver);
    if !readable_version(ver) {
        anyhow::bail!("NO_ROUTING_DATA: graph version {ver} is not supported (expected {VERSION}); update the routing data");
    }
    let m = file_len; // no array can be larger than the file
    let bad = || anyhow::anyhow!("corrupt graph file");
    let bbox: Vec<f64> = get(&mut r, 32)?;
    let speed: Vec<u8> = get(&mut r, 1)?;
    // v5 stores the projection; v4 (Israel only) used the fixed Israel constants
    let proj0 = if ver >= 5 {
        let p: Vec<f64> = get(&mut r, 32)?;
        match p.as_slice() {
            [lon0, lat0, kx, hscale] => Proj { lon0: *lon0, lat0: *lat0, kx: *kx, hscale: *hscale },
            _ => return Err(bad()),
        }
    } else {
        Proj::default()
    };
    let vlon: Vec<i32> = get(&mut r, m)?;
    let vlat: Vec<i32> = get(&mut r, m)?;
    let (vpx, vpy): (Vec<f32>, Vec<f32>) = if ver < 6 { (get(&mut r, m)?, get(&mut r, m)?) } else { (Vec::new(), Vec::new()) };
    let v_signal = get(&mut r, m)?;
    let e_from = get(&mut r, m)?;
    let e_to = get(&mut r, m)?;
    let e_len = get(&mut r, m)?;
    let e_speed = get(&mut r, m)?;
    let e_flags = get(&mut r, m)?;
    let e_class = get(&mut r, m)?;
    let e_name = get(&mut r, m)?;
    let e_bear_start = get(&mut r, m)?;
    let e_bear_end = get(&mut r, m)?;
    let e_geom_off: Vec<u32> = get(&mut r, m)?;
    let geom: Vec<i32> = if ver < 6 {
        get(&mut r, m)?
    } else {
        let buf: Vec<u8> = get(&mut r, m)?;
        let n = e_geom_off.last().copied().unwrap_or(0) as usize * 2;
        anyhow::ensure!(n as u64 <= m, "corrupt graph file (geometry)");
        decode_geom(&buf, n)?
    };
    let mut g = Graph {
        proj0,
        bbox: bbox.try_into().map_err(|_| bad())?,
        max_car_kmh: *speed.first().ok_or_else(bad)?,
        vlon,
        vlat,
        vpx,
        vpy,
        v_signal,
        e_from,
        e_to,
        e_len,
        e_speed,
        e_flags,
        e_class,
        e_name,
        e_bear_start,
        e_bear_end,
        e_geom_off,
        geom,
        name_buf: String::from_utf8(get(&mut r, m)?).map_err(|_| bad())?,
        name_off: get(&mut r, m)?,
        adj_off: get(&mut r, m)?,
        adj: get(&mut r, m)?,
        ..Default::default()
    };
    let rv: Vec<u32> = get(&mut r, m)?;
    let rf: Vec<u32> = get(&mut r, m)?;
    let rt: Vec<u32> = get(&mut r, m)?;
    let rb: Vec<u8> = get(&mut r, m)?;
    if ver < 6 {
        g.restr_off = get(&mut r, m)?;
    }
    let gp: Vec<f32> = get(&mut r, 12)?;
    let gn: Vec<u32> = get(&mut r, 8)?;
    g.grid = Grid {
        x0: *gp.first().ok_or_else(bad)?,
        y0: *gp.get(1).ok_or_else(bad)?,
        cell: *gp.get(2).ok_or_else(bad)?,
        nx: *gn.first().ok_or_else(bad)?,
        ny: *gn.get(1).ok_or_else(bad)?,
        off: get(&mut r, m)?,
        items: get(&mut r, m)?,
    };
    g.areas = Areas::from_parts(get(&mut r, m)?, get(&mut r, m)?, get(&mut r, m)?);
    let mut multi_parts: Option<(Vec<u32>, Vec<u32>, Vec<u8>)> = None;
    if ver >= 7 {
        g.e_speed_bwd = get(&mut r, m)?;
        g.e_limit_fwd = get(&mut r, m)?;
        g.e_limit_bwd = get(&mut r, m)?;
        g.e_rise = get(&mut r, m)?;
        g.e_fall = get(&mut r, m)?;
        g.ext = EdgeExt {
            edge: get(&mut r, m)?,
            road_ref: get(&mut r, m)?,
            junction: get(&mut r, m)?,
            dest_fwd: get(&mut r, m)?,
            dest_bwd: get(&mut r, m)?,
            lanes_fwd: get(&mut r, m)?,
            lanes_bwd: get(&mut r, m)?,
        };
        g.limits = Limits { edge: get(&mut r, m)?, height_dm: get(&mut r, m)?, weight_ht: get(&mut r, m)?, width_dm: get(&mut r, m)? };
        g.cameras = get(&mut r, m)?;
        multi_parts = Some((get(&mut r, m)?, get(&mut r, m)?, get(&mut r, m)?));
    }
    let mut end = [0u8; 4];
    r.read_exact(&mut end)?;
    if &end != END_MAGIC {
        return Err(bad());
    }
    if rv.len() != rf.len() || rv.len() != rt.len() || rv.len() != rb.len() {
        return Err(bad());
    }
    g.restrictions = (0..rv.len())
        .map(|i| {
            let (only, mask) = Restriction::from_byte(rb[i], ver);
            Restriction { via: rv[i], from_edge: rf[i], to_edge: rt[i], only, mask }
        })
        .collect();
    if let Some((off, edges, fl)) = multi_parts {
        if off.is_empty() || off.len() != fl.len() + 1 || off.last().is_some_and(|&o| o as usize != edges.len()) || off.windows(2).any(|w| w[0] > w[1]) {
            return Err(bad());
        }
        g.multi = (0..fl.len())
            .map(|i| {
                let (only, mask) = Restriction::from_byte(fl[i], ver);
                MultiRestriction { edges: edges[off[i] as usize..off[i + 1] as usize].to_vec(), only, mask }
            })
            .collect();
    }
    if ver >= 6 {
        // derived fields v6 no longer stores
        let n = g.vlon.len();
        g.restr_off = restriction_offsets(&g.restrictions, n);
        use rayon::prelude::*;
        let (px, py): (Vec<f32>, Vec<f32>) = (0..n)
            .into_par_iter()
            .map(|i| {
                let p = g.proj(g.vlon[i] as f64 * 1e-7, g.vlat[i] as f64 * 1e-7);
                (p[0], p[1])
            })
            .unzip();
        g.vpx = px;
        g.vpy = py;
    }
    g.validate().map_err(|e| anyhow::anyhow!("corrupt graph file: {e}"))?;
    g.areas.build_index();
    g.index_multi();
    log::info!(
        "graph loaded: {} vertices, {} edges, {} restrictions, {} area rings in {:?}",
        g.num_vertices(),
        g.num_edges(),
        g.restrictions.len(),
        g.areas.num_rings(),
        t0.elapsed()
    );
    Ok(g)
}

impl Graph {
    /// Cheap structural checks so a damaged file fails at load instead of panicking mid-search.
    fn validate(&self) -> Result<(), &'static str> {
        let (nv, ne) = (self.vlon.len(), self.e_from.len());
        if self.vlat.len() != nv || self.vpx.len() != nv || self.vpy.len() != nv || self.v_signal.len() != nv.div_ceil(8) {
            return Err("vertex arrays");
        }
        let edge_arrays = [
            self.e_to.len(),
            self.e_len.len(),
            self.e_speed.len(),
            self.e_flags.len(),
            self.e_class.len(),
            self.e_name.len(),
            self.e_bear_start.len(),
            self.e_bear_end.len(),
        ];
        if edge_arrays.iter().any(|&l| l != ne) || self.e_geom_off.len() != ne + 1 {
            return Err("edge arrays");
        }
        if self.e_geom_off.last().is_none_or(|&o| o as usize * 2 != self.geom.len()) {
            return Err("geometry");
        }
        if self.adj_off.len() != nv + 1 || self.adj_off.last().is_none_or(|&o| o as usize != self.adj.len()) {
            return Err("adjacency");
        }
        if self.restr_off.len() != nv + 1 || self.restr_off.last().is_none_or(|&o| o as usize != self.restrictions.len()) {
            return Err("restrictions");
        }
        if self.name_off.last().is_some_and(|&o| o as usize != self.name_buf.len()) {
            return Err("names");
        }
        self.areas.validate()?;
        let opt = |l: usize| l == 0 || l == ne;
        if !opt(self.e_speed_bwd.len())
            || !opt(self.e_limit_fwd.len())
            || self.e_limit_bwd.len() != self.e_limit_fwd.len()
            || !opt(self.e_rise.len())
            || self.e_fall.len() != self.e_rise.len()
        {
            return Err("v7 edge arrays");
        }
        let x = &self.ext;
        let nx = x.edge.len();
        if [x.road_ref.len(), x.junction.len(), x.dest_fwd.len(), x.dest_bwd.len(), x.lanes_fwd.len(), x.lanes_bwd.len()].iter().any(|&l| l != nx)
            || x.edge.windows(2).any(|w| w[0] >= w[1])
            || x.edge.last().is_some_and(|&e| e as usize >= ne)
        {
            return Err("edge extras");
        }
        let names = self.name_off.len().saturating_sub(1) as u32;
        if [&x.road_ref, &x.junction, &x.dest_fwd, &x.dest_bwd, &x.lanes_fwd, &x.lanes_bwd].iter().any(|v| v.iter().any(|&n| n != NO_NAME && n >= names)) {
            return Err("edge extras names");
        }
        let l = &self.limits;
        if [l.height_dm.len(), l.weight_ht.len(), l.width_dm.len()].iter().any(|&k| k != l.edge.len())
            || l.edge.windows(2).any(|w| w[0] >= w[1])
            || l.edge.last().is_some_and(|&e| e as usize >= ne)
        {
            return Err("limits");
        }
        if !self.cameras.len().is_multiple_of(2) {
            return Err("cameras");
        }
        if self.multi.iter().any(|m| m.edges.len() < 3 || m.edges.iter().any(|&e| e as usize >= ne)) {
            return Err("via-way restrictions");
        }
        if self.restrictions.iter().any(|r| r.from_edge as usize >= ne || r.to_edge as usize >= ne || r.via as usize >= nv) {
            return Err("restrictions");
        }
        let cells = self.grid.nx as usize * self.grid.ny as usize;
        if self.grid.off.len() != cells + 1 || self.grid.off.last().is_none_or(|&o| o as usize != self.grid.items.len()) {
            return Err("grid");
        }
        let nv32 = nv as u32;
        let ne32 = ne as u32;
        if self.e_from.iter().chain(self.e_to.iter()).any(|&v| v >= nv32)
            || self.adj.iter().any(|&e| e >= ne32)
            || self.grid.items.iter().any(|&e| e >= ne32)
        {
            return Err("index out of range");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- build

/// Everything the builder keeps of one routable way.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WayAttrs {
    pub class: Option<Class>,
    pub flags: u32,
    pub speed_fwd: u8,
    pub speed_bwd: u8,
    /// posted limits (0 = not tagged)
    pub limit_fwd: u8,
    pub limit_bwd: u8,
    /// display name (Hebrew first; "כביש N" when only a ref)
    pub name: Option<String>,
    pub road_ref: Option<String>,
    pub junction: Option<String>,
    pub dest_fwd: Option<String>,
    pub dest_bwd: Option<String>,
    pub lanes_fwd: Option<String>,
    pub lanes_bwd: Option<String>,
    pub height_dm: u8,
    pub weight_ht: u8,
    pub width_dm: u8,
    /// ferry crossing time (seconds) from `duration`
    pub ferry_secs: Option<u32>,
}

impl WayAttrs {
    fn has_ext(&self) -> bool {
        self.road_ref.is_some() || self.junction.is_some() || self.dest_fwd.is_some() || self.dest_bwd.is_some() || self.lanes_fwd.is_some() || self.lanes_bwd.is_some()
    }
    fn has_limits(&self) -> bool {
        self.height_dm > 0 || self.weight_ht > 0 || self.width_dm > 0
    }
}

struct WayRec {
    id: i64,
    refs: Vec<i64>,
    a: WayAttrs,
}

struct RelRec {
    from_way: i64,
    via_node: Option<i64>,
    via_ways: Vec<i64>,
    to_way: i64,
    only: bool,
    mask: u8,
}

/// Boundary relation of a restricted territory: kind + ordered outer member way ids.
struct AreaRel {
    kind: u8,
    ways: Vec<i64>,
}

#[derive(Default)]
struct Acc {
    ways: Vec<WayRec>,
    rels: Vec<RelRec>,
    /// member ways of area relations: (way id, node refs)
    area_ways: Vec<(i64, Vec<i64>)>,
    conditional: u64,
}

fn yes(v: &str) -> bool {
    matches!(v, "yes" | "true" | "1" | "designated" | "permissive" | "official")
}
fn no(v: &str) -> bool {
    matches!(v, "no" | "private" | "false" | "0" | "agricultural" | "forestry" | "delivery" | "customers" | "military" | "emergency")
}

/// Implicit Israeli / generic limits ("IL:urban" …) in km/h.
fn implicit_limit(v: &str) -> Option<u8> {
    let kind = v.split_once(':').map(|(_, k)| k).unwrap_or(v);
    Some(match kind {
        "urban" => 50,
        "rural" => 80,
        "motorway" => 110,
        "trunk" | "expressway" => 90,
        "living_street" => 20,
        "walk" => 10,
        "school" => 30,
        _ => return None,
    })
}

fn parse_maxspeed(v: &str) -> Option<u8> {
    let v = v.trim();
    if v.is_empty() || v == "none" || v == "signals" || v == "variable" {
        return None;
    }
    if v.contains(':') && !v.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return implicit_limit(v);
    }
    let (num, mph) = if let Some(n) = v.strip_suffix("mph") { (n.trim(), true) } else { (v, false) };
    let n: f64 = num.split(';').next()?.trim().parse().ok()?;
    let kmh = if mph { n * 1.609 } else { n };
    if kmh <= 0.0 {
        return None;
    }
    Some(kmh.min(130.0) as u8)
}

/// "3.5", "3.5 m", "3,5", "12'6\"" → metres.
fn parse_metres(v: &str) -> Option<f64> {
    let v = v.trim();
    if let Some((ft, rest)) = v.split_once('\'') {
        let ft: f64 = ft.trim().parse().ok()?;
        let inch: f64 = rest.trim().trim_end_matches('"').trim().parse().unwrap_or(0.0);
        return Some((ft * 12.0 + inch) * 0.0254);
    }
    let v = v.trim_end_matches('m').trim().replace(',', ".");
    v.parse::<f64>().ok().filter(|x| *x > 0.0 && *x < 30.0)
}

/// "7.5", "7.5 t", "7500 kg", "12 st" → tonnes.
fn parse_tonnes(v: &str) -> Option<f64> {
    let v = v.trim().replace(',', ".");
    let (num, kg) = if let Some(n) = v.strip_suffix("kg") { (n.trim().to_string(), true) } else { (v.trim_end_matches('t').trim().to_string(), false) };
    let x: f64 = num.parse().ok()?;
    let t = if kg { x / 1000.0 } else { x };
    (t > 0.0 && t < 200.0).then_some(t)
}

/// Ferry `duration`: "HH:MM", "HH:MM:SS", "MM" or ISO "PT1H20M" → seconds.
fn parse_duration(v: &str) -> Option<u32> {
    let v = v.trim();
    if let Some(iso) = v.strip_prefix("PT") {
        let (mut secs, mut num) = (0u32, String::new());
        for c in iso.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else {
                let n: u32 = num.parse().ok()?;
                num.clear();
                secs += match c {
                    'H' => n * 3600,
                    'M' => n * 60,
                    'S' => n,
                    _ => return None,
                };
            }
        }
        return (secs > 0).then_some(secs);
    }
    let parts: Vec<u32> = v.split(':').map(|p| p.trim().parse().ok()).collect::<Option<Vec<u32>>>()?;
    let secs = match parts.as_slice() {
        [m] => m * 60,
        [h, m] => h * 3600 + m * 60,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    (secs > 0).then_some(secs)
}

/// Wheelchair-relevant incline: "12%", "-8 %", "up"/"down" (unknown) → |percent|.
fn parse_incline(v: &str) -> Option<f32> {
    let v = v.trim().trim_end_matches('%').trim();
    v.parse::<f32>().ok().map(f32::abs)
}

/// Parse way tags. `class` is None when the way is not routable for any profile.
pub fn parse_way_tags<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Option<WayAttrs> {
    let mut highway = None;
    let mut route = None;
    let mut name = None;
    let mut name_he = None;
    let mut name_en = None;
    let mut r#ref = None;
    let mut oneway = None;
    let mut oneway_bike = None;
    let mut junction = None;
    let mut maxspeed = None;
    let mut maxspeed_fwd = None;
    let mut maxspeed_bwd = None;
    let mut maxspeed_type = None;
    let mut access = None;
    let mut motor = None;
    let mut foot = None;
    let mut bicycle = None;
    let mut hgv = None;
    let mut toll = false;
    let mut surface = None;
    let mut tracktype = None;
    let mut sidewalk = None;
    let mut sidewalk_none = false;
    let mut motorroad = false;
    let mut cycleway_any = false;
    let mut area = false;
    let mut junction_ref = None;
    let (mut dest, mut dest_he, mut dest_fwd, mut dest_bwd, mut dest_ref, mut dest_ref_fwd, mut dest_ref_bwd) = (None, None, None, None, None, None, None);
    let (mut lanes, mut lanes_fwd, mut lanes_bwd) = (None, None, None);
    let (mut maxheight, mut maxweight, mut maxwidth) = (None, None, None);
    let (mut wheelchair, mut ramp_wc, mut incline, mut smoothness) = (None, None, None, None);
    let mut duration = None;
    let mut destination_access = false;
    for (k, v) in tags {
        match k {
            "highway" => highway = Some(v),
            "route" => route = Some(v),
            "name" => name = Some(v),
            "name:he" => name_he = Some(v),
            "name:en" => name_en = Some(v),
            "ref" => r#ref = Some(v),
            "oneway" => oneway = Some(v),
            "oneway:bicycle" => oneway_bike = Some(v),
            "junction" => junction = Some(v),
            "junction:ref" => junction_ref = Some(v),
            "maxspeed" => maxspeed = Some(v),
            "maxspeed:forward" => maxspeed_fwd = Some(v),
            "maxspeed:backward" => maxspeed_bwd = Some(v),
            "maxspeed:type" | "source:maxspeed" | "zone:maxspeed" => maxspeed_type = Some(v),
            "access" => {
                destination_access |= v == "destination";
                access = Some(v)
            }
            "motor_vehicle" | "motorcar" | "vehicle" => {
                destination_access |= v == "destination";
                if motor.is_none() || k == "motor_vehicle" {
                    motor = Some(v)
                }
            }
            "foot" => foot = Some(v),
            "bicycle" => bicycle = Some(v),
            "hgv" => hgv = Some(v),
            "toll" => toll = v == "yes",
            "surface" => surface = Some(v),
            "tracktype" => tracktype = Some(v),
            "sidewalk" | "sidewalk:both" | "sidewalk:left" | "sidewalk:right" => {
                if v != "no" && v != "none" && v != "separate" {
                    sidewalk = Some(v)
                } else if v == "no" || v == "none" {
                    sidewalk_none = true
                }
            }
            "motorroad" | "expressway" => motorroad = motorroad || v == "yes",
            "cycleway" | "cycleway:left" | "cycleway:right" | "cycleway:both" => {
                if v != "no" && v != "none" && v != "separate" {
                    cycleway_any = true
                }
            }
            "area" => area = v == "yes",
            "destination" => dest = Some(v),
            "destination:he" => dest_he = Some(v),
            "destination:forward" => dest_fwd = Some(v),
            "destination:backward" => dest_bwd = Some(v),
            "destination:ref" => dest_ref = Some(v),
            "destination:ref:forward" => dest_ref_fwd = Some(v),
            "destination:ref:backward" => dest_ref_bwd = Some(v),
            "turn:lanes" => lanes = Some(v),
            "turn:lanes:forward" => lanes_fwd = Some(v),
            "turn:lanes:backward" => lanes_bwd = Some(v),
            "maxheight" | "maxheight:physical" => maxheight = maxheight.or(Some(v)),
            "maxweight" => maxweight = Some(v),
            "maxwidth" | "maxwidth:physical" => maxwidth = maxwidth.or(Some(v)),
            "wheelchair" => wheelchair = Some(v),
            "ramp:wheelchair" => ramp_wc = Some(v),
            "incline" => incline = Some(v),
            "smoothness" => smoothness = Some(v),
            "duration" => duration = Some(v),
            _ => {}
        }
    }
    let ferry = highway.is_none() && route == Some("ferry");
    let (class, link) = if ferry { (Class::Other, false) } else { Class::parse(highway?)? };
    if area && !matches!(class, Class::Pedestrian) {
        return None;
    }
    let mut f: u32 = 0;
    if link {
        f |= flags::LINK;
    }
    let roundabout = matches!(junction, Some("roundabout") | Some("circular"));
    if roundabout {
        f |= flags::ROUNDABOUT;
    }
    if matches!(class, Class::Motorway | Class::Trunk) {
        f |= flags::MOTORWAY;
    }
    if toll {
        f |= flags::TOLL;
    }
    if ferry {
        f |= flags::FERRY;
    }
    let unpaved = match surface {
        Some(s) => matches!(
            s,
            "unpaved" | "gravel" | "dirt" | "ground" | "sand" | "grass" | "compacted" | "fine_gravel" | "earth" | "mud" | "rock" | "pebblestone" | "woodchips" | "salt"
        ),
        None => matches!(tracktype, Some("grade2") | Some("grade3") | Some("grade4") | Some("grade5")) || class == Class::Track,
    };
    if unpaved {
        f |= flags::UNPAVED;
    }
    if class == Class::Cycleway || cycleway_any {
        f |= flags::CYCLEWAY;
    }
    if matches!(class, Class::Footway | Class::Pedestrian | Class::Path | Class::LivingStreet) {
        f |= flags::FOOTWAY;
    }
    if class == Class::Steps {
        f |= flags::STEPS;
    }
    if sidewalk.is_some() {
        f |= flags::SIDEWALK;
    }
    // wheelchair
    let ramp = ramp_wc.is_some_and(yes);
    match wheelchair {
        Some("no") => f |= flags::WHEELCHAIR_NO,
        Some(v) if yes(v) => f |= flags::WHEELCHAIR_YES,
        _ => {}
    }
    if ramp {
        f |= flags::WHEELCHAIR_YES;
    }
    if ((class == Class::Steps && !ramp)
        || incline.and_then(parse_incline).is_some_and(|i| i > 8.0)
        || matches!(smoothness, Some("bad" | "very_bad" | "horrible" | "very_horrible" | "impassable")))
        && f & flags::WHEELCHAIR_YES == 0
    {
        f |= flags::WHEELCHAIR_NO;
    }

    // --- access
    let access_no = access.map(no).unwrap_or(false);
    // car
    let car_class = matches!(
        class,
        Class::Motorway | Class::Trunk | Class::Primary | Class::Secondary | Class::Tertiary | Class::Unclassified | Class::Residential | Class::Service | Class::LivingStreet | Class::Track | Class::Road
    );
    let car = if ferry {
        // ferries carry cars unless tagged otherwise
        !motor.is_some_and(no) && !access_no
    } else {
        match motor {
            Some(m) if no(m) => false,
            Some(m) if yes(m) => car_class,
            _ => car_class && !access_no,
        }
    };
    if car && destination_access {
        f |= flags::DESTINATION;
    }
    if hgv.is_some_and(|v| no(v) || v == "destination") && car {
        f |= flags::HGV_NO;
    }
    // foot
    let foot_ok = match foot {
        Some(v) if no(v) => false,
        Some(v) if yes(v) => true,
        _ => match class {
            Class::Motorway => false,
            // Israeli urban trunk roads (route 90 through Tiberias, Jaffa road in Haifa…) rarely
            // carry a sidewalk tag although they have one; only an explicit "no sidewalk",
            // motorroad/expressway or access=no keeps pedestrians off
            Class::Trunk => sidewalk.is_some() || (!sidewalk_none && !motorroad && !access_no),
            Class::Cycleway => true,
            _ => !access_no,
        },
    };
    // bike
    let bike_ok = match bicycle {
        Some(v) if no(v) => false,
        Some(v) if yes(v) => true,
        _ => match class {
            Class::Motorway => false,
            Class::Trunk => sidewalk.is_some() || cycleway_any,
            Class::Steps => true, // pushing
            _ => !access_no,
        },
    };
    // oneway
    let ow = match oneway {
        Some("yes") | Some("true") | Some("1") => 1,
        Some("-1") | Some("reverse") => -1,
        Some("no") | Some("false") | Some("0") => 0,
        _ => {
            if roundabout || (class == Class::Motorway && !link) {
                1
            } else {
                0
            }
        }
    };
    let bike_ow = match oneway_bike {
        Some("no") | Some("false") | Some("0") => 0,
        Some("yes") | Some("true") | Some("1") => 1,
        Some("-1") => -1,
        _ => ow,
    };
    if car {
        if ow >= 0 {
            f |= flags::CAR_FWD;
        }
        if ow <= 0 {
            f |= flags::CAR_BWD;
        }
    }
    if foot_ok {
        f |= flags::FOOT;
    }
    if bike_ok {
        if bike_ow >= 0 {
            f |= flags::BIKE_FWD;
        }
        if bike_ow <= 0 {
            f |= flags::BIKE_BWD;
        }
    }
    if f & (flags::CAR_FWD | flags::CAR_BWD | flags::FOOT | flags::BIKE_FWD | flags::BIKE_BWD) == 0 {
        return None;
    }
    // posted limits and speed
    let both = maxspeed.and_then(parse_maxspeed).or_else(|| maxspeed_type.and_then(implicit_limit));
    let limit_fwd = maxspeed_fwd.and_then(parse_maxspeed).or(both);
    let limit_bwd = maxspeed_bwd.and_then(parse_maxspeed).or(both);
    let speed_of = |limit: Option<u8>| -> u8 {
        let mut speed = limit.unwrap_or_else(|| class.default_car_speed());
        if speed == 0 && car {
            speed = if ferry { 15 } else { 30 };
        }
        if link && limit.is_none() {
            speed = (speed as f32 * 0.7) as u8;
        }
        speed
    };
    // name: prefer Hebrew
    let n = name_he.or(name).or(name_en);
    let rf = r#ref.map(|r| r.trim()).filter(|r| !r.is_empty());
    let display = match (n, rf) {
        (Some(n), _) => Some(n.to_string()),
        (None, Some(r)) => Some(if r.chars().all(|c| c.is_ascii_digit()) { format!("כביש {r}") } else { r.to_string() }),
        (None, None) => None,
    };
    let clean = |v: Option<&str>| v.map(|x| x.split(';').map(str::trim).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(", ")).filter(|x| !x.is_empty());
    let dest_text = |d: Option<&str>, r: Option<&str>| -> Option<String> {
        match (clean(d), clean(r)) {
            (Some(d), _) => Some(d),
            (None, Some(r)) => Some(if r.chars().all(|c| c.is_ascii_digit()) { format!("כביש {r}") } else { r }),
            _ => None,
        }
    };
    let oneway_fwd = ow == 1;
    let oneway_bwd = ow == -1;
    let d_main = dest_he.or(dest);
    let dest_f = dest_text(dest_fwd.or(if !oneway_bwd { d_main } else { None }), dest_ref_fwd.or(if !oneway_bwd { dest_ref } else { None }));
    let dest_b = dest_text(dest_bwd.or(if oneway_bwd { d_main } else { None }), dest_ref_bwd.or(if oneway_bwd { dest_ref } else { None }));
    let lanes_f = lanes_fwd.or(if oneway_fwd { lanes } else { None }).map(str::to_string);
    let lanes_b = lanes_bwd.or(if oneway_bwd { lanes } else { None }).map(str::to_string);
    let dm = |m: Option<f64>| m.map(|x| (x * 10.0).round().clamp(1.0, 255.0) as u8).unwrap_or(0);
    let weight_ht = maxweight.and_then(parse_tonnes).map(|t| (t * 2.0).round().clamp(1.0, 255.0) as u8).unwrap_or(0);
    Some(WayAttrs {
        class: Some(class),
        flags: f,
        speed_fwd: speed_of(limit_fwd),
        speed_bwd: speed_of(limit_bwd),
        limit_fwd: limit_fwd.unwrap_or(0),
        limit_bwd: limit_bwd.unwrap_or(0),
        name: display,
        road_ref: rf.map(str::to_string),
        junction: junction_ref.map(|j| j.trim().to_string()).filter(|j| !j.is_empty()),
        dest_fwd: dest_f,
        dest_bwd: dest_b,
        lanes_fwd: lanes_f,
        lanes_bwd: lanes_b,
        height_dm: dm(maxheight.and_then(parse_metres)),
        weight_ht,
        width_dm: dm(maxwidth.and_then(parse_metres)),
        ferry_secs: if ferry { duration.and_then(parse_duration) } else { None },
    })
}

/// Restriction tags → (kind e.g. "no_left_turn", vehicle mask). None: not a usable restriction.
/// `conditional` counts restrictions that only exist with a condition (ignored).
fn restriction_kind<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>, conditional: &mut u64) -> Option<(bool, u8)> {
    let mut is_restriction = false;
    let mut generic = None;
    let mut specific: Option<(String, u8)> = None;
    let mut except = 0u8;
    let mut cond = false;
    for (k, v) in tags {
        match k {
            "type" => is_restriction = v == "restriction",
            "restriction" => generic = Some(v.to_string()),
            "restriction:motorcar" => specific = Some((v.to_string(), rmask::CAR)),
            "restriction:motor_vehicle" | "restriction:vehicle" => specific = Some((v.to_string(), rmask::CAR | rmask::TRUCK)),
            "restriction:hgv" => specific = Some((v.to_string(), rmask::TRUCK)),
            "restriction:bicycle" => {
                if specific.is_none() {
                    specific = Some((v.to_string(), rmask::BIKE));
                }
            }
            "except" => {
                for x in v.split(';').map(str::trim) {
                    except |= match x {
                        "bicycle" => rmask::BIKE,
                        "motorcar" => rmask::CAR,
                        "hgv" => rmask::TRUCK,
                        "motor_vehicle" | "vehicle" => rmask::CAR | rmask::TRUCK,
                        _ => 0,
                    };
                }
            }
            _ if k.starts_with("restriction") && k.ends_with(":conditional") => cond = true,
            _ => {}
        }
    }
    if !is_restriction {
        return None;
    }
    let (r, mask) = match (generic, specific) {
        (Some(g), _) => (g, rmask::ALL),
        (None, Some((s, m))) => (s, m),
        (None, None) => {
            if cond {
                *conditional += 1;
            }
            return None;
        }
    };
    let only = r.starts_with("only_");
    if !(only || r.starts_with("no_")) {
        return None;
    }
    let mask = mask & !except;
    (mask != 0).then_some((only, mask))
}

fn parse_restriction<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>, members: Vec<(RelMemberType, i64, String)>, conditional: &mut u64) -> Option<RelRec> {
    let (only, mask) = restriction_kind(tags, conditional)?;
    let mut from_way = None;
    let mut to_way = None;
    let mut via_node = None;
    let mut via_ways = Vec::new();
    for (t, id, role) in members {
        match (t, role.as_str()) {
            (RelMemberType::Way, "from") => from_way = Some(id),
            (RelMemberType::Way, "to") => to_way = Some(id),
            (RelMemberType::Node, "via") => via_node = Some(id),
            (RelMemberType::Way, "via") => via_ways.push(id),
            _ => {}
        }
    }
    if via_node.is_none() && via_ways.is_empty() {
        return None;
    }
    if via_node.is_some() {
        via_ways.clear();
    }
    Some(RelRec { from_way: from_way?, via_node, via_ways, to_way: to_way?, only, mask })
}

/// Where the DEM for elevation sampling is: `MO_DEM`, else the terrain archive of the data
/// directory that holds `out` (`<root>/routing/graph.bin…`).
fn dem_for(out: &Path) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("MO_DEM") {
        let p = std::path::PathBuf::from(p);
        return p.exists().then_some(p);
    }
    let root = out.parent()?.parent()?;
    let p = crate::paths::DataPaths::new(root.to_path_buf()).terrain_file_in_use();
    p.exists().then_some(p)
}

/// Accumulated ascent / descent with a hysteresis band (DEM noise on flat roads is not climbing).
pub fn climb(heights: &[f32]) -> (f32, f32) {
    const H: f32 = 1.5;
    let mut it = heights.iter().copied().filter(|h| h.is_finite());
    let Some(mut anchor) = it.next() else { return (0.0, 0.0) };
    let (mut up, mut down) = (0.0f32, 0.0f32);
    for h in it {
        if h - anchor > H {
            up += h - anchor;
            anchor = h;
        } else if anchor - h > H {
            down += anchor - h;
            anchor = h;
        }
    }
    (up, down)
}

impl Graph {
    /// Sample the DEM along every edge into `e_rise` / `e_fall` (parallel, spatially coherent
    /// chunks so each worker decodes few tiles). Edges with no DEM coverage stay 0.
    pub fn sample_elevation(&mut self, dem: &Path) -> anyhow::Result<()> {
        use rayon::prelude::*;
        let t0 = std::time::Instant::now();
        let ne = self.num_edges();
        const CHUNK: usize = 20_000;
        let ranges: Vec<(usize, usize)> = (0..ne).step_by(CHUNK).map(|a| (a, (a + CHUNK).min(ne))).collect();
        let parts: Vec<anyhow::Result<Vec<(u16, u16)>>> = ranges
            .par_iter()
            .map(|&(a, b)| {
                let mut pts: Vec<LonLat> = Vec::new();
                let mut off = Vec::with_capacity(b - a + 1);
                off.push(0usize);
                for e in a..b {
                    pts.extend(self.edge_points(e as u32));
                    off.push(pts.len());
                }
                let h = crate::data::terrain::elevations(dem, &pts)?;
                Ok((0..b - a)
                    .map(|i| {
                        let hs: Vec<f32> = h[off[i]..off[i + 1]].iter().map(|x| x.unwrap_or(f32::NAN)).collect();
                        let (up, down) = climb(&hs);
                        ((up * 10.0).round().min(65535.0) as u16, (down * 10.0).round().min(65535.0) as u16)
                    })
                    .collect())
            })
            .collect();
        let mut rise = Vec::with_capacity(ne);
        let mut fall = Vec::with_capacity(ne);
        for p in parts {
            for (r, f) in p? {
                rise.push(r);
                fall.push(f);
            }
        }
        let any = rise.iter().any(|&r| r > 0) || fall.iter().any(|&f| f > 0);
        if any {
            self.e_rise = rise;
            self.e_fall = fall;
        }
        log::info!("elevation sampled for {ne} edges in {:?} (data: {any})", t0.elapsed());
        Ok(())
    }
}

/// Build the graph from a PBF and write it to `out`.
pub fn build(pbf: &Path, out: &Path, progress: ProgressFn) -> anyhow::Result<()> {
    let t0 = std::time::Instant::now();
    progress(ProgressPhase::Processing, 0, 100, "קורא את רשת הדרכים מ-OSM");

    // ---- pass 0: restricted-territory boundary relations (Area A / B, Gaza) – their member
    // ways are needed in pass 1 and are not routable ways themselves.
    let reader = ElementReader::from_path(pbf)?;
    let area_rels: Vec<AreaRel> = reader.par_map_reduce(
        |el| match el {
            Element::Relation(r) => match areas::kind_for_tags(r.tags()) {
                Some(kind) => {
                    let ways: Vec<i64> = r
                        .members()
                        .filter(|m| m.member_type == RelMemberType::Way && matches!(m.role().unwrap_or(""), "" | "outer"))
                        .map(|m| m.member_id)
                        .collect();
                    log::info!("area relation {} kind {kind}: {} outer ways", r.id(), ways.len());
                    vec![AreaRel { kind, ways }]
                }
                None => Vec::new(),
            },
            _ => Vec::new(),
        },
        Vec::new,
        |mut a, mut b| {
            a.append(&mut b);
            a
        },
    )?;
    let area_way_ids: std::sync::Arc<FxHashSet<i64>> = std::sync::Arc::new(area_rels.iter().flat_map(|r| r.ways.iter().copied()).collect());
    if area_rels.is_empty() {
        log::warn!("no Area A/B boundary relations found in the extract – territory avoidance will be unavailable");
    }
    log::info!("pass 0: {} area relations, {} member ways in {:?}", area_rels.len(), area_way_ids.len(), t0.elapsed());

    // ---- pass 1: ways + restriction relations
    let counter = AtomicU64::new(0);
    let reader = ElementReader::from_path(pbf)?;
    let area_way_ids1 = area_way_ids.clone();
    let acc: Acc = reader.par_map_reduce(
        |el| {
            let c = counter.fetch_add(1, Ordering::Relaxed);
            if c.is_multiple_of(2_000_000) {
                progress(ProgressPhase::Processing, 5, 100, &format!("סורק נתוני OSM... {} מיליון רכיבים", c / 1_000_000));
            }
            match el {
                Element::Way(w) => {
                    let mut acc = Acc::default();
                    if area_way_ids1.contains(&w.id()) {
                        acc.area_ways.push((w.id(), w.refs().collect()));
                    }
                    if let Some(a) = parse_way_tags(w.tags()) {
                        let refs: Vec<i64> = w.refs().collect();
                        if refs.len() >= 2 {
                            acc.ways.push(WayRec { id: w.id(), refs, a });
                        }
                    }
                    acc
                }
                Element::Relation(r) => {
                    let members: Vec<(RelMemberType, i64, String)> =
                        r.members().map(|m| { let role = m.role().unwrap_or("").to_string(); (m.member_type, m.member_id, role) }).collect();
                    let mut acc = Acc::default();
                    if let Some(rr) = parse_restriction(r.tags(), members, &mut acc.conditional) {
                        acc.rels.push(rr);
                    }
                    acc
                }
                _ => Acc::default(),
            }
        },
        Acc::default,
        |mut a, mut b| {
            if a.ways.len() < b.ways.len() {
                std::mem::swap(&mut a, &mut b);
            }
            a.ways.append(&mut b.ways);
            a.rels.append(&mut b.rels);
            a.area_ways.append(&mut b.area_ways);
            a.conditional += b.conditional;
            a
        },
    )?;
    let Acc { ways, rels, area_ways, conditional } = acc;
    let area_ways: FxHashMap<i64, Vec<i64>> = area_ways.into_iter().collect();
    let n_via_way = rels.iter().filter(|r| r.via_node.is_none()).count();
    log::info!(
        "pass 1: {} ways, {} restrictions ({} via ways, {} conditional-only ignored), {} area ways in {:?}",
        ways.len(),
        rels.len(),
        n_via_way,
        conditional,
        area_ways.len(),
        t0.elapsed()
    );
    if !progress(ProgressPhase::Processing, 30, 100, &format!("נמצאו {} דרכים; קורא צמתים", ways.len())) {
        anyhow::bail!("cancelled");
    }

    // node usage: 2 = vertex
    let mut usage: FxHashMap<i64, u8> = FxHashMap::default();
    usage.reserve(ways.len() * 8);
    for w in &ways {
        let last = w.refs.len() - 1;
        for (i, r) in w.refs.iter().enumerate() {
            let c = usage.entry(*r).or_insert(0);
            if i == 0 || i == last {
                *c = 2;
            } else {
                *c = c.saturating_add(1);
            }
        }
    }
    // via nodes of restrictions must be vertices
    for r in &rels {
        if let Some(v) = r.via_node {
            if let Some(c) = usage.get_mut(&v) {
                *c = 2;
            }
        }
    }
    // area ring nodes: coordinates needed, but they never become vertices on their own
    for refs in area_ways.values() {
        for r in refs {
            usage.entry(*r).or_insert(0);
        }
    }

    // ---- pass 2: node coordinates (+ signals, speed cameras anywhere)
    let needed = std::sync::Arc::new(usage);
    let counter2 = AtomicU64::new(0);
    let reader = ElementReader::from_path(pbf)?;
    let needed2 = needed.clone();
    const N_SIGNAL: u8 = 1;
    const N_CAMERA: u8 = 2;
    const N_NEEDED: u8 = 4;
    let mut coords: Vec<(i64, i32, i32, u8)> = reader.par_map_reduce(
        move |el| {
            let c = counter2.fetch_add(1, Ordering::Relaxed);
            if c.is_multiple_of(2_000_000) {
                progress(ProgressPhase::Processing, 30 + (c / 1_000_000).min(20), 100, "קורא קואורדינטות");
            }
            // `highway=traffic_signals` on a junction node (not `crossing=traffic_signals`);
            // `highway=speed_camera` anywhere
            fn kind<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> u8 {
                let mut k = 0;
                for (key, v) in tags {
                    if key == "highway" {
                        if v == "traffic_signals" {
                            k |= N_SIGNAL;
                        } else if v == "speed_camera" {
                            k |= N_CAMERA;
                        }
                    }
                }
                k
            }
            let (id, lon, lat, k) = match el {
                Element::DenseNode(n) => {
                    let needed = needed2.contains_key(&n.id());
                    let k = kind(n.tags());
                    if !needed && k & N_CAMERA == 0 {
                        return Vec::new();
                    }
                    (n.id(), n.lon(), n.lat(), k | if needed { N_NEEDED } else { 0 })
                }
                Element::Node(n) => {
                    let needed = needed2.contains_key(&n.id());
                    let k = kind(n.tags());
                    if !needed && k & N_CAMERA == 0 {
                        return Vec::new();
                    }
                    (n.id(), n.lon(), n.lat(), k | if needed { N_NEEDED } else { 0 })
                }
                _ => return Vec::new(),
            };
            vec![(id, (lon * 1e7).round() as i32, (lat * 1e7).round() as i32, k)]
        },
        Vec::new,
        |mut a, mut b| {
            if a.len() < b.len() {
                std::mem::swap(&mut a, &mut b);
            }
            a.append(&mut b);
            a
        },
    )?;
    coords.sort_unstable_by_key(|c| c.0);
    coords.dedup_by_key(|c| c.0);
    let signal_nodes: FxHashSet<i64> = coords.iter().filter(|c| c.3 & N_SIGNAL != 0).map(|c| c.0).collect();
    let mut camera_pts: Vec<[i32; 2]> = coords.iter().filter(|c| c.3 & N_CAMERA != 0).map(|c| [c.1, c.2]).collect();
    camera_pts.sort_unstable();
    camera_pts.dedup();
    coords.retain(|c| c.3 & N_NEEDED != 0);
    log::info!("pass 2: {} node coords ({} traffic signals, {} speed cameras) in {:?}", coords.len(), signal_nodes.len(), camera_pts.len(), t0.elapsed());
    if !progress(ProgressPhase::Processing, 55, 100, "בונה גרף דרכים") {
        anyhow::bail!("cancelled");
    }
    let usage = needed;
    let lookup = |id: i64| -> Option<(i32, i32)> { coords.binary_search_by_key(&id, |c| c.0).ok().map(|i| (coords[i].1, coords[i].2)) };

    // ---- compaction
    let restricted_ways: FxHashSet<i64> = rels.iter().flat_map(|r| [r.from_way, r.to_way].into_iter().chain(r.via_ways.iter().copied())).collect();
    let mut way_edges: FxHashMap<i64, Vec<u32>> = FxHashMap::default();
    let mut vid: FxHashMap<i64, u32> = FxHashMap::default();
    let mut g = Graph::default();
    let mut names: FxHashMap<String, u32> = FxHashMap::default();
    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    g.e_geom_off.push(0);

    let mut vertex_for = |id: i64, lon: i32, lat: i32, g: &mut Graph| -> u32 {
        *vid.entry(id).or_insert_with(|| {
            g.vlon.push(lon);
            g.vlat.push(lat);
            (g.vlon.len() - 1) as u32
        })
    };
    fn intern(g: &mut Graph, names: &mut FxHashMap<String, u32>, n: Option<&String>) -> u32 {
        let Some(n) = n else { return NO_NAME };
        *names.entry(n.clone()).or_insert_with(|| {
            if g.name_off.is_empty() {
                g.name_off.push(0);
            }
            g.name_buf.push_str(n);
            g.name_off.push(g.name_buf.len() as u32);
            (g.name_off.len() - 2) as u32
        })
    }

    for (wi, w) in ways.iter().enumerate() {
        if wi % 100_000 == 0 && !progress(ProgressPhase::Processing, 55 + (wi * 25 / ways.len().max(1)) as u64, 100, "בונה גרף דרכים") {
            anyhow::bail!("cancelled");
        }
        let a = &w.a;
        let class = a.class.unwrap_or(Class::Other);
        let name_id = intern(&mut g, &mut names, a.name.as_ref());
        let ext = if a.has_ext() {
            Some([
                intern(&mut g, &mut names, a.road_ref.as_ref()),
                intern(&mut g, &mut names, a.junction.as_ref()),
                intern(&mut g, &mut names, a.dest_fwd.as_ref()),
                intern(&mut g, &mut names, a.dest_bwd.as_ref()),
                intern(&mut g, &mut names, a.lanes_fwd.as_ref()),
                intern(&mut g, &mut names, a.lanes_bwd.as_ref()),
            ])
        } else {
            None
        };
        // ferries: speed from the crossing time over the whole way
        let (speed_fwd, speed_bwd) = match a.ferry_secs {
            Some(secs) => {
                let pts: Vec<LonLat> = w.refs.iter().filter_map(|&r| lookup(r)).map(|(x, y)| [x as f64 * 1e-7, y as f64 * 1e-7]).collect();
                let len: f64 = pts.windows(2).map(|p| haversine_m(p[0], p[1])).sum();
                let kmh = (len / secs as f64 * 3.6).clamp(3.0, 60.0) as u8;
                (kmh, kmh)
            }
            None => (a.speed_fwd, a.speed_bwd),
        };
        // chain of points since last vertex
        let mut chain: Vec<(i64, i32, i32)> = Vec::new();
        let last = w.refs.len() - 1;
        for (i, &r) in w.refs.iter().enumerate() {
            let Some((lon, lat)) = lookup(r) else {
                // missing node (outside extract) – break the chain
                chain.clear();
                continue;
            };
            let is_vertex = i == 0 || i == last || usage.get(&r).copied().unwrap_or(0) >= 2;
            if chain.is_empty() {
                if is_vertex {
                    chain.push((r, lon, lat));
                }
                // else: dangling start after a missing node – skip until the next vertex
                continue;
            }
            chain.push((r, lon, lat));
            if is_vertex {
                // emit edge chain[0] .. chain[last]
                let (fid, flon, flat) = chain[0];
                let (tid, tlon, tlat) = *chain.last().unwrap();
                if fid != tid || chain.len() > 2 {
                    let from = vertex_for(fid, flon, flat, &mut g);
                    let to = vertex_for(tid, tlon, tlat, &mut g);
                    let pts: Vec<LonLat> = chain.iter().map(|c| [c.1 as f64 * 1e-7, c.2 as f64 * 1e-7]).collect();
                    let len: f64 = pts.windows(2).map(|p| haversine_m(p[0], p[1])).sum();
                    let bs = bearing_deg(pts[0], pts[1]).round() as u16 % 360;
                    let be = bearing_deg(pts[pts.len() - 2], pts[pts.len() - 1]).round() as u16 % 360;
                    let eid = g.e_from.len() as u32;
                    g.e_from.push(from);
                    g.e_to.push(to);
                    g.e_len.push(len.max(0.1) as f32);
                    g.e_speed.push(speed_fwd);
                    g.e_speed_bwd.push(speed_bwd);
                    g.e_limit_fwd.push(a.limit_fwd);
                    g.e_limit_bwd.push(a.limit_bwd);
                    g.e_flags.push(a.flags);
                    g.e_class.push(class as u8);
                    g.e_name.push(name_id);
                    g.e_bear_start.push(bs);
                    g.e_bear_end.push(be);
                    if let Some(x) = ext {
                        g.ext.edge.push(eid);
                        g.ext.road_ref.push(x[0]);
                        g.ext.junction.push(x[1]);
                        g.ext.dest_fwd.push(x[2]);
                        g.ext.dest_bwd.push(x[3]);
                        g.ext.lanes_fwd.push(x[4]);
                        g.ext.lanes_bwd.push(x[5]);
                    }
                    if a.has_limits() {
                        g.limits.edge.push(eid);
                        g.limits.height_dm.push(a.height_dm);
                        g.limits.weight_ht.push(a.weight_ht);
                        g.limits.width_dm.push(a.width_dm);
                    }
                    for c in &chain[1..chain.len() - 1] {
                        g.geom.push(c.1);
                        g.geom.push(c.2);
                    }
                    g.e_geom_off.push((g.geom.len() / 2) as u32);
                    for p in &pts {
                        bbox[0] = bbox[0].min(p[0]);
                        bbox[1] = bbox[1].min(p[1]);
                        bbox[2] = bbox[2].max(p[0]);
                        bbox[3] = bbox[3].max(p[1]);
                    }
                    if restricted_ways.contains(&w.id) {
                        way_edges.entry(w.id).or_default().push(eid);
                    }
                }
                chain.clear();
                chain.push((r, lon, lat));
            }
        }
    }
    drop(ways);
    if g.e_limit_fwd.iter().chain(g.e_limit_bwd.iter()).all(|&l| l == 0) {
        g.e_limit_fwd.clear();
        g.e_limit_bwd.clear();
    }
    if g.e_speed_bwd == g.e_speed {
        g.e_speed_bwd.clear();
    }

    // ---- restricted territories: stitch rings, flag edges
    for rel in &area_rels {
        let segs: Vec<Vec<[i32; 2]>> = rel
            .ways
            .iter()
            .filter_map(|w| area_ways.get(w))
            .map(|refs| refs.iter().filter_map(|&r| lookup(r)).map(|(lon, lat)| [lon, lat]).collect())
            .collect();
        let (rings, unclosed) = areas::stitch(segs);
        if unclosed > 0 {
            log::warn!("area kind {}: {} chains did not close (skipped), {} rings kept", rel.kind, unclosed, rings.len());
        }
        for ring in rings {
            g.areas.push_ring(rel.kind, &ring);
        }
    }
    drop(coords);
    g.areas.build_index();
    if !g.areas.is_empty() {
        let ta = std::time::Instant::now();
        let (mut na, mut nb) = (0usize, 0usize);
        for e in 0..g.e_from.len() as u32 {
            let k = g.areas.classify_edge(&g.edge_points(e));
            if k & areas::KIND_A != 0 {
                g.e_flags[e as usize] |= flags::AREA_A;
                na += 1;
            }
            if k & areas::KIND_B != 0 {
                g.e_flags[e as usize] |= flags::AREA_B;
                nb += 1;
            }
        }
        log::info!("areas: {} rings; {} edges in A, {} in B, classified in {:?}", g.areas.num_rings(), na, nb, ta.elapsed());
    }
    g.bbox = bbox;

    // adjacency CSR
    let nv = g.vlon.len();
    let ne = g.e_from.len();
    g.build_adjacency();

    // signalised junctions (only vertices that survived compaction, i.e. real junctions / ends)
    g.v_signal = vec![0u8; nv.div_ceil(8)];
    let mut n_sig = 0usize;
    for (id, &v) in &vid {
        if signal_nodes.contains(id) {
            g.v_signal[v as usize / 8] |= 1 << (v % 8);
            n_sig += 1;
        }
    }
    log::info!("{} signalised junction vertices", n_sig);

    // speed cameras within 60 m of a car road (others: on paths, wrongly tagged)
    {
        let car = |e: u32| g.e_flags[e as usize] & (flags::CAR_FWD | flags::CAR_BWD) != 0;
        // the grid is built later; a temporary one is cheap
        g.proj0 = Proj::for_bbox(g.bbox);
        g.vpx = (0..nv).map(|i| g.proj(g.vlon[i] as f64 * 1e-7, g.vlat[i] as f64 * 1e-7)[0]).collect();
        g.vpy = (0..nv).map(|i| g.proj(g.vlon[i] as f64 * 1e-7, g.vlat[i] as f64 * 1e-7)[1]).collect();
        g.grid = Grid::build(&g);
        let kept: Vec<i32> = camera_pts
            .iter()
            .filter(|c| g.snap([c[0] as f64 * 1e-7, c[1] as f64 * 1e-7], &car).is_some_and(|s| s.dist_m <= 60.0))
            .flat_map(|c| [c[0], c[1]])
            .collect();
        log::info!("{} speed cameras on car roads", kept.len() / 2);
        g.cameras = kept;
    }

    // restrictions
    let (mut restrictions, mut multi) = (Vec::new(), Vec::new());
    let (mut ambiguous, mut unresolved) = (0usize, 0usize);
    let edges_at = |way: i64, v: u32, g: &Graph| -> Vec<u32> {
        way_edges.get(&way).map(|es| es.iter().copied().filter(|&e| g.e_from[e as usize] == v || g.e_to[e as usize] == v).collect()).unwrap_or_default()
    };
    for r in &rels {
        if let Some(vn) = r.via_node {
            let Some(&via) = vid.get(&vn) else { continue };
            let (fs, ts) = (edges_at(r.from_way, via, &g), edges_at(r.to_way, via, &g));
            if fs.is_empty() || ts.is_empty() {
                unresolved += 1;
                continue;
            }
            if fs.len() > 1 || ts.len() > 1 {
                ambiguous += 1;
            }
            // a from / to way passing through the via node (not split there): the restriction
            // holds for each of its pieces at the via node
            for &fe in &fs {
                for &te in &ts {
                    if fe != te || r.from_way == r.to_way {
                        restrictions.push(Restriction { via, from_edge: fe, to_edge: te, only: r.only, mask: r.mask });
                    }
                }
            }
        } else {
            match via_way_chain(&g, &way_edges, r) {
                Some(edges) => multi.push(MultiRestriction { edges, only: r.only, mask: r.mask }),
                None => unresolved += 1,
            }
        }
    }
    restrictions.sort_by_key(|r| r.via);
    g.restrictions = restrictions;
    g.multi = multi;
    log::info!("restrictions: {} via node, {} via way, {} ambiguous, {} not resolvable", g.restrictions.len(), g.multi.len(), ambiguous, unresolved);

    log::info!(
        "graph: {} vertices, {} edges, {} geom pts, {} names, {} restrictions, built in {:?}",
        nv,
        ne,
        g.geom.len() / 2,
        g.name_off.len().saturating_sub(1),
        g.restrictions.len(),
        t0.elapsed()
    );
    g.finish_build();
    if let Some(dem) = dem_for(out) {
        progress(ProgressPhase::Processing, 82, 100, "מחשב שיפועים ממודל הגובה");
        if let Err(e) = g.sample_elevation(&dem) {
            log::warn!("elevation sampling failed (graph without elevation): {e:#}");
        }
    }
    progress(ProgressPhase::Finalizing, 85, 100, "שומר את גרף הדרכים");
    save(&g, out)?;
    progress(ProgressPhase::Finalizing, 100, 100, &format!("גרף הדרכים נבנה: {} צמתים, {} קטעי דרך", nv, ne));
    Ok(())
}

/// Edge sequence from, via…, to of a via-way restriction (None when the ways do not connect).
fn via_way_chain(g: &Graph, way_edges: &FxHashMap<i64, Vec<u32>>, r: &RelRec) -> Option<Vec<u32>> {
    let ends = |e: u32| (g.e_from[e as usize], g.e_to[e as usize]);
    let touches = |es: &[u32], v: u32| es.iter().copied().find(|&e| {
        let (a, b) = ends(e);
        a == v || b == v
    });
    let from_edges = way_edges.get(&r.from_way)?;
    let to_edges = way_edges.get(&r.to_way)?;
    // vertices of the from way
    let from_vs: FxHashSet<u32> = from_edges.iter().flat_map(|&e| [ends(e).0, ends(e).1]).collect();
    let mut chain: Vec<u32> = Vec::new();
    let mut cur: Option<u32> = None; // current vertex
    let mut from_edge = None;
    for (k, w) in r.via_ways.iter().enumerate() {
        let es = way_edges.get(w)?;
        // entry vertex: for the first via way the one shared with the from way, else the current
        let entry = match cur {
            Some(v) => v,
            None => {
                let v = es.iter().flat_map(|&e| [ends(e).0, ends(e).1]).find(|v| from_vs.contains(v))?;
                from_edge = touches(from_edges, v);
                v
            }
        };
        // walk the via way's edges from the entry vertex
        let mut used = vec![false; es.len()];
        let mut v = entry;
        while let Some(i) = (0..es.len()).find(|&i| !used[i] && (ends(es[i]).0 == v || ends(es[i]).1 == v)) {
            used[i] = true;
            chain.push(es[i]);
            let (a, b) = ends(es[i]);
            v = if a == v { b } else { a };
            // stop at the next via way / the to way
            let next_way_touch = match r.via_ways.get(k + 1) {
                Some(nw) => way_edges.get(nw).is_some_and(|ne| touches(ne, v).is_some()),
                None => touches(to_edges, v).is_some(),
            };
            if next_way_touch {
                break;
            }
        }
        if v == entry {
            return None;
        }
        cur = Some(v);
    }
    let exit = cur?;
    let to_edge = to_edges.iter().copied().find(|&e| {
        let (a, b) = ends(e);
        (a == exit || b == exit) && !chain.contains(&e)
    })?;
    let mut out = vec![from_edge?];
    out.extend(chain);
    out.push(to_edge);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maxspeed_parse() {
        assert_eq!(parse_maxspeed("50"), Some(50));
        assert_eq!(parse_maxspeed("30 mph"), Some(48));
        assert_eq!(parse_maxspeed("none"), None);
        assert_eq!(parse_maxspeed("abc"), None);
    }

    #[test]
    fn way_tags_residential() {
        let tags = vec![("highway", "residential"), ("name", "Herzl"), ("name:he", "הרצל")];
        let a = parse_way_tags(tags.into_iter()).unwrap();
        let (class, f, speed, name) = (a.class.unwrap(), a.flags, a.speed_fwd, a.name);
        assert_eq!(class, Class::Residential);
        assert!(f & flags::CAR_FWD != 0 && f & flags::CAR_BWD != 0);
        assert!(f & flags::FOOT != 0 && f & flags::BIKE_FWD != 0);
        assert_eq!(speed, 40);
        assert_eq!(name.as_deref(), Some("הרצל"));
    }

    #[test]
    fn way_tags_motorway_oneway_toll() {
        let tags = vec![("highway", "motorway"), ("ref", "6"), ("toll", "yes")];
        let a = parse_way_tags(tags.into_iter()).unwrap();
        let (class, f, speed, name) = (a.class.unwrap(), a.flags, a.speed_fwd, a.name.clone());
        assert_eq!(a.road_ref.as_deref(), Some("6"));
        assert_eq!(class, Class::Motorway);
        assert!(f & flags::CAR_FWD != 0 && f & flags::CAR_BWD == 0);
        assert!(f & flags::FOOT == 0 && f & flags::BIKE_FWD == 0);
        assert!(f & flags::TOLL != 0 && f & flags::MOTORWAY != 0);
        assert_eq!(speed, 110);
        assert_eq!(name.as_deref(), Some("כביש 6"));
    }

    #[test]
    fn way_tags_footway_and_private() {
        let tags = vec![("highway", "footway")];
        let f = parse_way_tags(tags.into_iter()).unwrap().flags;
        assert!(f & flags::FOOT != 0 && f & (flags::CAR_FWD | flags::CAR_BWD) == 0);
        let tags = vec![("highway", "service"), ("access", "private")];
        assert!(parse_way_tags(tags.into_iter()).is_none());
        let tags = vec![("highway", "service"), ("access", "private"), ("foot", "yes")];
        let f = parse_way_tags(tags.into_iter()).unwrap().flags;
        assert!(f & flags::FOOT != 0 && f & flags::CAR_FWD == 0);
    }

    /// xorshift64*: deterministic randomness without a dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn chance(&mut self, pct: u64) -> bool {
            self.below(100) < pct
        }
    }

    /// Random street network on a jittered grid (~150 m spacing): mixed classes / speeds,
    /// one-way streets, roundabout pieces, turn bans and "only" turns, real bearings so the
    /// turn costs apply.
    fn random_graph(rng: &mut Rng, side: usize) -> Graph {
        let mut g = Graph::default();
        let (lon0, lat0) = (34.80, 32.05);
        for y in 0..side {
            for x in 0..side {
                let jx = (rng.below(600) as f64 - 300.0) * 1e-6;
                let jy = (rng.below(600) as f64 - 300.0) * 1e-6;
                g.vlon.push(((lon0 + x as f64 * 0.0016 + jx) * 1e7) as i32);
                g.vlat.push(((lat0 + y as f64 * 0.0014 + jy) * 1e7) as i32);
            }
        }
        g.bbox = [lon0 - 0.01, lat0 - 0.01, lon0 + side as f64 * 0.0016 + 0.01, lat0 + side as f64 * 0.0014 + 0.01];
        g.e_geom_off.push(0);
        g.name_off.push(0);
        let classes = [Class::Primary, Class::Secondary, Class::Tertiary, Class::Residential, Class::Residential];
        fn bearing(g: &Graph, a: u32, b: u32) -> u16 {
            let (pa, pb) = (g.vertex(a), g.vertex(b));
            let dx = (pb[0] - pa[0]) * pa[1].to_radians().cos();
            let dy = pb[1] - pa[1];
            (((dx.atan2(dy).to_degrees() + 360.0) % 360.0).round() as u16) % 360
        }
        let add = |g: &mut Graph, rng: &mut Rng, a: u32, b: u32| {
            let class = classes[rng.below(classes.len() as u64) as usize];
            let mut f = flags::FOOT | flags::CAR_FWD | flags::BIKE_FWD;
            if !rng.chance(25) {
                f |= flags::CAR_BWD | flags::BIKE_BWD;
            }
            if rng.chance(10) {
                f |= flags::ROUNDABOUT;
            }
            g.e_from.push(a);
            g.e_to.push(b);
            g.e_len.push(crate::geo::haversine_m(g.vertex(a), g.vertex(b)) as f32);
            g.e_speed.push([30u8, 50, 70, 90][rng.below(4) as usize]);
            g.e_flags.push(f);
            g.e_class.push(class as u8);
            g.e_name.push(NO_NAME);
            let bs = bearing(g, a, b);
            g.e_bear_start.push(bs);
            g.e_bear_end.push(bs);
            g.e_geom_off.push((g.geom.len() / 2) as u32);
        };
        for y in 0..side {
            for x in 0..side {
                let v = (y * side + x) as u32;
                if x + 1 < side && !rng.chance(8) {
                    add(&mut g, rng, v, v + 1);
                }
                if y + 1 < side && !rng.chance(8) {
                    add(&mut g, rng, v, v + side as u32);
                }
                if x + 1 < side && y + 1 < side && rng.chance(12) {
                    add(&mut g, rng, v, v + side as u32 + 1);
                }
            }
        }
        g.build_adjacency();
        // turn restrictions between edges meeting at a vertex
        let ne = g.e_from.len() as u32;
        for _ in 0..(ne / 6) {
            let e1 = rng.below(ne as u64) as u32;
            let via = if rng.chance(50) { g.e_from[e1 as usize] } else { g.e_to[e1 as usize] };
            let others: Vec<u32> = g.incident(via).iter().copied().filter(|&e| e != e1).collect();
            if others.is_empty() {
                continue;
            }
            let e2 = others[rng.below(others.len() as u64) as usize];
            let only = rng.chance(20);
            g.restrictions.push(Restriction { via, from_edge: e1, to_edge: e2, only, mask: rmask::ALL });
        }
        g.finish_build();
        g
    }

    /// Plain forward Dijkstra over directed states with the same costs (turns, restrictions,
    /// partial first / last edges) as the bidirectional A*: the reference answer.
    fn dijkstra(g: &Graph, prof: &crate::routing::profile::Profile, from: &Snap, to: &Snap) -> Option<f32> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let fd = |snap: &Snap, dir: u32| if dir == 0 { snap.frac } else { 1.0 - snap.frac };
        let n = g.num_states();
        let mut dist = vec![f32::INFINITY; n];
        let mut heap = BinaryHeap::new();
        for d in 0..2u32 {
            let s = from.edge * 2 + d;
            if prof.allowed(g, s) {
                let c = prof.cost(g, s) * (1.0 - fd(from, d));
                dist[s as usize] = c;
                heap.push(Reverse(((c * 1000.0) as u64, s)));
            }
        }
        while let Some(Reverse((k, s))) = heap.pop() {
            let base = dist[s as usize];
            if k > (base * 1000.0) as u64 {
                continue;
            }
            let v = g.head(s);
            for &e2 in g.incident(v) {
                let s2 = g.state_from(v, e2);
                if !prof.allowed(g, s2) {
                    continue;
                }
                let Some(t) = prof.turn_cost(g, s, s2) else { continue };
                let nd = base + t + prof.cost(g, s2);
                if nd < dist[s2 as usize] {
                    dist[s2 as usize] = nd;
                    heap.push(Reverse(((nd * 1000.0) as u64, s2)));
                }
            }
        }
        (0..2u32)
            .map(|d| to.edge * 2 + d)
            .filter(|&s| dist[s as usize].is_finite())
            .map(|s| {
                let w = prof.cost(g, s);
                dist[s as usize] - w + w * fd(to, s & 1)
            })
            .min_by(|a, b| a.total_cmp(b))
    }

    #[test]
    fn astar_matches_dijkstra_on_random_graphs() {
        use crate::routing::astar::{search, SearchParams};
        use crate::routing::profile::Profile;
        use crate::types::{Avoid, Mode};
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut checked = 0;
        for round in 0..30 {
            let g = random_graph(&mut rng, 8 + (round % 5));
            let ne = g.num_edges() as u64;
            for (mode, hard) in [(Mode::Car, false), (Mode::Car, true), (Mode::Bike, false), (Mode::Foot, false)] {
                let prof = Profile::new(mode, Avoid { hard_turns: hard, ..Default::default() });
                for _ in 0..12 {
                    let (ea, eb) = (rng.below(ne) as u32, rng.below(ne) as u32);
                    if ea == eb {
                        continue;
                    }
                    let fa = rng.below(1000) as f32 / 1000.0;
                    let fb = rng.below(1000) as f32 / 1000.0;
                    let from = Snap { edge: ea, frac: fa, point: g.vertex(g.e_from[ea as usize]), dist_m: 0.0 };
                    let to = Snap { edge: eb, frac: fb, point: g.vertex(g.e_to[eb as usize]), dist_m: 0.0 };
                    let want = dijkstra(&g, &prof, &from, &to);
                    let params = SearchParams::new(&prof);
                    let got = search(&g, &params, &from, &to).map(|p| p.cost);
                    match (want, got) {
                        (None, None) => {}
                        (Some(w), Some(c)) => {
                            assert!((w - c).abs() <= 0.05 + w * 1e-4, "round {round} {mode:?} hard={hard} {ea}->{eb}: dijkstra {w} a* {c}");
                            checked += 1;
                        }
                        (w, c) => panic!("round {round} {mode:?} {ea}->{eb}: dijkstra {w:?} a* {c:?}"),
                    }
                }
            }
        }
        assert!(checked > 300, "only {checked} reachable pairs");
    }

    /// Tiny square network a-b-c-d (+ diagonal with a bend) for format / index tests.
    fn tiny() -> Graph {
        let mut g = Graph::default();
        let pts = [(34.78, 32.08), (34.79, 32.08), (34.79, 32.09), (34.78, 32.09)];
        for (lon, lat) in pts {
            g.vlon.push((lon * 1e7) as i32);
            g.vlat.push((lat * 1e7) as i32);
        }
        g.e_geom_off.push(0);
        g.name_off.push(0);
        g.name_buf.push_str("הרצל");
        g.name_off.push(g.name_buf.len() as u32);
        let edges = [(0u32, 1u32), (1, 2), (2, 3), (3, 0), (0, 2)];
        for (i, &(a, b)) in edges.iter().enumerate() {
            g.e_from.push(a);
            g.e_to.push(b);
            g.e_len.push(1000.0);
            g.e_speed.push(if i == 4 { 90 } else { 50 });
            g.e_flags.push(flags::CAR_FWD | flags::CAR_BWD | flags::FOOT);
            g.e_class.push(Class::Residential as u8);
            g.e_name.push(if i == 0 { 0 } else { NO_NAME });
            g.e_bear_start.push(0);
            g.e_bear_end.push(0);
            if i == 4 {
                // bend through the south-east corner area
                g.geom.extend_from_slice(&[(34.788 * 1e7) as i32, (32.082 * 1e7) as i32]);
            }
            g.e_geom_off.push((g.geom.len() / 2) as u32);
        }
        g.restrictions.push(Restriction { via: 1, from_edge: 0, to_edge: 1, only: false, mask: rmask::ALL });
        g.build_adjacency();
        g.finish_build();
        g
    }

    #[test]
    fn save_load_roundtrip() {
        let g = tiny();
        assert_eq!(g.max_car_kmh, 90);
        let dir = std::env::temp_dir().join(format!("mo-graph-test-{}", std::process::id()));
        let f = dir.join("g.bin");
        save(&g, &f).unwrap();
        let h = load(&f).unwrap();
        assert_eq!(h.vlon, g.vlon);
        assert_eq!(h.e_from, g.e_from);
        assert_eq!(h.e_to, g.e_to);
        assert_eq!(h.geom, g.geom);
        assert_eq!(h.adj, g.adj);
        assert_eq!(h.restr_off, g.restr_off);
        assert_eq!(h.grid.items, g.grid.items);
        assert_eq!(h.max_car_kmh, 90);
        let named: Vec<&str> = (0..h.num_edges() as u32).filter_map(|e| h.name(e)).collect();
        assert_eq!(named, vec!["הרצל"]);
        // the restriction survived the spatial renumbering: from the a-b edge into b-c at b
        let r = h.restrictions[0];
        let (fe, te) = (r.from_edge as usize, r.to_edge as usize);
        let ends = |e: usize| {
            let mut v = [h.vertex(h.e_from[e]), h.vertex(h.e_to[e])];
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v
        };
        assert!(h.restrictions_at(r.via).len() == 1);
        let via = h.vertex(r.via);
        assert!((via[0] - 34.79).abs() < 1e-6 && (via[1] - 32.08).abs() < 1e-6);
        assert!(ends(fe).iter().any(|p| (p[0] - 34.78).abs() < 1e-6 && (p[1] - 32.08).abs() < 1e-6));
        assert!(ends(te).iter().any(|p| (p[0] - 34.79).abs() < 1e-6 && (p[1] - 32.09).abs() < 1e-6));

        // truncated file is rejected, old version is rejected with a clear message
        let bytes = std::fs::read(&f).unwrap();
        std::fs::write(&f, &bytes[..bytes.len() - 10]).unwrap();
        assert!(load(&f).is_err());
        let mut old = bytes.clone();
        old[4..8].copy_from_slice(&1u32.to_le_bytes());
        std::fs::write(&f, &old).unwrap();
        let e = load(&f).err().unwrap().to_string();
        assert!(e.contains("version 1"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grid_snap() {
        let g = tiny();
        let any = |_e: u32| true;
        // just north of the a-b edge
        let s = g.snap([34.785, 32.0802], &any).unwrap();
        let p = g.edge_points(s.edge);
        assert!(p.iter().all(|q| (q[1] - 32.08).abs() < 1e-6), "snapped to {:?}", p);
        assert!(s.dist_m < 40.0 && (s.frac - 0.5).abs() < 0.05, "{s:?}");
        // next to the bend of the diagonal
        let s = g.snap([34.7881, 32.0821], &any).unwrap();
        assert_eq!(g.edge_points(s.edge).len(), 3);
        assert!(s.dist_m < 20.0);
        // far away: nothing within 8 km
        assert!(g.snap([35.5, 33.0], &any).is_none());
    }

    /// Graph from points (lon, lat) and undirected residential edges, bearings from coordinates.
    fn mk(pts: &[(f64, f64)], edges: &[(u32, u32)]) -> Graph {
        let mut g = Graph::default();
        for &(lon, lat) in pts {
            g.vlon.push((lon * 1e7) as i32);
            g.vlat.push((lat * 1e7) as i32);
        }
        g.bbox = [34.0, 31.0, 36.0, 33.0];
        g.e_geom_off.push(0);
        g.name_off.push(0);
        for &(a, b) in edges {
            let (pa, pb) = (g.vertex(a), g.vertex(b));
            let bs = bearing_deg(pa, pb).round() as u16 % 360;
            g.e_from.push(a);
            g.e_to.push(b);
            g.e_len.push(haversine_m(pa, pb) as f32);
            g.e_speed.push(50);
            g.e_flags.push(flags::CAR_FWD | flags::CAR_BWD | flags::FOOT | flags::BIKE_FWD | flags::BIKE_BWD);
            g.e_class.push(Class::Residential as u8);
            g.e_name.push(NO_NAME);
            g.e_bear_start.push(bs);
            g.e_bear_end.push(bs);
            g.e_geom_off.push(0);
        }
        g.build_adjacency();
        g
    }

    /// Edge ids of a path between the (pre-renumbering) vertices `a` and `b` found by A*, as
    /// pairs of vertex coordinates so the test does not depend on edge numbering.
    fn path_edges(g: &Graph, mode: crate::types::Mode, from_e: u32, to_e: u32) -> Option<Vec<u32>> {
        use crate::routing::astar::{search, SearchParams};
        use crate::routing::profile::Profile;
        let prof = Profile::new(mode, Default::default());
        let from = Snap { edge: from_e, frac: 0.0, point: g.vertex(g.e_from[from_e as usize]), dist_m: 0.0 };
        let to = Snap { edge: to_e, frac: 1.0, point: g.vertex(g.e_to[to_e as usize]), dist_m: 0.0 };
        search(g, &SearchParams::new(&prof), &from, &to).map(|p| p.states.iter().map(|s| s >> 1).collect())
    }

    /// Find the edge between two original points after `finish_build` renumbered everything.
    fn edge_between(g: &Graph, a: (f64, f64), b: (f64, f64)) -> u32 {
        let near = |v: u32, p: (f64, f64)| {
            let q = g.vertex(v);
            (q[0] - p.0).abs() < 1e-6 && (q[1] - p.1).abs() < 1e-6
        };
        (0..g.num_edges() as u32)
            .find(|&e| {
                let (f, t) = (g.e_from[e as usize], g.e_to[e as usize]);
                (near(f, a) && near(t, b)) || (near(f, b) && near(t, a))
            })
            .expect("edge")
    }

    #[test]
    fn bicycle_only_restriction_does_not_bind_cars() {
        use crate::types::Mode;
        // A - B - C(north of B), B - D (east): no left turn A→B→C for bicycles only
        let p = [(34.80, 32.05), (34.801, 32.05), (34.801, 32.051), (34.802, 32.05)];
        let mut g = mk(&p, &[(0, 1), (1, 2), (1, 3)]);
        g.restrictions.push(Restriction { via: 1, from_edge: 0, to_edge: 1, only: false, mask: rmask::BIKE });
        g.finish_build();
        let (ab, bc, bd) = (edge_between(&g, p[0], p[1]), edge_between(&g, p[1], p[2]), edge_between(&g, p[1], p[3]));
        let car = path_edges(&g, Mode::Car, ab, bc).unwrap();
        assert_eq!(car, vec![ab, bc], "car takes the direct turn");
        let bike = path_edges(&g, Mode::Bike, ab, bc).unwrap();
        assert!(bike.contains(&bd), "bike must detour (U-turn at D): {bike:?}");
    }

    #[test]
    fn several_only_restrictions_from_one_edge() {
        use crate::routing::profile::Profile;
        use crate::types::Mode;
        // from A–B at B: only to C or only to D (two lane groups); E is forbidden
        let p = [(34.80, 32.05), (34.801, 32.05), (34.801, 32.051), (34.802, 32.05), (34.801, 32.049)];
        let mut g = mk(&p, &[(0, 1), (1, 2), (1, 3), (1, 4)]);
        g.restrictions.push(Restriction { via: 1, from_edge: 0, to_edge: 1, only: true, mask: rmask::ALL });
        g.restrictions.push(Restriction { via: 1, from_edge: 0, to_edge: 2, only: true, mask: rmask::ALL });
        g.finish_build();
        let (ab, bc, bd, be) = (edge_between(&g, p[0], p[1]), edge_between(&g, p[1], p[2]), edge_between(&g, p[1], p[3]), edge_between(&g, p[1], p[4]));
        let prof = Profile::new(Mode::Car, Default::default());
        let b = g.restrictions[0].via;
        let into_b = g.state_from(g.e_from[ab as usize], ab);
        assert_eq!(g.head(into_b), b);
        assert!(prof.turn_cost(&g, into_b, g.state_from(b, bc)).is_some());
        assert!(prof.turn_cost(&g, into_b, g.state_from(b, bd)).is_some());
        assert!(prof.turn_cost(&g, into_b, g.state_from(b, be)).is_none());
    }

    #[test]
    fn via_way_restriction() {
        use crate::types::Mode;
        // A - B - C - D along a line, C - E north; detour A - F - E. "No A-B, B-C, C-E" (via way B-C).
        let p = [(34.80, 32.05), (34.801, 32.05), (34.802, 32.05), (34.803, 32.05), (34.802, 32.051), (34.80, 32.052)];
        let edges = [(0, 1), (1, 2), (2, 4), (2, 3), (0, 5), (5, 4)];
        let build = |only: bool, to: usize| {
            let mut g = mk(&p, &edges);
            g.multi.push(MultiRestriction { edges: vec![0, 1, to as u32], only, mask: rmask::CAR | rmask::TRUCK });
            g.finish_build();
            g
        };
        // without the restriction the straight way is used
        let mut free = mk(&p, &edges);
        free.finish_build();
        let (ab, ce) = (edge_between(&free, p[0], p[1]), edge_between(&free, p[2], p[4]));
        let r = path_edges(&free, Mode::Car, ab, ce).unwrap();
        assert!(r.contains(&edge_between(&free, p[1], p[2])), "{r:?}");

        // no_*: B-C then C-E is forbidden -> the detour through F
        let g = build(false, 2);
        let (ab, bc, ce, af) = (edge_between(&g, p[0], p[1]), edge_between(&g, p[1], p[2]), edge_between(&g, p[2], p[4]), edge_between(&g, p[0], p[5]));
        assert_eq!(g.multi.len(), 1);
        let r = path_edges(&g, Mode::Car, ab, ce).unwrap();
        assert!(!r.windows(2).any(|w| w == [bc, ce]), "{r:?}");
        assert!(r.contains(&af), "{r:?}");
        // the restriction is for cars and trucks only: walking may go straight
        let r = path_edges(&g, Mode::Foot, ab, ce).unwrap();
        assert!(r.contains(&bc), "{r:?}");

        // only_*: after A-B, B-C one must continue to D, so E is reached through F as well
        let g = build(true, 3);
        let (ab, bc, ce) = (edge_between(&g, p[0], p[1]), edge_between(&g, p[1], p[2]), edge_between(&g, p[2], p[4]));
        let r = path_edges(&g, Mode::Car, ab, ce).unwrap();
        assert!(!r.windows(2).any(|w| w == [bc, ce]), "{r:?}");
    }

    #[test]
    fn avoid_point_and_polygon() {
        use crate::types::{Avoid, Mode, RouteRequest, Waypoint};
        // A - B - C straight (short) and A - D - C around (long)
        let p = [(34.80, 32.05), (34.801, 32.05), (34.802, 32.05), (34.801, 32.052)];
        let mut g = mk(&p, &[(0, 1), (1, 2), (0, 3), (3, 2)]);
        g.name_buf = "הרצליפו".into();
        g.name_off = vec![0, "הרצל".len() as u32, "הרצליפו".len() as u32];
        g.e_name = vec![0, 0, 1, 1];
        g.finish_build();
        let wp = |i: usize| Waypoint { lon: p[i].0, lat: p[i].1, name: None };
        let req = |avoid: Avoid| RouteRequest { waypoints: vec![wp(0), wp(2)], mode: Mode::Car, avoid, alternatives: 1, ..Default::default() };
        let len = |r: &RouteRequest| g.route(r).unwrap().routes[0].distance_m;
        let direct = len(&req(Avoid::default()));
        let around = len(&req(Avoid { points: vec![[34.8015, 32.0501]], ..Default::default() }));
        assert!(direct < 200.0 && around > 300.0, "{direct} {around}");
        let poly = vec![[34.8013, 32.0495], [34.8017, 32.0495], [34.8017, 32.0505], [34.8013, 32.0505]];
        let around2 = len(&req(Avoid { polygons: vec![poly], ..Default::default() }));
        assert!(around2 > 300.0, "{around2}");
    }

    #[test]
    fn restriction_tags() {
        let mut c = 0;
        let t = |v: &[(&'static str, &'static str)], c: &mut u64| restriction_kind(v.iter().copied(), c);
        assert_eq!(t(&[("type", "restriction"), ("restriction", "no_left_turn")], &mut c), Some((false, rmask::ALL)));
        assert_eq!(t(&[("type", "restriction"), ("restriction:bicycle", "no_left_turn")], &mut c), Some((false, rmask::BIKE)));
        assert_eq!(t(&[("type", "restriction"), ("restriction:hgv", "only_straight_on")], &mut c), Some((true, rmask::TRUCK)));
        assert_eq!(t(&[("type", "restriction"), ("restriction", "no_u_turn"), ("except", "bicycle;psv")], &mut c), Some((false, rmask::CAR | rmask::TRUCK)));
        assert_eq!(t(&[("type", "restriction"), ("restriction:conditional", "no_left_turn @ (Mo-Fr 07:00-09:00)")], &mut c), None);
        assert_eq!(c, 1);
    }

    #[test]
    fn way_tags_v7() {
        let a = parse_way_tags(
            [
                ("highway", "motorway_link"),
                ("oneway", "yes"),
                ("junction:ref", "5"),
                ("destination", "ירושלים;תל אביב"),
                ("turn:lanes", "through|slight_right"),
                ("maxspeed", "IL:urban"),
                ("maxheight", "4.2"),
                ("maxweight", "7500 kg"),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(a.junction.as_deref(), Some("5"));
        assert_eq!(a.dest_fwd.as_deref(), Some("ירושלים, תל אביב"));
        assert_eq!(a.dest_bwd, None);
        assert_eq!(a.lanes_fwd.as_deref(), Some("through|slight_right"));
        assert_eq!((a.limit_fwd, a.limit_bwd), (50, 50));
        assert_eq!((a.height_dm, a.weight_ht), (42, 15));
        let a = parse_way_tags([("highway", "primary"), ("maxspeed:forward", "70"), ("maxspeed:backward", "50")].into_iter()).unwrap();
        assert_eq!((a.speed_fwd, a.speed_bwd, a.limit_fwd, a.limit_bwd), (70, 50, 70, 50));
        let a = parse_way_tags([("highway", "residential"), ("access", "destination")].into_iter()).unwrap();
        assert!(a.flags & flags::DESTINATION != 0 && a.flags & flags::CAR_FWD != 0);
        let a = parse_way_tags([("route", "ferry"), ("duration", "01:30")].into_iter()).unwrap();
        assert!(a.flags & flags::FERRY != 0 && a.flags & flags::CAR_FWD != 0);
        assert_eq!(a.ferry_secs, Some(5400));
        let a = parse_way_tags([("highway", "steps")].into_iter()).unwrap();
        assert!(a.flags & flags::WHEELCHAIR_NO != 0);
        let a = parse_way_tags([("highway", "service"), ("hgv", "no")].into_iter()).unwrap();
        assert!(a.flags & flags::HGV_NO != 0);
        assert_eq!(parse_duration("PT1H20M"), Some(4800));
        assert_eq!(parse_metres("12'6\""), Some((12.0 * 12.0 + 6.0) * 0.0254));
    }

    #[test]
    fn climb_hysteresis() {
        let (up, down) = climb(&[100.0, 100.5, 101.0, 100.4, 103.0, 110.0, 108.0, 104.0]);
        assert!((up - 10.0).abs() < 1e-4 && (down - 6.0).abs() < 1e-4, "{up} {down}");
        assert_eq!(climb(&[5.0, 5.4, 4.8, 5.2]), (0.0, 0.0));
    }

    #[test]
    fn v7_fields_roundtrip() {
        let mut g = tiny();
        let ne = g.num_edges();
        g.e_speed_bwd = vec![30; ne];
        g.e_limit_fwd = vec![50; ne];
        g.e_limit_bwd = vec![0; ne];
        g.e_rise = vec![12; ne];
        g.e_fall = vec![3; ne];
        g.name_buf.push_str("443");
        g.name_off.push(g.name_buf.len() as u32);
        let n443 = (g.name_off.len() - 2) as u32;
        g.ext = EdgeExt { edge: vec![2], road_ref: vec![n443], junction: vec![NO_NAME], dest_fwd: vec![0], dest_bwd: vec![NO_NAME], lanes_fwd: vec![NO_NAME], lanes_bwd: vec![NO_NAME] };
        g.limits = Limits { edge: vec![1], height_dm: vec![40], weight_ht: vec![0], width_dm: vec![0] };
        g.cameras = vec![347_850_000, 320_850_000];
        g.multi = vec![MultiRestriction { edges: vec![0, 1, 2], only: true, mask: rmask::CAR }];
        g.index_multi();
        let dir = std::env::temp_dir().join(format!("mo-graph-v7-{}", std::process::id()));
        let f = dir.join("g.bin");
        save(&g, &f).unwrap();
        let h = load(&f).unwrap();
        assert_eq!(h.e_speed_bwd, g.e_speed_bwd);
        assert_eq!(h.limit_s(0), Some(50));
        assert_eq!(h.limit_s(1), None);
        assert_eq!(h.climb_s(0), (1.2, 0.3));
        assert_eq!(h.climb_s(1), (0.3, 1.2));
        assert_eq!(h.road_ref(2), Some("443"));
        assert_eq!(h.destination_s(4), Some("הרצל"));
        assert_eq!(h.limits_of(1), Some((4.0, 0.0, 0.0)));
        assert_eq!(h.camera_points().count(), 1);
        assert_eq!(h.multi, g.multi);
        assert_eq!(h.multi_by_last.get(&1).map(|v| v.len()), Some(1));
        assert_eq!(h.restrictions[0].mask, rmask::ALL);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn isochrone_bands_grow() {
        use crate::types::Mode;
        let mut rng = Rng(0xDEAD_BEEF_1234_5678);
        let g = random_graph(&mut rng, 14);
        let o = g.vertex((g.num_vertices() / 2) as u32);
        let bands = crate::routing::iso::isochrone(&g, o, Mode::Foot, &[2.0, 4.0, 1.0]).unwrap();
        assert_eq!(bands.iter().map(|b| b.minutes).collect::<Vec<_>>(), vec![1.0, 2.0, 4.0]);
        let area = |rings: &[Vec<LonLat>]| -> f64 {
            rings.iter().map(|r| r.windows(2).map(|w| w[0][0] * w[1][1] - w[1][0] * w[0][1]).sum::<f64>() / 2.0).sum()
        };
        let a: Vec<f64> = bands.iter().map(|b| area(&b.rings)).collect();
        assert!(a[0] > 0.0 && a[0] <= a[1] && a[1] <= a[2], "{a:?}");
        // walking 4 minutes covers far less than the whole grid (~2 km across)
        assert!(a[2] < 0.02 * 0.02, "{a:?}");
    }

    #[test]
    fn walk_times_match_single_searches() {
        let mut rng = Rng(0x1357_9BDF_2468_ACE0);
        let g = random_graph(&mut rng, 10);
        let o = g.vertex(0);
        let targets: Vec<LonLat> = (1..12).map(|i| g.vertex((i * 7) as u32 % g.num_vertices() as u32)).collect();
        let many = crate::routing::walk_times_from(&g, o, &targets, 3600.0);
        for (t, m) in targets.iter().zip(&many) {
            let single = crate::routing::astar::walk_leg(&g, o, *t).map(|l| l.duration_sec as f32);
            match (single, m) {
                // the one-to-many search minimises real time, walk_leg the preference cost
                (Some(s), Some(m)) => assert!(*m <= s + 1.0 && s <= m * 1.3 + 30.0, "single {s} many {m}"),
                (None, None) => {}
                (s, m) => panic!("single {s:?} many {m:?}"),
            }
        }
    }

    #[test]
    fn hilbert_is_bijective_on_small_grid() {
        let mut seen = std::collections::HashSet::new();
        for x in (0..65536u32).step_by(4099) {
            for y in (0..65536u32).step_by(4099) {
                assert!(seen.insert(hilbert_d(x, y)));
            }
        }
        assert_eq!(hilbert_d(0, 0), 0);
    }

    #[test]
    fn projection_roundtrip() {
        let p = proj(34.78, 32.08);
        let ll = unproj(p[0], p[1]);
        assert!((ll[0] - 34.78).abs() < 1e-5 && (ll[1] - 32.08).abs() < 1e-5);
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn geom_varint_roundtrip() {
        let g = vec![347_812_345, 320_853_210, 347_812_399, 320_853_100, -1, i32::MAX, i32::MIN, 0, 5, -5];
        let enc = encode_geom(&g);
        assert_eq!(decode_geom(&enc, g.len()).unwrap(), g);
        assert!(decode_geom(&enc[..enc.len() - 1], g.len()).is_err());
    }

    /// The installed graph.bin (v5 or v6) saved as v6 and read back must be identical, field
    /// by field. cargo test --release -- --ignored v6_roundtrip_of_installed_graph --nocapture
    #[test]
    #[ignore]
    fn v6_roundtrip_of_installed_graph() {
        let src = crate::paths::DataPaths::new(crate::paths::DataPaths::default_root()).graph_file();
        let t0 = std::time::Instant::now();
        let a = load(&src).expect("installed graph.bin");
        let t_old = t0.elapsed();
        let out = std::env::temp_dir().join("mo-graph-v6-test.bin");
        save(&a, &out).unwrap();
        let t1 = std::time::Instant::now();
        let b = load(&out).unwrap();
        let t_new = t1.elapsed();
        let (s_old, s_new) = (std::fs::metadata(&src).unwrap().len(), std::fs::metadata(&out).unwrap().len());
        eprintln!("size {:.1} MB -> {:.1} MB; load {:?} -> {:?}", s_old as f64 / 1e6, s_new as f64 / 1e6, t_old, t_new);
        assert_eq!(a.vlon, b.vlon);
        assert_eq!(a.vlat, b.vlat);
        assert_eq!(a.vpx, b.vpx);
        assert_eq!(a.vpy, b.vpy);
        assert_eq!(a.geom, b.geom);
        assert_eq!(a.e_geom_off, b.e_geom_off);
        assert_eq!(a.restr_off, b.restr_off);
        assert_eq!(a.adj, b.adj);
        assert_eq!(a.e_flags, b.e_flags);
        assert_eq!(a.restrictions.len(), b.restrictions.len());
        assert_eq!(a.grid.items, b.grid.items);
        let _ = std::fs::remove_file(&out);
    }
}
