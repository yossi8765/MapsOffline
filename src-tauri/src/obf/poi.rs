//! OBF POI section and the mapping of OsmAnd POI types back to OSM tags.
//!
//! OsmAndPoiIndex: 1 name, 2 boundaries, 3 category tables (1 category, 3 subcategories),
//! 5 subtypes table (4 subtype: 1 name, 2 tag name, 3 is text, 8 values), 6 boxes (tree, not
//! needed: every leaf's data is also a top-level field 9), 9 box data (1 zoom, 2 x, 3 y, 5 atoms).
//! Atom: 2/3 dx/dy (24-zoom zigzag, relative to x << (24 - zoom)), 4 categories
//! (category index & 127, subcategory index >> 7), 5 additional subtypes, 6 name, 7 name_en,
//! 8 id, 14 text subtypes, 15 their values (same order), 16 precision.
//! Subtype references: even → index (t >> 1) & 31, value t >> 6; odd → index (t >> 1) & 0x7fff,
//! value t >> 16. The same object appears once per category (merge by id).

use anyhow::Result;

use super::reader::{c31, packed, string, Fields, ObfFile, SectionKind, Span, Val};

#[derive(Debug, Clone, Default)]
pub struct PoiSubtype {
    pub name: String,
    /// OSM-ish tag name (the stored tag name, else the subtype name)
    pub tag: String,
    pub is_text: bool,
    pub values: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PoiSection {
    pub name: String,
    pub span: Span,
    /// (category, subcategories)
    pub categories: Vec<(String, Vec<String>)>,
    pub subtypes: Vec<PoiSubtype>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Poi {
    pub id: u64,
    pub x31: u32,
    pub y31: u32,
    /// (category index, subcategory index)
    pub types: Vec<(u32, u32)>,
    /// (subtype index, value index) of non-text subtypes
    pub extra: Vec<(u32, u32)>,
    /// (subtype index, value) of text subtypes
    pub text: Vec<(u32, String)>,
    pub name: Option<String>,
    pub name_en: Option<String>,
}

pub fn poi_sections(obf: &ObfFile) -> Result<Vec<PoiSection>> {
    obf.sections_of(SectionKind::Poi).map(|s| PoiSection::parse(obf, s.span)).collect()
}

fn subtype_ref(t: u64) -> (u32, u32) {
    if t & 1 == 0 {
        (((t >> 1) & 31) as u32, (t >> 6) as u32)
    } else {
        (((t >> 1) & 0x7fff) as u32, (t >> 16) as u32)
    }
}

fn ints(buf: &[u8], v: Val, out: &mut Vec<u64>) -> Result<()> {
    match v {
        Val::Bytes(b) => out.extend(packed(buf, b)?),
        v => out.push(v.int()),
    }
    Ok(())
}

impl PoiSection {
    fn parse(obf: &ObfFile, span: Span) -> Result<Self> {
        let buf = obf.bytes();
        let mut s = PoiSection { name: String::new(), span, categories: Vec::new(), subtypes: Vec::new() };
        for f in obf.fields(span, &[14]) {
            match f? {
                (1, Val::Bytes(b)) => s.name = string(buf, b),
                (3, Val::Bytes(b)) => {
                    let (mut c, mut subs) = (String::new(), Vec::new());
                    for g in obf.fields(b, &[]) {
                        match g? {
                            (1, Val::Bytes(x)) => c = string(buf, x),
                            (3, Val::Bytes(x)) => subs.push(string(buf, x)),
                            _ => {}
                        }
                    }
                    s.categories.push((c, subs));
                }
                (5, Val::Bytes(b)) => {
                    for g in obf.fields(b, &[]) {
                        if let (4, Val::Bytes(x)) = g? {
                            let mut st = PoiSubtype::default();
                            for h in obf.fields(x, &[]) {
                                match h? {
                                    (1, Val::Bytes(y)) => st.name = string(buf, y),
                                    (2, Val::Bytes(y)) => st.tag = string(buf, y),
                                    (3, v) => st.is_text = v.int() != 0,
                                    (8, Val::Bytes(y)) => st.values.push(string(buf, y)),
                                    _ => {}
                                }
                            }
                            if st.tag.is_empty() {
                                st.tag = st.name.clone();
                            }
                            s.subtypes.push(st);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(s)
    }

    /// Call `f` with the POIs of each box-data record (one tile), in file order.
    pub fn for_each_box(&self, obf: &ObfFile, f: &mut dyn FnMut(Vec<Poi>) -> Result<()>) -> Result<()> {
        let buf = obf.bytes();
        for g in obf.fields(self.span, &[14]) {
            let (9, Val::Bytes(bd)) = g? else { continue };
            let (mut z, mut x, mut y, mut atoms) = (0u32, 0u64, 0u64, Vec::new());
            for h in obf.fields(bd, &[]) {
                match h? {
                    (1, v) => z = v.int() as u32,
                    (2, v) => x = v.int(),
                    (3, v) => y = v.int(),
                    (5, Val::Bytes(a)) => atoms.push(a),
                    _ => {}
                }
            }
            if z > 24 {
                continue;
            }
            let (bx, by) = ((x << (24 - z)) as i64, (y << (24 - z)) as i64);
            let mut out = Vec::with_capacity(atoms.len());
            for a in atoms {
                out.push(self.decode_atom(buf, a, bx, by)?);
            }
            f(out)?;
        }
        Ok(())
    }

    fn decode_atom(&self, buf: &[u8], span: Span, bx: i64, by: i64) -> Result<Poi> {
        let mut p = Poi::default();
        let (mut dx, mut dy) = (0i64, 0i64);
        let (mut cats, mut subs, mut tcats, mut tvals) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for g in Fields::new(buf, span, &[]) {
            match g? {
                (2, v) => dx = v.sint(),
                (3, v) => dy = v.sint(),
                (4, v) => ints(buf, v, &mut cats)?,
                (5, v) => ints(buf, v, &mut subs)?,
                (6, Val::Bytes(b)) => p.name = Some(string(buf, b)).filter(|s| !s.is_empty()),
                (7, Val::Bytes(b)) => p.name_en = Some(string(buf, b)).filter(|s| !s.is_empty()),
                (8, v) => p.id = v.int(),
                (14, v) => ints(buf, v, &mut tcats)?,
                (15, Val::Bytes(b)) => tvals.push(string(buf, b)),
                _ => {}
            }
        }
        p.x31 = c31((bx + dx) << 7);
        p.y31 = c31((by + dy) << 7);
        p.types = cats.into_iter().map(|c| ((c & 127) as u32, (c >> 7) as u32)).collect();
        p.extra = subs.into_iter().map(subtype_ref).collect();
        p.text = tcats.into_iter().zip(tvals).map(|(t, v)| (subtype_ref(t).0, v)).collect();
        Ok(p)
    }

    /// (category, subcategory) names of a type reference.
    pub fn type_name(&self, t: (u32, u32)) -> Option<(&str, &str)> {
        let (c, subs) = self.categories.get(t.0 as usize)?;
        Some((c.as_str(), subs.get(t.1 as usize)?.as_str()))
    }

    fn is_type_name(&self, n: &str) -> bool {
        self.categories.iter().any(|(_, s)| s.iter().any(|x| x == n))
    }

    /// OSM tags of a (merged) POI: the main `key=value` of each type, `name`, `name:en`, the
    /// additional subtypes (`religion=jewish`, `cuisine=pizza`) and the text tags
    /// (`opening_hours`, `phone`, `addr:street` …). Empty when no type maps to OSM.
    pub fn osm_tags(&self, p: &Poi) -> Vec<(String, String)> {
        let mut tags: Vec<(String, String)> = Vec::new();
        fn put(tags: &mut Vec<(String, String)>, k: &str, v: &str) {
            if k.is_empty() || v.is_empty() {
                return;
            }
            if let Some(e) = tags.iter_mut().find(|(a, _)| a == k) {
                // multi-valued keys merge, others keep the first value
                if matches!(k, "cuisine" | "sport" | "denomination") && !e.1.split(';').any(|x| x == v) {
                    e.1 = format!("{};{v}", e.1);
                }
                return;
            }
            tags.push((k.to_string(), v.to_string()));
        }
        for t in &p.types {
            let Some((cat, sub)) = self.type_name(*t) else { continue };
            for (k, v) in main_tags(cat, sub) {
                put(&mut tags, &k, &v);
            }
        }
        if tags.is_empty() {
            return tags;
        }
        if let Some(n) = &p.name {
            put(&mut tags, "name", n);
        }
        if let Some(n) = &p.name_en {
            put(&mut tags, "name:en", n);
        }
        for (i, v) in &p.text {
            let Some(st) = self.subtypes.get(*i as usize) else { continue };
            // text subtypes named after a POI type repeat the original OSM value; internal ones skipped
            if self.is_type_name(&st.name) || skip_tag(&st.tag) {
                continue;
            }
            put(&mut tags, &osm_key(&st.tag), v);
        }
        for (i, vi) in &p.extra {
            let Some(st) = self.subtypes.get(*i as usize) else { continue };
            let Some(v) = st.values.get(*vi as usize) else { continue };
            let base = st.tag.strip_suffix(&format!("_{v}")).unwrap_or(&st.tag);
            if skip_tag(base) {
                continue;
            }
            put(&mut tags, &osm_key(base), v);
        }
        tags
    }
}

fn skip_tag(t: &str) -> bool {
    t.starts_with("osmand_") || t.starts_with("travel_") || t.starts_with("wiki_") || matches!(t, "qrank" | "osmwiki" | "note_tag" | "brewery_additional")
}

/// OsmAnd writes ':' as '_' in some tag names ("addr_street"); restore the common ones.
pub fn osm_key(t: &str) -> String {
    const COLON: [&str; 9] = ["addr_", "contact_", "diet_", "payment_", "fuel_", "healthcare_", "internet_access_", "building_", "socket_"];
    for p in COLON {
        if let Some(rest) = t.strip_prefix(p) {
            return format!("{}:{rest}", &p[..p.len() - 1]);
        }
    }
    t.to_string()
}

/// OSM keys that prefix OsmAnd subtype names ("craft_tailor" → craft=tailor).
const PREFIX_KEYS: [&str; 19] =
    ["public_transport", "traffic_calming", "craft", "highway", "railway", "aeroway", "power", "leisure", "historic", "office", "landuse", "barrier", "emergency", "amenity", "shop", "tourism", "waterway", "place", "healthcare"];

const PLACES: [&str; 16] = ["city", "town", "village", "hamlet", "suburb", "neighbourhood", "quarter", "locality", "isolated_dwelling", "island", "islet", "state", "province", "country", "borough", "square"];

/// The OSM tags an OsmAnd POI type stands for.
pub fn main_tags(category: &str, sub: &str) -> Vec<(String, String)> {
    let one = |k: &str, v: &str| vec![(k.to_string(), v.to_string())];
    let pow = |religion: &str, denom: Option<&str>| {
        let mut v = vec![("amenity".to_string(), "place_of_worship".to_string()), ("religion".to_string(), religion.to_string())];
        if let Some(d) = denom {
            v.push(("denomination".to_string(), d.to_string()));
        }
        v
    };
    if category == "osmwiki" || sub.is_empty() || sub == "abandoned_poi" {
        return Vec::new();
    }
    // places of worship
    match sub {
        "place_of_worship" => return one("amenity", "place_of_worship"),
        "synagogue" | "jewish_synagogue" => return pow("jewish", None),
        "mosque" => return pow("muslim", None),
        "hindu_temple" => return pow("hindu", None),
        "buddhist_temple" => return pow("buddhist", None),
        "christian_church" | "church" => return pow("christian", None),
        "jehovahs_witness_kingdom_hall" => return pow("christian", Some("jehovahs_witness")),
        _ => {}
    }
    if let Some(d) = sub.strip_suffix("_church") {
        return pow("christian", Some(d));
    }
    if category == "administrative" && PLACES.contains(&sub) || sub == "square" {
        return one("place", sub);
    }
    // explicit exceptions to the category defaults
    let fixed: Option<(&str, &str)> = match sub {
        "marketplace" | "fountain" | "grave_yard" | "drinking_water" | "water_point" | "courthouse" | "townhall" | "prison" | "embassy" | "consulate" | "police" | "fire_station" | "post_office" | "post_box" | "monastery" | "internet_cafe" | "bus_station" | "fuel" | "parking" | "taxi" | "ferry_terminal" | "bicycle_parking" | "parking_space" | "charging_station" | "car_wash" | "car_rental" | "library" | "arts_centre" | "casino" | "cinema" | "community_centre" | "conference_centre" | "nightclub" | "theatre" | "bench" | "events_venue" | "social_centre" | "toilets" | "shelter" | "shower" | "recycling" | "waste_basket" | "waste_disposal" | "refugee_site" | "public_bath" | "bank" | "atm" | "bureau_de_change" | "money_transfer" | "pharmacy" | "hospital" | "clinic" | "doctors" | "dentist" | "veterinary" | "social_facility" | "school" | "kindergarten" | "college" | "university" | "restaurant" | "cafe" | "fast_food" | "bar" | "pub" | "ice_cream" | "food_court" | "biergarten" => Some(("amenity", sub)),
        "car_repair" | "car" | "car_parts" | "tyres" | "motorcycle" | "hairdresser" | "laundry" | "copyshop" | "dry_cleaning" | "beauty" | "mall" | "travel_agency" => Some(("shop", sub)),
        "shop_general" => Some(("shop", "general")),
        "government" => Some(("office", "government")),
        "memorial" | "monument" | "tomb" | "wayside_shrine" | "wayside_cross" | "castle" | "ruins" | "archaeological_site" | "fort" | "battlefield" => Some(("historic", sub)),
        "nature_reserve" | "park" | "playground" | "stadium" | "sports_centre" | "pitch" | "golf_course" | "swimming_pool" | "track" | "fitness_centre" | "marina" | "water_park" | "garden" | "dog_park" | "picnic_table" => Some(("leisure", sub)),
        "protected_area" => Some(("boundary", "protected_area")),
        "river" | "stream" | "canal" | "dam" | "weir" | "waterfall" | "drain" => Some(("waterway", sub)),
        "bus_stop" | "traffic_signals" | "crossing" | "motorway_junction" | "steps" | "speed_camera" | "stop" | "give_way" | "street_lamp" | "turning_circle" | "mini_roundabout" | "trailhead" => Some(("highway", sub)),
        "aerodrome" | "helipad" | "heliport" | "runway" | "apron" => Some(("aeroway", sub)),
        "toll_booth" | "border_control" | "lift_gate" | "gate" | "bollard" => Some(("barrier", sub)),
        "aeroway_terminal" => Some(("aeroway", "terminal")),
        "aeroway_gate" => Some(("aeroway", "gate")),
        "ford" | "junction" | "tunnel" => Some((sub, "yes")),
        "ford_stepping_stones" => Some(("ford", "stepping_stones")),
        "military_landuse" => Some(("landuse", "military")),
        "brownfield" | "commercial" | "farmland" | "farmyard" | "forest" | "grass" | "industrial" | "orchard" | "quarry" | "religious" | "retail" | "cemetery" | "residential" | "construction" | "landfill" | "meadow" | "vineyard" | "allotments" | "greenfield" => Some(("landuse", sub)),
        "water_well" | "water_tap" | "water_tower" | "water_works" | "wastewater_plant" | "pumping_station" | "tower" | "mast" | "antenna" | "chimney" | "survey_point" | "pier" | "bridge" | "works" => Some(("man_made", sub)),
        "internet_access_yes" => Some(("internet_access", "yes")),
        "attraction_animal" => Some(("tourism", "attraction")),
        "defibrillator" | "water_tank" | "ambulance_station" | "phone" => Some(("emergency", sub)),
        _ => None,
    };
    if let Some((k, v)) = fixed {
        return one(k, v);
    }
    for k in PREFIX_KEYS {
        if let Some(rest) = sub.strip_prefix(k).and_then(|r| r.strip_prefix('_')) {
            if !rest.is_empty() {
                return one(k, rest);
            }
        }
    }
    let key = match category {
        "sustenance" | "education" | "healthcare" | "finance" | "service" | "transportation" => "amenity",
        "shop" => "shop",
        "tourism" => "tourism",
        "entertainment" => "leisure",
        "administrative" => "amenity",
        "natural" => "natural",
        "man_made" => "man_made",
        "emergency" => "emergency",
        "office" => "office",
        "sport" => "sport",
        "military" => "military",
        "craft" => "craft",
        "leisure" => "leisure",
        "historic" => "historic",
        "power" => "power",
        _ => category,
    };
    one(key, sub)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(v: Vec<(String, String)>) -> Vec<(&'static str, String)> {
        v.into_iter().map(|(k, v)| (Box::leak(k.into_boxed_str()) as &str, v)).collect()
    }

    #[test]
    fn mapping() {
        assert_eq!(main_tags("sustenance", "restaurant"), vec![("amenity".into(), "restaurant".into())]);
        assert_eq!(main_tags("transportation", "fuel"), vec![("amenity".into(), "fuel".into())]);
        assert_eq!(main_tags("shop", "supermarket"), vec![("shop".into(), "supermarket".into())]);
        assert_eq!(main_tags("service", "craft_tailor"), vec![("craft".into(), "tailor".into())]);
        assert_eq!(main_tags("transportation", "public_transport_platform"), vec![("public_transport".into(), "platform".into())]);
        assert_eq!(main_tags("administrative", "town"), vec![("place".into(), "town".into())]);
        assert_eq!(kv(main_tags("tourism", "roman_catholic_church"))[2], ("denomination", "roman_catholic".into()));
        assert_eq!(kv(main_tags("tourism", "synagogue"))[1], ("religion", "jewish".into()));
        assert!(main_tags("osmwiki", "wiki_place").is_empty());
        assert_eq!(osm_key("addr_street"), "addr:street");
        assert_eq!(osm_key("name:fr"), "name:fr");
        assert_eq!(subtype_ref(5 << 1 | 3 << 6), (5, 3));
        assert_eq!(subtype_ref(1 | 300 << 1 | 2 << 16), (300, 2));
    }
}
