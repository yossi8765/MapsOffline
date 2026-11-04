import type * as maplibregl from "maplibre-gl";
import type { LonLat, MyLocation } from "../../lib/api";
import { mapColors } from "../colors";
import { ensureSource, setFeatures } from "../geojson";

export const SRC_ME = "my-location";

/** Metres per pixel at zoom 0 at this latitude. */
function mpp0(lat: number): number {
  return 156543.03392 * Math.cos((lat * Math.PI) / 180);
}

/** "My location": accuracy disc (metres, scaled with zoom) + blue dot. */
export function add(map: maplibregl.Map): void {
  ensureSource(map, SRC_ME);
  if (!map.getLayer("my-location-accuracy")) {
    map.addLayer({
      id: "my-location-accuracy",
      type: "circle",
      source: SRC_ME,
      paint: {
        // radius in px = accuracy_m / metres-per-pixel(zoom); r0 is the value at zoom 0
        "circle-radius": ["interpolate", ["exponential", 2], ["zoom"], 0, ["get", "r0"], 22, ["*", ["get", "r0"], 4194304]],
        "circle-color": mapColors(false).me,
        "circle-opacity": 0.12,
        "circle-stroke-color": mapColors(false).me,
        "circle-stroke-opacity": 0.3,
        "circle-stroke-width": 1,
      },
    });
  }
  if (!map.getLayer("my-location-dot")) {
    map.addLayer({
      id: "my-location-dot",
      type: "circle",
      source: SRC_ME,
      paint: { "circle-radius": 7, "circle-color": mapColors(false).me, "circle-stroke-color": "#ffffff", "circle-stroke-width": 2.5 },
    });
  }
}

/** The last fix with its accuracy disc, or nothing. */
export function sync(map: maplibregl.Map, loc: MyLocation | null): void {
  if (!map.getSource(SRC_ME)) return;
  if (!loc) {
    setFeatures(map, SRC_ME, []);
    return;
  }
  setFeatures(map, SRC_ME, [{ type: "Feature", properties: { r0: Math.max(loc.accuracyM, 5) / mpp0(loc.lat) }, geometry: { type: "Point", coordinates: [loc.lon, loc.lat] } }]);
}

/** Live navigation: the position snapped to the route, with a small fixed disc. */
export function showSnapped(map: maplibregl.Map, at: LonLat): void {
  setFeatures(map, SRC_ME, [{ type: "Feature", properties: { r0: 5 / mpp0(at[1]) }, geometry: { type: "Point", coordinates: at } }]);
}
