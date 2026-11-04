//! Reader of the old bincode `transit.bin` v2, for the data CLI only (`transit-convert`): it
//! turns an installed v2 file into v3 without the GTFS zip (no wheelchair / frequencies /
//! transfers.txt data, straight-line transfers as before). The app itself never reads v2.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::model::{Pattern, RouteInfo, Stop, Transit, MAGIC};

#[derive(Deserialize)]
struct TransitV2 {
    feed_start: i32,
    num_days: u32,
    strings: Vec<String>,
    stops: Vec<Stop>,
    agencies: Vec<u32>,
    routes: Vec<RouteInfo>,
    patterns: Vec<Pattern>,
    pattern_stops: Vec<u32>,
    pattern_flags: Vec<u8>,
    pattern_shape_idx: Vec<u32>,
    trip_service: Vec<u32>,
    trip_headsign: Vec<u32>,
    arr: Vec<u32>,
    dep: Vec<u32>,
    num_services: u32,
    service_words: u32,
    service_bits: Vec<u64>,
    stop_pat_start: Vec<u32>,
    stop_pat_pattern: Vec<u32>,
    stop_pat_pos: Vec<u32>,
    tr_start: Vec<u32>,
    tr_to: Vec<u32>,
    tr_dur: Vec<u32>,
    shape_start: Vec<u32>,
    shape_pts: Vec<[i32; 2]>,
}

/// Load a v2 file into the current model (indexes computed, validated).
pub fn load_v2(path: &Path) -> Result<Transit> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut r = BufReader::with_capacity(1 << 20, f);
    let mut hdr = [0u8; 8];
    r.read_exact(&mut hdr)?;
    if &hdr[..4] != MAGIC || u32::from_le_bytes(hdr[4..8].try_into()?) != 2 {
        bail!("{} is not a transit.bin v2", path.display());
    }
    let v: TransitV2 = bincode::deserialize_from(r).context("transit.bin v2: decode")?;
    let mut t = Transit {
        feed_start: v.feed_start,
        num_days: v.num_days,
        strings: v.strings,
        stops: v.stops,
        agencies: v.agencies,
        routes: v.routes,
        patterns: v.patterns,
        pattern_stops: v.pattern_stops,
        pattern_flags: v.pattern_flags,
        pattern_shape_idx: v.pattern_shape_idx,
        trip_service: v.trip_service,
        trip_headsign: v.trip_headsign,
        arr: v.arr,
        dep: v.dep,
        num_services: v.num_services,
        service_words: v.service_words,
        service_bits: v.service_bits,
        stop_pat_start: v.stop_pat_start,
        stop_pat_pattern: v.stop_pat_pattern,
        stop_pat_pos: v.stop_pat_pos,
        tr_start: v.tr_start,
        tr_to: v.tr_to,
        tr_dur: v.tr_dur,
        shape_start: v.shape_start,
        shape_pts: v.shape_pts,
        ..Default::default()
    };
    t.finalize();
    t.validate().map_err(|e| anyhow::anyhow!("transit.bin v2: {e}"))?;
    Ok(t)
}
