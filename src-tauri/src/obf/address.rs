//! OBF address section: cities / villages / postcodes with their streets and buildings.
//!
//! OsmAndAddressIndex: 1 name, 2 name_en, 3 boundaries, 4 attribute tag table (StringTable,
//! "name:fr" …), 6 city groups (wire type 6), 7 name index (not read). City group: 2 type
//! (1 cities/towns, 2 postcodes, 3 villages, other: boundaries), 5 cities.
//! City: 1 kind (0 city, 1 town, 2 village, 3 hamlet, 4 suburb, 5 district, 6 neighbourhood),
//! 2 name, 3 name_en, 4 id, 5 x31, 6 y31, 7/8 attribute tag ids / values, 10 shift (4-byte BE,
//! from the start of the city record INCLUDING its varint length) to its block.
//! Block: 10 buildings (relative to the city), 12 streets. Street: 1 name, 2 name_en, 3/4 x/y
//! (24-zoom zigzag delta to the city), 5 intersections, 6 id, 7/8 attributes, 12 buildings.
//! Building: 1 number, 2 number_en, 3 number2 (end of an interpolation), 5 interpolation,
//! 7/8 x/y (24-zoom zigzag delta to the street), 9/10 x2/y2, 13 id, 14 postcode.

use anyhow::Result;

use super::reader::{c31, message_at, packed, string, string_table, Fields, ObfFile, SectionKind, Span, Val};

/// OSM `place=*` for a city kind; None for districts / boundaries.
pub fn place_kind(kind: u32) -> Option<&'static str> {
    Some(match kind {
        0 => "city",
        1 => "town",
        2 => "village",
        3 => "hamlet",
        4 => "suburb",
        6 => "neighbourhood",
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct AddressSection {
    pub name: String,
    pub name_en: String,
    attr_tags: Vec<String>,
    /// (group type, span)
    groups: Vec<(u32, Span)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Building {
    pub number: String,
    /// end number of an interpolated range (at `xy2`)
    pub number2: Option<String>,
    pub interpolation: Option<i64>,
    pub x31: u32,
    pub y31: u32,
    pub xy2: Option<(u32, u32)>,
    pub postcode: Option<String>,
    pub id: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Street {
    pub name: String,
    pub name_en: Option<String>,
    pub x31: u32,
    pub y31: u32,
    /// other names ("name:fr", …)
    pub names: Vec<(String, String)>,
    pub buildings: Vec<Building>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct City {
    /// group type: 1 cities/towns, 2 postcodes, 3 villages, other = boundaries
    pub group: u32,
    pub kind: Option<u32>,
    pub name: String,
    pub name_en: Option<String>,
    pub id: Option<u64>,
    pub x31: u32,
    pub y31: u32,
    pub names: Vec<(String, String)>,
    /// filled only when streets are requested
    pub streets: Vec<Street>,
    /// buildings stored directly in the city block
    pub buildings: Vec<Building>,
}

pub fn address_sections(obf: &ObfFile) -> Result<Vec<AddressSection>> {
    obf.sections_of(SectionKind::Address).map(|s| AddressSection::parse(obf, s.span)).collect()
}

fn opt(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

impl AddressSection {
    fn parse(obf: &ObfFile, span: Span) -> Result<Self> {
        let buf = obf.bytes();
        let mut s = AddressSection { name: String::new(), name_en: String::new(), attr_tags: Vec::new(), groups: Vec::new() };
        for f in obf.fields(span, &[]) {
            match f? {
                (1, Val::Bytes(b)) => s.name = string(buf, b),
                (2, Val::Bytes(b)) => s.name_en = string(buf, b),
                (4, Val::Bytes(b)) => s.attr_tags = string_table(buf, b)?,
                (6, Val::Bytes(b)) => {
                    let mut t = 0u32;
                    for g in obf.fields(b, &[]) {
                        if let (2, v) = g? {
                            t = v.int() as u32;
                        }
                    }
                    s.groups.push((t, b));
                }
                _ => {}
            }
        }
        Ok(s)
    }

    /// Every city of every group; with `streets` its block (streets, buildings) is decoded too.
    pub fn for_each_city(&self, obf: &ObfFile, streets: bool, f: &mut dyn FnMut(City) -> Result<()>) -> Result<()> {
        for &(group, span) in &self.groups {
            for g in obf.fields(span, &[]) {
                if let (5, Val::Bytes(c)) = g? {
                    let city = self.decode_city(obf, group, c, streets)?;
                    f(city)?;
                }
            }
        }
        Ok(())
    }

    fn attrs(&self, ids: &[u64], vals: &[String]) -> Vec<(String, String)> {
        ids.iter().zip(vals).filter_map(|(i, v)| self.attr_tags.get(*i as usize).map(|t| (t.clone(), v.clone()))).collect()
    }

    fn decode_city(&self, obf: &ObfFile, group: u32, span: Span, with_block: bool) -> Result<City> {
        let buf = obf.bytes();
        let mut c = City { group, ..Default::default() };
        let (mut ids, mut vals, mut shift) = (Vec::new(), Vec::new(), None);
        for g in Fields::new(buf, span, &[10]) {
            match g? {
                (1, v) => c.kind = Some(v.int() as u32),
                (2, Val::Bytes(b)) => c.name = string(buf, b),
                (3, Val::Bytes(b)) => c.name_en = opt(string(buf, b)),
                (4, v) => c.id = Some(v.int()),
                (5, v) => c.x31 = v.int() as u32,
                (6, v) => c.y31 = v.int() as u32,
                (7, Val::Bytes(b)) => ids.extend(packed(buf, b)?),
                (7, v) => ids.push(v.int()),
                (8, Val::Bytes(b)) => vals.push(string(buf, b)),
                (10, v) => shift = Some(v.int() as usize),
                _ => {}
            }
        }
        c.names = self.attrs(&ids, &vals);
        if with_block {
            if let Some(shift) = shift.filter(|s| *s > 0) {
                let block = message_at(buf, span.head + shift)?;
                let (cx, cy) = (i64::from(c.x31 >> 7), i64::from(c.y31 >> 7));
                for g in Fields::new(buf, block, &[4]) {
                    match g? {
                        (10, Val::Bytes(b)) => {
                            if let Some(bd) = decode_building(obf, b, cx, cy)? {
                                c.buildings.push(bd);
                            }
                        }
                        (12, Val::Bytes(s)) => c.streets.push(self.decode_street(obf, s, cx, cy)?),
                        _ => {}
                    }
                }
            }
        }
        Ok(c)
    }

    fn decode_street(&self, obf: &ObfFile, span: Span, cx: i64, cy: i64) -> Result<Street> {
        let buf = obf.bytes();
        let mut s = Street::default();
        let (mut dx, mut dy, mut ids, mut vals, mut blds) = (0i64, 0i64, Vec::new(), Vec::new(), Vec::new());
        for g in obf.fields(span, &[]) {
            match g? {
                (1, Val::Bytes(b)) => s.name = string(buf, b),
                (2, Val::Bytes(b)) => s.name_en = opt(string(buf, b)),
                (3, v) => dx = v.sint(),
                (4, v) => dy = v.sint(),
                (7, Val::Bytes(b)) => ids.extend(packed(buf, b)?),
                (7, v) => ids.push(v.int()),
                (8, Val::Bytes(b)) => vals.push(string(buf, b)),
                (12, Val::Bytes(b)) => blds.push(b),
                _ => {}
            }
        }
        let (sx, sy) = (cx + dx, cy + dy);
        s.x31 = c31(sx << 7);
        s.y31 = c31(sy << 7);
        s.names = self.attrs(&ids, &vals);
        for b in blds {
            if let Some(bd) = decode_building(obf, b, sx, sy)? {
                s.buildings.push(bd);
            }
        }
        Ok(s)
    }
}

/// A building relative to its street (or city) at (px, py) in 24-zoom units.
fn decode_building(obf: &ObfFile, span: Span, px: i64, py: i64) -> Result<Option<Building>> {
    let buf = obf.bytes();
    let mut b = Building::default();
    let (mut dx, mut dy, mut dx2, mut dy2) = (0i64, 0i64, None, None);
    for g in obf.fields(span, &[]) {
        match g? {
            (1, Val::Bytes(s)) => b.number = string(buf, s),
            (3, Val::Bytes(s)) => b.number2 = opt(string(buf, s)),
            (5, v) => b.interpolation = Some(v.sint()),
            (7, v) => dx = v.sint(),
            (8, v) => dy = v.sint(),
            (9, v) => dx2 = Some(v.sint()),
            (10, v) => dy2 = Some(v.sint()),
            (13, v) => b.id = Some(v.int()),
            (14, Val::Bytes(s)) => b.postcode = opt(string(buf, s)),
            _ => {}
        }
    }
    if b.number.is_empty() {
        return Ok(None);
    }
    b.x31 = c31((px + dx) << 7);
    b.y31 = c31((py + dy) << 7);
    if let (Some(x2), Some(y2)) = (dx2, dy2) {
        b.xy2 = Some((c31((px + x2) << 7), c31((py + y2) << 7)));
    }
    Ok(Some(b))
}
