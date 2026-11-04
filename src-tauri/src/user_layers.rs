//! User layers: imported GPX / KML / GeoJSON (converted to GeoJSON by the webview), drawings and
//! recorded tracks. Each layer is `<data>/userlayers/<id>.geojson`; `index.json` lists them.
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use serde_json::Value;

use crate::paths::{atomic_replace, DataPaths};
use crate::types::{UserLayer, UserLayerPatch};

/// A layer bigger than this is refused (a GPX of a year of tracking is a few MB).
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
const COLORS: [&str; 6] = ["#d6336c", "#1c7ed6", "#2f9e44", "#f08c00", "#7048e8", "#0c8599"];

static LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

fn index_file(dir: &Path) -> PathBuf {
    dir.join("index.json")
}

fn layer_file(dir: &Path, id: &str) -> anyhow::Result<PathBuf> {
    if !valid_id(id) {
        bail!("INVALID_PATH: bad layer id {id:?}");
    }
    Ok(dir.join(format!("{id}.geojson")))
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn load_index(dir: &Path) -> Vec<UserLayer> {
    std::fs::read(index_file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_index(dir: &Path, v: &[UserLayer]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("index.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    atomic_replace(&index_file(dir), &tmp)?;
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("geojson.tmp");
    std::fs::write(&tmp, bytes)?;
    atomic_replace(path, &tmp)?;
    Ok(())
}

/// Any GeoJSON object → a FeatureCollection; returns it with its feature count and bbox.
pub fn normalize(text: &str) -> anyhow::Result<(Value, usize, Option<[f64; 4]>)> {
    if text.len() > MAX_BYTES {
        bail!("INVALID_LAYER: the layer is larger than {} MB", MAX_BYTES / 1024 / 1024);
    }
    let v: Value = serde_json::from_str(text).context("INVALID_LAYER: not GeoJSON")?;
    let fc = match v.get("type").and_then(Value::as_str) {
        Some("FeatureCollection") => {
            if !v.get("features").is_some_and(Value::is_array) {
                bail!("INVALID_LAYER: FeatureCollection without features");
            }
            v
        }
        Some("Feature") => serde_json::json!({ "type": "FeatureCollection", "features": [v] }),
        Some("Point" | "MultiPoint" | "LineString" | "MultiLineString" | "Polygon" | "MultiPolygon" | "GeometryCollection") => {
            serde_json::json!({ "type": "FeatureCollection", "features": [{ "type": "Feature", "properties": {}, "geometry": v }] })
        }
        _ => bail!("INVALID_LAYER: not GeoJSON"),
    };
    let n = fc["features"].as_array().map_or(0, Vec::len);
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    fn walk(v: &Value, b: &mut [f64; 4]) {
        match v {
            Value::Array(a) if a.len() >= 2 && a[0].is_number() && a[1].is_number() => {
                let (x, y) = (a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0));
                if (-180.0..=180.0).contains(&x) && (-90.0..=90.0).contains(&y) {
                    b[0] = b[0].min(x);
                    b[1] = b[1].min(y);
                    b[2] = b[2].max(x);
                    b[3] = b[3].max(y);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, b)),
            Value::Object(o) => {
                if let Some(c) = o.get("coordinates") {
                    walk(c, b);
                }
                for k in ["geometry", "geometries", "features"] {
                    if let Some(c) = o.get(k) {
                        walk(c, b);
                    }
                }
            }
            _ => {}
        }
    }
    walk(&fc, &mut b);
    let bbox = b[0].is_finite().then_some(b);
    Ok((fc, n, bbox))
}

pub fn list(paths: &DataPaths) -> Vec<UserLayer> {
    let dir = paths.userlayers_dir();
    let _g = LOCK.lock();
    // only entries whose file is still there
    load_index(&dir).into_iter().filter(|l| layer_file(&dir, &l.id).is_ok_and(|f| f.exists())).collect()
}

pub fn get(paths: &DataPaths, id: &str) -> anyhow::Result<String> {
    let f = layer_file(&paths.userlayers_dir(), id)?;
    std::fs::read_to_string(&f).with_context(|| format!("layer {id} not found"))
}

pub fn save(paths: &DataPaths, name: &str, kind: &str, geojson: &str, color: Option<&str>) -> anyhow::Result<UserLayer> {
    if !matches!(kind, "import" | "drawing" | "track") {
        bail!("INVALID_LAYER: unknown kind {kind}");
    }
    let (fc, features, bbox) = normalize(geojson)?;
    let dir = paths.userlayers_dir();
    std::fs::create_dir_all(&dir)?;
    let _g = LOCK.lock();
    let mut idx = load_index(&dir);
    let now = chrono::Local::now();
    let mut id = format!("{}{:03}", now.format("%Y%m%d%H%M%S"), now.timestamp_subsec_millis());
    while idx.iter().any(|l| l.id == id) {
        id.push('x');
    }
    let color = color.filter(|c| is_color(c)).map(str::to_string).unwrap_or_else(|| COLORS[idx.len() % COLORS.len()].to_string());
    write_atomic(&layer_file(&dir, &id)?, serde_json::to_string(&fc)?.as_bytes())?;
    let name = name.trim();
    let layer = UserLayer {
        id,
        name: if name.is_empty() { "שכבה".to_string() } else { name.chars().take(120).collect() },
        kind: kind.to_string(),
        color,
        visible: true,
        created_iso: now.to_rfc3339(),
        features,
        bbox,
    };
    idx.push(layer.clone());
    save_index(&dir, &idx)?;
    Ok(layer)
}

pub fn update(paths: &DataPaths, id: &str, p: &UserLayerPatch) -> anyhow::Result<UserLayer> {
    let dir = paths.userlayers_dir();
    let file = layer_file(&dir, id)?;
    let _g = LOCK.lock();
    let mut idx = load_index(&dir);
    let Some(l) = idx.iter_mut().find(|l| l.id == id) else { bail!("layer {id} not found") };
    if let Some(n) = &p.name {
        let n = n.trim();
        if !n.is_empty() {
            l.name = n.chars().take(120).collect();
        }
    }
    if let Some(c) = &p.color {
        if !is_color(c) {
            bail!("INVALID_COLOR: {c}");
        }
        l.color = c.clone();
    }
    if let Some(v) = p.visible {
        l.visible = v;
    }
    if let Some(g) = &p.geojson {
        let (fc, n, bbox) = normalize(g)?;
        write_atomic(&file, serde_json::to_string(&fc)?.as_bytes())?;
        l.features = n;
        l.bbox = bbox;
    }
    let out = l.clone();
    save_index(&dir, &idx)?;
    Ok(out)
}

pub fn delete(paths: &DataPaths, id: &str) -> anyhow::Result<()> {
    let dir = paths.userlayers_dir();
    let file = layer_file(&dir, id)?;
    let _g = LOCK.lock();
    let mut idx = load_index(&dir);
    idx.retain(|l| l.id != id);
    save_index(&dir, &idx)?;
    let _ = std::fs::remove_file(file);
    Ok(())
}

fn is_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_lifecycle() {
        let dir = std::env::temp_dir().join(format!("mo-userlayers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = DataPaths::new(&dir);
        let line = r#"{"type":"LineString","coordinates":[[34.7,32.0],[35.2,31.7]]}"#;
        let l = save(&paths, "מסלול", "import", line, None).unwrap();
        assert_eq!(l.features, 1);
        assert_eq!(l.bbox, Some([34.7, 31.7, 35.2, 32.0]));
        assert_eq!(list(&paths).len(), 1);
        let text = get(&paths, &l.id).unwrap();
        assert!(text.contains("FeatureCollection"));
        let pt = r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[35,31]}},{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[35.1,31.1]}}]}"#;
        let u = update(&paths, &l.id, &UserLayerPatch { name: Some("חדש".into()), color: Some("#00ff00".into()), visible: Some(false), geojson: Some(pt.into()) }).unwrap();
        assert_eq!((u.name.as_str(), u.color.as_str(), u.visible, u.features), ("חדש", "#00ff00", false, 2));
        assert!(update(&paths, &l.id, &UserLayerPatch { color: Some("green".into()), ..Default::default() }).is_err());
        assert!(save(&paths, "x", "import", "{\"a\":1}", None).is_err());
        assert!(get(&paths, "../settings").is_err());
        delete(&paths, &l.id).unwrap();
        assert!(list(&paths).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
