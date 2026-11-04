import type { GeoJSONSource } from "maplibre-gl";
import type * as maplibregl from "maplibre-gl";
import type { Feature, FeatureCollection } from "geojson";

export const EMPTY: FeatureCollection = { type: "FeatureCollection", features: [] };

/** Add an empty GeoJSON source unless it exists. */
export function ensureSource(map: maplibregl.Map, id: string): void {
  if (!map.getSource(id)) map.addSource(id, { type: "geojson", data: { type: "FeatureCollection", features: [] } as FeatureCollection });
}

/** Replace a GeoJSON source's features (no-op when the source is not there yet). */
export function setFeatures(map: maplibregl.Map, id: string, features: Feature[]): void {
  (map.getSource(id) as GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features });
}

/** `beforeId` when that layer exists (the base style may lack it), else undefined = on top. */
export function beforeIfPresent(map: maplibregl.Map, beforeId: string | undefined): string | undefined {
  return beforeId && map.getLayer(beforeId) ? beforeId : undefined;
}
