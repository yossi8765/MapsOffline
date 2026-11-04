/**
 * Printable picture of a route: an off-screen map (1600 × 1050 CSS px at pixel ratio 2), always
 * the light flavour and north up, fitted to the route or at a fixed scale (1:25,000 / 1:50,000)
 * with an optional ITM kilometre grid, with the route drawn on top. Used by the print sheet.
 * Pictures are cached per route and options, and MapView renders the selected route's picture in
 * the background (idle time), so printing usually finds it ready (report ד-23).
 */
import * as maplibregl from "maplibre-gl";
import type { FeatureCollection } from "geojson";
import type { Route } from "./api";
import { useStore } from "./store";
import { buildStyle } from "./mapStyle";
import { toItm } from "./coords";
import { gridStep, itmGrid, zoomForScale } from "../map/printGrid";
import type { PrintOptions } from "./print";

const W = 1600;
const H = 1050;

function cssVar(name: string, fallback: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

const cache = new Map<string, Promise<string>>();

/** Picture of `route` with `opts`, rendered once (cached; the last few kept). */
export function routeImage(route: Route, opts: PrintOptions): Promise<string> {
  const key = `${route.id}|${route.geometry.length}|${opts.scale}|${opts.grid}`;
  let p = cache.get(key);
  if (!p) {
    p = renderRouteImage(route, opts);
    cache.set(key, p);
    p.catch(() => cache.delete(key));
    if (cache.size > 4) cache.delete(cache.keys().next().value!);
  }
  return p;
}

/** Renders `route` and resolves with a PNG data URL (rejects after 30 s or on a map error). */
export async function renderRouteImage(route: Route, opts: PrintOptions = { scale: "fit", grid: false }): Promise<string> {
  const st = useStore.getState();
  const host = document.createElement("div");
  host.style.cssText = `position:fixed;left:-10000px;top:0;width:${W}px;height:${H}px;pointer-events:none;`;
  document.body.appendChild(host);
  const style = buildStyle({
    tilesUrl: st.tilesUrl,
    onlineTilesUrl: st.onlineTilesUrl,
    satelliteUrl: null,
    satellite: false,
    terrainUrl: st.terrainUrl,
    worldUrl: st.worldUrl,
    hillshade: false,
    terrain3d: false,
    roadShields: st.layers.roadShields,
    flavor: "light",
    lang: st.layers.labelLang,
    poi: st.layers.poi,
    buildings3d: false,
    theme: "standard",
  });
  const lons = route.geometry.map((p) => p[0]);
  const lats = route.geometry.map((p) => p[1]);
  const bounds: [[number, number], [number, number]] = [
    [Math.min(...lons), Math.min(...lats)],
    [Math.max(...lons), Math.max(...lats)],
  ];
  const center: [number, number] = [(bounds[0][0] + bounds[1][0]) / 2, (bounds[0][1] + bounds[1][1]) / 2];
  const fixed = opts.scale !== "fit";
  const map = new maplibregl.Map({
    container: host,
    style,
    ...(fixed ? { center, zoom: zoomForScale(opts.scale as number, center[1]) } : { bounds, fitBoundsOptions: { padding: 80 } }),
    bearing: 0,
    pitch: 0,
    interactive: false,
    attributionControl: false,
    pixelRatio: 2,
    canvasContextAttributes: { preserveDrawingBuffer: true, antialias: true },
  });
  const color = cssVar(route.mode === "foot" ? "--route-foot" : route.mode === "bike" ? "--route-bike" : "--route-car", "#1a66d9");
  try {
    return await new Promise<string>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("map image timed out")), 30_000);
      map.once("load", () => {
        if (opts.grid) {
          const b = map.getBounds();
          const box: [number, number, number, number] = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
          const widthM = Math.abs(toItm(box[2], center[1])[0] - toItm(box[0], center[1])[0]);
          map.addSource("print-grid", { type: "geojson", data: itmGrid(box, gridStep(opts.scale, widthM)) });
          map.addLayer({ id: "print-grid", type: "line", source: "print-grid", paint: { "line-color": "#1f4e8c", "line-width": 0.8, "line-opacity": 0.55 } });
          map.addLayer({
            id: "print-grid-label",
            type: "symbol",
            source: "print-grid",
            layout: { "symbol-placement": "line", "text-field": ["get", "label"], "text-font": ["Noto Sans Regular"], "text-size": 10, "symbol-spacing": 260, "text-keep-upright": true },
            paint: { "text-color": "#1f4e8c", "text-halo-color": "#ffffff", "text-halo-width": 1.5 },
          });
        }
        const data: FeatureCollection = { type: "FeatureCollection", features: [{ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: route.geometry } }] };
        map.addSource("print-route", { type: "geojson", data });
        map.addLayer({ id: "print-route-casing", type: "line", source: "print-route", layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": "#ffffff", "line-width": 10 } });
        map.addLayer({ id: "print-route", type: "line", source: "print-route", layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": color, "line-width": 6 } });
        // start: ring in the route colour; destination: red disc (the pins' colours on screen)
        const ends: FeatureCollection = {
          type: "FeatureCollection",
          features: [route.geometry[0], route.geometry[route.geometry.length - 1]].map((c, i) => ({ type: "Feature", properties: { end: i }, geometry: { type: "Point", coordinates: c } })),
        };
        map.addSource("print-ends", { type: "geojson", data: ends });
        map.addLayer({
          id: "print-ends",
          type: "circle",
          source: "print-ends",
          paint: {
            "circle-radius": 8,
            "circle-color": ["case", ["==", ["get", "end"], 0], "#ffffff", cssVar("--danger", "#c93636")],
            "circle-stroke-color": ["case", ["==", ["get", "end"], 0], color, "#ffffff"],
            "circle-stroke-width": ["case", ["==", ["get", "end"], 0], 4, 3],
          },
        });
        map.once("idle", () => {
          clearTimeout(timer);
          resolve(map.getCanvas().toDataURL("image/png"));
        });
      });
      map.on("error", (e) => console.warn("print map", e?.error?.message ?? e));
    });
  } finally {
    map.remove();
    host.remove();
  }
}
