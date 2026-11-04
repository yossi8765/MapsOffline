/**
 * Printable picture of a route: an off-screen map (1600 × 1050 CSS px at pixel ratio 2), always
 * the light flavour and north up, fitted to the route, with the route drawn on top. Used by the
 * print sheet (hebrew-maps does the same for its printouts).
 */
import * as maplibregl from "maplibre-gl";
import type { FeatureCollection } from "geojson";
import type { Route } from "./api";
import { useStore } from "./store";
import { buildStyle } from "./mapStyle";

const W = 1600;
const H = 1050;

function cssVar(name: string, fallback: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

/** Renders `route` and resolves with a PNG data URL (rejects after 20 s or on a map error). */
export async function renderRouteImage(route: Route): Promise<string> {
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
  const map = new maplibregl.Map({
    container: host,
    style,
    bounds,
    fitBoundsOptions: { padding: 80 },
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
      const timer = setTimeout(() => reject(new Error("map image timed out")), 20_000);
      map.once("load", () => {
        const data: FeatureCollection = { type: "FeatureCollection", features: [{ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: route.geometry } }] };
        map.addSource("print-route", { type: "geojson", data });
        map.addLayer({ id: "print-route-casing", type: "line", source: "print-route", layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": "#ffffff", "line-width": 10 } });
        map.addLayer({ id: "print-route", type: "line", source: "print-route", layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": color, "line-width": 6 } });
        const ends: FeatureCollection = {
          type: "FeatureCollection",
          features: [route.geometry[0], route.geometry[route.geometry.length - 1]].map((c, i) => ({ type: "Feature", properties: { end: i }, geometry: { type: "Point", coordinates: c } })),
        };
        map.addSource("print-ends", { type: "geojson", data: ends });
        map.addLayer({ id: "print-ends", type: "circle", source: "print-ends", paint: { "circle-radius": 8, "circle-color": ["case", ["==", ["get", "end"], 0], color, cssVar("--danger", "#c93636")], "circle-stroke-color": "#ffffff", "circle-stroke-width": 3 } });
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
