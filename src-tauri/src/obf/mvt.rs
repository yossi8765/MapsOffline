//! Minimal Mapbox Vector Tile (MVT 2.1) encoder: layers with deduplicated keys / values,
//! geometry command streams (MoveTo / LineTo / ClosePath, zigzag deltas, cursor reset per
//! feature), polygon winding per spec (exterior ring positive area = clockwise on screen,
//! interior rings the other way) and dropping of degenerate geometry.
//!
//! Features are built in two steps so the tile builder can spill them to disk: the geometry is
//! encoded first ([`encode_points`], [`encode_lines`], [`encode_polygons`]) into a [`Feature`],
//! and a whole tile is assembled later ([`encode_tile`]).

use std::collections::HashMap;

pub const EXTENT: u32 = 4096;

pub const GEOM_POINT: u8 = 1;
pub const GEOM_LINE: u8 = 2;
pub const GEOM_POLYGON: u8 = 3;

/// A property value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

#[derive(Hash, PartialEq, Eq)]
enum ValueKey {
    Str(String),
    Int(i64),
    Float(u64),
    Bool(bool),
}

impl ValueKey {
    fn of(v: &Value) -> Self {
        match v {
            Value::Str(s) => ValueKey::Str(s.clone()),
            Value::Int(i) => ValueKey::Int(*i),
            Value::Float(f) => ValueKey::Float(f.to_bits()),
            Value::Bool(b) => ValueKey::Bool(*b),
        }
    }
}

/// One feature with its geometry already encoded as MVT commands.
#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    /// [`GEOM_POINT`], [`GEOM_LINE`] or [`GEOM_POLYGON`]
    pub geom_type: u8,
    pub geometry: Vec<u32>,
    pub props: Vec<(String, Value)>,
}

// ---------------------------------------------------------------- protobuf helpers

pub(crate) fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            buf.push(b);
            return;
        }
        buf.push(b | 0x80);
    }
}

fn put_key(buf: &mut Vec<u8>, field: u32, wire: u32) {
    put_varint(buf, u64::from((field << 3) | wire));
}

fn put_bytes(buf: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    put_key(buf, field, 2);
    put_varint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

fn put_packed(buf: &mut Vec<u8>, field: u32, vals: &[u32]) {
    let mut tmp = Vec::with_capacity(vals.len() * 2);
    for &v in vals {
        put_varint(&mut tmp, u64::from(v));
    }
    put_bytes(buf, field, &tmp);
}

#[inline]
fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

#[inline]
fn command(id: u32, count: u32) -> u32 {
    (id & 7) | (count << 3)
}

// ---------------------------------------------------------------- geometry

/// Points (MultiPoint when more than one). Empty input → empty geometry.
pub fn encode_points(pts: &[(i32, i32)]) -> Vec<u32> {
    if pts.is_empty() {
        return Vec::new();
    }
    let mut g = Vec::with_capacity(1 + pts.len() * 2);
    g.push(command(1, pts.len() as u32));
    let (mut cx, mut cy) = (0i32, 0i32);
    for &(x, y) in pts {
        g.push(zigzag(x - cx));
        g.push(zigzag(y - cy));
        cx = x;
        cy = y;
    }
    g
}

fn dedup_consecutive(pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut v: Vec<(i32, i32)> = Vec::with_capacity(pts.len());
    for &p in pts {
        if v.last() != Some(&p) {
            v.push(p);
        }
    }
    v
}

/// Lines (MultiLineString when more than one); lines with fewer than 2 distinct points are dropped.
pub fn encode_lines(lines: &[Vec<(i32, i32)>]) -> Vec<u32> {
    let mut g = Vec::new();
    let (mut cx, mut cy) = (0i32, 0i32);
    for line in lines {
        let l = dedup_consecutive(line);
        if l.len() < 2 {
            continue;
        }
        g.push(command(1, 1));
        g.push(zigzag(l[0].0 - cx));
        g.push(zigzag(l[0].1 - cy));
        cx = l[0].0;
        cy = l[0].1;
        g.push(command(2, (l.len() - 1) as u32));
        for &(x, y) in &l[1..] {
            g.push(zigzag(x - cx));
            g.push(zigzag(y - cy));
            cx = x;
            cy = y;
        }
    }
    g
}

/// Twice the signed area by the surveyor's formula in tile coordinates (y down):
/// positive = clockwise on screen = MVT exterior ring.
pub fn ring_area2(r: &[(i32, i32)]) -> i64 {
    let n = r.len();
    if n < 3 {
        return 0;
    }
    let mut s = 0i64;
    for i in 0..n {
        let (x1, y1) = r[i];
        let (x2, y2) = r[(i + 1) % n];
        s += i64::from(x1) * i64::from(y2) - i64::from(x2) * i64::from(y1);
    }
    s
}

/// Clean one ring: drop the closing point and repeated points; `None` when degenerate.
fn clean_ring(r: &[(i32, i32)]) -> Option<Vec<(i32, i32)>> {
    let mut v = dedup_consecutive(r);
    while v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    // remove spikes (a, b, a) that leave zero-area slivers
    let mut changed = true;
    while changed && v.len() >= 3 {
        changed = false;
        let n = v.len();
        for i in 0..n {
            if v[(i + n - 1) % n] == v[(i + 1) % n] {
                let hi = i.max((i + 1) % n);
                let lo = i.min((i + 1) % n);
                v.remove(hi);
                v.remove(lo);
                changed = true;
                break;
            }
        }
    }
    (v.len() >= 3 && ring_area2(&v) != 0).then_some(v)
}

/// Polygons (MultiPolygon when more than one): each polygon is its exterior ring followed by
/// its holes. Rings may be open or closed; windings are fixed here; polygons whose exterior
/// is degenerate are dropped together with their holes.
pub fn encode_polygons(polys: &[Vec<Vec<(i32, i32)>>]) -> Vec<u32> {
    let mut g = Vec::new();
    let (mut cx, mut cy) = (0i32, 0i32);
    for poly in polys {
        let mut first = true;
        for ring in poly {
            let exterior = first;
            let Some(mut r) = clean_ring(ring) else {
                if exterior {
                    break; // no exterior → skip the holes too
                }
                continue;
            };
            first = false;
            let a = ring_area2(&r);
            if (exterior && a < 0) || (!exterior && a > 0) {
                r.reverse();
            }
            g.push(command(1, 1));
            g.push(zigzag(r[0].0 - cx));
            g.push(zigzag(r[0].1 - cy));
            cx = r[0].0;
            cy = r[0].1;
            g.push(command(2, (r.len() - 1) as u32));
            for &(x, y) in &r[1..] {
                g.push(zigzag(x - cx));
                g.push(zigzag(y - cy));
                cx = x;
                cy = y;
            }
            g.push(command(7, 1));
        }
    }
    g
}

// ---------------------------------------------------------------- tile

struct LayerBuilder<'a> {
    keys: Vec<&'a str>,
    key_idx: HashMap<&'a str, u32>,
    values: Vec<&'a Value>,
    val_idx: HashMap<ValueKey, u32>,
    features: Vec<u8>,
}

fn encode_value(v: &Value) -> Vec<u8> {
    let mut b = Vec::new();
    match v {
        Value::Str(s) => put_bytes(&mut b, 1, s.as_bytes()),
        Value::Float(f) => {
            put_key(&mut b, 3, 1);
            b.extend_from_slice(&f.to_le_bytes());
        }
        Value::Int(i) => {
            if *i >= 0 {
                put_key(&mut b, 5, 0);
                put_varint(&mut b, *i as u64);
            } else {
                put_key(&mut b, 6, 0);
                put_varint(&mut b, ((*i << 1) ^ (*i >> 63)) as u64);
            }
        }
        Value::Bool(x) => {
            put_key(&mut b, 7, 0);
            put_varint(&mut b, u64::from(*x));
        }
    }
    b
}

/// Encode a tile: `(layer name, features)` in drawing order; empty layers and features with an
/// empty geometry are left out. Returns the raw (uncompressed) protobuf.
pub fn encode_tile(layers: &[(&str, &[Feature])]) -> Vec<u8> {
    let mut tile = Vec::new();
    for (name, feats) in layers {
        let mut lb = LayerBuilder { keys: Vec::new(), key_idx: HashMap::new(), values: Vec::new(), val_idx: HashMap::new(), features: Vec::new() };
        let mut count = 0usize;
        for f in feats.iter() {
            if f.geometry.is_empty() {
                continue;
            }
            let mut tags = Vec::with_capacity(f.props.len() * 2);
            for (k, v) in &f.props {
                let ki = *lb.key_idx.entry(k.as_str()).or_insert_with(|| {
                    lb.keys.push(k.as_str());
                    (lb.keys.len() - 1) as u32
                });
                let vk = ValueKey::of(v);
                let vi = match lb.val_idx.get(&vk) {
                    Some(&i) => i,
                    None => {
                        lb.values.push(v);
                        let i = (lb.values.len() - 1) as u32;
                        lb.val_idx.insert(vk, i);
                        i
                    }
                };
                tags.push(ki);
                tags.push(vi);
            }
            let mut fb = Vec::with_capacity(f.geometry.len() * 2 + tags.len() + 8);
            put_packed(&mut fb, 2, &tags);
            put_key(&mut fb, 3, 0);
            put_varint(&mut fb, u64::from(f.geom_type));
            put_packed(&mut fb, 4, &f.geometry);
            put_bytes(&mut lb.features, 2, &fb);
            count += 1;
        }
        if count == 0 {
            continue;
        }
        let mut lbuf = Vec::with_capacity(lb.features.len() + 256);
        put_key(&mut lbuf, 15, 0);
        put_varint(&mut lbuf, 2);
        put_bytes(&mut lbuf, 1, name.as_bytes());
        lbuf.extend_from_slice(&lb.features);
        for k in &lb.keys {
            put_bytes(&mut lbuf, 3, k.as_bytes());
        }
        for v in &lb.values {
            put_bytes(&mut lbuf, 4, &encode_value(v));
        }
        put_key(&mut lbuf, 5, 0);
        put_varint(&mut lbuf, u64::from(EXTENT));
        put_bytes(&mut tile, 3, &lbuf);
    }
    tile
}

// ---------------------------------------------------------------- decoder (tests / verification)

/// A decoded feature: geometry as parts of absolute points (one part per MoveTo).
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub geom_type: u8,
    pub parts: Vec<Vec<(i32, i32)>>,
    pub props: Vec<(String, Value)>,
}

fn get_varint(b: &[u8], p: &mut usize) -> anyhow::Result<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let byte = *b.get(*p).ok_or_else(|| anyhow::anyhow!("truncated varint"))?;
        *p += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
        shift += 7;
        anyhow::ensure!(shift < 64, "varint too long");
    }
}

/// Iterate over (field, wire, payload-or-varint) of a message.
fn fields(b: &[u8]) -> anyhow::Result<Vec<(u32, u32, &[u8], u64)>> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < b.len() {
        let key = get_varint(b, &mut p)?;
        let (field, wire) = ((key >> 3) as u32, (key & 7) as u32);
        match wire {
            0 => {
                let v = get_varint(b, &mut p)?;
                out.push((field, 0, &b[0..0], v));
            }
            1 => {
                anyhow::ensure!(p + 8 <= b.len(), "truncated fixed64");
                out.push((field, 1, &b[p..p + 8], 0));
                p += 8;
            }
            2 => {
                let n = get_varint(b, &mut p)? as usize;
                anyhow::ensure!(p + n <= b.len(), "truncated bytes");
                out.push((field, 2, &b[p..p + n], 0));
                p += n;
            }
            5 => {
                anyhow::ensure!(p + 4 <= b.len(), "truncated fixed32");
                out.push((field, 5, &b[p..p + 4], 0));
                p += 4;
            }
            w => anyhow::bail!("unsupported wire type {w}"),
        }
    }
    Ok(out)
}

fn packed(b: &[u8]) -> anyhow::Result<Vec<u32>> {
    let mut v = Vec::new();
    let mut p = 0;
    while p < b.len() {
        v.push(get_varint(b, &mut p)? as u32);
    }
    Ok(v)
}

/// Decode geometry commands into absolute parts.
pub fn decode_geometry(g: &[u32]) -> anyhow::Result<Vec<Vec<(i32, i32)>>> {
    let mut parts: Vec<Vec<(i32, i32)>> = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    let mut i = 0;
    let unzig = |v: u32| ((v >> 1) as i32) ^ -((v & 1) as i32);
    while i < g.len() {
        let (id, count) = (g[i] & 7, g[i] >> 3);
        i += 1;
        match id {
            1 | 2 => {
                for _ in 0..count {
                    anyhow::ensure!(i + 2 <= g.len(), "truncated geometry");
                    let (dx, dy) = (unzig(g[i]), unzig(g[i + 1]));
                    i += 2;
                    x += dx;
                    y += dy;
                    if id == 1 {
                        parts.push(vec![(x, y)]);
                    } else {
                        parts.last_mut().ok_or_else(|| anyhow::anyhow!("LineTo before MoveTo"))?.push((x, y));
                    }
                }
            }
            7 => {}
            c => anyhow::bail!("bad command {c}"),
        }
    }
    Ok(parts)
}

/// Decode a raw (uncompressed) tile into `(layer name, extent, features)`.
pub fn decode_tile(b: &[u8]) -> anyhow::Result<Vec<(String, u32, Vec<Decoded>)>> {
    let mut out = Vec::new();
    for (f, w, layer, _) in fields(b)? {
        if f != 3 || w != 2 {
            continue;
        }
        let mut name = String::new();
        let mut extent = 4096;
        let mut keys = Vec::new();
        let mut values = Vec::new();
        let mut raw_feats = Vec::new();
        for (lf, _, payload, v) in fields(layer)? {
            match lf {
                1 => name = String::from_utf8(payload.to_vec())?,
                2 => raw_feats.push(payload),
                3 => keys.push(String::from_utf8(payload.to_vec())?),
                4 => {
                    let mut val = Value::Bool(false);
                    for (vf, _, vp, vv) in fields(payload)? {
                        val = match vf {
                            1 => Value::Str(String::from_utf8(vp.to_vec())?),
                            2 => Value::Float(f64::from(f32::from_le_bytes(vp.try_into()?))),
                            3 => Value::Float(f64::from_le_bytes(vp.try_into()?)),
                            4 => Value::Int(vv as i64),
                            5 => Value::Int(vv as i64),
                            6 => Value::Int(((vv >> 1) as i64) ^ -((vv & 1) as i64)),
                            7 => Value::Bool(vv != 0),
                            _ => val,
                        };
                    }
                    values.push(val);
                }
                5 => extent = v as u32,
                _ => {}
            }
        }
        let mut feats = Vec::new();
        for fb in raw_feats {
            let mut gt = 0u8;
            let mut tags = Vec::new();
            let mut geom = Vec::new();
            for (ff, _, payload, v) in fields(fb)? {
                match ff {
                    2 => tags = packed(payload)?,
                    3 => gt = v as u8,
                    4 => geom = packed(payload)?,
                    _ => {}
                }
            }
            let mut props = Vec::new();
            for kv in tags.chunks(2) {
                if let [k, v] = kv {
                    let key = keys.get(*k as usize).ok_or_else(|| anyhow::anyhow!("bad key index"))?.clone();
                    let val = values.get(*v as usize).ok_or_else(|| anyhow::anyhow!("bad value index"))?.clone();
                    props.push((key, val));
                }
            }
            feats.push(Decoded { geom_type: gt, parts: decode_geometry(&geom)?, props });
        }
        out.push((name, extent, feats));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop<'a>(d: &'a Decoded, k: &str) -> Option<&'a Value> {
        d.props.iter().find(|(kk, _)| kk == k).map(|(_, v)| v)
    }

    #[test]
    fn spec_example_geometries() {
        // MVT spec 4.3.5 examples
        assert_eq!(encode_points(&[(25, 17)]), vec![9, 50, 34]);
        assert_eq!(encode_points(&[(5, 7), (3, 2)]), vec![17, 10, 14, 3, 9]);
        assert_eq!(encode_lines(&[vec![(2, 2), (2, 10), (10, 10)]]), vec![9, 4, 4, 18, 0, 16, 16, 0]);
        assert_eq!(
            encode_lines(&[vec![(2, 2), (2, 10), (10, 10)], vec![(1, 1), (3, 5)]]),
            vec![9, 4, 4, 18, 0, 16, 16, 0, 9, 17, 17, 10, 4, 8]
        );
        // polygon (3,6) (8,12) (20,34): spec says exterior = positive area
        let g = encode_polygons(&[vec![vec![(3, 6), (8, 12), (20, 34), (3, 6)]]]);
        assert_eq!(g, vec![9, 6, 12, 18, 10, 12, 24, 44, 15]);
    }

    #[test]
    fn winding_is_fixed_and_degenerates_dropped() {
        // counter-clockwise on screen exterior gets reversed
        let ccw = vec![(0, 0), (0, 10), (10, 10), (10, 0)];
        let hole_cw = vec![(2, 2), (8, 2), (8, 8), (2, 8)];
        let g = encode_polygons(&[vec![ccw, hole_cw]]);
        let parts = decode_geometry(&g).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(ring_area2(&parts[0]) > 0);
        assert!(ring_area2(&parts[1]) < 0);
        // a flat ring and a 2-point line vanish
        assert!(encode_polygons(&[vec![vec![(0, 0), (5, 0), (10, 0)]]]).is_empty());
        assert!(encode_lines(&[vec![(1, 1), (1, 1)]]).is_empty());
        // spike removal
        let g = encode_polygons(&[vec![vec![(0, 0), (10, 0), (20, 0), (10, 0), (10, 10)]]]);
        let parts = decode_geometry(&g).unwrap();
        assert_eq!(parts[0].len(), 3);
    }

    #[test]
    fn tile_roundtrip_with_dedup() {
        let f1 = Feature { geom_type: GEOM_POINT, geometry: encode_points(&[(100, 200)]), props: vec![("kind".into(), "cafe".into()), ("min_zoom".into(), Value::Int(15)), ("neg".into(), Value::Int(-3))] };
        let f2 = Feature { geom_type: GEOM_LINE, geometry: encode_lines(&[vec![(0, 0), (4096, 4096)]]), props: vec![("kind".into(), "cafe".into()), ("h".into(), Value::Float(2.5)), ("b".into(), Value::Bool(true))] };
        let empty = Feature { geom_type: GEOM_LINE, geometry: vec![], props: vec![] };
        let feats = vec![f1, f2, empty];
        let t = encode_tile(&[("pois", &feats), ("nothing", &[])]);
        let d = decode_tile(&t).unwrap();
        assert_eq!(d.len(), 1);
        let (name, extent, fs) = &d[0];
        assert_eq!((name.as_str(), *extent, fs.len()), ("pois", 4096, 2));
        assert_eq!(fs[0].parts, vec![vec![(100, 200)]]);
        assert_eq!(prop(&fs[0], "kind"), Some(&Value::Str("cafe".into())));
        assert_eq!(prop(&fs[0], "min_zoom"), Some(&Value::Int(15)));
        assert_eq!(prop(&fs[0], "neg"), Some(&Value::Int(-3)));
        assert_eq!(prop(&fs[1], "h"), Some(&Value::Float(2.5)));
        assert_eq!(prop(&fs[1], "b"), Some(&Value::Bool(true)));
        assert_eq!(fs[1].parts, vec![vec![(0, 0), (4096, 4096)]]);
        // "kind" key and "cafe" value stored once
        assert_eq!(t.windows(4).filter(|w| w == b"cafe").count(), 1);
        assert_eq!(t.windows(4).filter(|w| w == b"kind").count(), 1);
    }
}
