//! OBF routing section: the road network OsmAnd routes on.
//!
//! OsmAndRoutingIndex: 1 name, 2 rules (3 tag, 5 value, 7 explicit id; otherwise 1-based by
//! order), 3 rootBoxes (detailed), 4 basemapBoxes (generalised), 5 blocks (reached through the
//! boxes' shifts). Box: 1-4 zigzag deltas to the parent (root boxes: absolute), 5 shift (BE)
//! from the box body to its block, 7 child boxes. Block: 5 idTable (1: zigzag deltas of the
//! stored ids), 6 ways, 7 restrictions (1 type, 2 from, 3 to, 4 via – idTable indexes),
//! 8 string table. Way: 1 points (zigzag deltas in 16-unit steps – x31 >> 4 – from the box's
//! left >> 4 / top >> 4; NOT >> 5 like the map section),
//! 4 point types ([point index, length, rule ids…]…), 5 point names ([index, rule, string]…),
//! 7 types (rule ids), 12 idTable index, 14 names ((rule, string index) pairs).
//!
//! Ways are not split at junctions; two ways meet where they have a point with identical
//! (x31, y31). Long OSM ways are stored as several pieces sharing the stored id.

use std::collections::HashSet;

use anyhow::{bail, Result};

use super::reader::{c31, message_at, packed, string, string_table, varint, zigzag, Fields, ObfFile, SectionKind, Span, Val};

/// Routing points are stored as x31 >> 4 (measured: with >> 5 every block's points span twice
/// its box and junctions between blocks do not meet).
pub const ROUTE_SHIFT: u32 = 4;

/// OsmAnd's restriction types → OSM `restriction=*`.
pub fn restriction_name(kind: u32) -> Option<&'static str> {
    Some(match kind {
        1 => "no_right_turn",
        2 => "no_left_turn",
        3 => "no_u_turn",
        4 => "no_straight_on",
        5 => "only_right_turn",
        6 => "only_left_turn",
        7 => "only_straight_on",
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct RouteSection {
    pub name: String,
    pub span: Span,
    /// rule id → (tag, value)
    rules: Vec<Option<(String, String)>>,
    roots: Vec<Span>,
    basemap: Vec<Span>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteWay {
    /// stored id (from the block's id table; OSM way id ≈ id >> 6), shared by the pieces of one way
    pub id: i64,
    /// (x31, y31), multiples of 16
    pub points: Vec<(u32, u32)>,
    /// rule ids
    pub types: Vec<u32>,
    /// (point index, rule ids)
    pub point_types: Vec<(u32, Vec<u32>)>,
    /// (rule id of the tag, value)
    pub names: Vec<(u32, String)>,
    /// (point index, rule id of the tag, value)
    pub point_names: Vec<(u32, u32, String)>,
}

/// A turn restriction with stored way ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteRestriction {
    pub kind: u32,
    pub from: i64,
    pub to: i64,
    pub via: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct RouteBlock {
    /// absolute file offset of the block (progress)
    pub offset: usize,
    pub ways: Vec<RouteWay>,
    pub restrictions: Vec<RouteRestriction>,
}

pub fn route_sections(obf: &ObfFile) -> Result<Vec<RouteSection>> {
    obf.sections_of(SectionKind::Routing).map(|s| RouteSection::parse(obf, s.span)).collect()
}

struct Bx {
    left: i64,
    right: i64,
    top: i64,
    bottom: i64,
}

impl RouteSection {
    fn parse(obf: &ObfFile, span: Span) -> Result<Self> {
        let buf = obf.bytes();
        let mut s = RouteSection { name: String::new(), span, rules: vec![None], roots: Vec::new(), basemap: Vec::new() };
        let mut order = 0usize;
        for f in obf.fields(span, &[]) {
            match f? {
                (1, Val::Bytes(b)) => s.name = string(buf, b),
                (2, Val::Bytes(b)) => {
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
                (3, Val::Bytes(b)) => s.roots.push(b),
                (4, Val::Bytes(b)) => s.basemap.push(b),
                _ => {}
            }
        }
        Ok(s)
    }

    pub fn rule(&self, id: u32) -> Option<(&str, &str)> {
        self.rules.get(id as usize).and_then(|r| r.as_ref()).map(|(t, v)| (t.as_str(), v.as_str()))
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len().saturating_sub(1)
    }

    /// Union of the detailed root boxes, [left, top, right, bottom].
    pub fn bbox31(&self, obf: &ObfFile) -> Result<Option<[u32; 4]>> {
        let mut acc: Option<[u32; 4]> = None;
        for r in &self.roots {
            let b = read_box(obf, *r, &Bx { left: 0, right: 0, top: 0, bottom: 0 })?.0;
            let b = [c31(b.left), c31(b.top), c31(b.right), c31(b.bottom)];
            acc = Some(match acc {
                None => b,
                Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
            });
        }
        Ok(acc)
    }

    /// Decode every block of the detailed (`detailed = true`) or basemap tree, each once.
    pub fn for_each_block(&self, obf: &ObfFile, detailed: bool, f: &mut dyn FnMut(RouteBlock) -> Result<()>) -> Result<()> {
        let mut seen = HashSet::new();
        let zero = Bx { left: 0, right: 0, top: 0, bottom: 0 };
        for r in if detailed { &self.roots } else { &self.basemap } {
            self.walk(obf, *r, &zero, 0, &mut seen, f)?;
        }
        Ok(())
    }

    fn walk(&self, obf: &ObfFile, span: Span, parent: &Bx, depth: u32, seen: &mut HashSet<usize>, f: &mut dyn FnMut(RouteBlock) -> Result<()>) -> Result<()> {
        if depth > 64 {
            bail!("OBF: routing box tree too deep");
        }
        let (bx, shift, kids) = read_box(obf, span, parent)?;
        if let Some(shift) = shift {
            let at = span.start + shift as usize;
            if seen.insert(at) {
                f(self.decode_block(obf, at, &bx)?)?;
            }
        }
        for k in kids {
            self.walk(obf, k, &bx, depth + 1, seen, f)?;
        }
        Ok(())
    }

    fn decode_block(&self, obf: &ObfFile, at: usize, bx: &Bx) -> Result<RouteBlock> {
        let buf = obf.bytes();
        let block = message_at(buf, at)?;
        let (mut ids, mut ways, mut restr, mut strings) = (Vec::<i64>::new(), Vec::new(), Vec::new(), Vec::new());
        for g in obf.fields(block, &[]) {
            match g? {
                (5, Val::Bytes(t)) => {
                    let mut id = 0i64;
                    for h in obf.fields(t, &[]) {
                        match h? {
                            (1, Val::Int(v)) => {
                                id = id.wrapping_add(zigzag(v));
                                ids.push(id);
                            }
                            // packed form
                            (1, Val::Bytes(p)) => {
                                for v in packed(buf, p)? {
                                    id = id.wrapping_add(zigzag(v));
                                    ids.push(id);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                (6, Val::Bytes(w)) => ways.push(w),
                (7, Val::Bytes(r)) => restr.push(r),
                (8, Val::Bytes(t)) => strings = string_table(buf, t)?,
                _ => {}
            }
        }
        let id_of = |i: u64| ids.get(i as usize).copied();
        let mut out = RouteBlock { offset: at, ..Default::default() };
        for w in ways {
            if let Some(way) = decode_way(obf, w, bx, &strings, &id_of)? {
                out.ways.push(way);
            }
        }
        for r in restr {
            let (mut kind, mut from, mut to, mut via) = (0u32, None, None, None);
            for g in obf.fields(r, &[]) {
                match g? {
                    (1, v) => kind = v.int() as u32,
                    (2, v) => from = id_of(v.int()),
                    (3, v) => to = id_of(v.int()),
                    (4, v) => via = Some(id_of(v.int())),
                    _ => {}
                }
            }
            // a via index that does not resolve drops the restriction (it would be wrong without it)
            let via = match via {
                None => Some(None),
                Some(Some(v)) => Some(Some(v)),
                Some(None) => None,
            };
            if let (Some(from), Some(to), Some(via)) = (from, to, via) {
                out.restrictions.push(RouteRestriction { kind, from, to, via });
            }
        }
        Ok(out)
    }
}

fn read_box(obf: &ObfFile, span: Span, parent: &Bx) -> Result<(Bx, Option<u32>, Vec<Span>)> {
    let mut bx = Bx { left: 0, right: 0, top: 0, bottom: 0 };
    let (mut shift, mut kids) = (None, Vec::new());
    for g in Fields::new(obf.bytes(), span, &[5]) {
        match g? {
            (1, v) => bx.left = parent.left + v.sint(),
            (2, v) => bx.right = parent.right + v.sint(),
            (3, v) => bx.top = parent.top + v.sint(),
            (4, v) => bx.bottom = parent.bottom + v.sint(),
            (5, v) => shift = Some(v.int() as u32),
            (7, Val::Bytes(k)) => kids.push(k),
            _ => {}
        }
    }
    Ok((bx, shift, kids))
}

fn decode_way(obf: &ObfFile, span: Span, bx: &Bx, strings: &[String], id_of: &dyn Fn(u64) -> Option<i64>) -> Result<Option<RouteWay>> {
    let buf = obf.bytes();
    let mut w = RouteWay::default();
    let mut id_index = None;
    for g in obf.fields(span, &[]) {
        match g? {
            (1, Val::Bytes(p)) => {
                let (mut px, mut py) = (bx.left >> ROUTE_SHIFT, bx.top >> ROUTE_SHIFT);
                let mut q = p.start;
                while q < p.end {
                    px += zigzag(varint(buf, &mut q)?);
                    py += zigzag(varint(buf, &mut q)?);
                    w.points.push((c31(px << ROUTE_SHIFT), c31(py << ROUTE_SHIFT)));
                }
            }
            (4, Val::Bytes(p)) => {
                let mut q = p.start;
                while q < p.end {
                    let idx = varint(buf, &mut q)? as u32;
                    let len = varint(buf, &mut q)? as usize;
                    let end = q + len;
                    if end > p.end {
                        bail!("OBF: point types overrun at {q}");
                    }
                    let mut t = Vec::new();
                    while q < end {
                        t.push(varint(buf, &mut q)? as u32);
                    }
                    w.point_types.push((idx, t));
                }
            }
            (5, Val::Bytes(p)) => {
                let v = packed(buf, p)?;
                for c in v.as_chunks::<3>().0 {
                    if let Some(s) = strings.get(c[2] as usize) {
                        w.point_names.push((c[0] as u32, c[1] as u32, s.clone()));
                    }
                }
            }
            (7, Val::Bytes(p)) => w.types = packed(buf, p)?.into_iter().map(|v| v as u32).collect(),
            (12, v) => id_index = Some(v.int()),
            (14, Val::Bytes(p)) => {
                let v = packed(buf, p)?;
                for c in v.as_chunks::<2>().0 {
                    if let Some(s) = strings.get(c[1] as usize) {
                        w.names.push((c[0] as u32, s.clone()));
                    }
                }
            }
            _ => {}
        }
    }
    if w.points.is_empty() {
        return Ok(None);
    }
    w.id = id_index.and_then(id_of).unwrap_or(-1);
    Ok(Some(w))
}
