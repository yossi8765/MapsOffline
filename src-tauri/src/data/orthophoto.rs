//! Aerial orthophoto of the Survey of Israel (מפ"י) from govmap's public tile CDN, rendered into
//! the satellite archive at up to ~0.13 m/pixel (vs. Sentinel-2's 10 m).
//!
//! The CDN is an ArcGIS tile cache on the Israeli TM Grid (ITM, EPSG:2039), not WebMercator:
//! `https://cdn.govmap.gov.il/<cache>/L<level 2 digits>/R<row 8 hex>/C<col 8 hex>.jpg`, origin
//! (-5403700, 7116700) top-left, 256 px tiles, levels L00–L11 (793.75 … 0.132 m/px). A tile that
//! does not exist (sea, outside the coverage) answers 403 AccessDenied (S3 behind CloudFront).
//! Every WebMercator tile is resampled from the ITM tiles under it (bilinear, supersampled when
//! the source is finer than the target). Grid and projection follow hebrew-maps (ADR 0003,
//! `hmaps/api/satellite.py`, `hmaps/core/itm.py`).
//!
//! Zooms 0–9 come from Sentinel-2 (they also cover the neighbouring countries and the sea);
//! zoom 10 and up from the orthophoto, inside a rough outline of the country.
//!
//! Licence: govmap's terms keep all rights with the Survey of Israel and allow personal use only;
//! the imagery is downloaded by each user to their own computer and never redistributed.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use reqwest::blocking::Client;

use super::download;
use super::satellite::{fetch_tile, tile_lat, tile_lon, tile_url};
use super::Cancel;

const HOST: &str = "https://cdn.govmap.gov.il";
/// Orthophoto flown 2023, published 2024 (~0.13 m/px).
pub const CACHE: &str = "B0BZ1ORTO23";
pub const ATTRIBUTION: &str = "תצלומי אוויר © המרכז למיפוי ישראל (govmap), לשימוש אישי; זומים 0–9: Sentinel-2 cloudless by EOX (CC BY-NC-SA 4.0)";
const ORIGIN: (f64, f64) = (-5_403_700.0, 7_116_700.0);
const RES: [f64; 12] = [
    793.751587503175,
    264.583862501058,
    132.291931250529,
    66.1459656252646,
    26.4583862501058,
    13.2291931250529,
    6.61459656252646,
    2.64583862501058,
    1.32291931250529,
    0.661459656252646,
    0.330729828126323,
    0.13229193125052918,
];
/// Zooms up to this one come from Sentinel-2 (EOX); above it from the orthophoto.
pub const EOX_MAX_ZOOM: u8 = 9;
/// Parallel renderers (each fetches its source tiles; CloudFront takes this easily).
pub const WORKERS: usize = 16;
/// Encoded source tiles kept in memory (~20–40 KB each, so at most ~80 MB). Was 8,192 (up to
/// ~330 MB); the decoded cache below now catches the re-use between neighbouring target tiles.
const CACHE_TILES: usize = 2048;
/// Decoded (256 × 256 RGB = 192 KB) source tiles shared by the renderers: neighbouring z17 target
/// tiles read the same 4–9 source tiles, which used to be JPEG-decoded again by every worker.
const DECODED_TILES: usize = 192;
const JPEG_QUALITY: u8 = 85;

/// Max zoom for the quality setting.
pub fn quality_zoom(q: &str) -> u8 {
    match q {
        "low" => 14,
        "medium" => 15,
        "max" => 17,
        _ => 16,
    }
}

pub fn archive_version(quality: &str) -> String {
    format!("govmap-{CACHE} z{}", quality_zoom(quality))
}

pub fn is_govmap_version(v: &str) -> bool {
    v.starts_with("govmap-")
}

/// Rough outline of Israel incl. Judea & Samaria, the Golan and the Gaza Strip (hebrew-maps'
/// `ISRAEL_POLYGON`), lon/lat.
const OUTLINE: &[(f64, f64)] = &[
    (35.10, 33.11), (35.07, 33.02), (35.05, 32.92), (34.99, 32.85), (34.94, 32.83), (34.93, 32.73),
    (34.90, 32.60), (34.86, 32.46), (34.83, 32.33), (34.78, 32.17), (34.73, 32.06), (34.68, 31.95),
    (34.60, 31.80), (34.50, 31.65), (34.40, 31.53), (34.27, 31.38), (34.19, 31.31), (34.23, 31.20),
    (34.33, 31.00), (34.38, 30.85), (34.48, 30.60), (34.56, 30.40), (34.68, 30.12), (34.80, 29.85),
    (34.86, 29.52), (34.88, 29.46), (34.95, 29.47), (34.99, 29.53), (35.06, 29.80), (35.13, 30.05),
    (35.17, 30.35), (35.23, 30.62), (35.31, 30.80), (35.42, 30.93), (35.46, 31.10), (35.50, 31.30), (35.55, 31.55),
    (35.58, 31.80), (35.58, 32.10), (35.60, 32.40), (35.61, 32.62), (35.70, 32.68), (35.80, 32.69),
    (35.87, 32.76), (35.92, 32.95), (35.89, 33.13), (35.87, 33.30), (35.86, 33.43), (35.78, 33.43), (35.70, 33.36),
    (35.62, 33.30), (35.55, 33.29), (35.53, 33.14), (35.42, 33.13), (35.25, 33.12), (35.12, 33.13),
];

fn in_outline(lon: f64, lat: f64) -> bool {
    let mut inside = false;
    let n = OUTLINE.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = OUTLINE[i];
        let (xj, yj) = OUTLINE[j];
        if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Does tile (z, x, y) touch the country outline? Zooms up to [`EOX_MAX_ZOOM`] keep the whole bbox.
pub fn wanted(z: u8, x: u32, y: u32) -> bool {
    if z <= EOX_MAX_ZOOM {
        return true;
    }
    let (w, e) = (tile_lon(x, z), tile_lon(x + 1, z));
    let (n, s) = (tile_lat(y, z), tile_lat(y + 1, z));
    // an outline vertex inside the tile, or a sample point of the tile inside the outline
    if OUTLINE.iter().any(|&(lo, la)| lo >= w && lo <= e && la >= s && la <= n) {
        return true;
    }
    const K: usize = 6;
    (0..=K).any(|i| {
        (0..=K).any(|j| in_outline(w + (e - w) * i as f64 / K as f64, s + (n - s) * j as f64 / K as f64))
    })
}

// ---------------------------------------------------------------- ITM projection

/// WGS 84 lon/lat (degrees) → ITM easting/northing (m): 7-parameter Helmert shift to the
/// Israel 1993 datum (EPSG:9676, position vector) and the Krüger-series transverse Mercator on GRS80.
pub mod itm {
    use std::sync::OnceLock;

    const A: f64 = 6_378_137.0;
    const F_WGS84: f64 = 1.0 / 298.257223563;
    const F_GRS80: f64 = 1.0 / 298.257222101;
    const LAT0_DEG: f64 = 31.734_393_611_111_1;
    const LON0_DEG: f64 = 35.204_516_944_444_4;
    const K0: f64 = 1.000_006_7;
    const X0: f64 = 219_529.584;
    const Y0: f64 = 626_907.39;
    const T: [f64; 3] = [23.772, 17.49, 17.859];
    const ROT_ARCSEC: [f64; 3] = [-0.3132, -1.85274, 1.67299];
    const S: f64 = -5.4262e-6;

    struct Consts {
        aa: f64,
        alpha: [f64; 4],
        e: f64,
        north0: f64,
        rx: f64,
        ry: f64,
        rz: f64,
    }

    fn consts() -> &'static Consts {
        static C: OnceLock<Consts> = OnceLock::new();
        C.get_or_init(|| {
            let n = F_GRS80 / (2.0 - F_GRS80);
            let (n2, n3, n4) = (n * n, n * n * n, n * n * n * n);
            let aa = A / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0);
            let alpha = [
                n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0,
                13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0,
                61.0 * n3 / 240.0 - 103.0 * n4 / 140.0,
                49561.0 * n4 / 161280.0,
            ];
            let e = (F_GRS80 * (2.0 - F_GRS80)).sqrt();
            let r = |a: f64| (a / 3600.0).to_radians();
            let mut c = Consts { aa, alpha, e, north0: 0.0, rx: r(ROT_ARCSEC[0]), ry: r(ROT_ARCSEC[1]), rz: r(ROT_ARCSEC[2]) };
            c.north0 = tm(&c, LAT0_DEG.to_radians(), LON0_DEG.to_radians()).1;
            c
        })
    }

    fn tm(c: &Consts, lat: f64, lon: f64) -> (f64, f64) {
        let s = lat.sin();
        let t = (s.atanh() - c.e * (c.e * s).atanh()).sinh();
        let dl = lon - LON0_DEG.to_radians();
        let xi_ = t.atan2(dl.cos());
        let eta_ = (dl.sin() / (1.0 + t * t).sqrt()).atanh();
        let (mut xi, mut eta) = (xi_, eta_);
        for (j, a) in c.alpha.iter().enumerate() {
            let k = 2.0 * (j + 1) as f64;
            xi += a * (k * xi_).sin() * (k * eta_).cosh();
            eta += a * (k * xi_).cos() * (k * eta_).sinh();
        }
        (K0 * c.aa * eta, K0 * c.aa * xi)
    }

    pub fn from_wgs84(lon_deg: f64, lat_deg: f64) -> (f64, f64) {
        let c = consts();
        let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
        // geodetic (WGS 84) → ECEF
        let e2 = F_WGS84 * (2.0 - F_WGS84);
        let sl = lat.sin();
        let nu = A / (1.0 - e2 * sl * sl).sqrt();
        let x = nu * lat.cos() * lon.cos() - T[0];
        let y = nu * lat.cos() * lon.sin() - T[1];
        let z = nu * (1.0 - e2) * sl - T[2];
        // WGS 84 → local: transpose of the position-vector rotation, then scale
        let k = 1.0 + S;
        let lx = (x + c.rz * y - c.ry * z) / k;
        let ly = (-c.rz * x + y + c.rx * z) / k;
        let lz = (c.ry * x - c.rx * y + z) / k;
        // ECEF → geodetic (GRS80)
        let e2g = F_GRS80 * (2.0 - F_GRS80);
        let p = lx.hypot(ly);
        let lo = ly.atan2(lx);
        let mut la = lz.atan2(p * (1.0 - e2g));
        for _ in 0..4 {
            let s = la.sin();
            let nu = A / (1.0 - e2g * s * s).sqrt();
            la = (lz + e2g * nu * s).atan2(p);
        }
        let (e, n) = tm(c, la, lo);
        (X0 + e, Y0 + (n - c.north0))
    }

    /// ITM metres → WGS 84 lon/lat (degrees): Newton iteration on [`from_wgs84`] (it is smooth;
    /// 4–5 steps reach millimetres inside Israel).
    pub fn to_wgs84(e: f64, n: f64) -> (f64, f64) {
        let (mut lon, mut lat) = (LON0_DEG + (e - X0) / 94_000.0, LAT0_DEG + (n - Y0) / 110_900.0);
        for _ in 0..8 {
            let (fe, fn_) = from_wgs84(lon, lat);
            let (de, dn) = (fe - e, fn_ - n);
            if de.abs() < 1e-4 && dn.abs() < 1e-4 {
                break;
            }
            const H: f64 = 1e-5;
            let (e1, n1) = from_wgs84(lon + H, lat);
            let (e2, n2) = from_wgs84(lon, lat + H);
            let (a, b, c, d) = ((e1 - fe) / H, (e2 - fe) / H, (n1 - fn_) / H, (n2 - fn_) / H);
            let det = a * d - b * c;
            if det.abs() < 1e-12 {
                break;
            }
            lon -= (d * de - b * dn) / det;
            lat -= (-c * de + a * dn) / det;
        }
        (lon, lat)
    }
}

// ---------------------------------------------------------------- source tiles

fn raw_url(level: usize, row: u32, col: u32) -> String {
    format!("{HOST}/{CACHE}/L{level:02}/R{row:08x}/C{col:08x}.jpg")
}

type Raw = Option<Arc<Vec<u8>>>;

/// Encoded ITM tiles shared by all renderers; concurrent requests for one tile wait for a
/// single download. Oldest entries are dropped first (tiles are rendered row by row).
pub struct Source {
    client: Client,
    map: Mutex<(HashMap<(usize, u32, u32), Arc<OnceLock<Raw>>>, VecDeque<(usize, u32, u32)>)>,
    decoded: Mutex<(HashMap<(usize, u32, u32), Arc<Vec<u8>>>, VecDeque<(usize, u32, u32)>)>,
    pace: Mutex<Pace>,
    max_rate: f64,
    /// Download jobs wait out a rate-limit challenge and retry; the live map gives up at once.
    patient: bool,
}

/// Shared request pacing. The CDN sits behind an AWS WAF rate rule: past some requests per
/// window it answers every request with a challenge (HTTP 202, `x-amzn-waf-action`) for minutes.
/// We never try to pass the challenge; we slow down (halve the rate, wait) and speed up again
/// slowly (+10 % per 400 good answers) up to [`MAX_RATE`].
struct Pace {
    next: Instant,
    interval: Duration,
    good: u32,
    resume_at: Option<Instant>,
}

/// Requests per second at the start and at most.
const START_RATE: f64 = 8.0;
const MAX_RATE: f64 = 12.0;
const MIN_RATE: f64 = 1.0;
/// Wait after a challenge before the next request.
const CHALLENGE_WAIT: Duration = Duration::from_secs(90);
/// Give up (temp data stays; "update" resumes later) after this many challenges in a row.
const CHALLENGE_LIMIT: u32 = 8;
pub const RATE_LIMITED_MSG: &str = "שרת תצלומי האוויר (govmap) הגביל זמנית את קצב ההורדה. מה שכבר ירד נשמר – אפשר ללחוץ ״עדכן״ שוב בעוד כרבע שעה וההורדה תמשיך מאותה נקודה.";

impl Source {
    pub fn new(client: Client) -> Self {
        Self::with_rates(client, START_RATE, MAX_RATE, true)
    }

    /// For the live map: short bursts of a screenful of tiles, no waiting on a challenge.
    pub fn interactive(client: Client) -> Self {
        Self::with_rates(client, 15.0, 20.0, false)
    }

    fn with_rates(client: Client, start: f64, max_rate: f64, patient: bool) -> Self {
        let pace = Pace { next: Instant::now(), interval: Duration::from_secs_f64(1.0 / start), good: 0, resume_at: None };
        Self { client, map: Mutex::new((HashMap::new(), VecDeque::new())), decoded: Mutex::new((HashMap::new(), VecDeque::new())), pace: Mutex::new(pace), max_rate, patient }
    }

    /// Block until this request's slot (or the end of a challenge pause).
    fn wait_turn(&self, cancel: &Cancel) -> anyhow::Result<()> {
        let slot = {
            let mut p = self.pace.lock();
            let now = Instant::now();
            if !self.patient && p.resume_at.is_some_and(|r| r > now) {
                bail!("{RATE_LIMITED_MSG}");
            }
            let mut slot = p.next.max(now);
            if let Some(r) = p.resume_at {
                if r > slot {
                    slot = r;
                } else {
                    p.resume_at = None;
                }
            }
            p.next = slot + p.interval;
            slot
        };
        while Instant::now() < slot {
            if cancel.is_cancelled() {
                bail!("cancelled");
            }
            std::thread::sleep((slot - Instant::now()).min(Duration::from_millis(250)));
        }
        Ok(())
    }

    fn on_good(&self) {
        let mut p = self.pace.lock();
        p.good += 1;
        if p.good >= 400 {
            p.good = 0;
            p.interval = Duration::from_secs_f64((p.interval.as_secs_f64() / 1.1).max(1.0 / self.max_rate));
        }
    }

    fn on_challenge(&self) {
        let mut p = self.pace.lock();
        let now = Instant::now();
        // one slowdown per pause, however many workers saw the challenge
        if p.resume_at.is_some_and(|r| r > now) {
            return;
        }
        p.good = 0;
        p.interval = Duration::from_secs_f64((p.interval.as_secs_f64() * 2.0).min(1.0 / MIN_RATE));
        p.resume_at = Some(now + CHALLENGE_WAIT);
        log::warn!("govmap rate limit (WAF challenge): pausing {CHALLENGE_WAIT:?}, then {:.1} req/s", 1.0 / p.interval.as_secs_f64());
    }

    fn get(&self, level: usize, row: u32, col: u32, cancel: &Cancel) -> anyhow::Result<Raw> {
        let key = (level, row, col);
        let cell = {
            let mut m = self.map.lock();
            if let Some(c) = m.0.get(&key) {
                c.clone()
            } else {
                let c = Arc::new(OnceLock::new());
                m.0.insert(key, c.clone());
                m.1.push_back(key);
                while m.1.len() > CACHE_TILES {
                    if let Some(old) = m.1.pop_front() {
                        m.0.remove(&old);
                    }
                }
                c
            }
        };
        if let Some(v) = cell.get() {
            return Ok(v.clone());
        }
        let mut err = None;
        let v = cell.get_or_init(|| match self.download(&raw_url(level, row, col), cancel) {
            Ok(v) => v.map(Arc::new),
            Err(e) => {
                err = Some(e);
                None
            }
        });
        if let Some(e) = err {
            // do not cache a failure as "no tile"
            let mut m = self.map.lock();
            m.0.remove(&key);
            return Err(e);
        }
        Ok(v.clone())
    }

    /// A source tile as RGB pixels (decoded once, shared through a small LRU).
    fn get_decoded(&self, level: usize, row: u32, col: u32, cancel: &Cancel) -> anyhow::Result<Option<Arc<Vec<u8>>>> {
        let key = (level, row, col);
        if let Some(t) = self.decoded.lock().0.get(&key) {
            return Ok(Some(t.clone()));
        }
        let Some(rgb) = self.get(level, row, col, cancel)?.and_then(|b| decode(&b)) else { return Ok(None) };
        let rgb = Arc::new(rgb);
        let mut d = self.decoded.lock();
        if d.0.insert(key, rgb.clone()).is_none() {
            d.1.push_back(key);
        }
        while d.1.len() > DECODED_TILES {
            if let Some(old) = d.1.pop_front() {
                d.0.remove(&old);
            }
        }
        Ok(Some(rgb))
    }

    fn download(&self, url: &str, cancel: &Cancel) -> anyhow::Result<Option<Vec<u8>>> {
        let mut last: Option<anyhow::Error> = None;
        let mut challenges = 0u32;
        let mut attempt = 0u32;
        let max_attempts = if self.patient { 5 } else { 2 };
        while attempt < max_attempts {
            if cancel.is_cancelled() {
                bail!("cancelled");
            }
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(400 << attempt));
            }
            attempt += 1;
            self.wait_turn(cancel)?;
            match self.client.get(url).send() {
                Ok(r) if r.headers().contains_key("x-amzn-waf-action") => {
                    challenges += 1;
                    if !self.patient || challenges >= CHALLENGE_LIMIT {
                        if !self.patient {
                            self.on_challenge();
                        }
                        bail!("{RATE_LIMITED_MSG}");
                    }
                    self.on_challenge();
                    attempt = 0; // the pause is the retry delay
                }
                Ok(r) if r.status().is_success() => match r.bytes() {
                    Ok(b) if b.len() > 3 && b[0] == 0xFF && b[1] == 0xD8 => {
                        self.on_good();
                        return Ok(Some(b.to_vec()));
                    }
                    Ok(b) if b.len() > 8 && b[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] => {
                        self.on_good();
                        return Ok(Some(b.to_vec()));
                    }
                    Ok(_) => last = Some(anyhow!("{url}: not an image (a content filter may be answering instead of the server)")),
                    Err(e) => last = Some(e.into()),
                },
                // S3 answers 403 AccessDenied for tiles that do not exist (sea, outside coverage)
                Ok(r) if matches!(r.status().as_u16(), 403 | 404 | 204) => return Ok(None),
                Ok(r) if r.status().as_u16() == 418 => bail!("{}", download::blocked_message(url)),
                Ok(r) => last = Some(anyhow!("HTTP {} for {url}", r.status())),
                // the live map does not retry a dead connection: the next screenful asks again
                Err(e) if !self.patient && (e.is_connect() || e.is_timeout()) => return Err(e.into()),
                Err(e) => last = Some(e.into()),
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("download failed: {url}")))
    }

    /// One known tile (Dizengoff Square, Tel Aviv, level 8) must be an image: the host is reachable,
    /// not filtered, and the cache still exists.
    pub fn probe(&self, cancel: &Cancel) -> anyhow::Result<()> {
        let (e, n) = (178_830.0, 665_030.0);
        let t = 256.0 * RES[8];
        let (row, col) = (((ORIGIN.1 - n) / t) as u32, ((e - ORIGIN.0) / t) as u32);
        let url = raw_url(8, row, col);
        // a challenge here fails at once (no pacing pause) so "check for updates" stays fast
        if let Ok(r) = self.client.get(&url).send() {
            if r.headers().contains_key("x-amzn-waf-action") {
                bail!("{RATE_LIMITED_MSG}");
            }
        }
        match self.download(&url, cancel)? {
            Some(_) => Ok(()),
            None => bail!("שכבת תצלומי האוויר {CACHE} לא נמצאה בשרת govmap"),
        }
    }
}

fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.first() == Some(&0x89) {
        let mut dec = png::Decoder::new(bytes);
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut r = dec.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).ok()?;
        if info.width != 256 || info.height != 256 {
            return None;
        }
        let ch = info.color_type.samples();
        let mut rgb = Vec::with_capacity(256 * 256 * 3);
        for px in buf[..info.buffer_size()].chunks_exact(ch) {
            match ch {
                1 | 2 => rgb.extend_from_slice(&[px[0], px[0], px[0]]),
                _ => rgb.extend_from_slice(&px[..3]),
            }
        }
        return Some(rgb);
    }
    let mut d = jpeg_decoder::Decoder::new(bytes);
    let px = d.decode().ok()?;
    let info = d.info()?;
    if info.width != 256 || info.height != 256 {
        return None;
    }
    match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => Some(px),
        jpeg_decoder::PixelFormat::L8 => Some(px.iter().flat_map(|&v| [v, v, v]).collect()),
        _ => None,
    }
}

// ---------------------------------------------------------------- rendering

/// Projection grid step in output pixels: ITM is smooth enough that bilinear interpolation of
/// exactly projected nodes 16 px apart is off by far less than a millimetre.
const STEP: usize = 16;
const NODES: usize = 256 / STEP + 1;

/// Render WebMercator tile (z, x, y) from the orthophoto: JPEG, PNG with transparency where the
/// source has no photo, or None when it has none at all.
pub fn render(src: &Source, z: u8, x: u32, y: u32, cancel: &Cancel) -> anyhow::Result<Option<Vec<u8>>> {
    // ITM metres at the grid nodes
    let n = f64::from(1u32 << z);
    let mut en = [[(0.0f64, 0.0f64); NODES]; NODES];
    for (j, row) in en.iter_mut().enumerate() {
        let ty = f64::from(y) + (j * STEP) as f64 / 256.0;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * ty / n)).sinh().atan().to_degrees();
        for (i, node) in row.iter_mut().enumerate() {
            let lon = (f64::from(x) + (i * STEP) as f64 / 256.0) / n * 360.0 - 180.0;
            *node = itm::from_wgs84(lon, lat);
        }
    }
    let mid = NODES / 2;
    let (a, b) = (en[mid][0], en[mid][NODES - 1]);
    let mpp = (b.0 - a.0).hypot(b.1 - a.1) / 256.0;
    let level = RES.iter().position(|&r| r <= mpp * 1.2).unwrap_or(RES.len() - 1);
    let res = RES[level];
    // source pixel coordinates (pixel centres on integers)
    let mut uv = [[(0.0f64, 0.0f64); NODES]; NODES];
    let (mut umin, mut umax, mut vmin, mut vmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for j in 0..NODES {
        for i in 0..NODES {
            let (e, nn) = en[j][i];
            let (u, v) = ((e - ORIGIN.0) / res - 0.5, (ORIGIN.1 - nn) / res - 0.5);
            uv[j][i] = (u, v);
            umin = umin.min(u);
            umax = umax.max(u);
            vmin = vmin.min(v);
            vmax = vmax.max(v);
        }
    }
    if umin < 0.0 || vmin < 0.0 {
        return Ok(None);
    }
    let (c0, c1) = ((umin / 256.0).floor() as u32, ((umax + 1.0) / 256.0).floor() as u32);
    let (r0, r1) = ((vmin / 256.0).floor() as u32, ((vmax + 1.0) / 256.0).floor() as u32);
    let (cols, rows) = ((c1 - c0 + 1) as usize, (r1 - r0 + 1) as usize);
    if cols * rows > 64 {
        bail!("too many source tiles for {z}/{x}/{y}");
    }
    let mut tiles: Vec<Option<Arc<Vec<u8>>>> = Vec::with_capacity(cols * rows);
    let mut any = false;
    for r in r0..=r1 {
        for c in c0..=c1 {
            let t = src.get_decoded(level, r, c, cancel)?;
            any |= t.is_some();
            tiles.push(t);
        }
    }
    if !any {
        return Ok(None);
    }
    let base_u = f64::from(c0 * 256);
    let base_v = f64::from(r0 * 256);
    let (w, h) = ((cols * 256) as i64, (rows * 256) as i64);
    let pixel = |pu: i64, pv: i64| -> Option<&[u8]> {
        if pu < 0 || pv < 0 || pu >= w || pv >= h {
            return None;
        }
        let t = tiles[(pv / 256) as usize * cols + (pu / 256) as usize].as_ref()?;
        let o = (((pv % 256) * 256 + (pu % 256)) * 3) as usize;
        Some(&t[o..o + 3])
    };
    let sample = |u: f64, v: f64| -> Option<[f64; 3]> {
        let (u, v) = (u - base_u, v - base_v);
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let p00 = pixel(x0, y0)?;
        let p10 = pixel(x0 + 1, y0)?;
        let p01 = pixel(x0, y0 + 1)?;
        let p11 = pixel(x0 + 1, y0 + 1)?;
        let mut o = [0.0; 3];
        for k in 0..3 {
            let top = f64::from(p00[k]) * (1.0 - fx) + f64::from(p10[k]) * fx;
            let bot = f64::from(p01[k]) * (1.0 - fx) + f64::from(p11[k]) * fx;
            o[k] = top * (1.0 - fy) + bot * fy;
        }
        Some(o)
    };
    // source pixels per output pixel → samples per axis (box filter against aliasing)
    let ss = (mpp / res).round().clamp(1.0, 3.0) as usize;
    let mut rgba = vec![0u8; 256 * 256 * 4];
    let mut opaque = 0usize;
    let interp = |px: f64, py: f64| -> (f64, f64) {
        let (gx, gy) = (px / STEP as f64, py / STEP as f64);
        let (i, j) = ((gx.floor() as usize).min(NODES - 2), (gy.floor() as usize).min(NODES - 2));
        let (fx, fy) = (gx - i as f64, gy - j as f64);
        let l = |a: (f64, f64), b: (f64, f64), f: f64| (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
        let top = l(uv[j][i], uv[j][i + 1], fx);
        let bot = l(uv[j + 1][i], uv[j + 1][i + 1], fx);
        l(top, bot, fy)
    };
    for py in 0..256usize {
        for px in 0..256usize {
            let mut acc = [0.0; 3];
            let mut cnt = 0usize;
            for sy in 0..ss {
                for sx in 0..ss {
                    let (u, v) = interp(px as f64 + (sx as f64 + 0.5) / ss as f64, py as f64 + (sy as f64 + 0.5) / ss as f64);
                    if let Some(c) = sample(u, v) {
                        acc[0] += c[0];
                        acc[1] += c[1];
                        acc[2] += c[2];
                        cnt += 1;
                    }
                }
            }
            if cnt * 2 >= ss * ss {
                let o = (py * 256 + px) * 4;
                for k in 0..3 {
                    rgba[o + k] = (acc[k] / cnt as f64 + 0.5).clamp(0.0, 255.0) as u8;
                }
                rgba[o + 3] = 255;
                opaque += 1;
            }
        }
    }
    if opaque == 0 {
        return Ok(None);
    }
    if opaque == 256 * 256 {
        let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let mut out = Vec::with_capacity(40_000);
        jpeg_encoder::Encoder::new(&mut out, JPEG_QUALITY).encode(&rgb, 256, 256, jpeg_encoder::ColorType::Rgb)?;
        return Ok(Some(out));
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 256, 256);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Best);
        enc.write_header()?.write_image_data(&rgba)?;
    }
    Ok(Some(out))
}

/// One tile of the archive: Sentinel-2 for zooms ≤ [`EOX_MAX_ZOOM`] (rendered from the orthophoto
/// when EOX cannot be reached), the orthophoto above.
pub fn tile(src: &Source, eox_layer: Option<&str>, z: u8, x: u32, y: u32, cancel: &Cancel) -> anyhow::Result<Option<Vec<u8>>> {
    if z <= EOX_MAX_ZOOM {
        if let Some(layer) = eox_layer {
            match fetch_tile(&src.client, &tile_url(layer, z, x, y), true, cancel) {
                Ok(t) => return Ok(t),
                Err(e) if cancel.is_cancelled() => return Err(e),
                Err(e) => log::warn!("Sentinel-2 {z}/{x}/{y}: {e:#}; using the orthophoto"),
            }
        }
    }
    render(src, z, x, y, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::BBOX_ISRAEL;

    #[test]
    fn itm_matches_reference_points() {
        // reference values from hmaps/core/itm.py (verified there against PROJ)
        let (e, n) = itm::from_wgs84(34.774_082, 32.077_885);
        assert!((e - REF_TA.0).abs() < 0.01 && (n - REF_TA.1).abs() < 0.01, "{e} {n}");
        let (e, n) = itm::from_wgs84(35.2045169444444, 31.7343936111111);
        assert!((e - REF_ORIGIN.0).abs() < 0.01 && (n - REF_ORIGIN.1).abs() < 0.01, "{e} {n}");
    }
    #[test]
    fn itm_inverse_roundtrip() {
        for (lon, lat) in [(34.774_082, 32.077_885), (34.95, 29.55), (35.6, 33.2)] {
            let (e, n) = itm::from_wgs84(lon, lat);
            let (lo, la) = itm::to_wgs84(e, n);
            assert!((lo - lon).abs() < 1e-8 && (la - lat).abs() < 1e-8, "{lo} {la}");
        }
    }
    const REF_TA: (f64, f64) = (178_824.110_426_244_38, 665_036.013_644_147_5);
    const REF_ORIGIN: (f64, f64) = (219_463.547_987_584_72, 626_866.426_598_637_7);

    #[test]
    fn outline() {
        assert!(in_outline(34.78, 32.08)); // Tel Aviv
        assert!(in_outline(35.21, 31.77)); // Jerusalem
        assert!(in_outline(34.95, 29.55)); // Eilat
        assert!(!in_outline(34.5, 32.5)); // sea
        assert!(!in_outline(35.9, 31.9)); // Jordan
        // tile counts per zoom, for the size estimates in the UI
        for z in 10..=17u8 {
            let n = super::super::satellite::tiles_for_bbox(BBOX_ISRAEL, z, false).into_iter().filter(|&(zz, x, y)| zz == z && wanted(zz, x, y)).count();
            eprintln!("z{z}: {n}");
        }
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// `ORTHO_OUT=<dir> cargo test --lib orthophoto::live -- --ignored` renders a few real tiles.
    #[test]
    #[ignore]
    fn render_samples() {
        let out = std::path::PathBuf::from(std::env::var("ORTHO_OUT").expect("ORTHO_OUT"));
        let src = Source::new(download::http_client().unwrap());
        let cancel = Cancel::default();
        src.probe(&cancel).unwrap();
        for (z, lon, lat) in [(17u8, 34.8925f64, 31.9505f64), (16, 34.8925, 31.9505), (15, 34.78, 32.08), (12, 35.0, 31.5), (14, 34.70, 31.95)] {
            let n = f64::from(1u32 << z);
            let x = ((lon + 180.0) / 360.0 * n) as u32;
            let y = ((1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * n) as u32;
            let t0 = std::time::Instant::now();
            let t = render(&src, z, x, y, &cancel).unwrap();
            let ext = match &t { Some(b) if b[0] == 0x89 => "png", _ => "jpg" };
            eprintln!("{z}/{x}/{y}: {:?} bytes in {:?}", t.as_ref().map(|b| b.len()), t0.elapsed());
            if let Some(b) = t {
                std::fs::write(out.join(format!("{z}_{x}_{y}.{ext}")), b).unwrap();
            }
        }
    }
}
