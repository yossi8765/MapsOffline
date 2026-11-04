//! Typed coordinates: parsing of what a user may paste into the search box, and the projections
//! behind it.
//!
//! * decimal degrees in either order ("32.08, 34.78" / "34.78 32.08", any place on earth),
//! * degrees-minutes-seconds with symbols, letters or Hebrew hemisphere words
//!   ("32°04′48″N 34°46′48″E", "32 4 48 צ 34 46 48 מ", "N32 4.8 E34 46.8"),
//! * Israeli Transverse Mercator (ITM / EPSG:2039, "178500 663500", "178500/663500"),
//! * UTM ("36R 676000 3550000", "36 S 676000 3550000"),
//! * Open Location Codes / Plus Codes, full ("8G3QQ7QJ+XX") and short ("QJ8V+2F", relative to `near`).
//!
//! ITM is the same Helmert + Krüger series as `data/orthophoto.rs::itm` and `src/lib/coords.ts`
//! (all three agree to the centimetre; the tests use the same reference points).

/// How a coordinate was typed (`coords_parse` → `format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoordFormat {
    Dd,
    Dms,
    Itm,
    Utm,
    Plus,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedCoord {
    pub lon: f64,
    pub lat: f64,
    pub format: CoordFormat,
    /// The input as understood ("ITM 178500 663500"); not serialised to the UI.
    #[serde(skip)]
    pub label: String,
}

// ---------------------------------------------------------------- transverse Mercator

const A: f64 = 6_378_137.0;
const F_WGS84: f64 = 1.0 / 298.257_223_563;
const F_GRS80: f64 = 1.0 / 298.257_222_101;

struct Tm {
    aa: f64,
    alpha: [f64; 4],
    e: f64,
}

fn tm_consts(f: f64) -> Tm {
    let n = f / (2.0 - f);
    let (n2, n3, n4) = (n * n, n * n * n, n * n * n * n);
    Tm {
        aa: A / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0),
        alpha: [
            n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0,
            13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0,
            61.0 * n3 / 240.0 - 103.0 * n4 / 140.0,
            49561.0 * n4 / 161_280.0,
        ],
        e: (f * (2.0 - f)).sqrt(),
    }
}

/// Krüger series: latitude/longitude (radians) → (easting, northing) on the unit-scale grid
/// before false easting / northing.
fn tm(c: &Tm, lat: f64, lon: f64, lon0: f64) -> (f64, f64) {
    let s = lat.sin();
    let t = (s.atanh() - c.e * (c.e * s).atanh()).sinh();
    let dl = lon - lon0;
    let xi_ = t.atan2(dl.cos());
    let eta_ = (dl.sin() / (1.0 + t * t).sqrt()).atanh();
    let (mut xi, mut eta) = (xi_, eta_);
    for (j, a) in c.alpha.iter().enumerate() {
        let k = 2.0 * (j + 1) as f64;
        xi += a * (k * xi_).sin() * (k * eta_).cosh();
        eta += a * (k * xi_).cos() * (k * eta_).sinh();
    }
    (c.aa * eta, c.aa * xi)
}

/// Invert a smooth forward projection by Newton iteration with a numerical Jacobian.
fn invert(f: impl Fn(f64, f64) -> (f64, f64), e: f64, n: f64, mut lon: f64, mut lat: f64) -> (f64, f64) {
    for _ in 0..12 {
        let (e0, n0) = f(lon, lat);
        let (de, dn) = (e - e0, n - n0);
        if de.abs() < 1e-4 && dn.abs() < 1e-4 {
            break;
        }
        let h = 1e-6;
        let (e1, n1) = f(lon + h, lat);
        let (e2, n2) = f(lon, lat + h);
        let (a, b, c, d) = ((e1 - e0) / h, (e2 - e0) / h, (n1 - n0) / h, (n2 - n0) / h);
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            break;
        }
        lon += (d * de - b * dn) / det;
        lat += (-c * de + a * dn) / det;
    }
    (lon, lat)
}

pub mod itm {
    //! Israeli Transverse Mercator (EPSG:2039): 7-parameter Helmert shift WGS 84 → Israel 1993
    //! (EPSG:9676, position vector) and the TM projection on GRS80.
    use super::*;
    use std::sync::OnceLock;

    const LAT0_DEG: f64 = 31.734_393_611_111_1;
    const LON0_DEG: f64 = 35.204_516_944_444_4;
    const K0: f64 = 1.000_006_7;
    const X0: f64 = 219_529.584;
    const Y0: f64 = 626_907.39;
    const T: [f64; 3] = [23.772, 17.49, 17.859];
    const ROT_ARCSEC: [f64; 3] = [-0.3132, -1.85274, 1.67299];
    const S: f64 = -5.4262e-6;

    struct Consts {
        tm: Tm,
        north0: f64,
        r: [f64; 3],
    }

    fn consts() -> &'static Consts {
        static C: OnceLock<Consts> = OnceLock::new();
        C.get_or_init(|| {
            let tm_c = tm_consts(F_GRS80);
            let north0 = tm(&tm_c, LAT0_DEG.to_radians(), LON0_DEG.to_radians(), LON0_DEG.to_radians()).1;
            let r = ROT_ARCSEC.map(|a| (a / 3600.0f64).to_radians());
            Consts { tm: tm_c, north0, r }
        })
    }

    /// WGS 84 lon/lat (degrees) → ITM (easting, northing) metres.
    pub fn from_wgs84(lon_deg: f64, lat_deg: f64) -> (f64, f64) {
        let c = consts();
        let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
        let e2 = F_WGS84 * (2.0 - F_WGS84);
        let sl = lat.sin();
        let nu = A / (1.0 - e2 * sl * sl).sqrt();
        let x = nu * lat.cos() * lon.cos() - T[0];
        let y = nu * lat.cos() * lon.sin() - T[1];
        let z = nu * (1.0 - e2) * sl - T[2];
        let k = 1.0 + S;
        let [rx, ry, rz] = c.r;
        let lx = (x + rz * y - ry * z) / k;
        let ly = (-rz * x + y + rx * z) / k;
        let lz = (ry * x - rx * y + z) / k;
        let e2g = F_GRS80 * (2.0 - F_GRS80);
        let p = lx.hypot(ly);
        let lo = ly.atan2(lx);
        let mut la = lz.atan2(p * (1.0 - e2g));
        for _ in 0..4 {
            let s = la.sin();
            let nu = A / (1.0 - e2g * s * s).sqrt();
            la = (lz + e2g * nu * s).atan2(p);
        }
        let (e, n) = tm(&c.tm, la, lo, LON0_DEG.to_radians());
        (X0 + K0 * e, Y0 + K0 * (n - c.north0))
    }

    /// ITM metres → WGS 84 (lon, lat) degrees.
    pub fn to_wgs84(e: f64, n: f64) -> (f64, f64) {
        invert(from_wgs84, e, n, LON0_DEG + (e - X0) / 94_000.0, LAT0_DEG + (n - Y0) / 110_900.0)
    }
}

pub mod utm {
    //! Universal Transverse Mercator on WGS 84.
    use super::*;
    use std::sync::OnceLock;

    const K0: f64 = 0.9996;
    const BANDS: &[u8] = b"CDEFGHJKLMNPQRSTUVWX";

    fn c() -> &'static Tm {
        static C: OnceLock<Tm> = OnceLock::new();
        C.get_or_init(|| tm_consts(F_WGS84))
    }

    fn lon0(zone: u32) -> f64 {
        (zone as f64 * 6.0 - 183.0).to_radians()
    }

    /// (zone, band letter, easting, northing)
    pub fn from_wgs84(lon: f64, lat: f64) -> (u32, char, f64, f64) {
        let zone = (((lon + 180.0) / 6.0).floor() as i64 + 1).clamp(1, 60) as u32;
        let (e, n) = project(zone, lon, lat);
        let band = BANDS[(((lat + 80.0) / 8.0).floor() as i64).clamp(0, 19) as usize] as char;
        (zone, band, e, n)
    }

    fn project(zone: u32, lon: f64, lat: f64) -> (f64, f64) {
        let (e, n) = tm(c(), lat.to_radians(), lon.to_radians(), lon0(zone));
        (500_000.0 + K0 * e, if lat < 0.0 { 10_000_000.0 } else { 0.0 } + K0 * n)
    }

    /// Zone, hemisphere and grid metres → (lon, lat).
    pub fn to_wgs84(zone: u32, north: bool, e: f64, n: f64) -> (f64, f64) {
        let lat0 = if north { n / 110_900.0 } else { (n - 10_000_000.0) / 110_900.0 };
        let lon_c = zone as f64 * 6.0 - 183.0;
        let guess_lon = lon_c + (e - 500_000.0) / (111_320.0 * lat0.to_radians().cos().max(0.1));
        invert(
            |lon, lat| {
                let (x, y) = tm(c(), lat.to_radians(), lon.to_radians(), lon0(zone));
                (500_000.0 + K0 * x, if north { 0.0 } else { 10_000_000.0 } + K0 * y)
            },
            e,
            n,
            guess_lon,
            lat0,
        )
    }

    /// A band letter (C..X without I and O): north of the equator from N on.
    pub fn band_is_north(b: char) -> Option<bool> {
        let b = b.to_ascii_uppercase() as u8;
        BANDS.contains(&b).then_some(b >= b'N')
    }
}

pub mod olc {
    //! Open Location Code (Plus Codes).
    const ALPHABET: &[u8] = b"23456789CFGHJMPQRVWX";
    const SEP_POS: usize = 8;

    fn idx(c: u8) -> Option<usize> {
        ALPHABET.iter().position(|&a| a == c.to_ascii_uppercase())
    }

    /// Is `s` (one token) shaped like a plus code?
    pub fn looks_like(s: &str) -> bool {
        let Some(p) = s.find('+') else { return false };
        let (head, tail) = (&s[..p], &s[p + 1..]);
        (2..=8).contains(&head.len())
            && head.len() % 2 == 0
            && tail.len() <= 7
            && head.bytes().all(|c| c == b'0' || idx(c).is_some())
            && tail.bytes().all(|c| idx(c).is_some())
            && (head.len() == SEP_POS || tail.len() >= 2)
    }

    /// Centre of a full code.
    pub fn decode(code: &str) -> Option<(f64, f64)> {
        let clean: Vec<u8> = code.bytes().filter(|&c| c != b'+' && c != b'0').collect();
        if clean.len() < 2 || code.find('+') != Some(SEP_POS) {
            return None;
        }
        let (mut lat, mut lng) = (-90.0f64, -180.0f64);
        let mut res = 20.0f64;
        let (mut lat_size, mut lng_size) = (res, res);
        let pairs = clean.len().min(10);
        let mut i = 0;
        while i + 1 < pairs {
            lat += idx(clean[i])? as f64 * res;
            lng += idx(clean[i + 1])? as f64 * res;
            lat_size = res;
            lng_size = res;
            res /= 20.0;
            i += 2;
        }
        if clean.len() > 10 {
            let (mut lr, mut gr) = (lat_size / 5.0, lng_size / 4.0);
            for &c in &clean[10..] {
                let d = idx(c)?;
                lat += (d / 4) as f64 * lr;
                lng += (d % 4) as f64 * gr;
                lat_size = lr;
                lng_size = gr;
                lr /= 5.0;
                gr /= 4.0;
            }
        }
        Some((lng + lng_size / 2.0, lat + lat_size / 2.0))
    }

    /// The first `len` (even) digits of the code of (lat, lng).
    pub fn encode_prefix(lat: f64, lng: f64, len: usize) -> String {
        let mut lat = (lat + 90.0).clamp(0.0, 179.999_999);
        let mut lng = (lng + 180.0).rem_euclid(360.0);
        let mut res = 20.0f64;
        let mut out = String::new();
        while out.len() < len {
            let a = ((lat / res).floor() as usize).min(19);
            let b = ((lng / res).floor() as usize).min(19);
            lat -= a as f64 * res;
            lng -= b as f64 * res;
            out.push(ALPHABET[a] as char);
            out.push(ALPHABET[b] as char);
            res /= 20.0;
        }
        out
    }

    /// Decode a full code, or a short one relative to `(ref_lon, ref_lat)` (recoverNearest).
    pub fn decode_near(code: &str, ref_lon: f64, ref_lat: f64) -> Option<(f64, f64)> {
        let code = code.to_ascii_uppercase();
        let sep = code.find('+')?;
        if sep == SEP_POS {
            return decode(&code);
        }
        let padding = SEP_POS - sep;
        let resolution = 20f64.powi(2 - (padding / 2) as i32);
        let half = resolution / 2.0;
        let rlat = (ref_lat / resolution).floor() * resolution;
        let rlng = (ref_lon / resolution).floor() * resolution;
        let full = format!("{}{}", encode_prefix(rlat, rlng, padding), code);
        let (mut lng, mut lat) = decode(&full)?;
        if ref_lat + half < lat && lat - resolution >= -90.0 {
            lat -= resolution;
        } else if ref_lat - half > lat && lat + resolution <= 90.0 {
            lat += resolution;
        }
        if ref_lon + half < lng {
            lng -= resolution;
        } else if ref_lon - half > lng {
            lng += resolution;
        }
        Some((lng, lat))
    }
}

// ---------------------------------------------------------------- parsing

/// Middle of Israel: the reference for short plus codes when there is no `near`.
const ISRAEL_REF: (f64, f64) = (34.9, 31.6);

fn in_israel(lon: f64, lat: f64) -> bool {
    let b = crate::paths::BBOX_ISRAEL;
    lon >= b[0] && lon <= b[2] && lat >= b[1] && lat <= b[3]
}

fn valid(lon: f64, lat: f64) -> bool {
    lon.is_finite() && lat.is_finite() && lat.abs() <= 90.0 && lon.abs() <= 180.0
}

/// Parse typed coordinates. `near` = reference for short plus codes; `prefer(lon, lat)` = where
/// coordinates are expected (Israel and installed countries) to decide the order of two bare
/// numbers that are both valid either way.
pub fn parse(text: &str, near: Option<[f64; 2]>, prefer: &dyn Fn(f64, f64) -> bool) -> Option<ParsedCoord> {
    let t = text.trim();
    if t.is_empty() || t.len() > 80 {
        return None;
    }
    parse_plus(t, near).or_else(|| parse_utm(t)).or_else(|| parse_itm(t)).or_else(|| parse_dms(t)).or_else(|| parse_dd(t, prefer))
}

fn parse_plus(t: &str, near: Option<[f64; 2]>) -> Option<ParsedCoord> {
    let tok = t.split(|c: char| c.is_whitespace() || c == ',').find(|s| s.contains('+'))?;
    if !olc::looks_like(tok) {
        return None;
    }
    let (rlon, rlat) = near.map(|p| (p[0], p[1])).unwrap_or(ISRAEL_REF);
    let (lon, lat) = olc::decode_near(tok, rlon, rlat)?;
    valid(lon, lat).then(|| ParsedCoord { lon, lat, format: CoordFormat::Plus, label: tok.to_ascii_uppercase() })
}

/// Numbers of a string split on spaces, commas, semicolons and slashes (None if any piece is not one).
fn numbers(t: &str) -> Option<Vec<f64>> {
    let v: Vec<&str> = t.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '/')).filter(|s| !s.is_empty()).collect();
    v.iter().map(|s| s.parse::<f64>().ok()).collect()
}

fn parse_utm(t: &str) -> Option<ParsedCoord> {
    let toks: Vec<&str> = t.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).collect();
    let (zone_s, band, rest) = match toks.as_slice() {
        [zb, e, n] if zb.len() >= 2 && zb.as_bytes()[zb.len() - 1].is_ascii_alphabetic() => {
            (&zb[..zb.len() - 1], zb.chars().last()?, vec![*e, *n])
        }
        [z, b, e, n] if b.len() == 1 && b.as_bytes()[0].is_ascii_alphabetic() => (*z, b.chars().next()?, vec![*e, *n]),
        _ => return None,
    };
    let zone: u32 = zone_s.parse().ok().filter(|z| (1..=60).contains(z))?;
    let north = utm::band_is_north(band)?;
    let e: f64 = rest[0].parse().ok()?;
    let n: f64 = rest[1].parse().ok()?;
    if !(100_000.0..=900_000.0).contains(&e) || !(0.0..=10_000_000.0).contains(&n) {
        return None;
    }
    let (lon, lat) = utm::to_wgs84(zone, north, e, n);
    valid(lon, lat).then(|| ParsedCoord {
        lon,
        lat,
        format: CoordFormat::Utm,
        label: format!("UTM {zone}{} {} {}", band.to_ascii_uppercase(), e.round(), n.round()),
    })
}

fn parse_itm(t: &str) -> Option<ParsedCoord> {
    let t = t.strip_prefix("ITM").or_else(|| t.strip_prefix("itm")).unwrap_or(t);
    let v = numbers(t)?;
    let [a, b] = v.as_slice() else { return None };
    let is_e = |x: f64| (100_000.0..=300_000.0).contains(&x);
    let is_n = |x: f64| (350_000.0..=850_000.0).contains(&x);
    let (e, n) = if is_e(*a) && is_n(*b) {
        (*a, *b)
    } else if is_n(*a) && is_e(*b) {
        (*b, *a)
    } else {
        return None;
    };
    let (lon, lat) = itm::to_wgs84(e, n);
    Some(ParsedCoord { lon, lat, format: CoordFormat::Itm, label: format!("ITM {} {}", e.round(), n.round()) })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Unit {
    None,
    Deg,
    Min,
    Sec,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Item {
    Num(f64, Unit),
    /// true = latitude letter; sign
    Hemi(bool, f64),
}

fn hemi_word(w: &str) -> Option<(bool, f64)> {
    let w = w.to_lowercase();
    Some(match w.as_str() {
        "n" | "צ" | "צפון" | "north" => (true, 1.0),
        "s" | "ד" | "דרום" | "south" => (true, -1.0),
        "e" | "מ" | "מזרח" | "east" => (false, 1.0),
        "w" | "מע" | "מערב" | "west" => (false, -1.0),
        _ => return None,
    })
}

fn dms_items(t: &str) -> Option<Vec<Item>> {
    let s: String = t
        .chars()
        .map(|c| match c {
            '׳' | '’' | '`' | '´' => '\'',
            '״' | '“' | '”' => '"',
            _ => c,
        })
        .collect();
    let mut items = Vec::new();
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_ascii_digit() || (c == '.' && i + 1 < cs.len() && cs[i + 1].is_ascii_digit()) || (c == '-' && i + 1 < cs.len() && cs[i + 1].is_ascii_digit()) {
            let st = i;
            i += 1;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            let v: f64 = cs[st..i].iter().collect::<String>().parse().ok()?;
            // unit right after the number (spaces allowed before a symbol)
            let mut j = i;
            while j < cs.len() && cs[j] == ' ' {
                j += 1;
            }
            let unit = match cs.get(j) {
                Some('°') | Some('º') => Unit::Deg,
                Some('\'') | Some('′') => {
                    // '' is seconds
                    if cs.get(j + 1) == Some(&'\'') {
                        j += 1;
                        Unit::Sec
                    } else {
                        Unit::Min
                    }
                }
                Some('"') | Some('″') => Unit::Sec,
                Some('d') if !cs.get(j + 1).is_some_and(|c| c.is_alphabetic()) => Unit::Deg,
                Some('m') if !cs.get(j + 1).is_some_and(|c| c.is_alphabetic()) => Unit::Min,
                Some('s') if !cs.get(j + 1).is_some_and(|c| c.is_alphabetic()) && cs[i..j].is_empty() => Unit::Sec,
                _ => Unit::None,
            };
            if unit != Unit::None {
                i = j + 1;
            }
            items.push(Item::Num(v, unit));
        } else if c.is_alphabetic() {
            let st = i;
            while i < cs.len() && cs[i].is_alphabetic() {
                i += 1;
            }
            let w: String = cs[st..i].iter().collect();
            items.push(match hemi_word(&w) {
                Some((lat, sign)) => Item::Hemi(lat, sign),
                None => return None,
            });
        } else if matches!(c, ' ' | ',' | ';' | '/' | '\t') {
            i += 1;
        } else {
            return None;
        }
    }
    Some(items)
}

/// One coordinate from up to three numbers (with or without units).
fn dms_value(nums: &[(f64, Unit)]) -> Option<f64> {
    if nums.is_empty() || nums.len() > 3 {
        return None;
    }
    let mut v = 0.0;
    let mut neg = false;
    for (k, &(x, u)) in nums.iter().enumerate() {
        let u = if u == Unit::None { [Unit::Deg, Unit::Min, Unit::Sec][k] } else { u };
        match u {
            Unit::Deg => {
                neg = x < 0.0;
                v += x.abs();
            }
            Unit::Min if (0.0..60.0).contains(&x) => v += x / 60.0,
            Unit::Sec if (0.0..60.0).contains(&x) => v += x / 3600.0,
            _ => return None,
        }
    }
    Some(if neg { -v } else { v })
}

fn parse_dms(t: &str) -> Option<ParsedCoord> {
    let items = dms_items(t)?;
    let has_unit = items.iter().any(|i| matches!(i, Item::Num(_, u) if *u != Unit::None));
    let has_hemi = items.iter().any(|i| matches!(i, Item::Hemi(..)));
    let n_nums = items.iter().filter(|i| matches!(i, Item::Num(..))).count();
    if !has_unit && !has_hemi && n_nums < 4 {
        return None; // two bare numbers: decimal degrees
    }
    // split into two groups (numbers, hemisphere)
    let mut groups: Vec<(Vec<(f64, Unit)>, Option<(bool, f64)>)> = Vec::new();
    let prefix = matches!(items.first(), Some(Item::Hemi(..)));
    let mut cur: Vec<(f64, Unit)> = Vec::new();
    let mut pending: Option<(bool, f64)> = None;
    for it in &items {
        match *it {
            Item::Hemi(lat, s) => {
                if prefix {
                    if !cur.is_empty() {
                        groups.push((std::mem::take(&mut cur), pending.take()));
                    }
                    pending = Some((lat, s));
                } else {
                    if cur.is_empty() {
                        return None;
                    }
                    groups.push((std::mem::take(&mut cur), Some((lat, s))));
                }
            }
            Item::Num(v, u) => {
                // a new degrees value starts the next coordinate
                if u == Unit::Deg && cur.iter().any(|(_, cu)| *cu != Unit::None) && !prefix && !has_hemi {
                    groups.push((std::mem::take(&mut cur), None));
                }
                cur.push((v, u));
            }
        }
    }
    if !cur.is_empty() {
        groups.push((cur, pending.take()));
    }
    if groups.len() == 1 && !has_hemi {
        // "32 4 48 34 46 48": split in half
        let (g, _) = groups.pop()?;
        if g.len() % 2 != 0 {
            return None;
        }
        let h = g.len() / 2;
        groups = vec![(g[..h].to_vec(), None), (g[h..].to_vec(), None)];
    }
    let [(ga, ha), (gb, hb)] = groups.as_slice() else { return None };
    let (va, vb) = (dms_value(ga)?, dms_value(gb)?);
    let apply = |v: f64, h: &Option<(bool, f64)>| h.map_or(v, |(_, s)| v.abs() * s);
    let (va, vb) = (apply(va, ha), apply(vb, hb));
    let (lat, lon) = match (ha.map(|h| h.0), hb.map(|h| h.0)) {
        (Some(false), _) | (_, Some(true)) => (vb, va),
        _ => (va, vb),
    };
    valid(lon, lat).then(|| ParsedCoord { lon, lat, format: CoordFormat::Dms, label: t.to_string() })
}

fn parse_dd(t: &str, prefer: &dyn Fn(f64, f64) -> bool) -> Option<ParsedCoord> {
    let v = numbers(t)?;
    let [a, b] = v.as_slice() else { return None };
    let (a, b) = (*a, *b);
    let decimal = t.contains('.');
    let ab = valid(b, a); // lat = a, lon = b
    let ba = valid(a, b);
    let (lat, lon) = match (ab, ba) {
        (true, true) => {
            if prefer(b, a) {
                (a, b)
            } else if prefer(a, b) || in_israel(a, b) {
                (b, a)
            } else {
                (a, b)
            }
        }
        (true, false) => (a, b),
        (false, true) => (b, a),
        _ => return None,
    };
    // two whole numbers are a coordinate only inside Israel ("32 35"), never e.g. "3 5"
    if !decimal && !in_israel(lon, lat) {
        return None;
    }
    Some(ParsedCoord { lon, lat, format: CoordFormat::Dd, label: format!("{lat:.5}, {lon:.5}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(t: &str) -> Option<ParsedCoord> {
        parse(t, None, &|lon, lat| in_israel(lon, lat))
    }
    fn close(a: (f64, f64), b: (f64, f64), tol: f64) -> bool {
        (a.0 - b.0).abs() < tol && (a.1 - b.1).abs() < tol
    }

    #[test]
    fn itm_matches_reference_points() {
        // the same reference points as data/orthophoto.rs and src/lib/coords.test.ts
        let (e, n) = itm::from_wgs84(34.774_082, 32.077_885);
        assert!((e - 178_824.110_426_244_38).abs() < 0.01 && (n - 665_036.013_644_147_5).abs() < 0.01, "{e} {n}");
        let (e, n) = itm::from_wgs84(35.204_516_944_444_4, 31.734_393_611_111_1);
        assert!((e - 219_463.547_987_584_72).abs() < 0.01 && (n - 626_866.426_598_637_7).abs() < 0.01, "{e} {n}");
        let (lon, lat) = itm::to_wgs84(178_824.110_426_244_38, 665_036.013_644_147_5);
        assert!(close((lon, lat), (34.774_082, 32.077_885), 1e-8), "{lon} {lat}");
        // and the same numbers as the orthophoto implementation
        let o = crate::data::orthophoto::itm::from_wgs84(35.57, 33.2);
        let m = itm::from_wgs84(35.57, 33.2);
        assert!((o.0 - m.0).abs() < 1e-6 && (o.1 - m.1).abs() < 1e-6);
    }

    #[test]
    fn utm_roundtrip() {
        let (z, b, e, n) = utm::from_wgs84(34.78, 32.08);
        assert_eq!((z, b), (36, 'S'));
        assert_eq!((e / 1000.0).round(), 668.0);
        assert_eq!((n / 1000.0).round(), 3551.0);
        let (lon, lat) = utm::to_wgs84(36, true, e, n);
        assert!(close((lon, lat), (34.78, 32.08), 1e-7));
        // southern hemisphere (Sydney, zone 56H)
        let (z, b, e, n) = utm::from_wgs84(151.2, -33.85);
        assert_eq!((z, b), (56, 'H'));
        let (lon, lat) = utm::to_wgs84(z, utm::band_is_north(b).unwrap(), e, n);
        assert!(close((lon, lat), (151.2, -33.85), 1e-7), "{lon} {lat}");
    }

    #[test]
    fn parses_all_formats() {
        let c = p("178824 665036").unwrap();
        assert_eq!(c.format, CoordFormat::Itm);
        assert!(close((c.lon, c.lat), (34.774_082, 32.077_885), 2e-5));
        let c = p("665036/178824").unwrap();
        assert_eq!(c.format, CoordFormat::Itm);
        let (_, _, e, n) = utm::from_wgs84(34.78, 32.08);
        let q = format!("36S {} {}", e.round(), n.round());
        let c = p(&q).unwrap();
        assert_eq!(c.format, CoordFormat::Utm);
        assert!(close((c.lon, c.lat), (34.78, 32.08), 2e-5), "{c:?}");
        assert_eq!(p(&format!("36 S {} {}", e.round(), n.round())).unwrap().format, CoordFormat::Utm);
        for q in ["32°04′48″N 34°46′48″E", "32°04'48\"N, 34°46'48\"E", "N32 4 48 E34 46 48", "32 4 48 צ 34 46 48 מ", "34°46′48″ מזרח 32°04′48″ צפון", "32d4m48s N 34d46m48s E", "32 4 48 34 46 48"] {
            let c = p(q).unwrap_or_else(|| panic!("{q}"));
            assert_eq!(c.format, CoordFormat::Dms, "{q}");
            assert!(close((c.lon, c.lat), (34.78, 32.08), 1e-6), "{q}: {c:?}");
        }
        let c = p("32°04.8'N 34°46.8'E").unwrap();
        assert!(close((c.lon, c.lat), (34.78, 32.08), 1e-6));
        let c = p("33°51′S 151°12′E").unwrap();
        assert!(close((c.lon, c.lat), (151.2, -33.85), 1e-6));
        // decimal degrees, both orders, anywhere
        let c = p("32.08, 34.78").unwrap();
        assert!(close((c.lon, c.lat), (34.78, 32.08), 1e-9));
        let c = p("34.78 32.08").unwrap();
        assert!(close((c.lon, c.lat), (34.78, 32.08), 1e-9));
        let c = p("48.2082, 16.3738").unwrap();
        assert!(close((c.lon, c.lat), (16.3738, 48.2082), 1e-9));
        let c = parse("16.3738 48.2082", None, &|lon, lat| (9.0..17.0).contains(&lon) && (46.0..49.0).contains(&lat)).unwrap();
        assert!(close((c.lon, c.lat), (16.3738, 48.2082), 1e-9));
        assert!(p("3 5").is_none());
        assert!(p("הרצל 12").is_none());
        assert!(p("12 34 56").is_none());
    }

    #[test]
    fn plus_codes() {
        // 8FVC9G8F+6X: Zürich, cell centre (47.36556, 8.52494)
        let (lon, lat) = olc::decode("8FVC9G8F+6X").unwrap();
        assert!(close((lon, lat), (8.524_937_5, 47.365_562_5), 1e-7), "{lon} {lat}");
        let c = parse("9G8F+6X Zurich", Some([8.5, 47.4]), &|_, _| false).unwrap();
        assert_eq!(c.format, CoordFormat::Plus);
        assert!(close((c.lon, c.lat), (8.524_937_5, 47.365_562_5), 1e-7));
        // grid refinement beyond 10 digits stays inside the cell
        let (lon2, lat2) = olc::decode("8FVC9G8F+6XQ").unwrap();
        assert!(close((lon2, lat2), (lon, lat), 1.3e-4));
        // round trip through a short code near Tel Aviv
        let full = format!("{}+", "8G4P3QJJ");
        let (lon, lat) = olc::decode(&full).unwrap();
        let short = format!("{}+", &full[4..8]);
        let (lon2, lat2) = olc::decode_near(&short, 34.8, 32.1).unwrap();
        assert!(close((lon, lat), (lon2, lat2), 1e-9), "{lon},{lat} vs {lon2},{lat2}");
        assert!(p("ab+cd").is_none());
    }
}
