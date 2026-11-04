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

/** The full search results of the markers now on the map, by feature `rid` (report ד-3: the
 *  features carry only the id, not the whole result serialised as JSON for the map worker). */
const results = new Map<string, GeoResult>();

/** A POI marker's GeoResult, its category as the picker's label. */
export function resultOf(rid: unknown, label: string): GeoResult | undefined {
  const r = typeof rid === "string" ? results.get(rid) : undefined;
  return r ? { ...r, category: r.categoryLabel || label || r.category } : undefined;
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
  results.clear();
  return pois.flatMap((p) => {
    const c = categoryOf(p.category ?? "", p.name, selectedCats);
    if (!c) return [];
    const rid = p.id || `${p.lon},${p.lat}`;
    results.set(rid, p);
    return [{ type: "Feature" as const, properties: { name: p.name, cat: c.id, label: t(c.label), color: c.color, rank: -(p.score ?? 0), rid }, geometry: { type: "Point" as const, coordinates: [p.lon, p.lat] } }];
  });
}

export function clear(map: maplibregl.Map): void {
  setFeatures(map, SRC_POIS, []);
}
