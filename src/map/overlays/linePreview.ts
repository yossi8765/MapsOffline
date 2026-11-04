import type * as maplibregl from "maplibre-gl";
import type { Feature } from "geojson";
import type { LineShape, LonLat } from "../../lib/api";
import { hexColor, mapColors } from "../colors";
import { beforeIfPresent, ensureSource, setFeatures } from "../geojson";

export const SRC_LINE = "line-preview";

/** A favorite transit line (favorites panel): its shape and stops, under the labels. */
export function add(map: maplibregl.Map, beforeId: string | undefined, dark: boolean): void {
  ensureSource(map, SRC_LINE);
  const before = beforeIfPresent(map, beforeId);
  const casing = mapColors(dark).casing;
  if (!map.getLayer("line-preview-casing")) {
    map.addLayer(
      { id: "line-preview-casing", type: "line", source: SRC_LINE, filter: ["==", ["geometry-type"], "LineString"], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": casing, "line-width": 9, "line-opacity": 0.9 } },
      before,
    );
  }
  if (!map.getLayer("line-preview")) {
    map.addLayer(
      { id: "line-preview", type: "line", source: SRC_LINE, filter: ["==", ["geometry-type"], "LineString"], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": ["get", "color"], "line-width": 5 } },
      before,
    );
  }
  if (!map.getLayer("line-preview-stops")) {
    map.addLayer(
      {
        id: "line-preview-stops",
        type: "circle",
        source: SRC_LINE,
        filter: ["==", ["geometry-type"], "Point"],
        minzoom: 11,
        paint: { "circle-radius": ["interpolate", ["linear"], ["zoom"], 11, 2.5, 15, 5], "circle-color": casing, "circle-stroke-color": ["get", "color"], "circle-stroke-width": 2 },
      },
      before,
    );
  }
}

/** Draw the line (or clear it) and fit the view to it. */
export function sync(map: maplibregl.Map, shape: LineShape | null): void {
  if (!map.getSource(SRC_LINE)) return;
  if (!shape) {
    setFeatures(map, SRC_LINE, []);
    return;
  }
  const color = shape.color ? hexColor(shape.color) : mapColors(false).bus;
  const features: Feature[] = [
    { type: "Feature", properties: { color }, geometry: { type: "LineString", coordinates: shape.geometry } },
    ...shape.stops.map((s): Feature => ({ type: "Feature", properties: { color, name: s.name }, geometry: { type: "Point", coordinates: [s.lon, s.lat] } })),
  ];
  setFeatures(map, SRC_LINE, features);
  const pts = shape.geometry.length ? shape.geometry : shape.stops.map((s) => [s.lon, s.lat] as LonLat);
  if (pts.length) {
    const lons = pts.map((p) => p[0]);
    const lats = pts.map((p) => p[1]);
    map.fitBounds([[Math.min(...lons), Math.min(...lats)], [Math.max(...lons), Math.max(...lats)]], { padding: 70, duration: 700, maxZoom: 15 });
  }
}
