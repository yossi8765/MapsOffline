/** Every app overlay on top of the base style; re-added after each style (re)load. */
import type * as maplibregl from "maplibre-gl";
import type { StyleSpecification } from "maplibre-gl";
import { firstSymbolLayer } from "../../lib/mapStyle";
import { loadPoiIcons } from "../icons";
import * as linePreview from "./linePreview";
import * as myLocation from "./myLocation";
import * as pois from "./pois";
import * as routes from "./routes";
import * as transitStops from "./transitStops";

export { linePreview, myLocation, pois, routes, transitStops };
export * as waypoints from "./waypoints";

/**
 * Add the overlays in their stacking order: stops, routes and the line preview under the base
 * map's labels, my location on top; the POI markers once their icons are drawn (above the base
 * labels, under the location dot), then `onPoisReady`.
 */
export function addAll(map: maplibregl.Map, dark: boolean, onPoisReady: () => void): void {
  const firstSymbol = firstSymbolLayer(map.getStyle() as StyleSpecification);
  transitStops.add(map, firstSymbol, dark);
  routes.add(map, firstSymbol, dark);
  linePreview.add(map, firstSymbol, dark);
  myLocation.add(map);
  void loadPoiIcons(map, dark).then(() => {
    if (!map.getStyle()) return;
    // above the base map's labels (under the location dot): the markers the user asked for
    // are placed first, so a house number or street name never hides one of them
    pois.add(map, "my-location-accuracy", dark);
    onPoisReady();
  });
}
