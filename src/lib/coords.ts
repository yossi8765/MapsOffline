// Coordinate formats for display and copy: decimal degrees, DMS, Israeli Transverse Mercator
// (ITM, EPSG:2039) and UTM. ITM is a port of src-tauri/src/data/orthophoto.rs::itm (7-parameter
// Helmert shift WGS 84 → Israel 1993, Krüger-series transverse Mercator on GRS80) and matches it
// to the centimetre. Parsing of typed coordinates is done in Rust (`coords_parse`).
import type { CoordFormat } from "./api";

const A = 6_378_137.0;
const F_WGS84 = 1 / 298.257223563;
const F_GRS80 = 1 / 298.257222101;
const rad = (d: number) => (d * Math.PI) / 180;

interface TmConsts {
  aa: number;
  alpha: number[];
  e: number;
}

function tmConsts(f: number): TmConsts {
  const n = f / (2 - f);
  const n2 = n * n,
    n3 = n2 * n,
    n4 = n3 * n;
  return {
    aa: (A / (1 + n)) * (1 + n2 / 4 + n4 / 64),
    alpha: [
      n / 2 - (2 * n2) / 3 + (5 * n3) / 16 + (41 * n4) / 180,
      (13 * n2) / 48 - (3 * n3) / 5 + (557 * n4) / 1440,
      (61 * n3) / 240 - (103 * n4) / 140,
      (49561 * n4) / 161280,
    ],
    e: Math.sqrt(f * (2 - f)),
  };
}

/** Transverse Mercator (Krüger series): radians → (easting, northing) before scale/offsets. */
function tm(c: TmConsts, lat: number, lon: number, lon0: number): [number, number] {
  const s = Math.sin(lat);
  const t = Math.sinh(Math.atanh(s) - c.e * Math.atanh(c.e * s));
  const dl = lon - lon0;
  const xi_ = Math.atan2(t, Math.cos(dl));
  const eta_ = Math.atanh(Math.sin(dl) / Math.sqrt(1 + t * t));
  let xi = xi_,
    eta = eta_;
  c.alpha.forEach((a, j) => {
    const k = 2 * (j + 1);
    xi += a * Math.sin(k * xi_) * Math.cosh(k * eta_);
    eta += a * Math.cos(k * xi_) * Math.sinh(k * eta_);
  });
  return [c.aa * eta, c.aa * xi];
}

// ---------------------------------------------------------------- ITM
const ITM_LAT0 = 31.7343936111111;
const ITM_LON0 = 35.2045169444444;
const ITM_K0 = 1.0000067;
const ITM_X0 = 219_529.584;
const ITM_Y0 = 626_907.39;
const ITM_T = [23.772, 17.49, 17.859];
const ITM_ROT = [-0.3132, -1.85274, 1.67299].map((a) => rad(a / 3600));
const ITM_S = -5.4262e-6;
const GRS = tmConsts(F_GRS80);
const ITM_NORTH0 = tm(GRS, rad(ITM_LAT0), rad(ITM_LON0), rad(ITM_LON0))[1] * ITM_K0;

/** WGS 84 lon/lat (degrees) → ITM [easting, northing] metres. */
export function toItm(lon: number, lat: number): [number, number] {
  const la = rad(lat),
    lo = rad(lon);
  const e2 = F_WGS84 * (2 - F_WGS84);
  const sl = Math.sin(la);
  const nu = A / Math.sqrt(1 - e2 * sl * sl);
  const x = nu * Math.cos(la) * Math.cos(lo) - ITM_T[0];
  const y = nu * Math.cos(la) * Math.sin(lo) - ITM_T[1];
  const z = nu * (1 - e2) * sl - ITM_T[2];
  const [rx, ry, rz] = ITM_ROT;
  const k = 1 + ITM_S;
  const lx = (x + rz * y - ry * z) / k;
  const ly = (-rz * x + y + rx * z) / k;
  const lz = (ry * x - rx * y + z) / k;
  const e2g = F_GRS80 * (2 - F_GRS80);
  const p = Math.hypot(lx, ly);
  const glon = Math.atan2(ly, lx);
  let glat = Math.atan2(lz, p * (1 - e2g));
  for (let i = 0; i < 4; i++) {
    const s = Math.sin(glat);
    const n = A / Math.sqrt(1 - e2g * s * s);
    glat = Math.atan2(lz + e2g * n * s, p);
  }
  const [e, n] = tm(GRS, glat, glon, rad(ITM_LON0));
  return [ITM_X0 + e * ITM_K0, ITM_Y0 + (n * ITM_K0 - ITM_NORTH0)];
}

/** ITM metres → WGS 84 [lon, lat] (Newton iteration on toItm; mm inside Israel). */
export function fromItm(e: number, n: number): [number, number] {
  let lon = ITM_LON0 + (e - ITM_X0) / 94_000,
    lat = ITM_LAT0 + (n - ITM_Y0) / 110_900;
  for (let i = 0; i < 8; i++) {
    const [e0, n0] = toItm(lon, lat);
    const h = 1e-6;
    const [e1, n1] = toItm(lon + h, lat);
    const [e2, n2] = toItm(lon, lat + h);
    const a = (e1 - e0) / h,
      b = (e2 - e0) / h,
      c = (n1 - n0) / h,
      d = (n2 - n0) / h;
    const det = a * d - b * c;
    const de = e - e0,
      dn = n - n0;
    lon += (d * de - b * dn) / det;
    lat += (-c * de + a * dn) / det;
    if (Math.abs(de) < 1e-4 && Math.abs(dn) < 1e-4) break;
  }
  return [lon, lat];
}

// ---------------------------------------------------------------- UTM (WGS 84)
const WGS = tmConsts(F_WGS84);

/** WGS 84 → UTM zone, band letter, easting, northing. */
export function toUtm(lon: number, lat: number): { zone: number; band: string; easting: number; northing: number } {
  const zone = Math.min(60, Math.max(1, Math.floor((lon + 180) / 6) + 1));
  const lon0 = rad(zone * 6 - 183);
  const [e, n] = tm(WGS, rad(lat), rad(lon), lon0);
  const k0 = 0.9996;
  const bands = "CDEFGHJKLMNPQRSTUVWXX";
  const band = bands[Math.min(20, Math.max(0, Math.floor((lat + 80) / 8)))];
  return { zone, band, easting: 500_000 + e * k0, northing: (lat < 0 ? 10_000_000 : 0) + n * k0 };
}

// ---------------------------------------------------------------- DMS
function dms(v: number, pos: string, neg: string): string {
  const h = v < 0 ? neg : pos;
  const a = Math.abs(v);
  let d = Math.floor(a);
  let m = Math.floor((a - d) * 60);
  let s = Math.round(((a - d) * 60 - m) * 60 * 10) / 10;
  if (s >= 60) {
    s = 0;
    m += 1;
  }
  if (m >= 60) {
    m = 0;
    d += 1;
  }
  return `${d}°${String(m).padStart(2, "0")}′${s.toFixed(1).padStart(4, "0")}″${h}`;
}

/** One coordinate as text in the chosen format (LTR text; wrap in <bdi dir="ltr">). */
export function formatCoord(lon: number, lat: number, format: CoordFormat = "dd"): string {
  switch (format) {
    case "dms":
      return `${dms(lat, "N", "S")} ${dms(lon, "E", "W")}`;
    case "itm": {
      const [e, n] = toItm(lon, lat);
      return `${Math.round(e)} ${Math.round(n)}`;
    }
    case "utm": {
      const u = toUtm(lon, lat);
      return `${u.zone}${u.band} ${Math.round(u.easting)} ${Math.round(u.northing)}`;
    }
    default:
      return `${lat.toFixed(5)}, ${lon.toFixed(5)}`;
  }
}

/** Short label of a format for menus. */
export const COORD_FORMATS: CoordFormat[] = ["dd", "dms", "itm", "utm"];
