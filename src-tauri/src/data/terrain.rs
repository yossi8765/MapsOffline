//! Elevation: Terrarium-encoded DEM tiles from Mapterhorn (open terrain tiles; Israel from a 10 m
//! DEM, elsewhere Copernicus GLO-30), cut to the Israel bbox at zoom 0–12 with `pmtiles extract`
//! straight from Mapterhorn's planet archive (HTTP range requests). Tiles are 512 px lossless
//! WebP, ~19 m per pixel at zoom 12 (the former AWS terrain tiles were 256 px PNG, ~38 m).
//! The map uses the archive for hillshading, 3D terrain, contour lines and elevation profiles.
//!
//! Mapterhorn is run by volunteers without an SLA: the weekly CI package mirrors the extract, and
//! the app takes it from there first (prebuilt.rs).
use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context};
use reqwest::blocking::Client;

use super::download::{self, http_client};
use super::satellite::{read_tile, verify};
use super::Cancel;
use crate::paths::{atomic_replace, DataPaths, BBOX_ISRAEL};
use crate::types::{DataComponent, LonLat, ProgressFn, ProgressPhase};

/// Mapterhorn's planet archive (zoom 0–12; zoom 13+ lives in regional files).
pub const SOURCE: &str = "https://download.mapterhorn.com/planet.pmtiles";
pub const MAX_ZOOM: u8 = 12;
/// Short attribution for the map (the full source list: mapterhorn.com/attribution).
pub const ATTRIBUTION: &str = "Elevation: Mapterhorn – 10 m DEM of Israel (ASTER GDEM, METI/NASA, processed by 4cast), Copernicus GLO-30 (© DLR/Airbus, provided under COPERNICUS by the EU and ESA)";
/// The Israel extract is ~106 MB; anything far smaller is a broken download.
const MIN_SIZE: u64 = 20 << 20;

/// Version of the dataset = Last-Modified (or ETag) of the planet archive.
pub(crate) fn remote_version(client: &Client) -> anyhow::Result<String> {
    let v = download::remote_version(client, SOURCE).context("איתור גרסת נתוני הגובה נכשל")?;
    Ok(format!("mapterhorn {}", v.unwrap_or_else(|| "unknown".into())))
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
    super::tiles::ensure_cli(paths, progress, cancel)?;
    if cancel.is_cancelled() {
        bail!("cancelled");
    }
    let tmp = dest.with_extension("pmtiles.tmp");
    let _ = std::fs::remove_file(&tmp);
    let b = BBOX_ISRAEL;
    let args = vec![format!("--bbox={},{},{},{}", b[0], b[1], b[2], b[3]), format!("--maxzoom={MAX_ZOOM}")];
    let label = "נתוני גובה (Mapterhorn)";
    progress(ProgressPhase::Downloading, 0, 100, label);
    let mut last_err = None;
    let mut attempt = 0;
    while attempt < 3 {
        match super::tiles::run_extract(paths, SOURCE, &tmp, &args, label, progress, cancel) {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e) => {
                if cancel.is_cancelled() {
                    return Err(e);
                }
                if e.to_string() == "paused" {
                    continue;
                }
                attempt += 1;
                log::warn!("terrain extract attempt {attempt} failed: {e:#}");
                progress(ProgressPhase::Downloading, 0, 100, &format!("ההורדה נכשלה, ניסיון חוזר ({attempt}/3)"));
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
    if let Some(e) = last_err {
        return Err(e);
    }
    progress(ProgressPhase::Finalizing, 0, 0, "מאמת את נתוני הגובה");
    let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if size < MIN_SIZE {
        let _ = std::fs::remove_file(&tmp);
        bail!("terrain extract too small ({size} bytes)");
    }
    if let Err(e) = verify(&tmp, false) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    atomic_replace(&dest, &tmp)?;
    // the former AWS archive (other file name, see paths.rs) is not used any more
    let _ = std::fs::remove_file(paths.legacy_terrain_file());
    Ok(Some(version))
}

// ---------------------------------------------------------------- sampling

/// Pixel coordinate of `lon` at zoom `z` for tiles `size` px wide.
fn lon_to_px(lon: f64, z: u8, size: u32) -> f64 {
    (lon + 180.0) / 360.0 * (u64::from(size) << z) as f64
}

fn lat_to_px(lat: f64, z: u8, size: u32) -> f64 {
    let r = lat.to_radians();
    (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * (u64::from(size) << z) as f64
}

struct DemTile {
    w: usize,
    rgb: Vec<u8>, // RGB or RGBA
    bpp: usize,
}

impl DemTile {
    /// Terrarium height of pixel (x, y), clamped to the tile.
    fn height(&self, x: usize, y: usize) -> f32 {
        let x = x.min(self.w - 1);
        let y = y.min(self.w - 1);
        let i = (y * self.w + x) * self.bpp;
        let (r, g, b) = (self.rgb[i] as f32, self.rgb[i + 1] as f32, self.rgb[i + 2] as f32);
        r * 256.0 + g + b / 256.0 - 32768.0
    }
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

fn decode_webp(bytes: &[u8]) -> anyhow::Result<DemTile> {
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes))?;
    let (w, _h) = d.dimensions();
    let bpp = if d.has_alpha() { 4 } else { 3 };
    let mut buf = vec![0u8; d.output_buffer_size().ok_or_else(|| anyhow!("webp too large"))?];
    d.read_image(&mut buf)?;
    Ok(DemTile { w: w as usize, rgb: buf, bpp })
}

/// A DEM tile in either format (Mapterhorn: WebP; archives from older versions: PNG).
fn decode_tile(bytes: &[u8]) -> anyhow::Result<DemTile> {
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        decode_webp(bytes)
    } else {
        decode_png(bytes)
    }
}

/// Elevation (m) at each point from the installed DEM, bilinear within the top-zoom tile.
/// Points outside the archive get `None`. Works for 256 and 512 px tiles.
pub fn elevations(archive: &Path, points: &[LonLat]) -> anyhow::Result<Vec<Option<f32>>> {
    let z = archive_max_zoom(archive).unwrap_or(MAX_ZOOM);
    let mut cache: HashMap<(u32, u32), Option<DemTile>> = HashMap::new();
    let mut out = Vec::with_capacity(points.len());
    for &[lon, lat] in points {
        // tile x/y do not depend on the tile size
        let (tx, ty) = (lon_to_px(lon, z, 1).floor() as u32, lat_to_px(lat, z, 1).floor() as u32);
        let tile = cache.entry((tx, ty)).or_insert_with(|| read_tile(archive, z, tx, ty).ok().flatten().and_then(|b| decode_tile(&b).ok()));
        let Some(t) = tile else {
            out.push(None);
            continue;
        };
        let size = t.w as u32;
        let (px, py) = (lon_to_px(lon, z, size), lat_to_px(lat, z, size));
        let (fx, fy) = (px - f64::from(tx) * f64::from(size), py - f64::from(ty) * f64::from(size));
        let (x0, y0) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
        let (ax, ay) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let h = t.height(x0, y0) * (1.0 - ax) * (1.0 - ay) + t.height(x0 + 1, y0) * ax * (1.0 - ay) + t.height(x0, y0 + 1) * (1.0 - ax) * ay + t.height(x0 + 1, y0 + 1) * ax * ay;
        out.push(Some(h));
    }
    Ok(out)
}

/// Highest zoom stored in a PMTiles archive (header byte 101).
fn archive_max_zoom(p: &Path) -> Option<u8> {
    use std::io::Read;
    let mut h = [0u8; 127];
    std::fs::File::open(p).ok()?.read_exact(&mut h).ok()?;
    (&h[..7] == b"PMTiles").then_some(h[101])
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIXELS: [u8; 12] = [128, 0, 0, 128, 10, 128, 128, 0, 0, 128, 0, 0];

    #[test]
    fn terrarium_decoding_png() {
        // a 2x2 RGB png: (R,G,B) = (128,0,0) => 0 m; (128,10,128) => 10.5 m
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 2, 2);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&PIXELS).unwrap();
        }
        let t = decode_tile(&png_bytes).unwrap();
        assert_eq!(t.w, 2);
        assert!((t.height(1, 0) - 10.5).abs() < 1e-3);
        assert!(t.height(0, 0).abs() < 1e-3);
    }

    #[test]
    fn terrarium_decoding_webp() {
        let mut buf = Vec::new();
        image_webp::WebPEncoder::new(&mut buf).encode(&PIXELS, 2, 2, image_webp::ColorType::Rgb8).unwrap();
        let t = decode_tile(&buf).unwrap();
        assert_eq!(t.w, 2);
        assert!((t.height(1, 0) - 10.5).abs() < 1e-3, "{}", t.height(1, 0));
    }

    /// Real Mapterhorn extract: Mount Meron ~1,200 m, the Dead Sea surface ~-430 m, Jerusalem ~780 m.
    /// MO_DEM=<path> cargo test real_mapterhorn_extract -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_mapterhorn_extract() {
        let p = std::env::var("MO_DEM").expect("MO_DEM=<path to israel extract>");
        let v = elevations(Path::new(&p), &[[35.4097, 32.9973], [35.50, 31.50], [35.2137, 31.7683]]).unwrap();
        eprintln!("{v:?}");
        let v: Vec<f32> = v.into_iter().map(|h| h.expect("inside")).collect();
        assert!(v[0] > 1000.0 && v[1] < -350.0 && v[2] > 600.0, "{v:?}");
    }
}
