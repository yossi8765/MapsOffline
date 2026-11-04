import type * as maplibregl from "maplibre-gl";
import type { Feature } from "geojson";
import type { StopInfo } from "../../lib/api";
import { mapColors } from "../colors";
import { beforeIfPresent, ensureSource, setFeatures } from "../geojson";
import { BUS_ICON, BUS_ICON_PX, drawBusIcon } from "../icons";

export const SRC_TSTOPS = "transit-stops";
export const TSTOPS_MIN_ZOOM = 13;
export const TSTOPS_LIMIT = 3000;

export function add(map: maplibregl.Map, beforeId: string | undefined, dark: boolean): void {
  ensureSource(map, SRC_TSTOPS);
  const before = beforeIfPresent(map, beforeId);
  const text = mapColors(dark).stopLabel;
  const halo = mapColors(dark).halo;
  if (!map.hasImage(BUS_ICON)) {
    try {
      const img = drawBusIcon(dark);
      map.addImage(BUS_ICON, img, { pixelRatio: img.width / BUS_ICON_PX });
    } catch (e) {
      console.warn("bus icon", e);
    }
  }
  if (!map.getLayer("transit-stops")) {
    map.addLayer(
      {
        id: "transit-stops",
        type: "symbol",
        source: SRC_TSTOPS,
        minzoom: TSTOPS_MIN_ZOOM,
        layout: {
          "icon-image": BUS_ICON,
          "icon-size": ["interpolate", ["linear"], ["zoom"], 13, 0.42, 16, 0.62, 19, 0.8],
          "icon-allow-overlap": true,
          "icon-padding": 1,
        },
        paint: {
          "icon-opacity": ["interpolate", ["linear"], ["zoom"], 13, 0.85, 15, 1],
        },
      },
      before,
    );
  }
  if (!map.getLayer("transit-stops-label")) {
    map.addLayer(
      {
        id: "transit-stops-label",
        type: "symbol",
        source: SRC_TSTOPS,
        minzoom: 15,
        layout: {
          "text-field": ["get", "name"],
          "text-font": ["Noto Sans Regular"],
          "text-size": 11,
          "text-offset": ["interpolate", ["linear"], ["zoom"], 15, ["literal", [0, 1.0]], 19, ["literal", [0, 1.35]]],
          "text-anchor": "top",
          "text-optional": true,
          "text-max-width": 8,
        },
        paint: { "text-color": text, "text-halo-color": halo, "text-halo-width": 1.3 },
      },
      before,
    );
  }
}

export function stopFeatures(stops: StopInfo[]): Feature[] {
  return stops.map((s) => ({
    type: "Feature",
    properties: { id: s.id, name: s.name, code: s.code ?? "" },
    geometry: { type: "Point", coordinates: [s.lon, s.lat] },
  }));
}

export function sync(map: maplibregl.Map, stops: StopInfo[]): void {
  setFeatures(map, SRC_TSTOPS, stopFeatures(stops));
}
