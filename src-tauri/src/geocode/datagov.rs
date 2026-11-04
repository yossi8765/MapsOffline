//! Extra points of interest from Israel's open-data portal (data.gov.il, CKAN datastore API),
//! for kinds OpenStreetMap often lacks: kindergartens / day-care / schools, sports facilities,
//! bank branches, fire stations, declared beaches and the energy ministry's register of fuel
//! stations (brand, address, phone). Fetched during the Israel geocoder
//! build and merged into its `pois` (an OSM point of the same category within ~100 m wins).
//! A dataset that cannot be fetched is skipped; the build never fails because of it.
use std::time::Duration;

use anyhow::{anyhow, Context};
use serde_json::{Map, Value};

use super::info::PoiInfo;
use crate::coords::itm;

const API: &str = "https://data.gov.il/api/3/action/datastore_search";
/// The portal's content filter answered 418 for very large pages; 5000 passes.
const PAGE: usize = 5000;

/// One point to add: `cat` is the OSM-style category value the geocoder uses ("school", "bank" …).
#[derive(Debug, Clone)]
pub struct ExtPoi {
    pub name: String,
    pub cat: &'static str,
    pub lon: f64,
    pub lat: f64,
    /// Details from the register; also completes an OSM point of the same kind nearby.
    pub info: Option<PoiInfo>,
}

fn records(client: &reqwest::blocking::Client, resource: &str) -> anyhow::Result<Vec<Map<String, Value>>> {
    records_fields(client, resource, None)
}

/// All records of a datastore resource, optionally only some columns (`fields=a,b`).
fn records_fields(client: &reqwest::blocking::Client, resource: &str, fields: Option<&str>) -> anyhow::Result<Vec<Map<String, Value>>> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let fields = fields.map(|f| format!("&fields={f}")).unwrap_or_default();
    loop {
        let url = format!("{API}?resource_id={resource}&limit={PAGE}&offset={offset}{fields}");
        let r = client.get(&url).timeout(Duration::from_secs(60)).send().with_context(|| format!("GET {url}"))?;
        if r.status().as_u16() == 418 {
            anyhow::bail!("{}", crate::data::download::blocked_message(&url));
        }
        let v: Value = r.error_for_status()?.json()?;
        let res = v.get("result").ok_or_else(|| anyhow!("no result"))?;
        let recs = res["records"].as_array().cloned().unwrap_or_default();
        let total = res["total"].as_u64().unwrap_or(0) as usize;
        let n = recs.len();
        out.extend(recs.into_iter().filter_map(|r| r.as_object().cloned()));
        offset += n;
        if n == 0 || offset >= total {
            return Ok(out);
        }
    }
}

fn s<'a>(r: &'a Map<String, Value>, k: &str) -> &'a str {
    r.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("")
}

fn num(r: &Map<String, Value>, k: &str) -> Option<f64> {
    match r.get(k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(t) => t.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

/// ITM easting / northing → lon/lat, rejecting values outside Israel's grid.
fn from_itm(e: Option<f64>, n: Option<f64>) -> Option<(f64, f64)> {
    let (e, n) = (e?, n?);
    if !(100_000.0..=300_000.0).contains(&e) || !(350_000.0..=820_000.0).contains(&n) {
        return None;
    }
    Some(itm::to_wgs84(e, n))
}

fn in_israel(lon: f64, lat: f64) -> bool {
    let b = crate::paths::BBOX_ISRAEL;
    lon >= b[0] && lon <= b[2] && lat >= b[1] && lat <= b[3]
}

/// Kindergartens, day-care centres and schools (Ministry of Education, `mosdot_edu`).
fn education(rs: &[Map<String, Value>]) -> Vec<ExtPoi> {
    rs.iter()
        .filter(|r| s(r, "TEUR_STATUS") == "פעיל" && s(r, "RAMAT_DIYUK_MIKUM") != "נמוכה")
        .filter_map(|r| {
            let (lon, lat) = from_itm(num(r, "X"), num(r, "Y"))?;
            let kind = s(r, "TEUR_TAT_SUG_MISGERET");
            let cat = if kind.contains("גן") {
                "kindergarten"
            } else if kind.contains("מעון") || kind.contains("משפחתון") {
                "childcare"
            } else {
                "school"
            };
            Some(ExtPoi { name: s(r, "SHEM_MOSAD").to_string(), cat, lon, lat, info: None })
        })
        .collect()
}

/// Sports halls, pools, stadiums, pitches (Ministry of Culture and Sport, dataset 408).
fn sports(rs: &[Map<String, Value>]) -> Vec<ExtPoi> {
    rs.iter()
        .filter_map(|r| {
            let kind = s(r, "סוג מתקן");
            let cat = if kind.contains("בריכ") {
                "swimming_pool"
            } else if kind.contains("אולם") || kind.contains("מכון לכושר") || kind.contains("אצטדיון") {
                "sports_centre"
            } else if kind.contains("מגרש") {
                "pitch"
            } else {
                return None; // outdoor fitness corners, "other"
            };
            let (lon, lat) = from_itm(num(r, "ציר X"), num(r, "ציר Y"))?;
            let name = s(r, "שם המתקן");
            let name = if name.is_empty() { kind.split(['–', '-']).next().unwrap_or(kind).trim() } else { name };
            Some(ExtPoi { name: name.to_string(), cat, lon, lat, info: None })
        })
        .collect()
}

/// Bank branches (Bank of Israel). The columns are swapped: X = latitude, Y = longitude.
fn banks(rs: &[Map<String, Value>]) -> Vec<ExtPoi> {
    rs.iter()
        .filter(|r| s(r, "Close_Date").is_empty())
        .filter_map(|r| {
            let (lat, lon) = (num(r, "X_Coordinate")?, num(r, "Y_Coordinate")?);
            in_israel(lon, lat).then(|| {
                let bank = s(r, "Bank_Name").trim_end_matches("בע\"מ").trim();
                ExtPoi { name: format!("{bank} – סניף {}", s(r, "Branch_Name")), cat: "bank", lon, lat, info: None }
            })
        })
        .collect()
}

/// Fire and rescue stations.
fn fire(rs: &[Map<String, Value>]) -> Vec<ExtPoi> {
    rs.iter()
        .filter_map(|r| {
            let (lat, lon) = (num(r, "Geo_Lat")?, num(r, "Geo_Lon")?);
            in_israel(lon, lat).then(|| ExtPoi { name: format!("תחנת כיבוי והצלה {}", s(r, "Station")), cat: "fire_station", lon, lat, info: None })
        })
        .collect()
}

/// Declared bathing beaches (two ITM end points; the labels are swapped: רוחב = easting).
fn beaches(rs: &[Map<String, Value>]) -> Vec<ExtPoi> {
    rs.iter()
        .filter(|r| !s(r, "סוג חוף").contains("אסור"))
        .filter_map(|r| {
            let a = from_itm(num(r, "נ.צ 1 רוחב"), num(r, "נ.צ 1 אורך"))?;
            let (lon, lat) = match from_itm(num(r, "נ.צ 2 רוחב"), num(r, "נ.צ 2 אורך")) {
                Some(b) => ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0),
                None => a,
            };
            let name = s(r, "שם החוף");
            let name = if name.starts_with("חוף") { name.to_string() } else { format!("חוף {name}") };
            Some(ExtPoi { name, cat: "beach", lon, lat, info: None })
        })
        .collect()
}

/// Public fuel stations (Ministry of Energy, "gas-station": company, name, address, WGS84 and
/// ITM) joined by station number with the register that has the phone ("fuelstationbynumber").
fn fuel(rs: &[Map<String, Value>], phones: &[Map<String, Value>]) -> Vec<ExtPoi> {
    let phone_of: rustc_hash::FxHashMap<i64, String> = phones
        .iter()
        .filter_map(|r| Some((num(r, "station_number")? as i64, s(r, "telephone").to_string())))
        .filter(|(_, p)| !p.is_empty())
        .collect();
    rs.iter()
        .filter_map(|r| {
            let (lon, lat) = match (num(r, "נ.צ. אורך"), num(r, "נ.צ. רוחב")) {
                (Some(lon), Some(lat)) if in_israel(lon, lat) => (lon, lat),
                _ => from_itm(num(r, "X"), num(r, "Y"))?,
            };
            let company = s(r, "חברה");
            let station = s(r, "שם_תחנה");
            let name = match (company.is_empty(), station.is_empty()) {
                (false, false) if station.starts_with(company) => station.to_string(),
                (false, false) => format!("{company} {station}"),
                (false, true) => company.to_string(),
                (true, _) => station.to_string(),
            };
            let town = s(r, "רשות_מקומית");
            let addr = s(r, "כתובת");
            let address = match (addr.is_empty(), town.is_empty() || addr.contains(town)) {
                (true, _) => None,
                (false, true) => Some(addr.to_string()),
                (false, false) => Some(format!("{addr}, {town}")),
            };
            let info = PoiInfo {
                brand: (!company.is_empty()).then(|| company.to_string()),
                address,
                phone: num(r, "מס_מינהל_הדלק").and_then(|n| phone_of.get(&(n as i64)).cloned()),
                ..Default::default()
            };
            Some(ExtPoi { name, cat: "fuel", lon, lat, info: Some(info) })
        })
        .collect()
}

const FUEL_STATIONS: &str = "5537a0ef-3eeb-449c-90c8-51e27564f0cb";
const FUEL_PHONES: &str = "ff3b653c-d268-4eb7-a86b-6de69e77043a";

type Parser = fn(&[Map<String, Value>]) -> Vec<ExtPoi>;

/// (label, resource id, parser)
const DATASETS: [(&str, &str, Parser); 5] = [
    ("מוסדות חינוך", "2055063b-93e8-4d6a-b2c5-c253d18c0cf6", education),
    ("מתקני ספורט", "f8dbd3ed-2c62-4d0e-bbaa-b6a15a0e5f7d", sports),
    ("סניפי בנקים", "2202bada-4baf-45f5-aa61-8c5bad9646d3", banks),
    ("תחנות כיבוי", "7f3009dd-b299-462c-8a6d-9c645b68059f", fire),
    ("חופי רחצה", "48cbb4b2-4492-4a33-93f9-b615e37d8eb5", beaches),
];

/// Every dataset that can be fetched now; `report` gets a Hebrew line per dataset.
pub fn fetch_all(report: &dyn Fn(&str)) -> Vec<ExtPoi> {
    let Ok(client) = crate::data::download::http_client() else { return Vec::new() };
    let mut out = Vec::new();
    for (label, id, parse) in DATASETS {
        report(&format!("מוריד נקודות עניין מ-data.gov.il: {label}"));
        match records(&client, id) {
            Ok(rs) => {
                let pts: Vec<ExtPoi> = parse(&rs).into_iter().filter(|p| !p.name.trim().is_empty()).collect();
                log::info!("data.gov.il {label}: {} records → {} points", rs.len(), pts.len());
                out.extend(pts);
            }
            Err(e) => log::warn!("data.gov.il {label} skipped: {e:#}"),
        }
    }
    report("מוריד נקודות עניין מ-data.gov.il: תחנות דלק");
    match records(&client, FUEL_STATIONS) {
        Ok(rs) => {
            let phones = records(&client, FUEL_PHONES).unwrap_or_else(|e| {
                log::warn!("data.gov.il fuel phones skipped: {e:#}");
                Vec::new()
            });
            let pts = fuel(&rs, &phones);
            log::info!("data.gov.il fuel stations: {} records → {} points", rs.len(), pts.len());
            out.extend(pts);
        }
        Err(e) => log::warn!("data.gov.il fuel stations skipped: {e:#}"),
    }
    out
}

/// One street of a town under all its registered names (official first).
#[derive(Debug, Clone)]
pub struct SynGroup {
    pub city: String,
    pub names: Vec<String>,
}

/// The Population Authority's street list with synonyms ("רשימת רחובות בישראל - מתוך מרשם
/// האוכלוסין", data.gov.il package `israel-streets-synom`, updated weekly): every name of a
/// street points at its official code. Only streets that have more than one name are returned.
const STREET_SYNONYMS: &str = "bf185c7f-1a4e-4662-88c5-fa118a244bda";

pub fn fetch_street_synonyms(report: &dyn Fn(&str)) -> Vec<SynGroup> {
    let Ok(client) = crate::data::download::http_client() else { return Vec::new() };
    report("מוריד שמות חלופיים של רחובות מ-data.gov.il");
    match records_fields(&client, STREET_SYNONYMS, Some("city_code,city_name,street_name,official_code")) {
        Ok(rs) => {
            let groups = synonym_groups(&rs);
            log::info!("data.gov.il street names: {} records → {} streets with synonyms", rs.len(), groups.len());
            groups
        }
        Err(e) => {
            log::warn!("data.gov.il street synonyms skipped: {e:#}");
            Vec::new()
        }
    }
}

fn synonym_groups(rs: &[Map<String, Value>]) -> Vec<SynGroup> {
    use rustc_hash::FxHashMap;
    let mut by: FxHashMap<(i64, i64), (String, Vec<String>)> = FxHashMap::default();
    let mut order: Vec<(i64, i64)> = Vec::new();
    for r in rs {
        let (Some(city), Some(off)) = (num(r, "city_code"), num(r, "official_code")) else { continue };
        let name = s(r, "street_name").split_whitespace().collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            continue;
        }
        let key = (city as i64, off as i64);
        let e = by.entry(key).or_insert_with(|| {
            order.push(key);
            (s(r, "city_name").to_string(), Vec::new())
        });
        if !e.1.contains(&name) {
            e.1.push(name);
        }
    }
    order
        .into_iter()
        .filter_map(|k| by.remove(&k))
        .filter(|(_, names)| names.len() > 1)
        .map(|(city, names)| SynGroup { city, names })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn groups_street_synonyms() {
        let rs: Vec<Map<String, Value>> = [
            json!({"city_code": 5000, "city_name": "תל אביב - יפו", "street_name": "דרך בגין", "official_code": 1555}),
            json!({"city_code": 5000, "city_name": "תל אביב - יפו", "street_name": "מנחם  בגין", "official_code": 1555}),
            json!({"city_code": 5000, "city_name": "תל אביב - יפו", "street_name": "דיזנגוף", "official_code": 1}),
            json!({"city_code": 4000, "city_name": "חיפה", "street_name": "דרך בגין", "official_code": 1555}),
        ]
        .into_iter()
        .map(rec)
        .collect();
        let g = synonym_groups(&rs);
        assert_eq!(g.len(), 1, "single-name streets and other towns' codes stay apart");
        assert_eq!(g[0].city, "תל אביב - יפו");
        assert_eq!(g[0].names, vec!["דרך בגין".to_string(), "מנחם בגין".to_string()]);
    }

    #[test]
    fn parses_samples() {
        let b = banks(&[rec(json!({"Bank_Name": "בנק יהב לעובדי המדינה בע\"מ", "Branch_Name": "עפולה", "Close_Date": "", "X_Coordinate": 32.603528, "Y_Coordinate": 35.294031}))]);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].name, "בנק יהב לעובדי המדינה – סניף עפולה");
        assert!((b[0].lon - 35.294).abs() < 0.001 && (b[0].lat - 32.6035).abs() < 0.001);

        let e = education(&[rec(json!({"TEUR_STATUS": "פעיל", "RAMAT_DIYUK_MIKUM": "גבוהה מאוד", "TEUR_TAT_SUG_MISGERET": "גן ילדים", "SHEM_MOSAD": "גן רימון", "X": 196066, "Y": 665919}))]);
        assert_eq!(e[0].cat, "kindergarten");
        assert!(in_israel(e[0].lon, e[0].lat), "{:?}", e[0]);

        // Eilat beach: easting under "רוחב", northing under "אורך"
        let bch = beaches(&[rec(json!({"שם החוף": "המפרץ", "סוג חוף": "מוכרז", "נ.צ 1 אורך": 384385, "נ.צ 1 רוחב": 196418}))]);
        assert_eq!(bch[0].name, "חוף המפרץ");
        assert!((bch[0].lat - 29.55).abs() < 0.05 && (bch[0].lon - 34.95).abs() < 0.05, "{:?}", bch[0]);

        let sp = sports(&[rec(json!({"סוג מתקן": "בריכת שחיה - 25X12.5 מ'", "שם המתקן": "בריכה עירונית", "ציר X": "210525", "ציר Y": "634529"}))]);
        assert_eq!(sp[0].cat, "swimming_pool");

        let f = fuel(
            &[rec(json!({"מס_מינהל_הדלק": 878, "חברה": "טן", "שם_תחנה": "טבריה", "כתובת": "רח' ויצמן", "רשות_מקומית": "טבריה", "X": 247265, "Y": 742945, "נ.צ. אורך": 35.50056731, "נ.צ. רוחב": 32.78044499}))],
            &[rec(json!({"station_number": 878, "telephone": "04-6737146"}))],
        );
        assert_eq!(f[0].name, "טן טבריה");
        let i = f[0].info.as_ref().unwrap();
        assert_eq!(i.brand.as_deref(), Some("טן"));
        assert_eq!(i.phone.as_deref(), Some("04-6737146"));
        assert_eq!(i.address.as_deref(), Some("רח' ויצמן, טבריה"));
    }
}
