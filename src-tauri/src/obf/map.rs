//! OBF map section (vector map objects per zoom band).
//!
//! CONTRACT between the reader (this file, `reader.rs`) and the tile builder (`tiles.rs`):
//! the types and the `MapSource` trait below are fixed; the reader implements `MapSource` for
//! its open file, the tile builder only consumes it. Coordinates are OsmAnd's 31-zoom integers
//! (already Web Mercator: tile x at zoom z = x31 >> (31 - z)).

/// One zoom band of the map section (`MapRootLevel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapLevel {
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// left, top, right, bottom in 31-zoom units
    pub bbox31: [u32; 4],
}

/// A box of the level's tree that OsmAnd marks as entirely sea (`ocean = true`) or entirely
/// land (`ocean = false`); boxes without the flag are not reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OceanBox {
    /// left, top, right, bottom in 31-zoom units
    pub bbox31: [u32; 4],
    pub ocean: bool,
}

/// One decoded map object.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapObject {
    /// object id as stored (block base id + delta); OSM id ≈ id >> 7 (not unique across levels)
    pub id: i64,
    /// main type first (`highway=secondary`), then the other types, the additional types
    /// (`surface=unpaved`) and the names (`name`, `name:he`, `ref`, … with their values)
    pub tags: Vec<(String, String)>,
    /// a point (1 pair), a line, or the outer ring of an area, as (x31, y31)
    pub coords: Vec<(u32, u32)>,
    /// inner rings of an area
    pub inner: Vec<Vec<(u32, u32)>>,
    /// stored as an area (`areaCoordinates`); closed lines may still be areas by their tags
    pub area: bool,
    /// label position, when the file gives one
    pub label: Option<(u32, u32)>,
}

/// Where the tile builder reads map objects from (implemented by the OBF reader).
pub trait MapSource: Sync {
    /// The zoom bands, most detailed first or in file order (the builder sorts as it needs).
    fn levels(&self) -> Vec<MapLevel>;
    /// Call `f` for every object of level `level` (index into `levels()`), each exactly once.
    fn for_each_object(&self, level: usize, f: &mut dyn FnMut(MapObject)) -> anyhow::Result<()>;
    /// The boxes of level `level` that carry an ocean / land flag.
    fn ocean_boxes(&self, level: usize) -> anyhow::Result<Vec<OceanBox>>;
}

// ---------------------------------------------------------------- decoder
//
// OsmAndMapIndex: 2 name, 4 rules (3 tag, 5 value, 7 explicit id; otherwise ids are 1-based by
// order), 5 levels. Level: 1 maxZoom, 2 minZoom, 3 left, 4 right, 5 top, 6 bottom, 7 boxes.
// Box: 1-4 zigzag deltas to the parent's left/right/top/bottom, 5 shift (BE) from the box body
// to its data block, 6 ocean flag, 7 child boxes. Block: 10 base id, 12 objects, 15 strings.
// Object: 1 coordinates / 2 area coordinates / 4 inner rings as (zigzag << 5) deltas starting
// at the leaf box's (left & !31, top & !31); 6 additional types, 7 types (rule ids), 8 label
// offset, 10 names (rule id, string index) pairs, 12 id delta.

use std::collections::HashSet;

use anyhow::Result;

use super::reader::{c31, message_at, packed, string, string_table, varint, zigzag, Fields, ObfFile, SectionKind, Span, Val};

/// One `OsmAndMapIndex` (usually one per file).
#[derive(Debug, Clone)]
pub struct MapSection {
    pub name: String,
    /// rule id → (tag, value)
    rules: Vec<Option<(String, String)>>,
    pub levels: Vec<LevelInfo>,
}

#[derive(Debug, Clone)]
pub struct LevelInfo {
    pub level: MapLevel,
    pub span: Span,
}

/// A box of a level tree with its absolute bounds.
struct BoxNode {
    left: i64,
    right: i64,
    top: i64,
    bottom: i64,
    span: Span,
}

type BoxVisitor<'a> = dyn FnMut(&BoxNode, Option<u32>, Option<bool>) -> Result<()> + 'a;

impl MapSection {
    fn parse(obf: &ObfFile, span: Span) -> Result<Self> {
        let buf = obf.bytes();
        let mut s = MapSection { name: String::new(), rules: vec![None], levels: Vec::new() };
        let mut order = 0usize;
        for f in obf.fields(span, &[]) {
            match f? {
                (2, Val::Bytes(b)) => s.name = string(buf, b),
                (4, Val::Bytes(b)) => {
                    order += 1;
                    let (mut tag, mut value, mut id) = (String::new(), String::new(), None);
                    for g in obf.fields(b, &[]) {
                        match g? {
                            (3, Val::Bytes(x)) => tag = string(buf, x),
                            (5, Val::Bytes(x)) => value = string(buf, x),
                            (7, v) => id = Some(v.int() as usize),
                            _ => {}
                        }
                    }
                    let id = id.unwrap_or(order);
                    if id < 1 << 24 {
                        if s.rules.len() <= id {
                            s.rules.resize(id + 1, None);
                        }
                        s.rules[id] = Some((tag, value));
                    }
                }
                (5, Val::Bytes(b)) => {
                    let mut h = [0i64; 6];
                    for g in obf.fields(b, &[]) {
                        let (fl, v) = g?;
                        if (1..=6).contains(&fl) {
                            h[fl as usize - 1] = i64::from(v.int() as u32 as i32);
                        }
                    }
                    let level = MapLevel { max_zoom: h[0].clamp(0, 31) as u8, min_zoom: h[1].clamp(0, 31) as u8, bbox31: [c31(h[2]), c31(h[4]), c31(h[3]), c31(h[5])] };
                    s.levels.push(LevelInfo { level, span: b });
                }
                _ => {}
            }
        }
        Ok(s)
    }

    pub fn rule(&self, id: u64) -> Option<(&str, &str)> {
        self.rules.get(id as usize).and_then(|r| r.as_ref()).map(|(t, v)| (t.as_str(), v.as_str()))
    }

    /// Walk the box tree of `level`; `f(box, shift to data, ocean flag)`.
    fn walk(&self, obf: &ObfFile, level: &LevelInfo, f: &mut BoxVisitor) -> Result<()> {
        let b = level.level.bbox31;
        let root = BoxNode { left: i64::from(b[0]), right: i64::from(b[2]), top: i64::from(b[1]), bottom: i64::from(b[3]), span: level.span };
        for g in obf.fields(level.span, &[]) {
            if let (7, Val::Bytes(sp)) = g? {
                walk_box(obf, sp, &root, 0, f)?;
            }
        }
        Ok(())
    }

    /// Decode every object of one level.
    pub fn for_each_object(&self, obf: &ObfFile, level: &LevelInfo, f: &mut dyn FnMut(MapObject)) -> Result<()> {
        let buf = obf.bytes();
        let mut seen: HashSet<usize> = HashSet::new();
        self.walk(obf, level, &mut |bx, shift, _| {
            let Some(shift) = shift else { return Ok(()) };
            let at = bx.span.start + shift as usize;
            if !seen.insert(at) {
                return Ok(());
            }
            let block = message_at(buf, at)?;
            let (mut base, mut objs, mut strings) = (0u64, Vec::new(), Vec::new());
            for g in obf.fields(block, &[]) {
                match g? {
                    (10, v) => base = v.int(),
                    (12, Val::Bytes(o)) => objs.push(o),
                    (15, Val::Bytes(t)) => strings = string_table(buf, t)?,
                    _ => {}
                }
            }
            let origin = (bx.left & !31, bx.top & !31);
            for o in objs {
                if let Some(obj) = self.decode_object(obf, o, base, origin, &strings)? {
                    f(obj);
                }
            }
            Ok(())
        })
    }

    fn decode_object(&self, obf: &ObfFile, span: Span, base: u64, origin: (i64, i64), strings: &[String]) -> Result<Option<MapObject>> {
        let buf = obf.bytes();
        let mut o = MapObject::default();
        let (mut types, mut extra, mut names, mut label) = (Vec::new(), Vec::new(), Vec::new(), None);
        for g in obf.fields(span, &[]) {
            match g? {
                (1, Val::Bytes(c)) => o.coords = coords(buf, c, origin)?,
                (2, Val::Bytes(c)) => {
                    o.coords = coords(buf, c, origin)?;
                    o.area = true;
                }
                (4, Val::Bytes(c)) => o.inner.push(coords(buf, c, origin)?),
                (6, Val::Bytes(c)) => extra = packed(buf, c)?,
                (7, Val::Bytes(c)) => types = packed(buf, c)?,
                (8, Val::Bytes(c)) => {
                    let v = packed(buf, c)?;
                    if v.len() >= 2 {
                        label = Some((zigzag(v[0]), zigzag(v[1])));
                    }
                }
                (10, Val::Bytes(c)) => names = packed(buf, c)?,
                (12, v) => o.id = (base as i64).wrapping_add(v.sint()),
                _ => {}
            }
        }
        if o.coords.is_empty() {
            return Ok(None);
        }
        for t in types.iter().chain(extra.iter()) {
            if let Some((k, v)) = self.rule(*t) {
                o.tags.push((k.to_string(), v.to_string()));
            }
        }
        for p in names.as_chunks::<2>().0 {
            if let (Some((k, _)), Some(s)) = (self.rule(p[0]), strings.get(p[1] as usize)) {
                o.tags.push((k.to_string(), s.clone()));
            }
        }
        // label: offset (26-zoom units) from the mean of the outer ring's points
        if let Some((dx, dy)) = label {
            let n = o.coords.len() as i64;
            let (sx, sy) = o.coords.iter().fold((0i64, 0i64), |a, p| (a.0 + i64::from(p.0), a.1 + i64::from(p.1)));
            o.label = Some((c31(sx / n + (dx << 5)), c31(sy / n + (dy << 5))));
        }
        Ok(Some(o))
    }

    pub fn ocean_boxes(&self, obf: &ObfFile, level: &LevelInfo) -> Result<Vec<OceanBox>> {
        let mut out = Vec::new();
        self.walk(obf, level, &mut |bx, _, ocean| {
            if let Some(ocean) = ocean {
                out.push(OceanBox { bbox31: [c31(bx.left), c31(bx.top), c31(bx.right), c31(bx.bottom)], ocean });
            }
            Ok(())
        })?;
        Ok(out)
    }
}

fn walk_box(obf: &ObfFile, span: Span, parent: &BoxNode, depth: u32, f: &mut BoxVisitor) -> Result<()> {
    if depth > 64 {
        anyhow::bail!("OBF: map box tree too deep");
    }
    let mut bx = BoxNode { left: 0, right: 0, top: 0, bottom: 0, span };
    let (mut shift, mut ocean, mut kids) = (None, None, Vec::new());
    for g in Fields::new(obf.bytes(), span, &[5]) {
        match g? {
            (1, v) => bx.left = parent.left + v.sint(),
            (2, v) => bx.right = parent.right + v.sint(),
            (3, v) => bx.top = parent.top + v.sint(),
            (4, v) => bx.bottom = parent.bottom + v.sint(),
            (5, v) => shift = Some(v.int() as u32),
            (6, v) => ocean = Some(v.int() != 0),
            (7, Val::Bytes(k)) => kids.push(k),
            _ => {}
        }
    }
    f(&bx, shift, ocean)?;
    for k in kids {
        walk_box(obf, k, &bx, depth + 1, f)?;
    }
    Ok(())
}

/// Delta-coded (x, y) pairs, `(zigzag << 5) + previous`, starting from `origin`.
fn coords(buf: &[u8], span: Span, origin: (i64, i64)) -> Result<Vec<(u32, u32)>> {
    let mut out = Vec::new();
    let (mut px, mut py) = origin;
    let mut p = span.start;
    while p < span.end {
        let dx = varint(buf, &mut p)?;
        let dy = varint(buf, &mut p)?;
        px += zigzag(dx) << 5;
        py += zigzag(dy) << 5;
        out.push((c31(px), c31(py)));
    }
    Ok(out)
}

impl ObfFile {
    /// The map sections (parsed once, cached in the file).
    pub fn map_sections(&self) -> Result<&[MapSection]> {
        let r = self
            .map_cache
            .get_or_init(|| self.sections_of(SectionKind::Map).map(|s| MapSection::parse(self, s.span)).collect::<Result<Vec<_>>>().map_err(|e| format!("{e:#}")));
        match r {
            Ok(v) => Ok(v),
            Err(e) => Err(anyhow::anyhow!("{e}")),
        }
    }

    /// (section, level) for the flat level index used by [`MapSource`].
    fn level_at(&self, level: usize) -> Result<(&MapSection, &LevelInfo)> {
        self.map_sections()?
            .iter()
            .flat_map(|s| s.levels.iter().map(move |l| (s, l)))
            .nth(level)
            .ok_or_else(|| anyhow::anyhow!("OBF: no map level {level}"))
    }
}

/// Levels of all map sections in file order (a file normally has one map section).
impl MapSource for ObfFile {
    fn levels(&self) -> Vec<MapLevel> {
        self.map_sections().map(|v| v.iter().flat_map(|s| s.levels.iter().map(|l| l.level)).collect()).unwrap_or_default()
    }

    fn for_each_object(&self, level: usize, f: &mut dyn FnMut(MapObject)) -> Result<()> {
        let (s, l) = self.level_at(level)?;
        s.for_each_object(self, l, f)
    }

    fn ocean_boxes(&self, level: usize) -> Result<Vec<OceanBox>> {
        let (s, l) = self.level_at(level)?;
        s.ocean_boxes(self, l)
    }
}
