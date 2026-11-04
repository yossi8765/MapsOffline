/** Printout maths (report ג-32): the zoom for a paper scale and the ITM kilometre grid. */
import type { Feature, FeatureCollection } from "geojson";
import { fromItm, toItm } from "../lib/coords";

/** Width of the picture in CSS px and on paper (A4 portrait, 15 mm margins). */
export const PRINT_W = 1600;
export const PAPER_WIDTH_MM = 180;

/** Zoom at which `scale` (1:N) holds when PRINT_W CSS px are printed PAPER_WIDTH_MM wide at `lat`. */
export function zoomForScale(scale: number, lat: number): number {
  const metersPerPx = (scale * (PAPER_WIDTH_MM / 1000)) / PRINT_W;
  // web-mercator metres per CSS px at zoom 0 (512 px world in MapLibre)
  const mpp0 = (40075016.686 * Math.cos((lat * Math.PI) / 180)) / 512;
  return Math.log2(mpp0 / metersPerPx);
}

/** Grid spacing in metres for the printed scale ("fit": chosen from the box width, ~8 lines). */
export function gridStep(scale: "fit" | number, widthM: number): number {
  if (scale === 25000) return 1000;
  if (scale === 50000) return 2000;
  const target = widthM / 8;
  for (const s of [250, 500, 1000, 2000, 5000, 10000, 20000, 50000]) if (s >= target) return s;
  return 100000;
}

/** ITM grid lines (labelled in km) covering a lon/lat box; empty when absurdly many. */
export function itmGrid(box: [number, number, number, number], step: number): FeatureCollection {
  const [w, s, e, n] = box;
  const corners = [toItm(w, s), toItm(w, n), toItm(e, s), toItm(e, n)];
  const minE = Math.floor(Math.min(...corners.map((c) => c[0])) / step) * step;
  const maxE = Math.ceil(Math.max(...corners.map((c) => c[0])) / step) * step;
  const minN = Math.floor(Math.min(...corners.map((c) => c[1])) / step) * step;
  const maxN = Math.ceil(Math.max(...corners.map((c) => c[1])) / step) * step;
  const features: Feature[] = [];
  if ((maxE - minE) / step > 200 || (maxN - minN) / step > 200) return { type: "FeatureCollection", features };
  const SEG = 16;
  const km = (v: number) => String(Math.round(v / 1000));
  for (let x = minE; x <= maxE; x += step) {
    const line = Array.from({ length: SEG + 1 }, (_, i) => fromItm(x, minN + ((maxN - minN) * i) / SEG));
    features.push({ type: "Feature", properties: { label: km(x), axis: "e" }, geometry: { type: "LineString", coordinates: line } });
  }
  for (let y = minN; y <= maxN; y += step) {
    const line = Array.from({ length: SEG + 1 }, (_, i) => fromItm(minE + ((maxE - minE) * i) / SEG, y));
    features.push({ type: "Feature", properties: { label: km(y), axis: "n" }, geometry: { type: "LineString", coordinates: line } });
  }
  return { type: "FeatureCollection", features };
}
