/**
 * Overlays of the map tools (stream F2): measuring, drawing, isochrone bands, the track being
 * recorded, live vehicles of a previewed line, and the user layers (imported files, drawings,
 * recorded tracks). Each `add*` is idempotent (re-run after every full style load); each `sync*`
 * only replaces GeoJSON data.
 */
import type * as maplibregl from "maplibre-gl";
import type { Feature, FeatureCollection } from "geojson";
import type { IsochroneBand, LonLat, TransitVehiclePos, UserLayer } from "../../lib/api";
import type { DrawState, MeasureState } from "../../lib/store/map";
import { beforeIfPresent, ensureSource, setFeatures } from "../geojson";
import { areaM2, distanceM, pathLengthM } from "../measure";

export const SRC_MEASURE = "tool-measure";
export const SRC_DRAW = "tool-draw";
export const SRC_ISO = "tool-isochrone";
export const SRC_TRACK = "tool-track";
export const SRC_VEHICLES = "tool-vehicles";
export const USER_PREFIX = "user-";

const addLayer = (map: maplibregl.Map, l: maplibregl.LayerSpecification, before?: string) => {
  if (!map.getLayer(l.id)) map.addLayer(l, before);
};

// ---------------------------------------------------------------- measure
export function addMeasure(map: maplibregl.Map, accent: string): void {
  ensureSource(map, SRC_MEASURE);
  addLayer(map, { id: "measure-fill", type: "fill", source: SRC_MEASURE, filter: ["==", ["geometry-type"], "Polygon"], paint: { "fill-color": accent, "fill-opacity": 0.12 } });
  addLayer(map, { id: "measure-line-casing", type: "line", source: SRC_MEASURE, filter: ["==", ["geometry-type"], "LineString"], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": "#ffffff", "line-width": 5 } });
  addLayer(map, { id: "measure-line", type: "line", source: SRC_MEASURE, filter: ["==", ["geometry-type"], "LineString"], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": accent, "line-width": 2.5, "line-dasharray": [2, 1.5] } });
  addLayer(map, { id: "measure-points", type: "circle", source: SRC_MEASURE, filter: ["==", ["geometry-type"], "Point"], paint: { "circle-radius": 4.5, "circle-color": "#ffffff", "circle-stroke-color": accent, "circle-stroke-width": 2.5 } });
  addLayer(map, {
    id: "measure-labels",
    type: "symbol",
    source: SRC_MEASURE,
    filter: ["all", ["==", ["geometry-type"], "Point"], ["has", "label"]],
    layout: { "text-field": ["get", "label"], "text-font": ["Noto Sans Medium"], "text-size": 11, "text-offset": [0, -1.2], "text-anchor": "bottom", "text-allow-overlap": true },
    paint: { "text-color": "#1f2328", "text-halo-color": "#ffffff", "text-halo-width": 1.6 },
  });
}

/** Cumulative distance label text per vertex (from the 2nd on). */
export function measureFeatures(m: MeasureState, fmt: (meters: number) => string): Feature[] {
  const pts = m.points;
  const out: Feature[] = [];
  if (pts.length >= 2) {
    const line = m.area && pts.length >= 3 ? [...pts, pts[0]] : pts;
    out.push({ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: line } });
  }
  if (m.area && pts.length >= 3) out.push({ type: "Feature", properties: {}, geometry: { type: "Polygon", coordinates: [[...pts, pts[0]]] } });
  let acc = 0;
  pts.forEach((p, i) => {
    if (i > 0) acc += distanceM(pts[i - 1], p);
    out.push({ type: "Feature", properties: i > 0 ? { label: fmt(acc) } : {}, geometry: { type: "Point", coordinates: p } });
  });
  return out;
}

export function syncMeasure(map: maplibregl.Map, m: MeasureState | null, fmt: (meters: number) => string): void {
  setFeatures(map, SRC_MEASURE, m ? measureFeatures(m, fmt) : []);
}

export function measureSummary(m: MeasureState): { lengthM: number; areaM2: number | null } {
  const closed = m.area && m.points.length >= 3;
  return { lengthM: pathLengthM(m.points, closed), areaM2: closed ? areaM2(m.points) : null };
}

// ---------------------------------------------------------------- drawing
export function addDraw(map: maplibregl.Map): void {
  ensureSource(map, SRC_DRAW);
  const color: maplibregl.ExpressionSpecification = ["coalesce", ["get", "color"], "#d6332b"];
  addLayer(map, { id: "draw-fill", type: "fill", source: SRC_DRAW, filter: ["==", ["geometry-type"], "Polygon"], paint: { "fill-color": color, "fill-opacity": 0.18 } });
  const lines: maplibregl.ExpressionSpecification = ["in", ["geometry-type"], ["literal", ["LineString", "Polygon"]]];
  addLayer(map, { id: "draw-line", type: "line", source: SRC_DRAW, filter: ["all", lines, ["!", ["has", "pending"]]], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": color, "line-width": 3 } });
  // the feature being drawn: dashed (line-dasharray is not data-driven, hence its own layer)
  addLayer(map, { id: "draw-pending", type: "line", source: SRC_DRAW, filter: ["all", lines, ["has", "pending"]], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": color, "line-width": 3, "line-dasharray": [2, 1.5] } });
  addLayer(map, { id: "draw-points", type: "circle", source: SRC_DRAW, filter: ["all", ["==", ["geometry-type"], "Point"], ["!", ["has", "text"]]], paint: { "circle-radius": ["case", ["get", "vertex"], 3.5, 6], "circle-color": ["case", ["get", "vertex"], "#ffffff", color], "circle-stroke-color": ["case", ["get", "vertex"], color, "#ffffff"], "circle-stroke-width": 2 } });
  addLayer(map, { id: "draw-text", type: "symbol", source: SRC_DRAW, filter: ["has", "text"], layout: { "text-field": ["get", "text"], "text-font": ["Noto Sans Medium"], "text-size": 14, "text-allow-overlap": true }, paint: { "text-color": color, "text-halo-color": "#ffffff", "text-halo-width": 2 } });
}

export function drawFeatures(d: DrawState): Feature[] {
  const out = [...d.features];
  const c = d.current;
  if (c.length >= 2) {
    const coords = d.mode === "polygon" && c.length >= 3 ? [...c, c[0]] : c;
    out.push({ type: "Feature", properties: { color: d.color, pending: true }, geometry: { type: "LineString", coordinates: coords } });
  }
  c.forEach((p) => out.push({ type: "Feature", properties: { color: d.color, vertex: true }, geometry: { type: "Point", coordinates: p } }));
  return out;
}

export function syncDraw(map: maplibregl.Map, d: DrawState | null): void {
  setFeatures(map, SRC_DRAW, d ? drawFeatures(d) : []);
}

// ---------------------------------------------------------------- isochrone
const ISO_COLORS = ["#16a34a", "#eab308", "#ea580c", "#dc2626"];

export function addIsochrone(map: maplibregl.Map, beforeId: string | undefined): void {
  ensureSource(map, SRC_ISO);
  const before = beforeIfPresent(map, beforeId);
  addLayer(map, { id: "iso-fill", type: "fill", source: SRC_ISO, filter: ["==", ["geometry-type"], "Polygon"], paint: { "fill-color": ["get", "color"], "fill-opacity": 0.16 } }, before);
  addLayer(map, { id: "iso-line", type: "line", source: SRC_ISO, filter: ["==", ["geometry-type"], "Polygon"], paint: { "line-color": ["get", "color"], "line-width": 1.5, "line-opacity": 0.8 } }, before);
  addLayer(map, { id: "iso-origin", type: "circle", source: SRC_ISO, filter: ["==", ["geometry-type"], "Point"], paint: { "circle-radius": 6, "circle-color": "#1f2328", "circle-stroke-color": "#ffffff", "circle-stroke-width": 2 } });
}

export function isoColor(i: number): string {
  return ISO_COLORS[Math.min(i, ISO_COLORS.length - 1)];
}

/** Bands drawn largest first, so the smaller (nearer) ones sit on top. */
export function isochroneFeatures(at: LonLat, bands: IsochroneBand[] | null): Feature[] {
  const out: Feature[] = [];
  const sorted = [...(bands ?? [])].sort((a, b) => b.minutes - a.minutes);
  const order = [...(bands ?? [])].sort((a, b) => a.minutes - b.minutes).map((b) => b.minutes);
  for (const b of sorted) {
    const color = isoColor(order.indexOf(b.minutes));
    for (const ring of b.rings) {
      if (ring.length >= 3) out.push({ type: "Feature", properties: { minutes: b.minutes, color }, geometry: { type: "Polygon", coordinates: [ring] } });
    }
  }
  out.push({ type: "Feature", properties: {}, geometry: { type: "Point", coordinates: at } });
  return out;
}

export function syncIsochrone(map: maplibregl.Map, at: LonLat | null, bands: IsochroneBand[] | null): void {
  setFeatures(map, SRC_ISO, at ? isochroneFeatures(at, bands) : []);
}

// ---------------------------------------------------------------- track recording
export function addTrack(map: maplibregl.Map): void {
  ensureSource(map, SRC_TRACK);
  addLayer(map, { id: "track-casing", type: "line", source: SRC_TRACK, layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": "#ffffff", "line-width": 7 } });
  addLayer(map, { id: "track-line", type: "line", source: SRC_TRACK, layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": "#dc2626", "line-width": 4 } });
}

export function syncTrack(map: maplibregl.Map, points: [number, number, number | null][] | null): void {
  setFeatures(map, SRC_TRACK, points && points.length >= 2 ? [{ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: points.map((p) => [p[0], p[1]]) } }] : []);
}

// ---------------------------------------------------------------- live vehicles of a line
export function addVehicles(map: maplibregl.Map): void {
  ensureSource(map, SRC_VEHICLES);
  addLayer(map, { id: "vehicles", type: "circle", source: SRC_VEHICLES, paint: { "circle-radius": 7, "circle-color": ["get", "color"], "circle-stroke-color": "#ffffff", "circle-stroke-width": 2.5 } });
  addLayer(map, { id: "vehicles-label", type: "symbol", source: SRC_VEHICLES, minzoom: 12, layout: { "text-field": ["get", "headsign"], "text-font": ["Noto Sans Regular"], "text-size": 10, "text-offset": [0, 1.3], "text-anchor": "top", "text-optional": true }, paint: { "text-color": "#1f2328", "text-halo-color": "#ffffff", "text-halo-width": 1.4 } });
}

export function syncVehicles(map: maplibregl.Map, list: TransitVehiclePos[], color: string): void {
  setFeatures(
    map,
    SRC_VEHICLES,
    list.map((v) => ({ type: "Feature", properties: { color, headsign: v.headsign ?? "" }, geometry: { type: "Point", coordinates: [v.lon, v.lat] } })),
  );
}

// ---------------------------------------------------------------- user layers
/** Layer ids of one user layer's source. */
export function userLayerIds(id: string): string[] {
  const s = USER_PREFIX + id;
  return [`${s}-fill`, `${s}-casing`, `${s}-line`, `${s}-point`, `${s}-label`];
}

/** The collection each user source shows now (to skip setData when nothing changed). */
const shownData = new Map<string, FeatureCollection>();

/** Add / update / remove the sources of the user layers to match `list` (visible ones only).
 *  `data(id)` gives the features once loaded. */
export function syncUserLayers(map: maplibregl.Map, list: UserLayer[], data: Map<string, FeatureCollection>, beforeId: string | undefined): void {
  const want = new Set(list.filter((l) => l.visible && data.has(l.id)).map((l) => l.id));
  // drop layers no longer wanted
  for (const s of Object.keys(map.getStyle()?.sources ?? {})) {
    if (!s.startsWith(USER_PREFIX)) continue;
    const id = s.slice(USER_PREFIX.length);
    if (want.has(id)) continue;
    for (const l of userLayerIds(id)) if (map.getLayer(l)) map.removeLayer(l);
    map.removeSource(s);
    shownData.delete(s);
  }
  const before = beforeIfPresent(map, beforeId);
  for (const l of list) {
    if (!want.has(l.id)) continue;
    const src = USER_PREFIX + l.id;
    const fc = data.get(l.id)!;
    const existing = map.getSource(src) as maplibregl.GeoJSONSource | undefined;
    if (!existing) map.addSource(src, { type: "geojson", data: fc });
    else if (shownData.get(src) !== fc) existing.setData(fc);
    shownData.set(src, fc);
    const [fill, casing, line, point, label] = userLayerIds(l.id);
    const c = l.color || "#d6332b";
    const color: maplibregl.ExpressionSpecification = ["coalesce", ["get", "color"], c];
    addLayer(map, { id: fill, type: "fill", source: src, filter: ["==", ["geometry-type"], "Polygon"], paint: { "fill-color": color, "fill-opacity": 0.15 } }, before);
    addLayer(map, { id: casing, type: "line", source: src, filter: ["in", ["geometry-type"], ["literal", ["LineString", "Polygon"]]], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": "#ffffff", "line-width": 6, "line-opacity": 0.85 } }, before);
    addLayer(map, { id: line, type: "line", source: src, filter: ["in", ["geometry-type"], ["literal", ["LineString", "Polygon"]]], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": color, "line-width": 3.5 } }, before);
    addLayer(map, { id: point, type: "circle", source: src, filter: ["all", ["==", ["geometry-type"], "Point"], ["!", ["has", "text"]]], paint: { "circle-radius": 5.5, "circle-color": color, "circle-stroke-color": "#ffffff", "circle-stroke-width": 2 } });
    addLayer(map, {
      id: label,
      type: "symbol",
      source: src,
      minzoom: 11,
      filter: ["any", ["has", "text"], ["all", ["==", ["geometry-type"], "Point"], ["has", "name"]]],
      layout: { "text-field": ["coalesce", ["get", "text"], ["get", "name"]], "text-font": ["Noto Sans Medium"], "text-size": 12, "text-offset": [0, 0.9], "text-anchor": "top", "text-optional": true },
      paint: { "text-color": color, "text-halo-color": "#ffffff", "text-halo-width": 1.6 },
    });
    // the layer colour may have changed
    if (map.getLayer(line)) {
      map.setPaintProperty(line, "line-color", color);
      map.setPaintProperty(fill, "fill-color", color);
      map.setPaintProperty(point, "circle-color", color);
      map.setPaintProperty(label, "text-color", color);
    }
  }
}

// ---------------------------------------------------------------- areas the route avoids
export const SRC_AVOID = "tool-avoid";

export function addAvoid(map: maplibregl.Map, beforeId: string | undefined): void {
  ensureSource(map, SRC_AVOID);
  const before = beforeIfPresent(map, beforeId);
  addLayer(map, { id: "avoid-fill", type: "fill", source: SRC_AVOID, paint: { "fill-color": "#c93636", "fill-opacity": 0.12 } }, before);
  addLayer(map, { id: "avoid-line", type: "line", source: SRC_AVOID, paint: { "line-color": "#c93636", "line-width": 2, "line-dasharray": [3, 2] } }, before);
}

/** The areas in the route request's avoid.polygons, shown while a trip is planned. */
export function syncAvoid(map: maplibregl.Map, polygons: LonLat[][] | undefined): void {
  setFeatures(
    map,
    SRC_AVOID,
    (polygons ?? [])
      .filter((r) => r.length >= 3)
      .map((r) => ({ type: "Feature", properties: {}, geometry: { type: "Polygon", coordinates: [r[0][0] === r[r.length - 1][0] && r[0][1] === r[r.length - 1][1] ? r : [...r, r[0]]] } })),
  );
}

/** Every tool overlay, in stacking order: user layers and isochrone under the labels, the rest on top. */
export function addTools(map: maplibregl.Map, firstSymbol: string | undefined, accent: string): void {
  addAvoid(map, firstSymbol);
  addIsochrone(map, firstSymbol);
  addVehicles(map);
  addTrack(map);
  addDraw(map);
  addMeasure(map, accent);
}
