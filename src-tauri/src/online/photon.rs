//! Online search when no local address index is installed: Photon (photon.komoot.io), the
//! search-as-you-type geocoder built on OpenStreetMap. Short timeouts; any failure simply means
//! no online results (the caller then reports the missing index).

use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;

use crate::types::{GeoExtra, GeoKind, GeoPrecision, GeoResult, LonLat};

const API: &str = "https://photon.komoot.io";

/// One client for the whole run: search-as-you-type sends a request per pause in typing, and a
/// kept-alive connection saves the TLS handshake each time (hundreds of ms on mobile data).
fn client() -> anyhow::Result<reqwest::blocking::Client> {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let c = reqwest::blocking::Client::builder()
        .user_agent(super::USER_AGENT)
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(7))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()?;
    Ok(CLIENT.get_or_init(|| c).clone())
}

/// Recent answers (query + rounded position → results): typing back over a query, or the same
/// search again, shows at once without a round trip.
type CacheKey = (String, i32, i32, usize);
static CACHE: Mutex<Vec<(CacheKey, Vec<GeoResult>)>> = Mutex::new(Vec::new());
const CACHE_LEN: usize = 64;

fn s(p: &Value, k: &str) -> Option<String> {
    p.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

/// One Photon feature → a GeoResult (None without coordinates).
fn to_result(f: &Value) -> Option<GeoResult> {
    let c = f.pointer("/geometry/coordinates")?.as_array()?;
    let (lon, lat) = (c.first()?.as_f64()?, c.get(1)?.as_f64()?);
    let p = f.get("properties")?;
    let city = s(p, "city").or_else(|| s(p, "locality")).or_else(|| s(p, "county"));
    let street = s(p, "street");
    let house = s(p, "housenumber");
    let name = s(p, "name");
    let key = s(p, "osm_key").unwrap_or_default();
    let value = s(p, "osm_value").unwrap_or_default();
    let typ = s(p, "type").unwrap_or_default();
    let id = format!("photon:{}{}", s(p, "osm_type").unwrap_or_default(), p.get("osm_id").and_then(Value::as_i64).unwrap_or(0));
    let with_city = |label: String| match &city {
        Some(c) if !label.contains(c.as_str()) => format!("{label}, {c}"),
        _ => label,
    };
    let (kind, precision, label, category) = match typ.as_str() {
        "house" if house.is_some() && (name.is_none() || key == "place") => {
            let label = match (&street, &house) {
                (Some(st), Some(h)) => format!("{st} {h}"),
                (Some(st), None) => st.clone(),
                _ => name.clone().unwrap_or_default(),
            };
            (GeoKind::Address, GeoPrecision::Exact, with_city(label), None)
        }
        "street" => (GeoKind::Street, GeoPrecision::Street, with_city(name.clone().or_else(|| street.clone())?), None),
        "city" | "district" | "locality" | "county" | "state" | "country" => (GeoKind::Place, GeoPrecision::City, name.clone()?, Some(value.clone())),
        _ if key == "place" => (GeoKind::Place, GeoPrecision::City, name.clone()?, Some(value.clone())),
        _ => (GeoKind::Poi, GeoPrecision::Exact, with_city(name.clone()?), Some(format!("{key}:{value}"))),
    };
    let address = match (&street, &house) {
        (Some(st), Some(h)) if kind == GeoKind::Poi => Some(with_city(format!("{st} {h}"))),
        _ => None,
    };
    let extra = GeoExtra { address, ..Default::default() };
    Some(GeoResult {
        id,
        name: label,
        kind,
        precision,
        lon,
        lat,
        city,
        street,
        house_number: house,
        category,
        score: 0.0,
        extra: (!extra.is_empty()).then(|| Box::new(extra)),
    })
}

fn parse(body: &Value, limit: usize) -> Vec<GeoResult> {
    let mut out: Vec<GeoResult> = Vec::new();
    for r in body.get("features").and_then(Value::as_array).into_iter().flatten().filter_map(to_result) {
        // OSM often has a place twice (its node and its boundary): same name and kind, a few km apart
        let dup = out.iter().any(|o| o.kind == r.kind && o.name == r.name && (o.lon - r.lon).abs() < 0.05 && (o.lat - r.lat).abs() < 0.05);
        if !dup {
            out.push(r);
        }
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Search `query` (results near `near` first; Israel's centre when none).
pub fn search(query: &str, near: Option<LonLat>, limit: usize) -> anyhow::Result<Vec<GeoResult>> {
    let [lon, lat] = near.unwrap_or([34.9, 31.8]);
    // ~1 km grid: the bias point moves with the map, the answer barely does
    let key: CacheKey = (query.trim().to_lowercase(), (lon * 100.0).round() as i32, (lat * 100.0).round() as i32, limit);
    if let Some((_, v)) = CACHE.lock().iter().find(|(k, _)| *k == key) {
        return Ok(v.clone());
    }
    let body: Value = client()?
        .get(format!("{API}/api/"))
        .query(&[("q", query), ("limit", &limit.clamp(1, 20).to_string()), ("lang", "default"), ("lon", &format!("{lon:.5}")), ("lat", &format!("{lat:.5}"))])
        .send()?
        .error_for_status()?
        .json()?;
    let out = parse(&body, limit);
    let mut c = CACHE.lock();
    if c.len() >= CACHE_LEN {
        c.remove(0);
    }
    c.push((key, out.clone()));
    Ok(out)
}

/// The nearest address / place to a point.
pub fn reverse(lon: f64, lat: f64) -> anyhow::Result<Option<GeoResult>> {
    let body: Value = client()?
        .get(format!("{API}/reverse"))
        .query(&[("lon", format!("{lon:.6}")), ("lat", format!("{lat:.6}")), ("lang", "default".into()), ("limit", "1".into())])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(parse(&body, 1).into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_photon_features() {
        let body: Value = serde_json::from_str(
            r#"{"features":[
              {"geometry":{"coordinates":[35.2,31.78]},"properties":{"osm_type":"N","osm_id":1,"osm_key":"place","osm_value":"house","type":"house","street":"זולטי","housenumber":"7","city":"ירושלים"}},
              {"geometry":{"coordinates":[35.21,31.77]},"properties":{"osm_type":"W","osm_id":2,"osm_key":"highway","osm_value":"residential","type":"street","name":"זולטי","city":"ירושלים"}},
              {"geometry":{"coordinates":[35.22,31.76]},"properties":{"osm_type":"N","osm_id":3,"osm_key":"amenity","osm_value":"cafe","type":"house","name":"קפה","street":"יפו","housenumber":"1","city":"ירושלים"}},
              {"geometry":{"coordinates":[35.21,31.78]},"properties":{"osm_type":"R","osm_id":4,"osm_key":"place","osm_value":"city","type":"city","name":"ירושלים"}}
            ]}"#,
        )
        .unwrap();
        let r = parse(&body, 10);
        assert_eq!(r.len(), 4);
        assert_eq!((r[0].kind, r[0].name.as_str()), (GeoKind::Address, "זולטי 7, ירושלים"));
        assert_eq!((r[1].kind, r[1].name.as_str()), (GeoKind::Street, "זולטי, ירושלים"));
        assert_eq!(r[2].kind, GeoKind::Poi);
        assert_eq!(r[2].extra.as_ref().and_then(|e| e.address.clone()).as_deref(), Some("יפו 1, ירושלים"));
        assert_eq!(r[3].kind, GeoKind::Place);
    }
}
