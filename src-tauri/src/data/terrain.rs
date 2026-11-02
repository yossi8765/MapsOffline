//! Elevation: Terrarium-encoded DEM tiles (AWS Open Data "terrain-tiles", SRTM/ASTER/… mosaic,
//! ~30 m) for the Israel bbox, zoom 0–12, packed into a raster PMTiles archive. The map uses it
//! for hillshading, 3D terrain, contour lines and route elevation profiles.
use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, Context};
use reqwest::blocking::Client;

use super::download::{self, http_client};
use super::satellite::{download_raster, read_tile, verify, RasterSpec};
use super::Cancel;
use crate::paths::{DataPaths, BBOX_ISRAEL};
use crate::types::{DataComponent, LonLat, ProgressFn, ProgressPhase};

const BASE: &str = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/";
pub const MAX_ZOOM: u8 = 12;
const ATTRIBUTION: &str = "Elevation: Mapzen terrain tiles (AWS Open Data) – SRTM, ASTER GDEM, EU-DEM and others";

fn tile_url(z: u8, x: u32, y: u32) -> String {
    format!("{BASE}{z}/{x}/{y}.png")
}

/// Version of the dataset = Last-Modified of the world tile (the mosaic is static since 2017).
pub(crate) fn remote_version(client: &Client) -> anyhow::Result<String> {
    let url = tile_url(0, 0, 0);
    let v = download::remote_version(client, &url).context("איתור גרסת נתוני הגובה נכשל")?;
    Ok(format!("terrarium {}", v.unwrap_or_else(|| "unknown".into())))
}

pub fn update_terrain(paths: &DataPaths, progress: ProgressFn, cancel: &Cancel) -> anyhow::Result<Option<String>> {
    paths.ensure_dirs()?;
    progress(ProgressPhase::Processing, 0, 0, "בודק את גרסת נתוני הגובה");
    let client = http_client()?;
    let version = remote_version(&client)?;
    let dest = paths.terrain_file();
    let m = super::manifest::Manifest::load(paths);
    if dest.exists() && m.entry(DataComponent::Terrain).and_then(|e| e.version.as_deref()) == Some(version.as_str()) && verify(&dest, false).is_ok() {
        progress(ProgressPhase::Processing, 1, 1, "נתוני הגובה כבר מעודכנים");
        return Ok(Some(version));
    }
    let spec = RasterSpec {
        label: "נתוני גובה",
        url: &tile_url,
        max_zoom: MAX_ZOOM,
        skip_sea: false,
        tile_type: 2,
        placeholders: false,
        name: "Terrain (Terrarium DEM)".into(),
        attribution: ATTRIBUTION.into(),
        bbox: BBOX_ISRAEL,
    };
    download_raster(&dest, &version, &spec, &client, progress, cancel)?;
    Ok(Some(version))
}

// ---------------------------------------------------------------- sampling

fn lon_to_px(lon: f64, z: u8) -> f64 {
    (lon + 180.0) / 360.0 * (256u64 << z) as f64
}

fn lat_to_px(lat: f64, z: u8) -> f64 {
    let r = lat.to_radians();
    (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * (256u64 << z) as f64
}

struct DemTile {
    w: usize,
    rgb: Vec<u8>, // RGB or RGBA
    bpp: usize,
}

fn decode_png(bytes: &[u8]) -> anyhow::Result<DemTile> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let bpp = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => return Err(anyhow!("unsupported DEM png colour type {other:?}")),
    };
    buf.truncate(info.buffer_size());
    Ok(DemTile { w: info.width as usize, rgb: buf, bpp })
}

/// Elevation (m) at each point from the installed DEM, bilinear within the zoom-12 tile.
/// Points outside the archive get `None`.
pub fn elevations(archive: &Path, points: &[LonLat]) -> anyhow::Result<Vec<Option<f32>>> {
    let z = MAX_ZOOM;
    let mut cache: HashMap<(u32, u32), Option<DemTile>> = HashMap::new();
    let mut out = Vec::with_capacity(points.len());
    for &[lon, lat] in points {
        let (px, py) = (lon_to_px(lon, z), lat_to_px(lat, z));
        let (tx, ty) = ((px / 256.0).floor() as u32, (py / 256.0).floor() as u32);
        let tile = cache.entry((tx, ty)).or_insert_with(|| read_tile(archive, z, tx, ty).ok().flatten().and_then(|b| decode_png(&b).ok()));
        let Some(t) = tile else {
            out.push(None);
            continue;
        };
        let sample = |x: usize, y: usize| -> f32 {
            let x = x.min(t.w - 1);
            let y = y.min(t.w - 1);
            let i = (y * t.w + x) * t.bpp;
            let (r, g, b) = (t.rgb[i] as f32, t.rgb[i + 1] as f32, t.rgb[i + 2] as f32);
            r * 256.0 + g + b / 256.0 - 32768.0
        };
        let (fx, fy) = (px - tx as f64 * 256.0, py - ty as f64 * 256.0);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (ax, ay) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let h = sample(x0, y0) * (1.0 - ax) * (1.0 - ay) + sample(x0 + 1, y0) * ax * (1.0 - ay) + sample(x0, y0 + 1) * (1.0 - ax) * ay + sample(x0 + 1, y0 + 1) * ax * ay;
        out.push(Some(h));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrarium_decoding() {
        // a 2x2 RGB png: (R,G,B) = (128,0,0) => 0 m; (128,10,128) => 10.5 m
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 2, 2);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[128, 0, 0, 128, 10, 128, 128, 0, 0, 128, 0, 0]).unwrap();
        }
        let t = decode_png(&png_bytes).unwrap();
        assert_eq!(t.w, 2);
        let i = (0 * 2 + 1) * 3;
        let h = t.rgb[i] as f32 * 256.0 + t.rgb[i + 1] as f32 + t.rgb[i + 2] as f32 / 256.0 - 32768.0;
        assert!((h - 10.5).abs() < 1e-3);
    }
}
