/** Geodesic measuring on the WGS 84 sphere approximation (report ג-1): path length, polygon
 *  area, elevation gain. Accurate to ~0.3 % – the map's measuring tool, not surveying. */
import type { LonLat } from "../lib/api";

const R = 6_371_008.8; // mean Earth radius, metres
const rad = (d: number) => (d * Math.PI) / 180;

/** Great-circle distance in metres (haversine). */
export function distanceM(a: LonLat, b: LonLat): number {
  const dLat = rad(b[1] - a[1]);
  const dLon = rad(b[0] - a[0]);
  const h = Math.sin(dLat / 2) ** 2 + Math.cos(rad(a[1])) * Math.cos(rad(b[1])) * Math.sin(dLon / 2) ** 2;
  return 2 * R * Math.asin(Math.min(1, Math.sqrt(h)));
}

/** Length of a polyline in metres (closed: back to the first point too). */
export function pathLengthM(pts: LonLat[], closed = false): number {
  let d = 0;
  for (let i = 1; i < pts.length; i++) d += distanceM(pts[i - 1], pts[i]);
  if (closed && pts.length > 2) d += distanceM(pts[pts.length - 1], pts[0]);
  return d;
}

/** Area of a polygon ring in m² on the sphere (Chamberlain & Duquette; ring open or closed). */
export function areaM2(ring: LonLat[]): number {
  const pts = ring.length > 1 && ring[0][0] === ring[ring.length - 1][0] && ring[0][1] === ring[ring.length - 1][1] ? ring.slice(0, -1) : ring;
  if (pts.length < 3) return 0;
  let s = 0;
  for (let i = 0; i < pts.length; i++) {
    const p1 = pts[i];
    const p2 = pts[(i + 1) % pts.length];
    s += rad(p2[0] - p1[0]) * (2 + Math.sin(rad(p1[1])) + Math.sin(rad(p2[1])));
  }
  return Math.abs((s * R * R) / 2);
}

/** Total climb and descent in metres over a profile (gaps skipped; < 1 m jitter ignored). */
export function climb(ele: (number | null)[]): { up: number; down: number } {
  let up = 0,
    down = 0;
  let last: number | null = null;
  for (const e of ele) {
    if (e === null) continue;
    if (last !== null) {
      const d = e - last;
      if (d > 1) up += d;
      else if (d < -1) down -= d;
      else continue;
    }
    last = e;
  }
  return { up, down };
}

/** Points along a polyline every `stepM` metres (and the vertices), for elevation sampling. */
export function densify(pts: LonLat[], stepM: number, max = 256): LonLat[] {
  if (pts.length < 2) return pts;
  const total = pathLengthM(pts);
  const step = Math.max(stepM, total / max);
  const out: LonLat[] = [pts[0]];
  for (let i = 1; i < pts.length; i++) {
    const a = pts[i - 1],
      b = pts[i];
    const d = distanceM(a, b);
    const n = Math.floor(d / step);
    for (let k = 1; k <= n; k++) {
      const t = (k * step) / d;
      if (t >= 1) break;
      out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
    }
    out.push(b);
  }
  return out;
}

/** "1.24 ק״מ" / "850 מ׳" style text without the unit words (the caller formats with i18n). */
export function fmtArea(m2: number, he: boolean): string {
  if (m2 >= 1_000_000) return `${(m2 / 1_000_000).toFixed(m2 >= 10_000_000 ? 1 : 2)} ${he ? "קמ״ר" : "km²"}`;
  if (m2 >= 10_000) return `${(m2 / 1000).toFixed(m2 >= 100_000 ? 0 : 1)} ${he ? "דונם" : "dunam"}`;
  return `${Math.round(m2)} ${he ? "מ״ר" : "m²"}`;
}
