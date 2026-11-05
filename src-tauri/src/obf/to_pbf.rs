//! OBF → OSM PBF for the existing builders (`geocode::build_region`, `routing::build_graph`).
//!
//! What goes in:
//!   - routing ways (detailed tree) → OSM ways with their rule tags and names; node ids are
//!     synthesised so that points with identical (x31, y31) share ONE node (that is what makes
//!     junctions); point types (traffic signals, crossings, barriers …) become node tags;
//!     restrictions → `type=restriction` relations when from / via / to resolve to ways meeting
//!     at a node;
//!   - POIs → nodes with OSM-style tags (`poi::main_tags`), merged by id;
//!   - address: cities / towns / villages / suburbs → `place=*` nodes (unless a POI place of the
//!     same name lies within 3 km), buildings → `addr:housenumber` + `addr:street` +
//!     `addr:city` (+ `addr:postcode`) nodes. Streets need no own nodes: the geocoder indexes
//!     named highway ways, which the routing ways supply. OsmAnd's address street names carry
//!     the suburb ("1ère Avenue (Rohero II)"); the suffix is dropped when it names a place so
//!     `addr:street` matches the way name.
//!
//! Not included: map-section objects (tiles come from them), HH routing, transport, postcode
//! groups (their buildings are duplicates; postcodes come with the buildings).
//!
//! Memory: the route points' keys (`x31 << 32 | y31`, 8 bytes each) are collected, sorted and
//! deduplicated (node id = index + 1, coordinates = the key itself), so there is no hash map
//! and no coordinate table; ways are decoded twice instead of being held.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{bail, Result};
use osmpbf::HeaderBBox;
use rustc_hash::{FxHashMap, FxHashSet};

use super::address::{address_sections, place_kind};
use super::poi::{poi_sections, Poi};
use super::reader::{lat31, lon31, ObfFile};
use super::route::{restriction_name, route_sections, RouteRestriction};
use crate::data::pbf_writer::PbfWriter;
use crate::types::{ProgressFn, ProgressPhase};

#[derive(Debug, Clone, Default)]
pub struct ObfSummary {
    pub route_ways: u64,
    pub route_nodes: u64,
    pub restrictions: u64,
    /// restrictions whose ways could not be matched at a common node
    pub restrictions_skipped: u64,
    pub pois: u64,
    pub places: u64,
    pub addresses: u64,
    /// [west, south, east, north]
    pub bbox: [f64; 4],
    /// suggested display name ("Burundi")
    pub name: String,
    pub pbf_bytes: u64,
}

struct Prog<'a> {
    f: ProgressFn<'a>,
    last: u64,
}
impl Prog<'_> {
    fn at(&mut self, pct: u64, msg: &str) -> Result<()> {
        let pct = pct.min(100);
        if pct != self.last {
            self.last = pct;
            if !(self.f)(ProgressPhase::Processing, pct, 100, msg) {
                bail!("cancelled");
            }
        }
        Ok(())
    }
}

fn key(p: (u32, u32)) -> u64 {
    (u64::from(p.0) << 32) | u64::from(p.1)
}
fn e7(d: f64) -> i32 {
    (d * 1e7).round() as i32
}
/// (lat, lon) in 1e-7 degrees.
fn latlon(x: u32, y: u32) -> (i32, i32) {
    (e7(lat31(y)), e7(lon31(x)))
}
fn keep_tag(k: &str, v: &str) -> bool {
    !k.is_empty() && !v.is_empty() && !k.starts_with("osmand_")
}
fn tags_ref(t: &[(String, String)]) -> impl Iterator<Item = (&str, &str)> {
    t.iter().map(|(k, v)| (k.as_str(), v.as_str()))
}

/// "1ère Avenue (Rohero II)" → "1ère Avenue" when "Rohero II" is a place of the file.
fn plain_street<'a>(name: &'a str, places: &HashSet<String>) -> &'a str {
    if let (Some(open), true) = (name.rfind(" ("), name.ends_with(')')) {
        if places.contains(&name[open + 2..name.len() - 1]) {
            return &name[..open];
        }
    }
    name
}

fn near_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = (a.0 - b.0) * a.1.to_radians().cos();
    dx.hypot(a.1 - b.1) * 111_320.0
}

/// Merge the atoms of one tile that share an id (one atom per category).
fn merge_by_id(atoms: Vec<Poi>) -> Vec<Poi> {
    let mut out: Vec<Poi> = Vec::with_capacity(atoms.len());
    let mut at: HashMap<u64, usize> = HashMap::new();
    for a in atoms {
        if a.id != 0 {
            if let Some(&i) = at.get(&a.id) {
                let m = &mut out[i];
                m.types.extend(a.types);
                m.extra.extend(a.extra);
                m.text.extend(a.text);
                if m.name.is_none() {
                    m.name = a.name;
                }
                if m.name_en.is_none() {
                    m.name_en = a.name_en;
                }
                continue;
            }
            at.insert(a.id, out.len());
        }
        out.push(a);
    }
    out
}

pub fn convert(obf: &ObfFile, out_pbf: &Path, progress: ProgressFn) -> Result<ObfSummary> {
    let t0 = std::time::Instant::now();
    let mut prog = Prog { f: progress, last: u64::MAX };
    let mut sum = ObfSummary { name: obf.display_name(), ..Default::default() };
    prog.at(0, "קורא את רשת הדרכים מקובץ OsmAnd")?;

    // ---- pass A: route points, point tags, restrictions
    let routes = route_sections(obf)?;
    let route_bytes: usize = routes.iter().map(|r| r.span.len()).sum::<usize>().max(1);
    let mut keys: Vec<u64> = Vec::new();
    let mut compact_at = 1usize << 22;
    let mut point_tags: FxHashMap<u64, Vec<(String, String)>> = FxHashMap::default();
    let mut restrictions: Vec<(usize, RouteRestriction)> = Vec::new();
    let mut done_bytes = 0usize;
    for (si, rs) in routes.iter().enumerate() {
        rs.for_each_block(obf, true, &mut |b| {
            for w in &b.ways {
                keys.extend(w.points.iter().map(|p| key(*p)));
                for (idx, types) in &w.point_types {
                    let Some(p) = w.points.get(*idx as usize) else { continue };
                    for t in types {
                        if let Some((k, v)) = rs.rule(*t).filter(|(k, v)| keep_tag(k, v)) {
                            let e = point_tags.entry(key(*p)).or_default();
                            if !e.iter().any(|(a, _)| a == k) {
                                e.push((k.to_string(), v.to_string()));
                            }
                        }
                    }
                }
                for (idx, rule, s) in &w.point_names {
                    let (Some(p), Some((k, _))) = (w.points.get(*idx as usize), rs.rule(*rule)) else { continue };
                    if keep_tag(k, s) {
                        let e = point_tags.entry(key(*p)).or_default();
                        if !e.iter().any(|(a, _)| a == k) {
                            e.push((k.to_string(), s.clone()));
                        }
                    }
                }
            }
            restrictions.extend(b.restrictions.iter().map(|r| (si, *r)));
            if keys.len() >= compact_at {
                keys.sort_unstable();
                keys.dedup();
                compact_at = (keys.len() * 2).max(1 << 22);
            }
            let pos = done_bytes + b.offset.saturating_sub(rs.span.start);
            prog.at((pos * 30 / route_bytes) as u64, "קורא את רשת הדרכים מקובץ OsmAnd")
        })?;
        done_bytes += rs.span.len();
    }
    keys.sort_unstable();
    keys.dedup();
    keys.shrink_to_fit();
    sum.route_nodes = keys.len() as u64;
    let node_id = |p: (u32, u32)| keys.binary_search(&key(p)).ok().map(|i| i as i64 + 1);
    log::info!("obf→pbf pass A {:?}: {} nodes, {} tagged, {} restrictions", t0.elapsed(), keys.len(), point_tags.len(), restrictions.len());

    // ---- header
    let bbox = obf.bbox().or_else(|| {
        let (first, last) = (keys.first()?, keys.last()?);
        let (mut miny, mut maxy) = (u32::MAX, 0);
        for k in &keys {
            miny = miny.min(*k as u32);
            maxy = maxy.max(*k as u32);
        }
        Some([lon31((first >> 32) as u32), lat31(maxy), lon31((last >> 32) as u32), lat31(miny)])
    });
    sum.bbox = bbox.unwrap_or_default();
    let mut w = PbfWriter::create(out_pbf)?;
    let hb = bbox.map(|b| HeaderBBox { left: b[0], bottom: b[1], right: b[2], top: b[3] });
    w.write_header(hb.as_ref(), "MapsOffline obf", None)?;

    // ---- route nodes (ids 1..=N)
    let empty: Vec<(String, String)> = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        let (lat, lon) = latlon((k >> 32) as u32, *k as u32);
        let t = point_tags.get(k).unwrap_or(&empty);
        w.node(i as i64 + 1, lat, lon, tags_ref(t))?;
        if i % (1 << 18) == 0 {
            prog.at(30 + (i as u64 * 15 / keys.len().max(1) as u64), "כותב צמתים")?;
        }
    }
    drop(point_tags);
    let mut next_id = keys.len() as i64 + 1;

    // ---- POIs
    prog.at(45, "נקודות עניין")?;
    let mut seen: FxHashSet<u64> = FxHashSet::default();
    let mut poi_places: Vec<(String, (f64, f64))> = Vec::new();
    for ps in poi_sections(obf)? {
        ps.for_each_box(obf, &mut |atoms| {
            for p in merge_by_id(atoms) {
                if p.id != 0 && !seen.insert(p.id) {
                    continue;
                }
                let tags = ps.osm_tags(&p);
                if tags.is_empty() {
                    continue;
                }
                if tags.iter().any(|(k, _)| k == "place") {
                    if let Some((_, n)) = tags.iter().find(|(k, _)| k == "name") {
                        poi_places.push((n.clone(), (lon31(p.x31), lat31(p.y31))));
                    }
                    sum.places += 1;
                }
                let (lat, lon) = latlon(p.x31, p.y31);
                w.node(next_id, lat, lon, tags_ref(&tags))?;
                next_id += 1;
                sum.pois += 1;
            }
            Ok(())
        })?;
    }
    drop(seen);

    // ---- address: places and buildings
    prog.at(55, "כתובות")?;
    let addrs = address_sections(obf)?;
    let mut place_names: HashSet<String> = HashSet::new();
    for a in &addrs {
        a.for_each_city(obf, false, &mut |c| {
            place_names.insert(c.name);
            Ok(())
        })?;
    }
    // Towns (city / town / village / hamlet) first, then suburbs and neighbourhoods: OsmAnd lists
    // a building under its town AND its suburb; the suburb copy is dropped (it would show the
    // suburb as the address's city). Street → town from the towns' street lists, so the roads
    // below can carry `addr:city` (the geocoder otherwise guesses the nearest town centre, which
    // puts e.g. a Ramat Gan street into Bnei Brak).
    let is_town = |c: &crate::obf::address::City| c.kind.is_some_and(|k| k <= 3);
    let addr_key = |street: &str, n: &str, x: u32, y: u32| (street.to_string(), n.to_string(), x >> 12, y >> 12);
    let mut seen_addr: FxHashSet<(String, String, u32, u32)> = FxHashSet::default();
    let mut street_town: FxHashMap<String, Vec<(String, (f64, f64))>> = FxHashMap::default();
    for towns_pass in [true, false] {
    for a in &addrs {
        a.for_each_city(obf, true, &mut |c| {
            if c.group == 2 {
                return Ok(()); // postcodes
            }
            let town = is_town(&c);
            if town != towns_pass {
                return Ok(());
            }
            if let Some(kind) = c.kind.and_then(place_kind) {
                let pos = (lon31(c.x31), lat31(c.y31));
                let dup = poi_places.iter().any(|(n, q)| *n == c.name && near_m(pos, *q) < 3000.0);
                if !dup && !c.name.is_empty() {
                    let mut t = vec![("place".to_string(), kind.to_string()), ("name".to_string(), c.name.clone())];
                    if let Some(en) = &c.name_en {
                        t.push(("name:en".into(), en.clone()));
                    }
                    for (k, v) in &c.names {
                        if k.starts_with("name:") && !k.contains("admin_level") && !t.iter().any(|(a, _)| a == k) {
                            t.push((k.clone(), v.clone()));
                        }
                    }
                    let (lat, lon) = latlon(c.x31, c.y31);
                    w.node(next_id, lat, lon, tags_ref(&t))?;
                    next_id += 1;
                    sum.places += 1;
                }
            }
            let mut put = |street: Option<&str>, n: &str, x: u32, y: u32, pc: Option<&String>| -> Result<()> {
                if let Some(s) = street {
                    let k = addr_key(s, n, x, y);
                    if !town && seen_addr.contains(&k) {
                        return Ok(()); // the town already has this building
                    }
                    if town {
                        seen_addr.insert(k);
                    }
                }
                let mut t = vec![("addr:housenumber".to_string(), n.to_string())];
                match street {
                    Some(s) => t.push(("addr:street".into(), s.to_string())),
                    None => t.push(("addr:place".into(), c.name.clone())),
                }
                // a suburb is not the address's town: leave it to the geocoder's nearest town
                if street.is_some() && town && !c.name.is_empty() {
                    t.push(("addr:city".into(), c.name.clone()));
                }
                if let Some(pc) = pc {
                    t.push(("addr:postcode".into(), pc.clone()));
                }
                let (lat, lon) = latlon(x, y);
                w.node(next_id, lat, lon, tags_ref(&t))?;
                next_id += 1;
                sum.addresses += 1;
                Ok(())
            };
            for s in &c.streets {
                let sn = plain_street(&s.name, &place_names);
                if town && !c.name.is_empty() && !sn.is_empty() {
                    street_town.entry(sn.to_string()).or_default().push((c.name.clone(), (lon31(s.x31), lat31(s.y31))));
                }
                for b in &s.buildings {
                    put(Some(sn), &b.number, b.x31, b.y31, b.postcode.as_ref())?;
                    if let (Some(n2), Some((x2, y2))) = (&b.number2, b.xy2) {
                        put(Some(sn), n2, x2, y2, b.postcode.as_ref())?;
                    }
                }
            }
            for b in &c.buildings {
                put(None, &b.number, b.x31, b.y31, b.postcode.as_ref())?;
            }
            Ok(())
        })?;
    }
    }
    drop(seen_addr);
    // the town of a named road: the nearest street of that name in a town's list (≤ 3 km)
    let town_of = |name: &str, at: (f64, f64)| -> Option<&str> {
        street_town
            .get(name)?
            .iter()
            .map(|(t, p)| (t, near_m(at, *p)))
            .filter(|(_, d)| *d < 3000.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(t, _)| t.as_str())
    };

    // ---- pass B: ways
    prog.at(60, "כותב דרכים")?;
    let needed: FxHashSet<(usize, i64)> = restrictions.iter().flat_map(|(s, r)| [Some((*s, r.from)), Some((*s, r.to)), r.via.map(|v| (*s, v))]).flatten().collect();
    let mut pieces: FxHashMap<(usize, i64), Vec<(i64, Vec<i64>)>> = FxHashMap::default();
    let mut way_id = 1i64;
    let mut done_bytes = 0usize;
    for (si, rs) in routes.iter().enumerate() {
        rs.for_each_block(obf, true, &mut |b| {
            for rw in &b.ways {
                let mut refs: Vec<i64> = Vec::with_capacity(rw.points.len());
                for p in &rw.points {
                    let Some(id) = node_id(*p) else { continue };
                    if refs.last() != Some(&id) {
                        refs.push(id);
                    }
                }
                if refs.len() < 2 {
                    continue;
                }
                let mut tags: Vec<(&str, &str)> = Vec::with_capacity(rw.types.len() + rw.names.len());
                for t in &rw.types {
                    if let Some((k, v)) = rs.rule(*t).filter(|(k, v)| keep_tag(k, v)) {
                        if !tags.iter().any(|(a, _)| *a == k) {
                            tags.push((k, v));
                        }
                    }
                }
                for (r, s) in &rw.names {
                    if let Some((k, _)) = rs.rule(*r).filter(|(k, _)| keep_tag(k, s)) {
                        if !tags.iter().any(|(a, _)| *a == k) {
                            tags.push((k, s.as_str()));
                        }
                    }
                }
                if let Some(name) = tags.iter().find(|(k, _)| *k == "name").map(|(_, v)| *v) {
                    let mid = rw.points[rw.points.len() / 2];
                    if let Some(t) = town_of(name, (lon31(mid.0), lat31(mid.1))) {
                        tags.push(("addr:city", t));
                    }
                }
                w.way(way_id, refs.iter().copied(), tags.into_iter())?;
                if needed.contains(&(si, rw.id)) {
                    pieces.entry((si, rw.id)).or_default().push((way_id, refs));
                }
                way_id += 1;
            }
            let pos = done_bytes + b.offset.saturating_sub(rs.span.start);
            prog.at(60 + (pos * 35 / route_bytes) as u64, "כותב דרכים")
        })?;
        done_bytes += rs.span.len();
    }
    sum.route_ways = (way_id - 1) as u64;

    // ---- restrictions
    prog.at(95, "הגבלות פנייה")?;
    let mut rel_id = 1i64;
    let none: Vec<(i64, Vec<i64>)> = Vec::new();
    for (si, r) in &restrictions {
        let Some(kind) = restriction_name(r.kind) else {
            sum.restrictions_skipped += 1;
            continue;
        };
        let froms = pieces.get(&(*si, r.from)).unwrap_or(&none);
        let tos = pieces.get(&(*si, r.to)).unwrap_or(&none);
        let mut members: Option<Vec<(u8, i64, &str)>> = None;
        match r.via {
            None if r.from != r.to => {
                'outer: for (fw, fr) in froms {
                    for (tw, tr) in tos {
                        let ts: HashSet<i64> = tr.iter().copied().collect();
                        // a shared end of either way first, then any shared node
                        let ends = [fr[0], fr[fr.len() - 1], tr[0], tr[tr.len() - 1]];
                        let via = ends.iter().copied().find(|n| ts.contains(n) && fr.contains(n)).or_else(|| fr.iter().copied().find(|n| ts.contains(n)));
                        if let Some(n) = via {
                            members = Some(vec![(1, *fw, "from"), (0, n, "via"), (1, *tw, "to")]);
                            break 'outer;
                        }
                    }
                }
            }
            Some(v) => {
                let vias = pieces.get(&(*si, v)).unwrap_or(&none);
                let meets = |a: &[i64], b: &[i64]| a.iter().any(|n| b.contains(n));
                'outer2: for (fw, fr) in froms {
                    for (vw, vr) in vias {
                        if !meets(fr, vr) {
                            continue;
                        }
                        for (tw, tr) in tos {
                            if meets(vr, tr) {
                                members = Some(vec![(1, *fw, "from"), (1, *vw, "via"), (1, *tw, "to")]);
                                break 'outer2;
                            }
                        }
                    }
                }
            }
            None => {}
        }
        match members {
            Some(m) => {
                w.relation(rel_id, m.into_iter(), [("type", "restriction"), ("restriction", kind)].into_iter())?;
                rel_id += 1;
                sum.restrictions += 1;
            }
            None => sum.restrictions_skipped += 1,
        }
    }
    w.finish()?;
    sum.pbf_bytes = std::fs::metadata(out_pbf).map(|m| m.len()).unwrap_or(0);
    prog.at(100, "הסתיים")?;
    log::info!("obf→pbf {:?}: {sum:?}", t0.elapsed());
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obf::reader::{x31, y31};
    use osmpbf::{Element, ElementReader, RelMemberType};

    fn v(o: &mut Vec<u8>, x: u64) {
        crate::data::pbf_writer::varint(o, x);
    }
    fn zz(x: i64) -> u64 {
        crate::data::pbf_writer::zigzag(x)
    }
    fn int(o: &mut Vec<u8>, f: u32, x: u64) {
        v(o, u64::from(f << 3));
        v(o, x);
    }
    fn bytes(o: &mut Vec<u8>, f: u32, b: &[u8]) {
        v(o, u64::from(f << 3 | 2));
        v(o, b.len() as u64);
        o.extend_from_slice(b);
    }
    fn packed(xs: &[u64]) -> Vec<u8> {
        let mut o = Vec::new();
        for x in xs {
            v(&mut o, *x);
        }
        o
    }

    /// A one-block routing section: way A (3 points, a traffic signal in the middle, a name),
    /// way B starting at A's middle point, and "no left turn" from A to B.
    fn synthetic_obf() -> Vec<u8> {
        let (l, t) = (x31(29.36) & !31, y31(-3.38) & !31);
        let mut block = Vec::new();
        let mut ids = Vec::new();
        int(&mut ids, 1, zz(6400));
        int(&mut ids, 1, zz(64));
        bytes(&mut block, 5, &ids);
        let mut a = Vec::new();
        bytes(&mut a, 1, &packed(&[zz(0), zz(0), zz(100), zz(0), zz(100), zz(0)]));
        bytes(&mut a, 4, &packed(&[1, 1, 3]));
        bytes(&mut a, 7, &packed(&[1]));
        int(&mut a, 12, 0);
        bytes(&mut a, 14, &packed(&[2, 0]));
        bytes(&mut block, 6, &a);
        let mut b = Vec::new();
        bytes(&mut b, 1, &packed(&[zz(100), zz(0), zz(0), zz(100)]));
        bytes(&mut b, 7, &packed(&[1, 4]));
        int(&mut b, 12, 1);
        bytes(&mut block, 6, &b);
        let mut r = Vec::new();
        int(&mut r, 1, 2);
        int(&mut r, 2, 0);
        int(&mut r, 3, 1);
        bytes(&mut block, 7, &r);
        let mut st = Vec::new();
        bytes(&mut st, 1, "Main St".as_bytes());
        bytes(&mut block, 8, &st);

        let mut sec = Vec::new();
        bytes(&mut sec, 1, b"Test");
        for (k, val) in [("highway", "residential"), ("name", ""), ("highway", "traffic_signals"), ("oneway", "yes")] {
            let mut rule = Vec::new();
            bytes(&mut rule, 3, k.as_bytes());
            bytes(&mut rule, 5, val.as_bytes());
            bytes(&mut sec, 2, &rule);
        }
        let mut bx = Vec::new();
        int(&mut bx, 1, zz(i64::from(l)));
        int(&mut bx, 2, zz(i64::from(l) + 100_000));
        int(&mut bx, 3, zz(i64::from(t)));
        int(&mut bx, 4, zz(i64::from(t) + 100_000));
        // field 5 shift (wire 5, BE), patched below
        bx.push((5 << 3) | 5);
        let shift_at = bx.len();
        bx.extend([0, 0, 0, 0]);
        // section field 3 (wire 6): tag, 4-byte length, body
        sec.push((3 << 3) | 6);
        sec.extend((bx.len() as u32).to_be_bytes());
        let box_body = sec.len();
        sec.extend(&bx);
        // section field 5: the block, its varint length right after the tag
        sec.push((5 << 3) | 2);
        let shift = (sec.len() - box_body) as u32;
        sec[box_body + shift_at..box_body + shift_at + 4].copy_from_slice(&shift.to_be_bytes());
        v(&mut sec, block.len() as u64);
        sec.extend(&block);

        let mut file = Vec::new();
        int(&mut file, 1, 2);
        file.push((9 << 3) | 6);
        file.extend((sec.len() as u32).to_be_bytes());
        file.extend(&sec);
        int(&mut file, 32, 2);
        file
    }

    #[test]
    fn synthetic_routing_to_pbf() {
        let obf = ObfFile::from_bytes(synthetic_obf()).unwrap();
        let dir = std::env::temp_dir().join(format!("mo-obf-syn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pbf = dir.join("t.pbf");
        let s = convert(&obf, &pbf, &|_, _, _, _| true).unwrap();
        assert_eq!((s.route_nodes, s.route_ways, s.restrictions), (4, 2, 1), "{s:?}");
        let (mut nodes, mut ways, mut rels) = (Vec::new(), Vec::new(), Vec::new());
        ElementReader::from_path(&pbf)
            .unwrap()
            .for_each(|el| match el {
                Element::DenseNode(n) => nodes.push((n.id(), n.lon(), n.lat(), n.tags().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>())),
                Element::Way(w) => ways.push((w.id(), w.refs().collect::<Vec<_>>(), w.tags().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>())),
                Element::Relation(r) => rels.push((r.members().map(|m| { let role = m.role().unwrap().to_string(); (m.member_type, m.member_id, role) }).collect::<Vec<_>>(), r.tags().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>())),
                _ => {}
            })
            .unwrap();
        assert_eq!(nodes.len(), 4);
        assert!(nodes.windows(2).all(|w| w[0].0 < w[1].0));
        assert!((nodes[0].1 - 29.36).abs() < 1e-3 && (nodes[0].2 + 3.38).abs() < 1e-3);
        let signal = nodes.iter().find(|n| n.3 == ["highway=traffic_signals"]).unwrap().0;
        assert_eq!(ways[0].1.len(), 3);
        assert_eq!(ways[0].2, ["highway=residential", "name=Main St"]);
        assert_eq!(ways[1].2, ["highway=residential", "oneway=yes"]);
        // the junction: A's middle point is B's first point, one node
        assert_eq!(ways[0].1[1], signal);
        assert_eq!(ways[1].1[0], signal);
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].0, vec![(RelMemberType::Way, 1, "from".into()), (RelMemberType::Node, signal, "via".into()), (RelMemberType::Way, 2, "to".into())]);
        assert_eq!(rels[0].1, ["type=restriction", "restriction=no_left_turn"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn street_suffix() {
        let places: HashSet<String> = ["Rohero II".to_string()].into();
        assert_eq!(plain_street("1ère Avenue (Rohero II)", &places), "1ère Avenue");
        assert_eq!(plain_street("Rue (X)", &places), "Rue (X)");
        assert_eq!(plain_street("Rue", &places), "Rue");
    }

    /// Real data: `cargo test --release --features cli --lib obf -- --ignored --nocapture`
    /// (MO_OBF_FILE=<.obf or .obf.zip>, default: the Burundi download).
    #[test]
    #[ignore]
    fn burundi_end_to_end() {
        use crate::types::{GeoKind, Mode, RouteRequest, Waypoint};
        use std::time::Instant;
        let src = std::env::var("MO_OBF_FILE").unwrap_or_else(|_| r"C:\Users\איצקוביץ\Downloads\Compressed\Burundi_africa_2.obf.zip".into());
        let dir = std::env::temp_dir().join("mo-obf-e2e");
        std::fs::create_dir_all(&dir).unwrap();
        let mb = |p: &Path| std::fs::metadata(p).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0);
        let quiet = |_: ProgressPhase, _: u64, _: u64, _: &str| true;

        let t = Instant::now();
        let obf = ObfFile::open_auto(Path::new(&src), &dir).unwrap();
        println!("open {:?}: version {} sections {:?} bbox {:?} name {}", t.elapsed(), obf.version, obf.sections.iter().map(|s| s.kind).collect::<Vec<_>>(), obf.bbox(), obf.display_name());
        let pbf = dir.join("burundi.osm.pbf");
        let t = Instant::now();
        let s = convert(&obf, &pbf, &quiet).unwrap();
        println!("convert {:?}: {s:?} ({:.1} MB)", t.elapsed(), mb(&pbf));
        assert!(s.route_ways > 100_000 && s.pois > 5_000 && s.places > 100 && s.addresses > 100, "{s:?}");
        drop(obf);

        let geo = dir.join("geocode.sqlite");
        let _ = std::fs::remove_file(&geo);
        let t = Instant::now();
        crate::geocode::build_region(&pbf, &geo, -3.4, &quiet).unwrap();
        println!("geocode build {:?} ({:.1} MB)", t.elapsed(), mb(&geo));
        let graph = dir.join("graph.bin");
        let t = Instant::now();
        crate::routing::build_graph(&pbf, &graph, &quiet).unwrap();
        println!("graph build {:?} ({:.1} MB)", t.elapsed(), mb(&graph));

        let g = crate::geocode::open(&geo).unwrap();
        let bujumbura = |r: &crate::types::GeoResult| (29.30..29.45).contains(&r.lon) && (-3.45..-3.30).contains(&r.lat);
        let near = Some([29.36, -3.38]);
        for (q, kind) in [("Bujumbura", GeoKind::Place), ("Boulevard de l'Uprona", GeoKind::Street), ("Rohero", GeoKind::Place), ("Avenue de l'Université", GeoKind::Street)] {
            let r = g.search(q, near, 5).unwrap();
            println!("{q} -> {:?}", r.iter().map(|x| (x.kind, x.name.clone(), x.lon, x.lat)).collect::<Vec<_>>());
            assert!(r.iter().any(|x| x.kind == kind && bujumbura(x)), "{q}");
        }
        for q in ["בית חולים", "מסעדה", "תחנת דלק", "בנק", "כנסייה"] {
            let r = g.search(q, near, 5).unwrap();
            println!("{q} -> {:?}", r.iter().map(|x| (x.kind, x.name.clone(), x.category.clone())).collect::<Vec<_>>());
            assert!(r.iter().any(|x| x.kind == GeoKind::Poi && bujumbura(x)), "{q}");
        }
        let r = g.search("10ème Avenue 67", near, 5).unwrap();
        println!("address -> {:?}", r.iter().map(|x| (x.kind, x.name.clone(), x.lon, x.lat)).collect::<Vec<_>>());

        let gr = crate::routing::load_graph(&graph).unwrap();
        let req = RouteRequest {
            waypoints: vec![Waypoint { lon: 29.3600, lat: -3.3830, name: None }, Waypoint { lon: 29.3780, lat: -3.3620, name: None }],
            mode: Mode::Car,
            alternatives: 1,
            ..Default::default()
        };
        let t = Instant::now();
        let resp = gr.route(&req).unwrap();
        let r = &resp.routes[0];
        println!("route {:?}: {:.0} m {:.0} s", t.elapsed(), r.distance_m, r.duration_sec);
        assert!(r.distance_m > 2500.0 && r.distance_m < 9000.0);
    }
}
