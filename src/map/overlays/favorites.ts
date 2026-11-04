import type * as maplibregl from "maplibre-gl";
import type { Favorite } from "../../lib/api";
import { cssVar } from "../colors";
import { ensureSource, setFeatures } from "../geojson";

export const SRC_FAVS = "favorite-groups";

/** Favorites of the groups shown on the map: a coloured dot with a white ring and the name. */
export function add(map: maplibregl.Map, beforeId: string | undefined): void {
  ensureSource(map, SRC_FAVS);
  if (!map.getLayer("favorite-groups-dot")) {
    map.addLayer(
      {
        id: "favorite-groups-dot",
        type: "circle",
        source: SRC_FAVS,
        paint: { "circle-radius": 6, "circle-color": ["get", "color"], "circle-stroke-color": "#ffffff", "circle-stroke-width": 2 },
      },
      beforeId,
    );
  }
  if (!map.getLayer("favorite-groups-label")) {
    map.addLayer({
      id: "favorite-groups-label",
      type: "symbol",
      source: SRC_FAVS,
      minzoom: 11,
      layout: {
        "text-field": ["get", "name"],
        "text-font": ["Noto Sans Medium"],
        "text-size": 11,
        "text-offset": [0, 1.1],
        "text-anchor": "top",
        "text-optional": true,
      },
      paint: { "text-color": ["get", "color"], "text-halo-color": "#ffffff", "text-halo-width": 1.5 },
    });
  }
}

/** Favorites with a place on the map (lines have none) that belong to one of `groups`
 *  ("" = favorites without a group). */
export function visibleFavorites(favs: Favorite[], groups: string[]): Favorite[] {
  if (!groups.length) return [];
  const set = new Set(groups);
  return favs.filter((f) => f.kind !== "line" && set.has(f.group ?? "") && Number.isFinite(f.lon) && Number.isFinite(f.lat));
}

export function sync(map: maplibregl.Map, favs: Favorite[], groups: string[]): void {
  if (!map.getSource(SRC_FAVS)) return;
  const fallback = cssVar("--accent", "#1a66d9");
  setFeatures(
    map,
    SRC_FAVS,
    visibleFavorites(favs, groups).map((f) => ({
      type: "Feature",
      properties: { id: f.id, name: f.name, color: f.color || fallback },
      geometry: { type: "Point", coordinates: [f.lon, f.lat] },
    })),
  );
}
