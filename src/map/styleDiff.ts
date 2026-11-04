/**
 * Style updates without a full reload (report ד-1).
 *
 * The app's overlays (routes, stops, POI markers, user layers, measure …) are added to the map
 * after the base style loads; a plain `setStyle(next)` would drop them (and with `diff: false`
 * rebuild everything: glyphs, sprite, tiles, icons). `keepOverlays` is the `transformStyle` hook of
 * `map.setStyle(next, { diff: true, transformStyle: keepOverlays })`: it carries every overlay
 * source and layer of the current style into the new one, each at the same place relative to the
 * base layers, so MapLibre's diff only touches what really changed (a layer's visibility, a
 * source URL) and the overlays keep their data.
 */
import type { LayerSpecification, SourceSpecification, StyleSpecification } from "maplibre-gl";

/** Sources that belong to the base style (mapStyle.buildStyle); everything else is an overlay. */
export const BASE_SOURCES = new Set(["protomaps", "world", "satellite", "dem", "contours", "trails"]);

export function isBaseSource(id: string): boolean {
  return BASE_SOURCES.has(id) || id.startsWith("custom-");
}

function sourceOf(l: LayerSpecification): string | undefined {
  return "source" in l && typeof l.source === "string" ? l.source : undefined;
}

/** `next` with the overlays of `prev` (sources not built by buildStyle, and their layers). */
export function keepOverlays(prev: StyleSpecification | undefined, next: StyleSpecification): StyleSpecification {
  if (!prev) return next;
  const overlaySources = Object.keys(prev.sources).filter((id) => !isBaseSource(id) && !(id in next.sources));
  if (!overlaySources.length) return next;
  const isOverlay = (l: LayerSpecification) => {
    const s = sourceOf(l);
    return !!s && overlaySources.includes(s);
  };
  const sources: Record<string, SourceSpecification> = { ...next.sources };
  for (const id of overlaySources) sources[id] = prev.sources[id];
  const layers = [...next.layers];
  const nextIds = new Set(next.layers.map((l) => l.id));
  prev.layers.forEach((l, i) => {
    if (!isOverlay(l) || nextIds.has(l.id)) return;
    // anchor: the first base layer after it in the old style that the new style still has
    let anchor: string | undefined;
    for (let j = i + 1; j < prev.layers.length; j++) {
      const c = prev.layers[j];
      if (!isOverlay(c) && nextIds.has(c.id)) {
        anchor = c.id;
        break;
      }
    }
    const at = anchor ? layers.findIndex((x) => x.id === anchor) : -1;
    if (at < 0) layers.push(l);
    else layers.splice(at, 0, l);
  });
  return { ...next, sources, layers };
}

/** Layer-visibility helper for toggles that need no style change at all. */
export function setVisible(map: { getLayer(id: string): unknown; setLayoutProperty(id: string, k: string, v: unknown): unknown }, ids: string[], on: boolean): void {
  for (const id of ids) if (map.getLayer(id)) map.setLayoutProperty(id, "visibility", on ? "visible" : "none");
}
