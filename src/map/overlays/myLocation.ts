import type * as maplibregl from "maplibre-gl";
import type { LonLat, MyLocation } from "../../lib/api";
import { mapColors } from "../colors";
import { ensureSource, setFeatures } from "../geojson";
import { VEHICLE_KINDS, vehicleImage, vehiclePaint, type VehicleColor, type VehicleKind } from "../vehicles";

export const SRC_ME = "my-location";

/** Metres per pixel at zoom 0 at this latitude. */
function mpp0(lat: number): number {
  return 156543.03392 * Math.cos((lat * Math.PI) / 180);
}

/** Body colour of the vehicle markers (Settings → navigation); MapView keeps it current. */
let bodyColor: VehicleColor = "accent";
export function setVehicleColor(c: VehicleColor | undefined): void {
  bodyColor = c ?? "accent";
}

/** (Re)draw the vehicle markers in the chosen body colour (the accent by default). */
export function addVehicleImages(map: maplibregl.Map): void {
  const paint = vehiclePaint(bodyColor, mapColors(false).me);
  for (const k of VEHICLE_KINDS) {
    const name = `vehicle-${k}`;
    const img = vehicleImage(k, paint);
    if (map.hasImage(name)) map.updateImage(name, img.data);
    else map.addImage(name, img.data, { pixelRatio: img.pixelRatio });
  }
}

/** "My location": accuracy disc (metres, scaled with zoom) + blue dot; while navigating by car,
 *  the chosen vehicle marker (rotated with the route) instead of the dot. */
export function add(map: maplibregl.Map): void {
  ensureSource(map, SRC_ME);
  addVehicleImages(map);
  if (!map.getLayer("my-location-accuracy")) {
    map.addLayer({
      id: "my-location-accuracy",
      type: "circle",
      source: SRC_ME,
      filter: ["!", ["has", "vehicle"]],
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
      filter: ["!", ["has", "vehicle"]],
      paint: { "circle-radius": 7, "circle-color": mapColors(false).me, "circle-stroke-color": "#ffffff", "circle-stroke-width": 2.5 },
    });
  }
  if (!map.getLayer("my-location-vehicle")) {
    map.addLayer({
      id: "my-location-vehicle",
      type: "symbol",
      source: SRC_ME,
      filter: ["has", "vehicle"],
      layout: {
        "icon-image": ["concat", "vehicle-", ["get", "vehicle"]],
        "icon-rotate": ["get", "bearing"],
        "icon-rotation-alignment": "map",
        "icon-pitch-alignment": "map",
        "icon-allow-overlap": true,
        "icon-ignore-placement": true,
        "icon-size": 0.9,
      },
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

/** Live navigation: the position snapped to the route – a small fixed disc, or the vehicle
 *  marker heading along `bearing` when one is given (car navigation). */
export function showSnapped(map: maplibregl.Map, at: LonLat, vehicle?: VehicleKind | null, bearing = 0): void {
  const props: Record<string, unknown> = { r0: 5 / mpp0(at[1]) };
  if (vehicle) {
    props.vehicle = vehicle;
    props.bearing = bearing;
  }
  setFeatures(map, SRC_ME, [{ type: "Feature", properties: props, geometry: { type: "Point", coordinates: at } }]);
}
