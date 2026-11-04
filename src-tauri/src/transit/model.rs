//! Compact GTFS model used by RAPTOR.
//!
//! Built once from the Ministry of Transport GTFS zip (streamed – `stop_times.txt` is ~1.5 GB of
//! text and is never held in memory as a string) and written as raw little-endian arrays behind a
//! `b"MOTR" + u32 VERSION` header and an `END!` trailer (`binfmt`); the indexes the planner needs
//! (stop modes, the stop grid, search keys) are computed at build time and stored too, so loading
//! is a handful of bulk reads. [`Transit::validate`] checks every index before the model is used:
//! a damaged file is an error (`TRANSIT_DATA_CORRUPT`), never a panic.
//!
//! Layout (all indices are dense `u32`):
//! * `stops`          – name / code / city (interned strings) + coordinates.
//! * `routes`         – GTFS route info (agency, names, type, colour).
//! * `patterns`       – unique stop sequences of a route ("route patterns"), split further so
//!   that the trips of one pattern never overtake each other (FIFO), which
//!   RAPTOR's binary search relies on.
//! * per pattern, trips are sorted by departure; times are stored trip-major in `arr`/`dep`
//!   (`times_start + trip * n + pos`) as seconds since service-day midnight (may exceed 24h).
//! * `service_bits`   – one bitmap per service_id over the feed's date range.
//! * foot transfers   – CSR, stops within 400 m straight-line (and siblings of one parent
//!   station), ≤ 30 per stop; duration = the walk on the road network when routing data is
//!   present at build time, else dist / 1.2 m/s + 60 s; `transfers.txt` overrides.
//! * optional per-stop / per-trip data (empty when the feed has none): wheelchair flags,
//!   minimum change time at a stop (`transfers.txt` from = to), parent station.
//! * shapes           – Douglas-Peucker simplified (≈4 m), 1e-6° fixed point; every pattern
//!   position is mapped to a shape segment for leg geometry.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{Datelike, NaiveDate};
use csv::ByteRecord;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use super::binfmt::{put, put_strings, Reader};
use crate::geo::haversine_m;
use crate::types::{GeoKind, GeoPrecision, GeoResult, LonLat, ProgressFn, ProgressPhase};

pub const MAGIC: &[u8; 4] = b"MOTR";
/// 2: `dep` is empty when every departure equals the arrival (the MoT feed has no dwell times;
/// that was 47 MB of duplicates).
/// 3: raw arrays instead of bincode, stored indexes, END trailer, wheelchair / transfers.txt /
/// frequencies.txt / parent stations, agency time zones.
pub const VERSION: u32 = 3;
const END_MAGIC: &[u8; 4] = b"END!";
pub const NONE: u32 = u32::MAX;
/// pattern_flags bits
pub const NO_PICKUP: u8 = 1;
pub const NO_DROPOFF: u8 = 2;

/// stop_modes bits
pub const MODE_BUS: u8 = 1;
pub const MODE_RAIL: u8 = 2;
pub const MODE_LIGHT_RAIL: u8 = 4;
pub const MODE_SUBWAY: u8 = 8;
pub const MODE_OTHER: u8 = 16;

pub const TRANSFER_RADIUS_M: f64 = 400.0;
pub const TRANSFER_MAX_PER_STOP: usize = 30;
pub const TRANSFER_SPEED_MPS: f64 = 1.2;
pub const TRANSFER_PENALTY_S: f64 = 60.0;
/// Stops of one parent station are transfer candidates up to this far apart.
pub const SIBLING_RADIUS_M: f64 = 1000.0;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Stop {
    pub name: u32,
    pub code: u32,
    pub city: u32,
    pub lon: f64,
    pub lat: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RouteInfo {
    pub agency: u32,
    pub short_name: u32,
    pub long_name: u32,
    pub route_type: u16,
    /// 0xRRGGBB
    pub color: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Pattern {
    pub route: u32,
    pub stops_start: u32,
    pub n: u32,
    /// Global index of the first trip (into trip_service / trip_headsign).
    pub trips_start: u32,
    pub ntrips: u32,
    /// Index into arr/dep of (trip 0, pos 0).
    pub times_start: u32,
    /// Shape index or NONE.
    pub shape: u32,
}

#[derive(Default)]
pub struct Transit {
    /// `NaiveDate::num_days_from_ce()` of day 0 of the service bitmaps.
    pub feed_start: i32,
    pub num_days: u32,
    pub strings: Vec<String>,
    pub stops: Vec<Stop>,
    pub agencies: Vec<u32>,
    pub routes: Vec<RouteInfo>,
    pub patterns: Vec<Pattern>,
    pub pattern_stops: Vec<u32>,
    pub pattern_flags: Vec<u8>,
    /// Per pattern position: shape segment index (relative to the shape) or NONE.
    pub pattern_shape_idx: Vec<u32>,
    pub trip_service: Vec<u32>,
    pub trip_headsign: Vec<u32>,
    pub arr: Vec<u32>,
    /// Departure times, parallel to `arr`; empty when they all equal the arrivals.
    pub dep: Vec<u32>,
    pub num_services: u32,
    pub service_words: u32,
    pub service_bits: Vec<u64>,
    pub stop_pat_start: Vec<u32>,
    pub stop_pat_pattern: Vec<u32>,
    pub stop_pat_pos: Vec<u32>,
    pub tr_start: Vec<u32>,
    pub tr_to: Vec<u32>,
    pub tr_dur: Vec<u32>,
    pub shape_start: Vec<u32>,
    pub shape_pts: Vec<[i32; 2]>,
    // ----- optional GTFS data (empty vectors when the feed has none)
    /// Interned parent_station id per stop, NONE without one.
    pub stop_parent: Vec<u32>,
    /// GTFS wheelchair_boarding per stop: 0 unknown, 1 accessible, 2 not accessible.
    pub stop_wheelchair: Vec<u8>,
    /// GTFS wheelchair_accessible per (global) trip: 0 unknown, 1 yes, 2 no.
    pub trip_wheelchair: Vec<u8>,
    /// Minimum change time at a stop in seconds (transfers.txt with from = to); 0 = default.
    pub stop_min_change: Vec<u16>,
    /// Interned agency_timezone per agency.
    pub agency_tz: Vec<u32>,
    // ----- indexes computed at build time (`finalize`), stored in the file
    pub stop_modes: Vec<u8>,
    /// Stop grid (GRID_DEG cells) as CSR over sorted cell keys.
    pub(crate) grid_keys: Vec<u64>,
    pub(crate) grid_start: Vec<u32>,
    pub(crate) grid_stops: Vec<u32>,
    pub(crate) search_keys: Vec<String>,
}

const GRID_DEG: f64 = 0.01;

#[inline]
fn grid_code(x: i32, y: i32) -> u64 {
    ((x as u32 as u64) << 32) | y as u32 as u64
}

impl Transit {
    #[inline]
    pub fn s(&self, i: u32) -> &str {
        if i == NONE {
            ""
        } else {
            &self.strings[i as usize]
        }
    }
    #[inline]
    pub fn lonlat(&self, stop: u32) -> LonLat {
        let s = &self.stops[stop as usize];
        [s.lon, s.lat]
    }
    /// GTFS stop_code (the number on the sign), if the feed has one.
    pub fn stop_code(&self, stop: u32) -> Option<String> {
        self.strings.get(self.stops[stop as usize].code as usize).filter(|c| !c.is_empty()).cloned()
    }
    pub fn stop_name(&self, stop: u32) -> &str {
        self.s(self.stops[stop as usize].name)
    }
    #[inline]
    pub fn pstops(&self, p: u32) -> &[u32] {
        let pt = &self.patterns[p as usize];
        &self.pattern_stops[pt.stops_start as usize..(pt.stops_start + pt.n) as usize]
    }
    #[inline]
    pub fn pflags(&self, p: u32) -> &[u8] {
        let pt = &self.patterns[p as usize];
        &self.pattern_flags[pt.stops_start as usize..(pt.stops_start + pt.n) as usize]
    }
    #[inline]
    pub fn tidx(&self, p: u32, trip: u32, pos: u32) -> usize {
        let pt = &self.patterns[p as usize];
        (pt.times_start + trip * pt.n + pos) as usize
    }
    #[inline]
    pub fn dep_at(&self, p: u32, trip: u32, pos: u32) -> i32 {
        let i = self.tidx(p, trip, pos);
        if self.dep.is_empty() { self.arr[i] as i32 } else { self.dep[i] as i32 }
    }
    #[inline]
    pub fn arr_at(&self, p: u32, trip: u32, pos: u32) -> i32 {
        self.arr[self.tidx(p, trip, pos)] as i32
    }
    pub fn stop_patterns(&self, s: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        let a = self.stop_pat_start[s as usize] as usize;
        let b = self.stop_pat_start[s as usize + 1] as usize;
        (a..b).map(move |i| (self.stop_pat_pattern[i], self.stop_pat_pos[i]))
    }
    pub fn transfers(&self, s: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        let a = self.tr_start[s as usize] as usize;
        let b = self.tr_start[s as usize + 1] as usize;
        (a..b).map(move |i| (self.tr_to[i], self.tr_dur[i]))
    }
    pub fn stop_is_served(&self, s: u32) -> bool {
        self.stop_pat_start[s as usize + 1] > self.stop_pat_start[s as usize]
    }
    pub fn date_of_day(&self, day: u32) -> NaiveDate {
        NaiveDate::from_num_days_from_ce_opt(self.feed_start + day as i32).unwrap()
    }
    pub fn day_index(&self, date: NaiveDate) -> Option<u32> {
        let d = date.num_days_from_ce() - self.feed_start;
        if d >= 0 && (d as u32) < self.num_days {
            Some(d as u32)
        } else {
            None
        }
    }
    #[inline]
    pub fn service_active(&self, svc: u32, day: u32) -> bool {
        let w = self.service_bits[(svc * self.service_words + day / 64) as usize];
        (w >> (day % 64)) & 1 == 1
    }
    /// Bitset over services active on `day`.
    pub fn active_services(&self, day: u32) -> Vec<u64> {
        let mut v = vec![0u64; (self.num_services as usize).div_ceil(64).max(1)];
        for s in 0..self.num_services {
            if self.service_active(s, day) {
                v[(s / 64) as usize] |= 1 << (s % 64);
            }
        }
        v
    }

    pub fn vehicle_of_route(&self, r: u32) -> crate::types::TransitVehicle {
        use crate::types::TransitVehicle as V;
        match self.routes[r as usize].route_type {
            0 | 900..=999 => V::LightRail,
            1 | 400..=499 => V::Subway,
            2 | 100..=199 => V::Rail,
            3 | 700..=799 | 200..=299 => V::Bus,
            4 | 1000..=1099 | 1200 => V::Ferry,
            _ => V::Other,
        }
    }

    /// Longest pattern of the line `short_name` run by `agency`; a pattern whose trips carry
    /// `headsign` wins over a longer one in the other direction.
    pub fn line_shape(&self, short_name: &str, agency: &str, headsign: Option<&str>) -> Option<crate::types::LineShape> {
        self.line_shape_of(short_name, agency, headsign, None)
    }

    /// [`line_shape`](Self::line_shape) restricted to one line among the same-numbered lines of
    /// the agency ("Egged 1" runs in Gedera, Eilat, Jerusalem, Karmiel...): `long_name` is any
    /// route_long_name of that line, see [`line_group`].
    pub fn line_shape_of(&self, short_name: &str, agency: &str, headsign: Option<&str>, long_name: Option<&str>) -> Option<crate::types::LineShape> {
        let group = long_name.filter(|l| !l.is_empty()).map(line_group);
        let mut best: Option<(u32, bool, u32)> = None; // (pattern, headsign match, stops)
        for (pi, pt) in self.patterns.iter().enumerate() {
            let r = &self.routes[pt.route as usize];
            if self.s(r.short_name) != short_name {
                continue;
            }
            if group.as_ref().is_some_and(|g| line_group(self.s(r.long_name)) != *g) {
                continue;
            }
            let ag = if r.agency == NONE { "" } else { self.s(self.agencies[r.agency as usize]) };
            if ag != agency {
                continue;
            }
            let hs = headsign.is_none_or(|h| (0..pt.ntrips.min(8)).any(|i| self.s(self.trip_headsign[(pt.trips_start + i) as usize]) == h));
            if best.is_none_or(|(_, m, n)| (hs, pt.n) > (m, n)) {
                best = Some((pi as u32, hs, pt.n));
            }
        }
        let (pi, _, n) = best?;
        let pt = &self.patterns[pi as usize];
        let r = &self.routes[pt.route as usize];
        let stops = self
            .pstops(pi)
            .iter()
            .map(|&s| {
                let ll = self.lonlat(s);
                crate::types::StopInfo { id: s, name: self.stop_name(s).to_string(), code: self.stop_code(s), lon: ll[0], lat: ll[1] }
            })
            .collect();
        let hs = if pt.ntrips > 0 { self.s(self.trip_headsign[pt.trips_start as usize]).to_string() } else { String::new() };
        Some(crate::types::LineShape {
            short_name: short_name.to_string(),
            long_name: self.s(r.long_name).to_string(),
            agency: agency.to_string(),
            headsign: hs,
            color: r.color.map(|c| format!("#{c:06x}")),
            stops,
            geometry: self.leg_geometry(pi, 0, n.saturating_sub(1)),
        })
    }

    /// Online journeys carry stop names and positions but no stop codes: take the code of the
    /// nearest local stop (within 40 m) with the same name, else the nearest one within 15 m.
    pub fn fill_stop_codes(&self, routes: &mut [crate::types::Route]) {
        for r in routes.iter_mut() {
            for leg in r.legs.iter_mut() {
                let Some(tr) = leg.transit.as_mut() else { continue };
                for st in tr.stops.iter_mut().filter(|s| s.code.is_none()) {
                    let near = self.stops_near([st.lon, st.lat], 40.0);
                    let pick = near
                        .iter()
                        .find(|(id, _)| self.stop_name(*id) == st.name)
                        .or_else(|| near.first().filter(|(_, d)| *d <= 15.0))
                        .map(|(id, _)| *id);
                    if let Some(id) = pick {
                        st.code = self.stop_code(id);
                    }
                }
            }
        }
    }

    /// Served stops within `radius_m` of `p`, sorted by distance.
    pub fn stops_near(&self, p: LonLat, radius_m: f64) -> Vec<(u32, f64)> {
        let mut out = Vec::new();
        let dlat = radius_m / 111_000.0;
        let dlon = radius_m / (111_000.0 * p[1].to_radians().cos().max(0.1));
        let (x0, y0) = grid_key([p[0] - dlon, p[1] - dlat]);
        let (x1, y1) = grid_key([p[0] + dlon, p[1] + dlat]);
        for x in x0..=x1 {
            for y in y0..=y1 {
                for &s in self.grid_cell(x, y) {
                    let d = haversine_m(p, self.lonlat(s));
                    if d <= radius_m {
                        out.push((s, d));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.1.total_cmp(&b.1));
        out
    }

    /// Served stops of one grid cell.
    fn grid_cell(&self, x: i32, y: i32) -> &[u32] {
        match self.grid_keys.binary_search(&grid_code(x, y)) {
            Ok(i) => &self.grid_stops[self.grid_start[i] as usize..self.grid_start[i + 1] as usize],
            Err(_) => &[],
        }
    }

    /// Served stops inside `bbox` = [min_lon, min_lat, max_lon, max_lat], up to `limit`.
    /// Walks the grid when the box covers fewer cells than the grid has, else scans every stop.
    pub fn stops_in_bbox(&self, bbox: [f64; 4], limit: usize) -> Vec<u32> {
        let inside = |s: u32| {
            let st = &self.stops[s as usize];
            st.lon >= bbox[0] && st.lon <= bbox[2] && st.lat >= bbox[1] && st.lat <= bbox[3]
        };
        let (x0, y0) = grid_key([bbox[0], bbox[1]]);
        let (x1, y1) = grid_key([bbox[2], bbox[3]]);
        let cells = (x1 as i64 - x0 as i64 + 1).max(0) * (y1 as i64 - y0 as i64 + 1).max(0);
        let mut out = Vec::new();
        if cells <= self.grid_keys.len() as i64 {
            'outer: for x in x0..=x1 {
                for y in y0..=y1 {
                    for &s in self.grid_cell(x, y) {
                        if inside(s) {
                            out.push(s);
                            if out.len() >= limit {
                                break 'outer;
                            }
                        }
                    }
                }
            }
        } else {
            for s in 0..self.stops.len() as u32 {
                if self.stop_is_served(s) && inside(s) {
                    out.push(s);
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
        out
    }

    /// Minimum change time (seconds) when changing vehicles at stop `s`.
    #[inline]
    pub fn min_change(&self, s: u32) -> i32 {
        match self.stop_min_change.get(s as usize) {
            Some(&v) if v > 0 => v as i32,
            _ => super::raptor::TRANSFER_SLACK,
        }
    }

    /// A wheelchair user can board / alight at `s` (unknown counts as yes).
    #[inline]
    pub fn stop_accessible(&self, s: u32) -> bool {
        self.stop_wheelchair.get(s as usize).is_none_or(|&w| w != 2)
    }

    /// Global trip `g` is wheelchair accessible. When the feed has trip data only trips marked 1
    /// count; a feed without it imposes no restriction.
    #[inline]
    pub fn trip_accessible(&self, g: u32) -> bool {
        self.trip_wheelchair.is_empty() || self.trip_wheelchair.get(g as usize) == Some(&1)
    }

    /// Time zone of the feed (first agency's agency_timezone), "" when not given.
    pub fn timezone(&self) -> &str {
        self.agency_tz.first().map(|&z| self.s(z)).unwrap_or("")
    }

    /// Geometry of riding pattern `p` from `from` to `to` (positions), along the shape if any.
    pub fn leg_geometry(&self, p: u32, from: u32, to: u32) -> Vec<LonLat> {
        let pt = &self.patterns[p as usize];
        let stops = self.pstops(p);
        let a = self.lonlat(stops[from as usize]);
        let b = self.lonlat(stops[to as usize]);
        if pt.shape != NONE {
            let base = pt.stops_start as usize;
            let sa = self.pattern_shape_idx[base + from as usize];
            let sb = self.pattern_shape_idx[base + to as usize];
            if sa != NONE && sb != NONE && sa <= sb {
                let s0 = self.shape_start[pt.shape as usize] as usize;
                let mut g = Vec::with_capacity((sb - sa) as usize + 2);
                g.push(a);
                for i in (sa + 1)..=sb {
                    let q = self.shape_pts[s0 + i as usize];
                    g.push([q[0] as f64 * 1e-6, q[1] as f64 * 1e-6]);
                }
                g.push(b);
                return g;
            }
        }
        (from..=to).map(|i| self.lonlat(stops[i as usize])).collect()
    }

    /// Approximate heap usage in bytes (diagnostics).
    pub fn heap_bytes(&self) -> usize {
        use std::mem::size_of;
        fn v<T>(x: &[T]) -> usize {
            std::mem::size_of_val(x)
        }
        self.strings.iter().map(|s| s.capacity() + size_of::<String>()).sum::<usize>()
            + v(&self.stops)
            + v(&self.agencies)
            + v(&self.routes)
            + v(&self.patterns)
            + v(&self.pattern_stops)
            + v(&self.pattern_flags)
            + v(&self.pattern_shape_idx)
            + v(&self.trip_service)
            + v(&self.trip_headsign)
            + v(&self.arr)
            + v(&self.dep)
            + v(&self.service_bits)
            + v(&self.stop_pat_start)
            + v(&self.stop_pat_pattern)
            + v(&self.stop_pat_pos)
            + v(&self.tr_start)
            + v(&self.tr_to)
            + v(&self.tr_dur)
            + v(&self.shape_start)
            + v(&self.shape_pts)
            + v(&self.stop_modes)
            + v(&self.stop_parent)
            + v(&self.stop_wheelchair)
            + v(&self.trip_wheelchair)
            + v(&self.stop_min_change)
            + v(&self.grid_keys)
            + v(&self.grid_start)
            + v(&self.grid_stops)
            + self.search_keys.iter().map(|s| s.capacity() + size_of::<String>()).sum::<usize>()
    }

    /// Build derived (non-serialized) indexes.
    pub fn finalize(&mut self) {
        let mut modes = vec![0u8; self.stops.len()];
        for (pi, p) in self.patterns.iter().enumerate() {
            let m = match self.vehicle_of_route(p.route) {
                crate::types::TransitVehicle::Bus => MODE_BUS,
                crate::types::TransitVehicle::Rail => MODE_RAIL,
                crate::types::TransitVehicle::LightRail | crate::types::TransitVehicle::Tram => MODE_LIGHT_RAIL,
                crate::types::TransitVehicle::Subway => MODE_SUBWAY,
                _ => MODE_OTHER,
            };
            for &s in self.pstops(pi as u32) {
                modes[s as usize] |= m;
            }
        }
        self.stop_modes = modes;
        let mut cells: Vec<(u64, u32)> = (0..self.stops.len() as u32)
            .filter(|&i| self.stop_is_served(i))
            .map(|i| {
                let s = &self.stops[i as usize];
                let (x, y) = grid_key([s.lon, s.lat]);
                (grid_code(x, y), i)
            })
            .collect();
        cells.sort_unstable();
        self.grid_keys.clear();
        self.grid_start.clear();
        self.grid_stops.clear();
        for (k, s) in cells {
            if self.grid_keys.last() != Some(&k) {
                self.grid_keys.push(k);
                self.grid_start.push(self.grid_stops.len() as u32);
            }
            self.grid_stops.push(s);
        }
        self.grid_start.push(self.grid_stops.len() as u32);
        self.search_keys = self.stops.iter().map(|s| search_key(self.s(s.name))).collect();
    }
}

fn grid_key(p: LonLat) -> (i32, i32) {
    ((p[0] / GRID_DEG).floor() as i32, (p[1] / GRID_DEG).floor() as i32)
}

// ====================================================================== load / save

/// Load and validate `transit.bin`. An older format is `NO_TRANSIT_DATA` (the data panel
/// offers the update); any damage is `TRANSIT_DATA_CORRUPT` – never a panic.
pub fn load(path: &Path) -> Result<Transit> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let len = f.metadata()?.len();
    let mut r = Reader::new(BufReader::with_capacity(1 << 20, f), len);
    let magic = r.magic().map_err(|_| anyhow!("TRANSIT_DATA_CORRUPT: transit.bin is too short"))?;
    if &magic != MAGIC {
        bail!("TRANSIT_DATA_CORRUPT: transit.bin has a bad header");
    }
    let ver = u32::from_le_bytes(r.magic().map_err(|_| anyhow!("TRANSIT_DATA_CORRUPT: transit.bin is too short"))?);
    if ver != VERSION {
        bail!("NO_TRANSIT_DATA: transit version {ver} is not supported by this program version (expected {VERSION}); update the public transport data");
    }
    let t = read_body(&mut r).map_err(|e| anyhow!("TRANSIT_DATA_CORRUPT: {e:#}"))?;
    if &r.magic().map_err(|_| anyhow!("TRANSIT_DATA_CORRUPT: transit.bin is truncated"))? != END_MAGIC || r.left() != 0 {
        bail!("TRANSIT_DATA_CORRUPT: transit.bin is truncated or has trailing data");
    }
    t.validate().map_err(|e| anyhow!("TRANSIT_DATA_CORRUPT: {e}"))?;
    Ok(t)
}

fn read_body<R: Read>(r: &mut Reader<R>) -> Result<Transit> {
    let mut t = Transit::default();
    let head: Vec<i64> = r.get::<u64>()?.into_iter().map(|x| x as i64).collect();
    if head.len() != 4 {
        bail!("bad header block");
    }
    t.feed_start = head[0] as i32;
    t.num_days = head[1] as u32;
    t.num_services = head[2] as u32;
    t.service_words = head[3] as u32;
    t.strings = r.strings()?;
    // stops (structure of arrays)
    let names: Vec<u32> = r.get()?;
    let codes: Vec<u32> = r.get()?;
    let cities: Vec<u32> = r.get()?;
    let lons: Vec<f64> = r.get()?;
    let lats: Vec<f64> = r.get()?;
    let n = names.len();
    if codes.len() != n || cities.len() != n || lons.len() != n || lats.len() != n {
        bail!("stop arrays differ in length");
    }
    t.stops = (0..n).map(|i| Stop { name: names[i], code: codes[i], city: cities[i], lon: lons[i], lat: lats[i] }).collect();
    t.agencies = r.get()?;
    t.agency_tz = r.get()?;
    // routes
    let ra: Vec<u32> = r.get()?;
    let rs: Vec<u32> = r.get()?;
    let rl: Vec<u32> = r.get()?;
    let rt: Vec<u16> = r.get()?;
    let rc: Vec<u32> = r.get()?;
    let n = ra.len();
    if rs.len() != n || rl.len() != n || rt.len() != n || rc.len() != n {
        bail!("route arrays differ in length");
    }
    t.routes = (0..n)
        .map(|i| RouteInfo { agency: ra[i], short_name: rs[i], long_name: rl[i], route_type: rt[i], color: (rc[i] != NONE).then_some(rc[i]) })
        .collect();
    // patterns: 7 u32 per pattern
    let pf: Vec<u32> = r.get()?;
    if !pf.len().is_multiple_of(7) {
        bail!("pattern array length");
    }
    t.patterns = pf
        .as_chunks::<7>()
        .0
        .iter()
        .map(|c| Pattern { route: c[0], stops_start: c[1], n: c[2], trips_start: c[3], ntrips: c[4], times_start: c[5], shape: c[6] })
        .collect();
    t.pattern_stops = r.get()?;
    t.pattern_flags = r.get()?;
    t.pattern_shape_idx = r.get()?;
    t.trip_service = r.get()?;
    t.trip_headsign = r.get()?;
    t.arr = r.get()?;
    t.dep = r.get()?;
    t.service_bits = r.get()?;
    t.stop_pat_start = r.get()?;
    t.stop_pat_pattern = r.get()?;
    t.stop_pat_pos = r.get()?;
    t.tr_start = r.get()?;
    t.tr_to = r.get()?;
    t.tr_dur = r.get()?;
    t.shape_start = r.get()?;
    let sp: Vec<i32> = r.get()?;
    if !sp.len().is_multiple_of(2) {
        bail!("shape point array length");
    }
    t.shape_pts = sp.as_chunks::<2>().0.iter().map(|c| [c[0], c[1]]).collect();
    t.stop_parent = r.get()?;
    t.stop_wheelchair = r.get()?;
    t.trip_wheelchair = r.get()?;
    t.stop_min_change = r.get()?;
    t.stop_modes = r.get()?;
    t.grid_keys = r.get()?;
    t.grid_start = r.get()?;
    t.grid_stops = r.get()?;
    t.search_keys = r.strings()?;
    Ok(t)
}

/// Write `transit.bin` v3 (`finalize` must have run).
pub fn save(t: &Transit, out: &Path) -> Result<()> {
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    let f = File::create(out).with_context(|| format!("create {}", out.display()))?;
    let mut w = BufWriter::with_capacity(1 << 20, f);
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    put(&mut w, &[t.feed_start as i64 as u64, t.num_days as u64, t.num_services as u64, t.service_words as u64])?;
    put_strings(&mut w, &t.strings)?;
    put(&mut w, &t.stops.iter().map(|s| s.name).collect::<Vec<_>>())?;
    put(&mut w, &t.stops.iter().map(|s| s.code).collect::<Vec<_>>())?;
    put(&mut w, &t.stops.iter().map(|s| s.city).collect::<Vec<_>>())?;
    put(&mut w, &t.stops.iter().map(|s| s.lon).collect::<Vec<_>>())?;
    put(&mut w, &t.stops.iter().map(|s| s.lat).collect::<Vec<_>>())?;
    put(&mut w, &t.agencies)?;
    put(&mut w, &t.agency_tz)?;
    put(&mut w, &t.routes.iter().map(|r| r.agency).collect::<Vec<_>>())?;
    put(&mut w, &t.routes.iter().map(|r| r.short_name).collect::<Vec<_>>())?;
    put(&mut w, &t.routes.iter().map(|r| r.long_name).collect::<Vec<_>>())?;
    put(&mut w, &t.routes.iter().map(|r| r.route_type).collect::<Vec<_>>())?;
    put(&mut w, &t.routes.iter().map(|r| r.color.unwrap_or(NONE)).collect::<Vec<_>>())?;
    let pf: Vec<u32> = t.patterns.iter().flat_map(|p| [p.route, p.stops_start, p.n, p.trips_start, p.ntrips, p.times_start, p.shape]).collect();
    put(&mut w, &pf)?;
    put(&mut w, &t.pattern_stops)?;
    put(&mut w, &t.pattern_flags)?;
    put(&mut w, &t.pattern_shape_idx)?;
    put(&mut w, &t.trip_service)?;
    put(&mut w, &t.trip_headsign)?;
    put(&mut w, &t.arr)?;
    put(&mut w, &t.dep)?;
    put(&mut w, &t.service_bits)?;
    put(&mut w, &t.stop_pat_start)?;
    put(&mut w, &t.stop_pat_pattern)?;
    put(&mut w, &t.stop_pat_pos)?;
    put(&mut w, &t.tr_start)?;
    put(&mut w, &t.tr_to)?;
    put(&mut w, &t.tr_dur)?;
    put(&mut w, &t.shape_start)?;
    put(&mut w, &t.shape_pts.iter().flat_map(|p| [p[0], p[1]]).collect::<Vec<i32>>())?;
    put(&mut w, &t.stop_parent)?;
    put(&mut w, &t.stop_wheelchair)?;
    put(&mut w, &t.trip_wheelchair)?;
    put(&mut w, &t.stop_min_change)?;
    put(&mut w, &t.stop_modes)?;
    put(&mut w, &t.grid_keys)?;
    put(&mut w, &t.grid_start)?;
    put(&mut w, &t.grid_stops)?;
    put_strings(&mut w, &t.search_keys)?;
    w.write_all(END_MAGIC)?;
    w.flush()?;
    Ok(())
}

impl Transit {
    /// Check every index the planner dereferences (CSR bounds, ranges, time table extents), so
    /// that a damaged or hand-made model is an error at load time instead of a panic later.
    pub fn validate(&self) -> std::result::Result<(), String> {
        macro_rules! ensure {
            ($c:expr, $($m:tt)*) => {
                if !$c {
                    return Err(format!($($m)*));
                }
            };
        }
        let ns = self.strings.len() as u64;
        let str_ok = |i: u32| i == NONE || (i as u64) < ns;
        let nstops = self.stops.len();
        ensure!(self.num_days >= 1 && self.num_days <= 4000, "num_days {}", self.num_days);
        ensure!(
            NaiveDate::from_num_days_from_ce_opt(self.feed_start).is_some()
                && NaiveDate::from_num_days_from_ce_opt(self.feed_start + self.num_days as i32).is_some(),
            "feed start"
        );
        ensure!(self.service_words == self.num_days.div_ceil(64), "service words");
        ensure!(self.service_bits.len() as u64 == self.num_services as u64 * self.service_words as u64, "service bitmap size");
        for (i, s) in self.stops.iter().enumerate() {
            ensure!(str_ok(s.name) && str_ok(s.code) && str_ok(s.city), "stop {i} strings");
            ensure!(s.lon.is_finite() && s.lat.is_finite(), "stop {i} position");
        }
        ensure!(self.agencies.iter().all(|&a| str_ok(a)), "agency names");
        ensure!(self.agency_tz.is_empty() || self.agency_tz.len() == self.agencies.len(), "agency time zones");
        ensure!(self.agency_tz.iter().all(|&a| str_ok(a)), "agency time zones");
        for (i, r) in self.routes.iter().enumerate() {
            ensure!(r.agency == NONE || (r.agency as usize) < self.agencies.len(), "route {i} agency");
            ensure!(str_ok(r.short_name) && str_ok(r.long_name), "route {i} names");
        }
        let nshapes = self.shape_start.len().saturating_sub(1);
        ensure!(self.shape_start.first().is_none_or(|&x| x == 0), "shape start");
        ensure!(self.shape_start.windows(2).all(|w| w[0] <= w[1]), "shape start order");
        ensure!(self.shape_start.last().is_none_or(|&x| x as usize == self.shape_pts.len()), "shape points");
        let np = self.pattern_stops.len();
        ensure!(self.pattern_flags.len() == np && self.pattern_shape_idx.len() == np, "pattern arrays");
        ensure!(self.pattern_stops.iter().all(|&s| (s as usize) < nstops), "pattern stop index");
        let ntrips = self.trip_service.len();
        ensure!(self.trip_headsign.len() == ntrips, "trip arrays");
        ensure!(self.trip_service.iter().all(|&s| s < self.num_services), "trip service index");
        ensure!(self.trip_headsign.iter().all(|&h| str_ok(h)), "trip headsign index");
        ensure!(self.trip_wheelchair.is_empty() || self.trip_wheelchair.len() == ntrips, "trip wheelchair");
        ensure!(self.dep.is_empty() || self.dep.len() == self.arr.len(), "departure times");
        for (i, p) in self.patterns.iter().enumerate() {
            ensure!((p.route as usize) < self.routes.len(), "pattern {i} route");
            ensure!(p.n >= 1 && p.stops_start as u64 + p.n as u64 <= np as u64, "pattern {i} stops");
            ensure!(p.trips_start as u64 + p.ntrips as u64 <= ntrips as u64, "pattern {i} trips");
            ensure!(p.times_start as u64 + p.ntrips as u64 * p.n as u64 <= self.arr.len() as u64, "pattern {i} times");
            ensure!(p.shape == NONE || (p.shape as usize) < nshapes, "pattern {i} shape");
            if p.shape != NONE {
                let seg = (self.shape_start[p.shape as usize + 1] - self.shape_start[p.shape as usize]) as u64;
                let a = p.stops_start as usize;
                ensure!(
                    self.pattern_shape_idx[a..a + p.n as usize].iter().all(|&x| x == NONE || (x as u64) < seg),
                    "pattern {i} shape index"
                );
            }
        }
        ensure!(self.stop_pat_start.len() == nstops + 1, "stop→pattern index size");
        ensure!(self.stop_pat_start.first() == Some(&0), "stop→pattern start");
        ensure!(self.stop_pat_start.windows(2).all(|w| w[0] <= w[1]), "stop→pattern order");
        ensure!(
            *self.stop_pat_start.last().unwrap() as usize == self.stop_pat_pattern.len() && self.stop_pat_pos.len() == self.stop_pat_pattern.len(),
            "stop→pattern arrays"
        );
        for (k, &p) in self.stop_pat_pattern.iter().enumerate() {
            ensure!((p as usize) < self.patterns.len() && self.stop_pat_pos[k] < self.patterns[p as usize].n, "stop→pattern entry {k}");
        }
        ensure!(self.tr_start.len() == nstops + 1, "transfer index size");
        ensure!(self.tr_start.first() == Some(&0), "transfer start");
        ensure!(self.tr_start.windows(2).all(|w| w[0] <= w[1]), "transfer order");
        ensure!(*self.tr_start.last().unwrap() as usize == self.tr_to.len() && self.tr_dur.len() == self.tr_to.len(), "transfer arrays");
        ensure!(self.tr_to.iter().all(|&s| (s as usize) < nstops), "transfer target");
        for (name, len) in [
            ("stop parent", self.stop_parent.len()),
            ("stop wheelchair", self.stop_wheelchair.len()),
            ("stop change time", self.stop_min_change.len()),
        ] {
            ensure!(len == 0 || len == nstops, "{name} size");
        }
        ensure!(self.stop_parent.iter().all(|&p| str_ok(p)), "stop parent");
        ensure!(self.stop_modes.len() == nstops && self.search_keys.len() == nstops, "stop indexes");
        ensure!(self.grid_start.len() == self.grid_keys.len() + 1, "grid size");
        ensure!(self.grid_start.first() == Some(&0) && self.grid_start.windows(2).all(|w| w[0] <= w[1]), "grid order");
        ensure!(*self.grid_start.last().unwrap() as usize == self.grid_stops.len(), "grid stops");
        ensure!(self.grid_keys.windows(2).all(|w| w[0] < w[1]), "grid keys");
        ensure!(self.grid_stops.iter().all(|&s| (s as usize) < nstops), "grid stop index");
        Ok(())
    }
}

// ====================================================================== search

/// Search key of a stop name or a query: the geocoder's normalization (`geocode::norm::norm`:
/// lower case, no niqqud / geresh / punctuation, final letters folded – one normalizer for
/// stops and places, so their scores mean the same) with the abbreviations the MOT feed uses in
/// stop names spelled out, so that "תחנה מרכזית ירושלים" matches "ת. מרכזית ירושלים/יפו".
pub fn search_key(s: &str) -> String {
    let n = crate::geocode::norm::norm(s);
    let mut out = String::with_capacity(n.len() + 8);
    for (i, tok) in n.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        // (final letters are folded by norm: "קופת חולימ")
        out.push_str(match tok {
            "ת" => "תחנה",
            "רח" => "רחוב",
            "שד" => "שדרות",
            "ביס" => "בית ספר",
            "ביח" => "בית חולימ",
            "קופח" => "קופת חולימ",
            "ביהכנ" | "ביהכנס" => "בית כנסת",
            _ => tok,
        });
    }
    out
}

pub fn search_stops(t: &Transit, query: &str, near: Option<LonLat>, limit: usize) -> Vec<GeoResult> {
    let q = search_key(query);
    if q.is_empty() || limit == 0 || t.stops.is_empty() {
        return vec![];
    }
    let qcode = q.replace(' ', "");
    let word_q = format!(" {q}");
    let mut hits: Vec<(u32, f64, u32)> = Vec::new(); // (score, dist, stop)
    for (i, st) in t.stops.iter().enumerate() {
        let i = i as u32;
        if !t.stop_is_served(i) {
            continue;
        }
        let name = &t.search_keys[i as usize];
        let code = t.s(st.code);
        // Scores are on the geocoder's scale (exact address 200, exact POI name ~105): an exact
        // stop code beats everything, an exact stop name ranks with an exact place name.
        let score = if !code.is_empty() && code == qcode {
            300
        } else if *name == q {
            100
        } else if name.starts_with(&q) {
            72
        } else if name.contains(&word_q) {
            52
        } else if !code.is_empty() && qcode.len() >= 2 && code.starts_with(&qcode) {
            45
        } else if name.contains(&q) {
            30
        } else {
            continue;
        };
        let d = near.map(|p| haversine_m(p, [st.lon, st.lat])).unwrap_or(0.0);
        hits.push((score, d, i));
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.total_cmp(&b.1)));
    // The feed lists some stops several times (one record per platform / per operator feed)
    // with the same code and name: show each stop once, keeping its best-scored record.
    // (A Vec + contains() here was quadratic: "א" matches ~30k stops, 129 ms per keystroke.)
    let mut seen: rustc_hash::FxHashSet<(&str, &str)> = rustc_hash::FxHashSet::default();
    let mut unique = Vec::with_capacity(limit);
    for h in hits {
        let st = &t.stops[h.2 as usize];
        let code = t.s(st.code);
        if seen.insert((code, if code.is_empty() { t.s(st.name) } else { "" })) {
            unique.push(h);
            if unique.len() == limit {
                break;
            }
        }
    }
    let hits = unique;
    hits.into_iter()
        .map(|(score, d, i)| {
            let st = &t.stops[i as usize];
            let code = t.s(st.code).to_string();
            let city = t.s(st.city);
            GeoResult {
                id: format!("stop:{}", if code.is_empty() { i.to_string() } else { code.clone() }),
                name: t.s(st.name).to_string(),
                kind: GeoKind::Stop,
                precision: GeoPrecision::Exact,
                lon: st.lon,
                lat: st.lat,
                city: if city.is_empty() { None } else { Some(city.to_string()) },
                street: None,
                house_number: None,
                category: Some(format!("stop:{code}")),
                // far-away stops fall behind nearby places of the same name: -1 per km, max -60
                score: score as f64 - (d / 1000.0).min(60.0),
                extra: None,
            }
        })
        .collect()
}

// ====================================================================== build helpers

#[derive(Default)]
struct Interner {
    map: FxHashMap<String, u32>,
    strings: Vec<String>,
}
impl Interner {
    fn get(&mut self, s: &str) -> u32 {
        if s.is_empty() {
            return NONE;
        }
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.map.insert(s.to_string(), i);
        i
    }
}

struct Cols(FxHashMap<String, usize>);
impl Cols {
    fn new(h: &ByteRecord) -> Self {
        let mut m = FxHashMap::default();
        for (i, f) in h.iter().enumerate() {
            let s = String::from_utf8_lossy(f);
            let s = s.trim_start_matches('\u{feff}').trim().to_ascii_lowercase();
            m.insert(s, i);
        }
        Cols(m)
    }
    fn idx(&self, name: &str) -> Option<usize> {
        self.0.get(name).copied()
    }
    fn need(&self, name: &str, file: &str) -> Result<usize> {
        self.idx(name).ok_or_else(|| anyhow!("{file}: missing column {name}"))
    }
}

#[inline]
fn field(rec: &ByteRecord, i: Option<usize>) -> &[u8] {
    match i.and_then(|i| rec.get(i)) {
        Some(b) => b.trim_ascii(),
        None => b"",
    }
}
#[inline]
fn fstr(rec: &ByteRecord, i: Option<usize>) -> String {
    String::from_utf8_lossy(field(rec, i)).into_owned()
}

fn parse_u32(b: &[u8]) -> Option<u32> {
    if b.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((c - b'0') as u32)?;
    }
    Some(v)
}

/// "H:MM:SS" / "HH:MM:SS" (hours may exceed 24) → seconds.
pub(crate) fn parse_time(b: &[u8]) -> Option<u32> {
    let b = b.trim_ascii();
    if b.is_empty() {
        return None;
    }
    let mut parts = [0u32; 3];
    let mut n = 0;
    for p in b.split(|&c| c == b':') {
        if n >= 3 {
            return None;
        }
        parts[n] = parse_u32(p.trim_ascii())?;
        n += 1;
    }
    if n < 2 {
        return None;
    }
    Some(parts[0] * 3600 + parts[1] * 60 + parts[2])
}

fn parse_date(b: &[u8]) -> Option<NaiveDate> {
    let s = std::str::from_utf8(b.trim_ascii()).ok()?;
    NaiveDate::parse_from_str(s, "%Y%m%d").ok()
}

/// Find the zip entry named `name` (at root or inside a folder).
fn entry_name(zip: &zip::ZipArchive<File>, name: &str) -> Option<String> {
    let suffix = format!("/{name}");
    zip.file_names()
        .find(|n| n.eq_ignore_ascii_case(name) || n.to_ascii_lowercase().ends_with(&suffix))
        .map(|s| s.to_string())
}

/// Stream a CSV table from the zip. `f(cols, record, bytes_read, total_bytes)`.
fn read_table(
    zip: &mut zip::ZipArchive<File>,
    name: &str,
    required: bool,
    mut f: impl FnMut(&Cols, &ByteRecord, u64, u64) -> Result<()>,
) -> Result<bool> {
    let Some(en) = entry_name(zip, name) else {
        if required {
            bail!("GTFS zip has no {name}");
        }
        return Ok(false);
    };
    let entry = zip.by_name(&en)?;
    let total = entry.size();
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(true)
        .buffer_capacity(1 << 20)
        .from_reader(entry);
    let headers = rdr.byte_headers()?.clone();
    let cols = Cols::new(&headers);
    let mut rec = ByteRecord::new();
    loop {
        match rdr.read_byte_record(&mut rec) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                // tolerate a broken line, abort on I/O errors
                if matches!(e.kind(), csv::ErrorKind::Io(_)) {
                    return Err(e.into());
                }
                log::warn!("{name}: skipping bad record: {e}");
                continue;
            }
        }
        f(&cols, &rec, rdr.position().byte(), total)?;
    }
    Ok(true)
}

fn parse_color(s: &[u8]) -> Option<u32> {
    let s = std::str::from_utf8(s).ok()?.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    u32::from_str_radix(s, 16).ok()
}

/// City from Israeli stop_desc: "רחוב: ... עיר: <city> רציף: ... קומה: ...".
fn city_from_desc(desc: &str) -> &str {
    const CITY: &str = "\u{05E2}\u{05D9}\u{05E8}:"; // עיר:
    const PLATFORM: &str = "\u{05E8}\u{05E6}\u{05D9}\u{05E3}:"; // רציף:
    let Some(i) = desc.find(CITY) else { return "" };
    let rest = &desc[i + CITY.len()..];
    let end = rest.find(PLATFORM).unwrap_or(rest.len());
    rest[..end].trim()
}

// ---------------------------------------------------------------- geometry helpers (build)

fn to_xy(p: [f64; 2], lat0: f64) -> [f64; 2] {
    [p[0] * 111_320.0 * lat0.to_radians().cos(), p[1] * 110_540.0]
}

fn seg_dist2(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (x, y) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    x * x + y * y
}

/// Douglas-Peucker on local metric coordinates; returns kept indices.
fn simplify(xy: &[[f64; 2]], tol: f64) -> Vec<usize> {
    let n = xy.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let tol2 = tol * tol;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut best = 0.0;
        let mut bi = a;
        for i in a + 1..b {
            let d = seg_dist2(xy[i], xy[a], xy[b]);
            if d > best {
                best = d;
                bi = i;
            }
        }
        if best > tol2 {
            keep[bi] = true;
            stack.push((a, bi));
            stack.push((bi, b));
        }
    }
    (0..n).filter(|&i| keep[i]).collect()
}

/// Map each stop (in order) to a shape segment index, monotonically.
fn map_stops_to_shape(shape: &[[f64; 2]], stops: &[[f64; 2]]) -> Vec<u32> {
    let nseg = shape.len().saturating_sub(1);
    if nseg == 0 {
        return vec![NONE; stops.len()];
    }
    let mut out = Vec::with_capacity(stops.len());
    let mut prev = 0usize;
    const NEAR2: f64 = 60.0 * 60.0;
    for &s in stops {
        let d = |i: usize| seg_dist2(s, shape[i], shape[i + 1]);
        let mut found = None;
        let mut i = prev;
        while i < nseg {
            if d(i) < NEAR2 {
                let mut j = i;
                while j + 1 < nseg && d(j + 1) <= d(j) {
                    j += 1;
                }
                found = Some(j);
                break;
            }
            i += 1;
        }
        let j = found.unwrap_or_else(|| {
            (prev..nseg).min_by(|&a, &b| d(a).total_cmp(&d(b))).unwrap_or(prev)
        });
        out.push(j as u32);
        prev = j;
    }
    out
}

// ====================================================================== build

struct CalRow {
    svc: u32,
    days: [bool; 7],
    start: NaiveDate,
    end: NaiveDate,
}

/// Walking seconds from one point to each target (None = no way found); used for the stop-to-stop
/// transfers at build time. See `transit::walk_times_from`.
pub type TransferWalk<'a> = &'a (dyn Fn(LonLat, &[LonLat]) -> Vec<Option<f32>> + Sync);

/// Build without a road network (straight-line transfers).
pub fn build(gtfs_zip: &Path, out: &Path, progress: ProgressFn) -> Result<()> {
    build_with(gtfs_zip, out, progress, None)
}

pub fn build_with(gtfs_zip: &Path, out: &Path, progress: ProgressFn, walk: Option<TransferWalk>) -> Result<()> {
    let t0 = std::time::Instant::now();
    let p = |done: u64, total: u64, msg: &str| progress(ProgressPhase::Processing, done, total, msg);
    let f = File::open(gtfs_zip).with_context(|| format!("open {}", gtfs_zip.display()))?;
    let mut zip = zip::ZipArchive::new(f).context("GTFS zip")?;
    let mut strings = Interner::default();

    // ------------------------------------------------ agency
    if !p(0, 0, "GTFS: מפעילים") {
        bail!("cancelled");
    }
    let mut agency_map: FxHashMap<String, u32> = FxHashMap::default();
    let mut agencies: Vec<u32> = Vec::new();
    let mut agency_tz: Vec<u32> = Vec::new();
    let mut ix: Option<(Option<usize>, Option<usize>, Option<usize>)> = None;
    read_table(&mut zip, "agency.txt", false, |c, r, _, _| {
        let (i_id, i_name, i_tz) = *ix.get_or_insert_with(|| (c.idx("agency_id"), c.idx("agency_name"), c.idx("agency_timezone")));
        let id = fstr(r, i_id);
        let name = strings.get(&fstr(r, i_name));
        agency_map.insert(id, agencies.len() as u32);
        agencies.push(name);
        agency_tz.push(strings.get(&fstr(r, i_tz)));
        Ok(())
    })?;
    for &z in &agency_tz {
        let tz = if z == NONE { "" } else { strings.strings[z as usize].as_str() };
        if !tz.is_empty() && tz != "Asia/Jerusalem" {
            log::warn!("GTFS: agency_timezone {tz}: times are interpreted as Asia/Jerusalem");
        }
    }

    // ------------------------------------------------ routes
    if !p(0, 0, "GTFS: קווים") {
        bail!("cancelled");
    }
    let mut route_map: FxHashMap<String, u32> = FxHashMap::default();
    let mut routes: Vec<RouteInfo> = Vec::new();
    let mut ix: Option<[Option<usize>; 6]> = None;
    read_table(&mut zip, "routes.txt", true, |c, r, _, _| {
        let i = *ix.get_or_insert_with(|| {
            [
                c.idx("route_id"),
                c.idx("agency_id"),
                c.idx("route_short_name"),
                c.idx("route_long_name"),
                c.idx("route_type"),
                c.idx("route_color"),
            ]
        });
        let id = fstr(r, i[0]);
        let agency = agency_map
            .get(&fstr(r, i[1]))
            .copied()
            .unwrap_or(if agencies.len() == 1 { 0 } else { NONE });
        let info = RouteInfo {
            agency,
            short_name: strings.get(&fstr(r, i[2])),
            long_name: strings.get(&fstr(r, i[3])),
            route_type: parse_u32(field(r, i[4])).unwrap_or(3) as u16,
            color: parse_color(field(r, i[5])),
        };
        route_map.insert(id, routes.len() as u32);
        routes.push(info);
        Ok(())
    })?;

    // ------------------------------------------------ calendar
    if !p(0, 0, "GTFS: ימי פעילות") {
        bail!("cancelled");
    }
    let mut svc_map: FxHashMap<String, u32> = FxHashMap::default();
    let mut cal: Vec<CalRow> = Vec::new();
    let mut cal_dates: Vec<(u32, NaiveDate, bool)> = Vec::new();
    {
        let mut ix: Option<[Option<usize>; 10]> = None;
        read_table(&mut zip, "calendar.txt", false, |c, r, _, _| {
            let i = *ix.get_or_insert_with(|| {
                [
                    c.idx("service_id"),
                    c.idx("monday"),
                    c.idx("tuesday"),
                    c.idx("wednesday"),
                    c.idx("thursday"),
                    c.idx("friday"),
                    c.idx("saturday"),
                    c.idx("sunday"),
                    c.idx("start_date"),
                    c.idx("end_date"),
                ]
            });
            let sid = fstr(r, i[0]);
            let n = svc_map.len() as u32;
            let svc = *svc_map.entry(sid).or_insert(n);
            let mut days = [false; 7];
            for d in 0..7 {
                days[d] = field(r, i[1 + d]) == b"1";
            }
            if let (Some(start), Some(end)) = (parse_date(field(r, i[8])), parse_date(field(r, i[9]))) {
                cal.push(CalRow { svc, days, start, end });
            }
            Ok(())
        })?;
        let mut ix: Option<[Option<usize>; 3]> = None;
        read_table(&mut zip, "calendar_dates.txt", false, |c, r, _, _| {
            let i = *ix.get_or_insert_with(|| [c.idx("service_id"), c.idx("date"), c.idx("exception_type")]);
            let sid = fstr(r, i[0]);
            let n = svc_map.len() as u32;
            let svc = *svc_map.entry(sid).or_insert(n);
            if let Some(d) = parse_date(field(r, i[1])) {
                cal_dates.push((svc, d, field(r, i[2]) == b"1"));
            }
            Ok(())
        })?;
    }
    let mut dmin: Option<NaiveDate> = None;
    let mut dmax: Option<NaiveDate> = None;
    let mut upd = |d: NaiveDate| {
        dmin = Some(dmin.map_or(d, |m| m.min(d)));
        dmax = Some(dmax.map_or(d, |m| m.max(d)));
    };
    for c in &cal {
        upd(c.start);
        upd(c.end);
    }
    for &(_, d, add) in &cal_dates {
        if add {
            upd(d);
        }
    }
    let (Some(dmin), Some(dmax)) = (dmin, dmax) else { bail!("GTFS: empty calendar") };
    let feed_start = dmin.num_days_from_ce();
    let num_days = ((dmax.num_days_from_ce() - feed_start + 1).clamp(1, 800)) as u32;
    let num_services = svc_map.len() as u32;
    let service_words = num_days.div_ceil(64);
    let mut service_bits = vec![0u64; (num_services * service_words) as usize];
    let set_bit = |bits: &mut Vec<u64>, svc: u32, day: i32, on: bool| {
        if day < 0 || day as u32 >= num_days {
            return;
        }
        let i = (svc * service_words + day as u32 / 64) as usize;
        if on {
            bits[i] |= 1 << (day as u32 % 64);
        } else {
            bits[i] &= !(1 << (day as u32 % 64));
        }
    };
    for c in &cal {
        let a = (c.start.num_days_from_ce() - feed_start).max(0);
        let b = (c.end.num_days_from_ce() - feed_start).min(num_days as i32 - 1);
        for day in a..=b {
            let date = NaiveDate::from_num_days_from_ce_opt(feed_start + day).unwrap();
            if c.days[date.weekday().num_days_from_monday() as usize] {
                set_bit(&mut service_bits, c.svc, day, true);
            }
        }
    }
    for &(svc, d, add) in &cal_dates {
        set_bit(&mut service_bits, svc, d.num_days_from_ce() - feed_start, add);
    }
    drop(cal);
    drop(cal_dates);

    // ------------------------------------------------ stops
    if !p(0, 0, "GTFS: תחנות") {
        bail!("cancelled");
    }
    let mut stop_map: FxHashMap<String, u32> = FxHashMap::default();
    let mut stops: Vec<Stop> = Vec::new();
    let mut stop_parent: Vec<u32> = Vec::new();
    let mut stop_wheelchair: Vec<u8> = Vec::new();
    // wheelchair_boarding of stations, inherited by their stops that say 0
    let mut station_wb: FxHashMap<u32, u8> = FxHashMap::default();
    let mut ix: Option<[Option<usize>; 9]> = None;
    read_table(&mut zip, "stops.txt", true, |c, r, _, _| {
        let i = *ix.get_or_insert_with(|| {
            [
                c.idx("stop_id"),
                c.idx("stop_code"),
                c.idx("stop_name"),
                c.idx("stop_desc"),
                c.idx("stop_lat"),
                c.idx("stop_lon"),
                c.idx("location_type"),
                c.idx("parent_station"),
                c.idx("wheelchair_boarding"),
            ]
        });
        let lt = field(r, i[6]);
        let wb = parse_u32(field(r, i[8])).unwrap_or(0).min(2) as u8;
        if lt == b"1" {
            let id = strings.get(&fstr(r, i[0]));
            if wb != 0 && id != NONE {
                station_wb.insert(id, wb);
            }
            return Ok(());
        }
        if !(lt.is_empty() || lt == b"0") {
            return Ok(());
        }
        let lat: f64 = std::str::from_utf8(field(r, i[4])).ok().and_then(|s| s.parse().ok()).unwrap_or(f64::NAN);
        let lon: f64 = std::str::from_utf8(field(r, i[5])).ok().and_then(|s| s.parse().ok()).unwrap_or(f64::NAN);
        if !lat.is_finite() || !lon.is_finite() {
            return Ok(());
        }
        let desc = fstr(r, i[3]);
        let st = Stop {
            name: strings.get(&fstr(r, i[2])),
            code: strings.get(&fstr(r, i[1])),
            city: strings.get(city_from_desc(&desc)),
            lon,
            lat,
        };
        stop_map.insert(fstr(r, i[0]), stops.len() as u32);
        stops.push(st);
        stop_parent.push(strings.get(&fstr(r, i[7])));
        stop_wheelchair.push(wb);
        Ok(())
    })?;
    for (k, w) in stop_wheelchair.iter_mut().enumerate() {
        if *w == 0 {
            if let Some(&pw) = station_wb.get(&stop_parent[k]) {
                *w = pw;
            }
        }
    }
    if stop_wheelchair.iter().all(|&w| w == 0) {
        stop_wheelchair = Vec::new();
    }
    if stop_parent.iter().all(|&p| p == NONE) {
        stop_parent = Vec::new();
    }

    // ------------------------------------------------ trips
    if !p(0, 0, "GTFS: נסיעות") {
        bail!("cancelled");
    }
    let mut trip_map: FxHashMap<Box<str>, u32> = FxHashMap::default();
    let mut trip_route: Vec<u32> = Vec::new();
    let mut trip_svc: Vec<u32> = Vec::new();
    let mut trip_head: Vec<u32> = Vec::new();
    let mut trip_shape: Vec<u32> = Vec::new();
    let mut trip_wb: Vec<u8> = Vec::new();
    let mut shape_map: FxHashMap<Box<str>, u32> = FxHashMap::default();
    let mut ix: Option<[Option<usize>; 6]> = None;
    read_table(&mut zip, "trips.txt", true, |c, r, _, _| {
        let i = *ix.get_or_insert_with(|| {
            [c.idx("route_id"), c.idx("service_id"), c.idx("trip_id"), c.idx("trip_headsign"), c.idx("shape_id"), c.idx("wheelchair_accessible")]
        });
        let Some(&route) = route_map.get(&fstr(r, i[0])) else { return Ok(()) };
        let Some(&svc) = svc_map.get(&fstr(r, i[1])) else { return Ok(()) };
        let shp = fstr(r, i[4]);
        let shape = if shp.is_empty() {
            NONE
        } else {
            let n = shape_map.len() as u32;
            *shape_map.entry(shp.into_boxed_str()).or_insert(n)
        };
        trip_map.insert(fstr(r, i[2]).into_boxed_str(), trip_route.len() as u32);
        trip_route.push(route);
        trip_svc.push(svc);
        trip_head.push(strings.get(&fstr(r, i[3])));
        trip_shape.push(shape);
        trip_wb.push(parse_u32(field(r, i[5])).unwrap_or(0).min(2) as u8);
        Ok(())
    })?;
    let ntrips = trip_route.len();
    let has_trip_wb = trip_wb.iter().any(|&w| w != 0);

    // ------------------------------------------------ frequencies (trips repeated every headway)
    let mut freq: FxHashMap<u32, Vec<(u32, u32, u32)>> = FxHashMap::default();
    {
        let mut ix: Option<[Option<usize>; 4]> = None;
        read_table(&mut zip, "frequencies.txt", false, |c, r, _, _| {
            let i = *ix.get_or_insert_with(|| [c.idx("trip_id"), c.idx("start_time"), c.idx("end_time"), c.idx("headway_secs")]);
            let Some(&trip) = std::str::from_utf8(field(r, i[0])).ok().and_then(|s| trip_map.get(s)) else { return Ok(()) };
            if let (Some(a), Some(b), Some(h)) = (parse_time(field(r, i[1])), parse_time(field(r, i[2])), parse_u32(field(r, i[3]))) {
                if h > 0 && b > a {
                    freq.entry(trip).or_default().push((a, b, h));
                }
            }
            Ok(())
        })?;
        if !freq.is_empty() {
            log::info!("GTFS: frequencies.txt: {} trips repeated by headway", freq.len());
        }
    }
    log::info!("GTFS: {} stops, {} routes, {} trips, {} services, {} days", stops.len(), routes.len(), ntrips, num_services, num_days);

    // ------------------------------------------------ stop_times (streamed)
    let st_size = entry_name(&zip, "stop_times.txt")
        .and_then(|n| zip.by_name(&n).ok().map(|e| e.size()))
        .unwrap_or(0);
    let est_rows = (st_size / 40).min(80_000_000) as usize;
    let mut st_stop: Vec<u32> = Vec::with_capacity(est_rows);
    let mut st_seq: Vec<u32> = Vec::with_capacity(est_rows);
    let mut st_arr: Vec<u32> = Vec::with_capacity(est_rows);
    let mut st_dep: Vec<u32> = Vec::with_capacity(est_rows);
    let mut st_flags: Vec<u8> = Vec::with_capacity(est_rows);
    // first segment per trip (start, len); extra segments for non-contiguous trips
    let mut trip_seg: Vec<(u32, u32)> = vec![(0, 0); ntrips];
    let mut extra_seg: FxHashMap<u32, Vec<(u32, u32)>> = FxHashMap::default();
    {
        let mut last_tid: Vec<u8> = Vec::new();
        let mut cur_trip = NONE;
        let mut seg_start = 0u32;
        let mut rows: u64 = 0;
        let mut ix: Option<[Option<usize>; 7]> = None;
        let mut close = |trip: u32, start: u32, end: u32, trip_seg: &mut Vec<(u32, u32)>| {
            if trip == NONE || end <= start {
                return;
            }
            let s = &mut trip_seg[trip as usize];
            if s.1 == 0 {
                *s = (start, end - start);
            } else {
                extra_seg.entry(trip).or_default().push((start, end - start));
            }
        };
        read_table(&mut zip, "stop_times.txt", true, |c, r, pos, total| {
            let i = match ix {
                Some(i) => i,
                None => {
                    let v = [
                        Some(c.need("trip_id", "stop_times.txt")?),
                        c.idx("arrival_time"),
                        c.idx("departure_time"),
                        Some(c.need("stop_id", "stop_times.txt")?),
                        c.idx("stop_sequence"),
                        c.idx("pickup_type"),
                        c.idx("drop_off_type"),
                    ];
                    ix = Some(v);
                    v
                }
            };
            rows += 1;
            if rows.is_multiple_of(1_000_000)
                && !p(pos, total, &format!("GTFS: זמני עצירה – {} מיליון שורות", rows / 1_000_000)) {
                    bail!("cancelled");
                }
            let tid = field(r, i[0]);
            if tid != last_tid.as_slice() {
                let n = st_stop.len() as u32;
                close(cur_trip, seg_start, n, &mut trip_seg);
                last_tid.clear();
                last_tid.extend_from_slice(tid);
                cur_trip = std::str::from_utf8(tid).ok().and_then(|s| trip_map.get(s).copied()).unwrap_or(NONE);
                seg_start = n;
            }
            if cur_trip == NONE {
                return Ok(());
            }
            let Some(stop) = std::str::from_utf8(field(r, i[3])).ok().and_then(|s| stop_map.get(s).copied()) else {
                return Ok(());
            };
            let arr = parse_time(field(r, i[1]));
            let dep = parse_time(field(r, i[2]));
            let (arr, dep) = match (arr, dep) {
                (Some(a), Some(d)) => (a, d.max(a)),
                (Some(a), None) => (a, a),
                (None, Some(d)) => (d, d),
                (None, None) => (NONE, NONE),
            };
            let mut fl = 0u8;
            if field(r, i[5]) == b"1" {
                fl |= NO_PICKUP;
            }
            if field(r, i[6]) == b"1" {
                fl |= NO_DROPOFF;
            }
            st_stop.push(stop);
            st_seq.push(parse_u32(field(r, i[4])).unwrap_or(st_seq.len() as u32));
            st_arr.push(arr);
            st_dep.push(dep);
            st_flags.push(fl);
            Ok(())
        })?;
        let n = st_stop.len() as u32;
        close(cur_trip, seg_start, n, &mut trip_seg);
        if !p(total_or(st_size), total_or(st_size), &format!("GTFS: זמני עצירה – {rows} שורות")) {
            bail!("cancelled");
        }
    }
    drop(trip_map);
    log::info!("GTFS: stop_times {} rows kept, {} non-contiguous trips ({:.1}s)", st_stop.len(), extra_seg.len(), t0.elapsed().as_secs_f64());

    // ------------------------------------------------ group trips into raw patterns
    if !p(0, ntrips as u64, "GTFS: בניית תבניות קווים") {
        bail!("cancelled");
    }
    let mut rows_buf: Vec<u32> = Vec::new();
    let trip_rows = |trip: u32, buf: &mut Vec<u32>| {
        buf.clear();
        let (s, l) = trip_seg[trip as usize];
        buf.extend(s..s + l);
        if let Some(ex) = extra_seg.get(&trip) {
            for &(s, l) in ex {
                buf.extend(s..s + l);
            }
        }
        if !buf.windows(2).all(|w| st_seq[w[0] as usize] < st_seq[w[1] as usize]) {
            buf.sort_by_key(|&r| st_seq[r as usize]);
            buf.dedup_by_key(|r| st_seq[*r as usize]);
        }
    };
    let mut key_map: FxHashMap<Vec<u32>, u32> = FxHashMap::default();
    let mut raw_keys: Vec<Vec<u32>> = Vec::new(); // route, then stop*4+flags
    let mut raw_trips: Vec<Vec<u32>> = Vec::new();
    let mut key: Vec<u32> = Vec::new();
    for trip in 0..ntrips as u32 {
        if trip_seg[trip as usize].1 == 0 {
            continue;
        }
        trip_rows(trip, &mut rows_buf);
        if rows_buf.len() < 2 || rows_buf.iter().all(|&r| st_arr[r as usize] == NONE) {
            continue;
        }
        key.clear();
        key.push(trip_route[trip as usize]);
        let n = rows_buf.len();
        for (k, &r) in rows_buf.iter().enumerate() {
            let mut fl = st_flags[r as usize];
            if k == 0 {
                fl |= NO_DROPOFF;
            }
            if k == n - 1 {
                fl |= NO_PICKUP;
            }
            key.push(st_stop[r as usize] * 4 + fl as u32);
        }
        let pid = match key_map.get(&key) {
            Some(&p) => p,
            None => {
                let p = raw_keys.len() as u32;
                key_map.insert(key.clone(), p);
                raw_keys.push(key.clone());
                raw_trips.push(Vec::new());
                p
            }
        };
        raw_trips[pid as usize].push(trip);
    }
    drop(key_map);
    drop(st_flags);
    log::info!("GTFS: {} raw patterns", raw_keys.len());

    // ------------------------------------------------ FIFO split + time tables
    let total_rows: usize = st_stop.len();
    drop(st_stop);
    let mut arr_out: Vec<u32> = Vec::with_capacity(total_rows);
    let mut dep_out: Vec<u32> = Vec::with_capacity(total_rows);
    let mut patterns: Vec<Pattern> = Vec::new();
    let mut pattern_stops: Vec<u32> = Vec::new();
    let mut pattern_flags: Vec<u8> = Vec::new();
    let mut trip_service: Vec<u32> = Vec::new();
    let mut trip_headsign: Vec<u32> = Vec::new();
    let mut trip_wheelchair: Vec<u8> = Vec::new();
    let mut pattern_rep_shape: Vec<u32> = Vec::new();
    let mut done_trips = 0u64;
    for (pid, trips) in raw_trips.iter().enumerate() {
        let k = &raw_keys[pid];
        let n = k.len() - 1;
        // times per trip
        let mut tt: Vec<(u32, Vec<u32>, Vec<u32>)> = Vec::with_capacity(trips.len());
        for &trip in trips {
            trip_rows(trip, &mut rows_buf);
            let mut a: Vec<u32> = rows_buf.iter().map(|&r| st_arr[r as usize]).collect();
            let mut d: Vec<u32> = rows_buf.iter().map(|&r| st_dep[r as usize]).collect();
            if !interpolate(&mut a, &mut d) {
                continue;
            }
            match freq.get(&trip) {
                // a frequency-based trip: its stop_times are a template, shifted to every start
                Some(windows) => {
                    for &(start, end, headway) in windows {
                        for t in expand_headway(start, end, headway) {
                            let off = t as i64 - d[0] as i64;
                            let sh = |v: &Vec<u32>| v.iter().map(|&x| (x as i64 + off).max(0) as u32).collect::<Vec<u32>>();
                            tt.push((trip, sh(&a), sh(&d)));
                        }
                    }
                }
                None => tt.push((trip, a, d)),
            }
        }
        done_trips += trips.len() as u64;
        if tt.is_empty() {
            continue;
        }
        tt.sort_by(|x, y| x.2[0].cmp(&y.2[0]).then(x.1[n - 1].cmp(&y.1[n - 1])));
        // greedy FIFO split
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (ti, (_, a, d)) in tt.iter().enumerate() {
            let g = groups.iter().position(|g| {
                let (_, pa, pd) = &tt[*g.last().unwrap()];
                (0..n).all(|i| a[i] >= pa[i] && d[i] >= pd[i])
            });
            match g {
                Some(g) => groups[g].push(ti),
                None => groups.push(vec![ti]),
            }
        }
        for g in groups {
            let stops_start = pattern_stops.len() as u32;
            for &e in &k[1..] {
                pattern_stops.push(e / 4);
                pattern_flags.push((e % 4) as u8);
            }
            let times_start = arr_out.len();
            if times_start + g.len() * n > u32::MAX as usize {
                bail!("GTFS too large");
            }
            let trips_start = trip_service.len() as u32;
            let mut shape = NONE;
            for &ti in &g {
                let (trip, a, d) = &tt[ti];
                arr_out.extend_from_slice(a);
                dep_out.extend_from_slice(d);
                trip_service.push(trip_svc[*trip as usize]);
                trip_headsign.push(trip_head[*trip as usize]);
                if has_trip_wb {
                    trip_wheelchair.push(trip_wb[*trip as usize]);
                }
                if shape == NONE {
                    shape = trip_shape[*trip as usize];
                }
            }
            patterns.push(Pattern {
                route: k[0],
                stops_start,
                n: n as u32,
                trips_start,
                ntrips: g.len() as u32,
                times_start: times_start as u32,
                shape: NONE,
            });
            pattern_rep_shape.push(shape);
        }
        if pid % 2000 == 0
            && !p(done_trips, ntrips as u64, &format!("GTFS: {} תבניות, {} נסיעות", patterns.len(), done_trips)) {
                bail!("cancelled");
            }
    }
    drop(st_arr);
    drop(st_dep);
    drop(st_seq);
    drop(raw_trips);
    drop(raw_keys);
    log::info!("GTFS: {} patterns, {} trips, {} times ({:.1}s)", patterns.len(), trip_service.len(), arr_out.len(), t0.elapsed().as_secs_f64());

    // ------------------------------------------------ shapes (only those used by patterns)
    if !p(0, 0, "GTFS: תוואי קווים") {
        bail!("cancelled");
    }
    let nshapes_all = shape_map.len();
    let mut needed = vec![false; nshapes_all];
    for &s in &pattern_rep_shape {
        if s != NONE {
            needed[s as usize] = true;
        }
    }
    let mut raw_shapes: Vec<Vec<(u32, i32, i32)>> = vec![Vec::new(); nshapes_all];
    {
        let mut last: Vec<u8> = Vec::new();
        let mut cur = NONE;
        let mut ix: Option<[Option<usize>; 4]> = None;
        let mut rows = 0u64;
        read_table(&mut zip, "shapes.txt", false, |c, r, pos, total| {
            let i = *ix.get_or_insert_with(|| {
                [c.idx("shape_id"), c.idx("shape_pt_lat"), c.idx("shape_pt_lon"), c.idx("shape_pt_sequence")]
            });
            rows += 1;
            if rows.is_multiple_of(1_000_000)
                && !p(pos, total, &format!("GTFS: תוואי קווים – {} מיליון נקודות", rows / 1_000_000)) {
                    bail!("cancelled");
                }
            let sid = field(r, i[0]);
            if sid != last.as_slice() {
                last.clear();
                last.extend_from_slice(sid);
                cur = std::str::from_utf8(sid)
                    .ok()
                    .and_then(|s| shape_map.get(s).copied())
                    .filter(|&s| needed[s as usize])
                    .unwrap_or(NONE);
            }
            if cur == NONE {
                return Ok(());
            }
            let lat: f64 = std::str::from_utf8(field(r, i[1])).ok().and_then(|s| s.parse().ok()).unwrap_or(f64::NAN);
            let lon: f64 = std::str::from_utf8(field(r, i[2])).ok().and_then(|s| s.parse().ok()).unwrap_or(f64::NAN);
            if !lat.is_finite() || !lon.is_finite() {
                return Ok(());
            }
            let seq = parse_u32(field(r, i[3])).unwrap_or(raw_shapes[cur as usize].len() as u32);
            raw_shapes[cur as usize].push((seq, (lon * 1e6).round() as i32, (lat * 1e6).round() as i32));
            Ok(())
        })?;
    }
    // simplify in parallel
    let simplified: Vec<Vec<[i32; 2]>> = raw_shapes
        .into_par_iter()
        .map(|mut v| {
            if v.len() < 2 {
                return Vec::new();
            }
            if !v.windows(2).all(|w| w[0].0 <= w[1].0) {
                v.sort_by_key(|x| x.0);
            }
            let lat0 = v[0].2 as f64 * 1e-6;
            let xy: Vec<[f64; 2]> = v.iter().map(|x| to_xy([x.1 as f64 * 1e-6, x.2 as f64 * 1e-6], lat0)).collect();
            simplify(&xy, 4.0).into_iter().map(|i| [v[i].1, v[i].2]).collect()
        })
        .collect();
    let mut shape_remap = vec![NONE; nshapes_all];
    let mut shape_start: Vec<u32> = vec![0];
    let mut shape_pts: Vec<[i32; 2]> = Vec::new();
    for (i, s) in simplified.iter().enumerate() {
        if s.len() >= 2 {
            shape_remap[i] = (shape_start.len() - 1) as u32;
            shape_pts.extend_from_slice(s);
            shape_start.push(shape_pts.len() as u32);
        }
    }
    drop(simplified);
    for (pi, pt) in patterns.iter_mut().enumerate() {
        let s = pattern_rep_shape[pi];
        pt.shape = if s == NONE { NONE } else { shape_remap[s as usize] };
    }
    // map pattern stops onto shapes
    let pattern_shape_idx: Vec<u32> = {
        let per: Vec<Vec<u32>> = patterns
            .par_iter()
            .map(|pt| {
                let st = &pattern_stops[pt.stops_start as usize..(pt.stops_start + pt.n) as usize];
                if pt.shape == NONE {
                    return vec![NONE; st.len()];
                }
                let a = shape_start[pt.shape as usize] as usize;
                let b = shape_start[pt.shape as usize + 1] as usize;
                let pts = &shape_pts[a..b];
                let lat0 = pts[0][1] as f64 * 1e-6;
                let sxy: Vec<[f64; 2]> =
                    pts.iter().map(|q| to_xy([q[0] as f64 * 1e-6, q[1] as f64 * 1e-6], lat0)).collect();
                let stxy: Vec<[f64; 2]> =
                    st.iter().map(|&s| to_xy([stops[s as usize].lon, stops[s as usize].lat], lat0)).collect();
                map_stops_to_shape(&sxy, &stxy)
            })
            .collect();
        per.into_iter().flatten().collect()
    };
    log::info!("GTFS: {} shapes, {} shape points ({:.1}s)", shape_start.len() - 1, shape_pts.len(), t0.elapsed().as_secs_f64());

    // ------------------------------------------------ stop → patterns
    if !p(0, 0, "GTFS: אינדקסים ומעברים בין תחנות") {
        bail!("cancelled");
    }
    let nstops = stops.len();
    let mut cnt = vec![0u32; nstops + 1];
    for &s in &pattern_stops {
        cnt[s as usize + 1] += 1;
    }
    for i in 0..nstops {
        cnt[i + 1] += cnt[i];
    }
    let stop_pat_start = cnt.clone();
    let mut fill = cnt;
    let mut stop_pat_pattern = vec![0u32; pattern_stops.len()];
    let mut stop_pat_pos = vec![0u32; pattern_stops.len()];
    for (pi, pt) in patterns.iter().enumerate() {
        for pos in 0..pt.n {
            let s = pattern_stops[(pt.stops_start + pos) as usize] as usize;
            let k = fill[s] as usize;
            stop_pat_pattern[k] = pi as u32;
            stop_pat_pos[k] = pos;
            fill[s] += 1;
        }
    }

    // ------------------------------------------------ foot transfers
    if !p(0, 0, if walk.is_some() { "GTFS: מעברים בין תחנות ברשת הדרכים" } else { "GTFS: מעברים בין תחנות" }) {
        bail!("cancelled");
    }
    let served: Vec<bool> = (0..nstops).map(|s| stop_pat_start[s + 1] > stop_pat_start[s]).collect();
    // stops of one parent station are always transfer candidates (up to SIBLING_RADIUS_M)
    let mut siblings: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
    for (i, &par) in stop_parent.iter().enumerate() {
        if par != NONE && served[i] {
            siblings.entry(par).or_default().push(i as u32);
        }
    }
    let cell = 0.005f64;
    let mut grid: FxHashMap<(i32, i32), Vec<u32>> = FxHashMap::default();
    for (i, s) in stops.iter().enumerate() {
        if served[i] {
            grid.entry(((s.lon / cell).floor() as i32, (s.lat / cell).floor() as i32)).or_default().push(i as u32);
        }
    }
    let lists: Vec<Vec<(u32, u32)>> = (0..nstops)
        .into_par_iter()
        .map(|i| {
            if !served[i] {
                return Vec::new();
            }
            let s = &stops[i];
            let (cx, cy) = ((s.lon / cell).floor() as i32, (s.lat / cell).floor() as i32);
            let mut c: Vec<(f64, u32)> = Vec::new();
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(v) = grid.get(&(cx + dx, cy + dy)) {
                        for &j in v {
                            if j as usize == i {
                                continue;
                            }
                            let d = haversine_m([s.lon, s.lat], [stops[j as usize].lon, stops[j as usize].lat]);
                            if d <= TRANSFER_RADIUS_M {
                                c.push((d, j));
                            }
                        }
                    }
                }
            }
            if let Some(sib) = stop_parent.get(i).filter(|&&p| p != NONE).and_then(|p| siblings.get(p)) {
                for &j in sib {
                    if j as usize != i && !c.iter().any(|x| x.1 == j) {
                        let d = haversine_m([s.lon, s.lat], [stops[j as usize].lon, stops[j as usize].lat]);
                        if d <= SIBLING_RADIUS_M {
                            c.push((d, j));
                        }
                    }
                }
            }
            c.sort_by(|a, b| a.0.total_cmp(&b.0));
            c.truncate(TRANSFER_MAX_PER_STOP);
            // walking time on the road network when it is available (None = no path found:
            // keep the straight-line estimate), else straight line at 1.2 m/s
            let road: Vec<Option<f32>> = match walk {
                Some(w) if !c.is_empty() => {
                    let targets: Vec<LonLat> = c.iter().map(|&(_, j)| [stops[j as usize].lon, stops[j as usize].lat]).collect();
                    w([s.lon, s.lat], &targets)
                }
                _ => Vec::new(),
            };
            c.into_iter()
                .enumerate()
                .map(|(k, (d, j))| {
                    let walk_s = road.get(k).copied().flatten().map(|x| x as f64).unwrap_or(d / TRANSFER_SPEED_MPS);
                    (j, (walk_s + TRANSFER_PENALTY_S).round() as u32)
                })
                .collect()
        })
        .collect();
    let mut lists = lists;
    // transfers.txt: type 2 sets the time (from = to: minimum change time at the stop), type 3
    // forbids the transfer; types 0/1 keep the computed walk.
    let mut stop_min_change: Vec<u16> = Vec::new();
    {
        let mut ix: Option<[Option<usize>; 4]> = None;
        let mut rows = 0usize;
        read_table(&mut zip, "transfers.txt", false, |c, r, _, _| {
            let i = *ix.get_or_insert_with(|| [c.idx("from_stop_id"), c.idx("to_stop_id"), c.idx("transfer_type"), c.idx("min_transfer_time")]);
            let get = |k: usize| std::str::from_utf8(field(r, i[k])).ok().and_then(|s| stop_map.get(s).copied());
            let (Some(a), Some(b)) = (get(0), get(1)) else { return Ok(()) };
            let kind = parse_u32(field(r, i[2])).unwrap_or(0);
            let min = parse_u32(field(r, i[3]));
            rows += 1;
            if a == b {
                if let (2, Some(m)) = (kind, min) {
                    if stop_min_change.is_empty() {
                        stop_min_change = vec![0; nstops];
                    }
                    stop_min_change[a as usize] = m.min(u16::MAX as u32) as u16;
                }
                return Ok(());
            }
            let l = &mut lists[a as usize];
            match (kind, min) {
                (3, _) => l.retain(|x| x.0 != b),
                (2, Some(m)) if served[a as usize] && served[b as usize] => match l.iter_mut().find(|x| x.0 == b) {
                    Some(x) => x.1 = m,
                    None => l.push((b, m)),
                },
                _ => {}
            }
            Ok(())
        })?;
        if rows > 0 {
            log::info!("GTFS: transfers.txt: {rows} rules applied");
        }
    }
    let mut tr_start = Vec::with_capacity(nstops + 1);
    let mut tr_to = Vec::new();
    let mut tr_dur = Vec::new();
    tr_start.push(0u32);
    for l in lists {
        for (j, d) in l {
            tr_to.push(j);
            tr_dur.push(d);
        }
        tr_start.push(tr_to.len() as u32);
    }

    let mut t = Transit {
        feed_start,
        num_days,
        strings: strings.strings,
        stops,
        agencies,
        agency_tz,
        stop_parent,
        stop_wheelchair,
        trip_wheelchair,
        stop_min_change,
        routes,
        patterns,
        pattern_stops,
        pattern_flags,
        pattern_shape_idx,
        trip_service,
        trip_headsign,
        dep: if dep_out == arr_out { Vec::new() } else { dep_out },
        arr: arr_out,
        num_services,
        service_words,
        service_bits,
        stop_pat_start,
        stop_pat_pattern,
        stop_pat_pos,
        tr_start,
        tr_to,
        tr_dur,
        shape_start,
        shape_pts,
        ..Default::default()
    };
    t.finalize();
    t.validate().map_err(|e| anyhow!("GTFS: the built model is inconsistent: {e}"))?;
    progress(ProgressPhase::Finalizing, 0, 0, "GTFS: שמירת קובץ הנתונים");
    save(&t, out)?;
    log::info!(
        "GTFS: transit model written: {} stops, {} patterns, {} trips, {} transfers in {:.1}s",
        t.stops.len(),
        t.patterns.len(),
        t.trip_service.len(),
        t.tr_to.len(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

fn total_or(x: u64) -> u64 {
    x.max(1)
}

/// Start times of a frequencies.txt window: `start`, `start + headway`, … before `end`.
fn expand_headway(start: u32, end: u32, headway: u32) -> impl Iterator<Item = u32> {
    (0..).map(move |k: u32| start + k * headway).take_while(move |&t| t < end).take(2000)
}

/// Fill missing (NONE) times by linear interpolation by position. Ensures monotonicity.
/// Returns false if the trip has no usable times.
fn interpolate(a: &mut [u32], d: &mut [u32]) -> bool {
    let n = a.len();
    let known: Vec<usize> = (0..n).filter(|&i| a[i] != NONE).collect();
    if known.is_empty() {
        return false;
    }
    if known.len() < n {
        for i in 0..n {
            if a[i] != NONE {
                continue;
            }
            let prev = known.iter().rev().find(|&&k| k < i).copied();
            let next = known.iter().find(|&&k| k > i).copied();
            let v = match (prev, next) {
                (Some(p), Some(q)) => {
                    let (tp, tq) = (d[p] as f64, a[q] as f64);
                    (tp + (tq - tp) * (i - p) as f64 / (q - p) as f64).round() as u32
                }
                (Some(p), None) => d[p],
                (None, Some(q)) => a[q],
                (None, None) => return false,
            };
            a[i] = v;
            d[i] = v;
        }
    }
    // enforce monotone non-decreasing
    for i in 0..n {
        if i > 0 && a[i] < d[i - 1] {
            a[i] = d[i - 1];
        }
        if d[i] < a[i] {
            d[i] = a[i];
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(parse_time(b"08:00:00"), Some(8 * 3600));
        assert_eq!(parse_time(b" 8:05:30"), Some(8 * 3600 + 330));
        assert_eq!(parse_time(b"25:10:00"), Some(25 * 3600 + 600));
        assert_eq!(parse_time(b""), None);
    }

    #[test]
    fn city() {
        let d = "\u{05E8}\u{05D7}\u{05D5}\u{05D1}: X 5 \u{05E2}\u{05D9}\u{05E8}: \u{05EA}\u{05DC} \u{05D0}\u{05D1}\u{05D9}\u{05D1} \u{05E8}\u{05E6}\u{05D9}\u{05E3}:  ";
        assert_eq!(city_from_desc(d), "\u{05EA}\u{05DC} \u{05D0}\u{05D1}\u{05D9}\u{05D1}");
    }

    #[test]
    fn stop_search_keys() {
        // the geocoder's normalizer folds final letters (ם → מ): stops and queries alike
        assert_eq!(search_key("ת. מרכזית ירושלים/יפו"), "תחנה מרכזית ירושלימ יפו");
        assert_eq!(search_key("תחנה מרכזית ירושלים"), search_key("ת. מרכזית ירושלים"));
        assert_eq!(search_key("ביה\"ס אורט"), "ביהס אורט");
        assert_eq!(search_key("בי\"ס רמב\"ם"), "בית ספר רמבמ");
        assert_eq!(search_key("ביח מאיר"), search_key("בית חולים מאיר"));
    }

    #[test]
    fn interp() {
        let mut a = vec![100, NONE, NONE, 400];
        let mut d = vec![100, NONE, NONE, 400];
        assert!(interpolate(&mut a, &mut d));
        assert_eq!(a, vec![100, 200, 300, 400]);
    }
}

/// The identity of a numbered line among same-numbered lines of one agency, from its GTFS
/// route_long_name. Israeli long names read "<origin stop>-<origin town><-><dest stop>-<dest
/// town>-<direction>#" ("ת. מרכזית/רציפים-אילת<->...-אילת-1#"); both directions and every variant
/// of a line share the pair of towns. Other formats: the whole name.
pub fn line_group(long_name: &str) -> String {
    let Some((a, b)) = long_name.split_once("<->") else { return long_name.trim().to_string() };
    let town = |side: &str| -> String {
        let side = side.trim().trim_end_matches('#');
        // drop the trailing direction / variant number ("-1", "-2א")
        let side = match side.rsplit_once('-') {
            Some((head, tail)) if tail.chars().next().is_some_and(|c| c.is_ascii_digit()) => head,
            _ => side,
        };
        side.rsplit_once('-').map_or(side, |(_, t)| t).trim().to_string()
    };
    let (mut x, mut y) = (town(a), town(b));
    if x > y {
        std::mem::swap(&mut x, &mut y);
    }
    format!("{x}|{y}")
}

#[cfg(test)]
mod line_group_tests {
    use super::line_group;

    #[test]
    fn same_line_both_directions() {
        let a = line_group("האירוסים/פרג-רכסים<->בית ספר בית יעקב/הרב משקובסקי-רכסים-1#");
        let b = line_group("בית ספר בית יעקב/הרב משקובסקי-רכסים<->האירוסים/פרג-רכסים-2#");
        assert_eq!(a, "רכסים|רכסים");
        assert_eq!(a, b);
        let x = line_group("ת. מרכזית חוף הכרמל/רציפים-חיפה<->מסוף אלנבי-תל אביב יפו-1#");
        let y = line_group("מסוף אלנבי-תל אביב יפו<->ת. מרכזית חוף הכרמל/הורדה-חיפה-2#");
        assert_eq!(x, y);
        assert_ne!(x, a);
        assert_eq!(line_group("Line 5"), "Line 5");
    }
}
