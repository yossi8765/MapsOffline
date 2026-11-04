import type * as maplibregl from "maplibre-gl";
import type { Feature } from "geojson";
import type { GeoResult } from "../../lib/api";
import type { I18nKey } from "../../lib/i18n";
import { categoryOf, type PoiCategory } from "../../lib/poiCategories";
import { mapColors } from "../colors";
import { beforeIfPresent, ensureSource, setFeatures } from "../geojson";

export const SRC_POIS = "poi-layer";
export const POIS_MIN_ZOOM = 11;
export const POIS_LIMIT = 1500;

/** A POI marker's GeoResult (kept as JSON in the feature), its category as the picker's label. */
export function parseResult(json: unknown, label: string): GeoResult | undefined {
  if (typeof json !== "string") return undefined;
  try {
    const r = JSON.parse(json) as GeoResult;
    return { ...r, category: r.categoryLabel || label || r.category };
  } catch {
    return undefined;
  }
}

export function add(map: maplibregl.Map, beforeId: string | undefined, dark: boolean): void {
  ensureSource(map, SRC_POIS);
  const before = beforeIfPresent(map, beforeId);
  if (!map.getLayer("poi-layer")) {
    map.addLayer(
      {
        id: "poi-layer",
        type: "symbol",
        source: SRC_POIS,
        minzoom: POIS_MIN_ZOOM,
        // Fewer, named markers (hebrew-maps / Google): from zoom 13 a marker is drawn only with
        // its name next to it (text-optional false), the name in the category's colour, placed on
        // whichever side has room. Server-side thinning (minzoom per POI) keeps the density even.
        layout: {
          "icon-image": ["concat", "poi-", ["get", "cat"]],
          "icon-size": ["interpolate", ["linear"], ["zoom"], 11, 0.55, 15, 0.75, 18, 0.9],
          "icon-allow-overlap": false,
          "icon-padding": 2,
          "text-field": ["step", ["zoom"], "", 13, ["get", "name"]],
          "text-font": ["Noto Sans Regular"],
          "text-size": 11,
          "text-variable-anchor": ["top", "right", "left", "bottom"],
          "text-radial-offset": 1.15,
          "text-justify": "auto",
          "text-optional": false,
          "text-max-width": 8,
          "symbol-sort-key": ["get", "rank"],
        },
        paint: { "text-color": ["get", "color"], "text-halo-color": mapColors(dark).halo, "text-halo-width": 1.4 },
      },
      before,
    );
  }
}

/** Marker features of the POIs that belong to one of the selected picker categories. */
export function poiFeatures(pois: GeoResult[], selectedCats: PoiCategory[], t: (k: I18nKey) => string): Feature[] {
  return pois.flatMap((p) => {
    const c = categoryOf(p.category ?? "", p.name, selectedCats);
    if (!c) return [];
    return [{ type: "Feature" as const, properties: { name: p.name, cat: c.id, label: t(c.label), color: c.color, rank: -(p.score ?? 0), json: JSON.stringify(p) }, geometry: { type: "Point" as const, coordinates: [p.lon, p.lat] } }];
  });
}

export function clear(map: maplibregl.Map): void {
  setFeatures(map, SRC_POIS, []);
}
