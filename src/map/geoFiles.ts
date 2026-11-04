/**
 * Imported map files → GeoJSON (report ג-2): GPX (tracks, routes, waypoints, with elevation),
 * KML (Placemark Point / LineString / Polygon / MultiGeometry, gx:Track) and GeoJSON. Parsed in
 * the webview with DOMParser; the result is stored as a user layer by the backend. Coordinates
 * are [lon, lat] or [lon, lat, ele].
 */
import type { Feature, FeatureCollection, Geometry, Position } from "geojson";
import type { LonLat } from "../lib/api";

export type GeoFileKind = "gpx" | "kml" | "geojson";

/** File kind by extension, else by a look at the content. */
export function detectKind(name: string, text: string): GeoFileKind | null {
  const ext = /\.([a-z0-9]+)$/i.exec(name)?.[1]?.toLowerCase();
  if (ext === "gpx") return "gpx";
  if (ext === "kml") return "kml";
  if (ext === "geojson" || ext === "json") return "geojson";
  const head = text.slice(0, 600).trimStart();
  if (head.startsWith("{")) return "geojson";
  if (/<gpx[\s>]/i.test(head)) return "gpx";
  if (/<kml[\s>]/i.test(head)) return "kml";
  return null;
}

function fail(msg: string): never {
  throw new Error(`PARSE: ${msg}`);
}

function xml(text: string): Document {
  const doc = new DOMParser().parseFromString(text, "application/xml");
  if (doc.getElementsByTagName("parsererror").length) fail("xml");
  return doc;
}

/** Child elements by local name (namespace-agnostic: GPX 1.0/1.1, KML 2.2, gx:). */
function kids(el: Element | Document, name: string): Element[] {
  return Array.from(el.getElementsByTagName("*")).filter((e) => e.localName === name);
}

function directKids(el: Element, name: string): Element[] {
  return Array.from(el.children).filter((e) => e.localName === name);
}

function text(el: Element | undefined, name: string): string | undefined {
  const c = el ? directKids(el, name)[0] : undefined;
  const v = c?.textContent?.trim();
  return v || undefined;
}

function gpxPoint(p: Element): Position | null {
  const lat = Number(p.getAttribute("lat"));
  const lon = Number(p.getAttribute("lon"));
  if (!Number.isFinite(lat) || !Number.isFinite(lon)) return null;
  const ele = Number(text(p, "ele"));
  return Number.isFinite(ele) && text(p, "ele") !== undefined ? [lon, lat, ele] : [lon, lat];
}

export function parseGpx(src: string): FeatureCollection {
  const doc = xml(src);
  const features: Feature[] = [];
  for (const trk of kids(doc, "trk")) {
    const lines = directKids(trk, "trkseg")
      .map((seg) => directKids(seg, "trkpt").map(gpxPoint).filter((p): p is Position => !!p))
      .filter((l) => l.length >= 2);
    if (!lines.length) continue;
    const geometry: Geometry = lines.length === 1 ? { type: "LineString", coordinates: lines[0] } : { type: "MultiLineString", coordinates: lines };
    features.push({ type: "Feature", properties: { name: text(trk, "name") ?? "", kind: "track" }, geometry });
  }
  for (const rte of kids(doc, "rte")) {
    const pts = directKids(rte, "rtept").map(gpxPoint).filter((p): p is Position => !!p);
    if (pts.length >= 2) features.push({ type: "Feature", properties: { name: text(rte, "name") ?? "", kind: "route" }, geometry: { type: "LineString", coordinates: pts } });
  }
  for (const w of kids(doc, "wpt")) {
    const p = gpxPoint(w);
    if (p) features.push({ type: "Feature", properties: { name: text(w, "name") ?? "", desc: text(w, "desc") ?? "" }, geometry: { type: "Point", coordinates: p } });
  }
  return { type: "FeatureCollection", features };
}

/** KML "lon,lat[,alt] lon,lat …" */
function kmlCoords(s: string | null | undefined): Position[] {
  if (!s) return [];
  return s
    .trim()
    .split(/\s+/)
    .map((t) => t.split(",").map(Number))
    .filter((c) => c.length >= 2 && Number.isFinite(c[0]) && Number.isFinite(c[1]))
    .map((c) => (c.length >= 3 && Number.isFinite(c[2]) && c[2] !== 0 ? [c[0], c[1], c[2]] : [c[0], c[1]]));
}

function kmlGeometries(el: Element): Geometry[] {
  const out: Geometry[] = [];
  for (const c of Array.from(el.children)) {
    switch (c.localName) {
      case "Point": {
        const p = kmlCoords(text(c, "coordinates"))[0];
        if (p) out.push({ type: "Point", coordinates: p });
        break;
      }
      case "LineString": {
        const l = kmlCoords(text(c, "coordinates"));
        if (l.length >= 2) out.push({ type: "LineString", coordinates: l });
        break;
      }
      case "Polygon": {
        const outer = kids(c, "outerBoundaryIs").flatMap((b) => kids(b, "coordinates"))[0];
        const ring = kmlCoords(outer?.textContent);
        const holes = kids(c, "innerBoundaryIs").map((b) => kmlCoords(kids(b, "coordinates")[0]?.textContent)).filter((r) => r.length >= 4);
        if (ring.length >= 4) out.push({ type: "Polygon", coordinates: [ring, ...holes] });
        break;
      }
      case "Track": {
        // gx:Track: <gx:coord>lon lat alt</gx:coord>
        const pts = directKids(c, "coord")
          .map((e) => (e.textContent ?? "").trim().split(/\s+/).map(Number))
          .filter((v) => v.length >= 2 && Number.isFinite(v[0]) && Number.isFinite(v[1]))
          .map((v) => (v.length >= 3 && Number.isFinite(v[2]) ? [v[0], v[1], v[2]] : [v[0], v[1]]));
        if (pts.length >= 2) out.push({ type: "LineString", coordinates: pts });
        break;
      }
      case "MultiGeometry":
      case "MultiTrack":
        out.push(...kmlGeometries(c));
        break;
    }
  }
  return out;
}

export function parseKml(src: string): FeatureCollection {
  const doc = xml(src);
  const features: Feature[] = [];
  for (const pm of kids(doc, "Placemark")) {
    const name = text(pm, "name") ?? "";
    const desc = text(pm, "description") ?? "";
    for (const g of kmlGeometries(pm)) features.push({ type: "Feature", properties: { name, desc }, geometry: g });
  }
  return { type: "FeatureCollection", features };
}

export function parseGeoJson(src: string): FeatureCollection {
  let v: unknown;
  try {
    v = JSON.parse(src);
  } catch {
    fail("json");
  }
  const o = v as { type?: string; features?: unknown[]; geometry?: unknown; coordinates?: unknown };
  if (o?.type === "FeatureCollection" && Array.isArray(o.features)) {
    return { type: "FeatureCollection", features: (o.features as Feature[]).filter((f) => f && f.type === "Feature" && f.geometry) };
  }
  if (o?.type === "Feature" && o.geometry) return { type: "FeatureCollection", features: [{ ...(o as Feature), properties: (o as Feature).properties ?? {} }] };
  if (o && typeof o.type === "string" && "coordinates" in o) return { type: "FeatureCollection", features: [{ type: "Feature", properties: {}, geometry: o as unknown as Geometry }] };
  fail("geojson");
}

export function parseGeoFile(name: string, src: string): FeatureCollection {
  const kind = detectKind(name, src);
  if (kind === "gpx") return parseGpx(src);
  if (kind === "kml") return parseKml(src);
  if (kind === "geojson") return parseGeoJson(src);
  fail("kind");
}

/** The first line of a collection as [lon, lat] points (LineString, or the longest part of a
 *  MultiLineString), for map matching / elevation. */
export function firstLine(fc: FeatureCollection): LonLat[] | null {
  for (const f of fc.features) {
    const g = f.geometry;
    if (!g) continue;
    if (g.type === "LineString") return g.coordinates.map((c) => [c[0], c[1]]);
    if (g.type === "MultiLineString" && g.coordinates.length) {
      const longest = [...g.coordinates].sort((a, b) => b.length - a.length)[0];
      return longest.map((c) => [c[0], c[1]]);
    }
  }
  return null;
}

/** Elevations stored in a line's third coordinate, or null when the file had none. */
export function lineElevations(fc: FeatureCollection): (number | null)[] | null {
  for (const f of fc.features) {
    const g = f.geometry;
    const coords = g?.type === "LineString" ? g.coordinates : g?.type === "MultiLineString" ? g.coordinates.flat() : null;
    if (!coords) continue;
    const eles = coords.map((c) => (c.length >= 3 && Number.isFinite(c[2]) ? c[2] : null));
    return eles.some((e) => e !== null) ? eles : null;
  }
  return null;
}

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** A line (and its elevations) as a GPX track. */
export function lineToGpx(name: string, pts: LonLat[], ele?: (number | null)[]): string {
  const trkpts = pts
    .map((p, i) => {
      const e = ele?.[i];
      return `      <trkpt lat="${p[1].toFixed(7)}" lon="${p[0].toFixed(7)}">${e != null ? `<ele>${e.toFixed(1)}</ele>` : ""}</trkpt>`;
    })
    .join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="MapsOffline" xmlns="http://www.topografix.com/GPX/1/1">
  <trk>
    <name>${esc(name)}</name>
    <trkseg>
${trkpts}
    </trkseg>
  </trk>
</gpx>
`;
}
