import type * as maplibregl from "maplibre-gl";
import type { Feature } from "geojson";
import type { KohanimArea } from "../../lib/api";
import { beforeIfPresent, ensureSource, setFeatures } from "../geojson";

export const SRC_KOHANIM = "kohanim-areas";

/** Colours of the association's map: level א pink, ב brown, ג orange. */
const LEVEL_COLOR: maplibregl.ExpressionSpecification = ["match", ["get", "level"], 1, "#FF1493", 2, "#8B4513", 3, "#FF8C00", "#FF1493"];

/** Route for kohanim: the areas the route avoids, under the routes and labels. */
export function add(map: maplibregl.Map, beforeId: string | undefined): void {
  ensureSource(map, SRC_KOHANIM);
  const before = beforeIfPresent(map, beforeId);
  if (!map.getLayer("kohanim-fill")) {
    map.addLayer({ id: "kohanim-fill", type: "fill", source: SRC_KOHANIM, minzoom: 11, paint: { "fill-color": LEVEL_COLOR, "fill-opacity": 0.3 } }, before);
  }
  if (!map.getLayer("kohanim-line")) {
    map.addLayer({ id: "kohanim-line", type: "line", source: SRC_KOHANIM, minzoom: 11, paint: { "line-color": LEVEL_COLOR, "line-width": 2, "line-opacity": 0.8 } }, before);
  }
}

/** Show the areas of `level` and the milder levels (null / 0 = none). */
export function sync(map: maplibregl.Map, areas: KohanimArea[] | null, level: number): void {
  if (!map.getSource(SRC_KOHANIM)) return;
  const features: Feature[] =
    areas && level > 0
      ? areas
          .filter((a) => a.level <= level)
          .map((a) => ({
            type: "Feature",
            properties: { level: a.level, name: a.name, city: a.city, street: a.street, description: a.description },
            geometry: { type: "Polygon", coordinates: [a.ring] },
          }))
      : [];
  setFeatures(map, SRC_KOHANIM, features);
}
