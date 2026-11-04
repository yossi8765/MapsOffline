import * as maplibregl from "maplibre-gl";
import type { Feature, Point } from "geojson";
import type { Route } from "../../lib/api";
import type { HighlightState } from "../../lib/store";
import { legColor, mapColors } from "../colors";
import { beforeIfPresent, ensureSource, setFeatures } from "../geojson";

export const SRC_ALTS = "route-alts";
export const SRC_SEL = "route-selected";
export const SRC_HL = "route-highlight";
export const SRC_STOPS = "route-stops";

/** Map features of the route results: the alternatives, the selected route's legs and its stops. */
export function routeFeatures(routes: Route[], selectedId: string | null) {
  const alts: Feature[] = [];
  const sel: Feature[] = [];
  const stops: Feature[] = [];
  for (const r of routes) {
    if (r.id !== selectedId) {
      alts.push({ type: "Feature", properties: { id: r.id }, geometry: { type: "LineString", coordinates: r.geometry } });
      continue;
    }
    r.legs.forEach((leg, i) => {
      sel.push({
        type: "Feature",
        properties: { id: r.id, leg: i, kind: leg.kind, color: legColor(r, i), walk: leg.kind === "walk" && r.mode === "transit" },
        geometry: { type: "LineString", coordinates: leg.geometry },
      });
      if (leg.transit) {
        const ts = leg.transit;
        ts.stops.forEach((s, j) => {
          const isEnd = j === 0 || j === ts.stops.length - 1;
          stops.push({
            type: "Feature",
            properties: { name: s.name, end: isEnd, color: legColor(r, i) },
            geometry: { type: "Point", coordinates: [s.lon, s.lat] },
          });
        });
      }
    });
  }
  return { alts, sel, stops };
}

/** Route layers under the base map's labels (`beforeId` = first symbol layer). */
export function add(map: maplibregl.Map, beforeId: string | undefined, dark: boolean): void {
  const mc = mapColors(dark);
  const casing = mc.casing;
  for (const id of [SRC_ALTS, SRC_SEL, SRC_HL, SRC_STOPS]) ensureSource(map, id);
  const before = beforeIfPresent(map, beforeId);
  const addLayer = (l: maplibregl.LayerSpecification) => {
    if (!map.getLayer(l.id)) map.addLayer(l, before);
  };
  addLayer({
    id: "route-alts-casing",
    type: "line",
    source: SRC_ALTS,
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": casing, "line-width": 8, "line-opacity": 0.6 },
  });
  addLayer({
    id: "route-alts",
    type: "line",
    source: SRC_ALTS,
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": mc.alt, "line-width": 5, "line-opacity": 0.55 },
  });
  addLayer({
    id: "route-sel-casing",
    type: "line",
    source: SRC_SEL,
    filter: ["!", ["get", "walk"]],
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": casing, "line-width": 10, "line-opacity": 0.9 },
  });
  addLayer({
    id: "route-sel",
    type: "line",
    source: SRC_SEL,
    filter: ["!", ["get", "walk"]],
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": ["get", "color"], "line-width": 6 },
  });
  addLayer({
    id: "route-sel-walk",
    type: "line",
    source: SRC_SEL,
    filter: ["get", "walk"],
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": ["get", "color"], "line-width": 5, "line-dasharray": [0.2, 1.6] },
  });
  addLayer({
    id: "route-hl",
    type: "line",
    source: SRC_HL,
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": mc.highlight, "line-width": 10, "line-opacity": 0.75 },
  });
  addLayer({
    id: "route-stops",
    type: "circle",
    source: SRC_STOPS,
    paint: {
      "circle-radius": ["case", ["get", "end"], 6, 4],
      "circle-color": casing,
      "circle-stroke-color": ["get", "color"],
      "circle-stroke-width": ["case", ["get", "end"], 3, 2],
    },
  });
  addLayer({
    id: "route-stops-label",
    type: "symbol",
    source: SRC_STOPS,
    minzoom: 13,
    filter: ["get", "end"],
    layout: {
      "text-field": ["get", "name"],
      "text-font": ["Noto Sans Regular"],
      "text-size": 11,
      "text-offset": [0, 1.1],
      "text-anchor": "top",
      "text-optional": true,
    },
    paint: { "text-color": mc.label, "text-halo-color": casing, "text-halo-width": 1.5 },
  });
}

export function sync(map: maplibregl.Map, routes: Route[], selectedId: string | null): void {
  const { alts, sel, stops } = routeFeatures(routes, selectedId);
  setFeatures(map, SRC_ALTS, alts);
  setFeatures(map, SRC_SEL, sel);
  setFeatures(map, SRC_STOPS, stops);
}

/** The highlighted leg / step of a route (hover in the step list). */
export function syncHighlight(map: maplibregl.Map, routes: Route[], highlight: HighlightState | null): void {
  if (!map.getSource(SRC_HL)) return;
  const r = routes.find((x) => x.id === highlight?.routeId);
  if (!r || !highlight) {
    setFeatures(map, SRC_HL, []);
    return;
  }
  const leg = r.legs[highlight.legIndex];
  if (!leg) return;
  let coords = leg.geometry;
  if (highlight.stepIndex !== undefined && leg.steps?.length) {
    const st = leg.steps[highlight.stepIndex];
    const next = leg.steps[highlight.stepIndex + 1];
    const a = st?.geometryStart ?? 0;
    const b = next ? next.geometryStart + 1 : leg.geometry.length;
    coords = leg.geometry.slice(a, Math.max(b, a + 2));
  }
  setFeatures(map, SRC_HL, coords.length > 1 ? [{ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: coords } }] : []);
}

/** Clicks on the route overlays: an alternative selects it, a stop shows its name. */
export function bindClicks(map: maplibregl.Map, opts: { onSelect: (id: string) => void; showPopup: (p: maplibregl.Popup) => void }): void {
  map.on("click", "route-alts", (e) => {
    const id = e.features?.[0]?.properties?.id as string | undefined;
    if (id) opts.onSelect(id);
  });
  map.on("mouseenter", "route-alts", () => (map.getCanvas().style.cursor = "pointer"));
  map.on("mouseleave", "route-alts", () => (map.getCanvas().style.cursor = ""));
  map.on("click", "route-stops", (e) => {
    const f = e.features?.[0];
    if (!f) return;
    const [lon, lat] = (f.geometry as Point).coordinates;
    opts.showPopup(new maplibregl.Popup({ offset: 10 }).setLngLat([lon, lat]).setText(String(f.properties?.name ?? "")));
  });
}

/** Fit every route into the view, leaving room for the side panel (`sideStart`: the panel's
 *  side, "right" in Hebrew). */
export function fit(map: maplibregl.Map, routes: Route[], panelWidth: number, panelSide: "left" | "right"): void {
  const b = new maplibregl.LngLatBounds();
  for (const r of routes) for (const p of r.geometry) b.extend(p);
  if (b.isEmpty()) return;
  const pad = { top: 60, bottom: 60, left: 60, right: 60 };
  if (panelWidth > 0) pad[panelSide] = panelWidth;
  map.fitBounds(b, { padding: pad, maxZoom: 16, duration: 600 });
}
