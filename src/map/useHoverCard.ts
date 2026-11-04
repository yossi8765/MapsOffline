import { useCallback, useRef, useState } from "react";
import type * as maplibregl from "maplibre-gl";
import type { Point } from "geojson";
import { placeHoverCard, type HoverTarget } from "../components/MapHoverCard";
import { resultOf } from "./overlays/pois";
import { useStore } from "../lib/store";

/** A map tool takes the clicks (measure, draw). */
export function toolActive(): boolean {
  const s = useStore.getState();
  return !!(s.measure || s.drawing);
}

/** Layers whose features open the hover card. */
export const HOVER_LAYERS = ["poi-layer", "transit-stops"];

const SHOW_MS = 200;
const HIDE_MS = 260;

/**
 * Hover card (desktop) over POI markers and transit stops: 200 ms before it opens, 260 ms of
 * grace to move onto it; a click pins it until the map is clicked elsewhere. A pinned card
 * follows its marker; an unpinned one closes when the map moves.
 */
export function useHoverCard() {
  const [hover, setHover] = useState<HoverTarget | null>(null);
  const hoverRef = useRef<HoverTarget | null>(null);
  hoverRef.current = hover;
  const timers = useRef<{ show?: ReturnType<typeof setTimeout>; hide?: ReturnType<typeof setTimeout> }>({});
  const cardRef = useRef<HTMLDivElement | null>(null);

  /** Wire the map events (call once, when the map is created). `beforePin` runs before a click
   *  pins a card (closes other bubbles). */
  const bind = useCallback((map: maplibregl.Map, beforePin: () => void) => {
    const targetOf = (f: maplibregl.MapGeoJSONFeature, pinned: boolean): HoverTarget => {
      const [lon, lat] = (f.geometry as Point).coordinates as [number, number];
      const pt = map.project([lon, lat]);
      const stop = f.layer.id === "transit-stops";
      return {
        kind: stop ? "stop" : "poi",
        name: String(f.properties?.name ?? ""),
        sub: stop ? undefined : String(f.properties?.label ?? ""),
        code: stop && f.properties?.code ? String(f.properties.code) : undefined,
        color: stop ? undefined : (f.properties?.color as string | undefined),
        // the full search result of a POI marker: the place card needs its category (synagogue →
        // prayer times), opening hours and kosher flag
        result: stop ? undefined : resultOf(f.properties?.rid, String(f.properties?.label ?? "")),
        lonlat: [lon, lat],
        x: pt.x,
        y: pt.y,
        pinned,
      };
    };
    const sameTarget = (a: HoverTarget | null, b: HoverTarget) => !!a && a.lonlat[0] === b.lonlat[0] && a.lonlat[1] === b.lonlat[1];
    let pending: HoverTarget | null = null;
    for (const layer of HOVER_LAYERS) {
      map.on("mousemove", layer, (e) => {
        map.getCanvas().style.cursor = "pointer";
        const f = e.features?.[0];
        if (!f || hoverRef.current?.pinned) return;
        const tgt = targetOf(f, false);
        clearTimeout(timers.current.hide);
        if (sameTarget(hoverRef.current, tgt) || sameTarget(pending, tgt)) return;
        pending = tgt;
        clearTimeout(timers.current.show);
        timers.current.show = setTimeout(() => {
          pending = null;
          setHover(tgt);
        }, SHOW_MS);
      });
      map.on("mouseleave", layer, () => {
        map.getCanvas().style.cursor = "";
        pending = null;
        clearTimeout(timers.current.show);
        if (hoverRef.current?.pinned) return;
        timers.current.hide = setTimeout(() => setHover(null), HIDE_MS);
      });
      map.on("click", layer, (e) => {
        const f = e.features?.[0];
        // measuring / drawing: a click is a vertex, not "open this marker"
        if (!f || toolActive()) return;
        clearTimeout(timers.current.show);
        clearTimeout(timers.current.hide);
        beforePin();
        setHover(targetOf(f, true));
      });
    }
    map.on("click", (e) => {
      const present = HOVER_LAYERS.filter((l) => map.getLayer(l));
      if (!present.length || !map.queryRenderedFeatures(e.point, { layers: present }).length) setHover(null);
    });
    map.on("move", () => {
      const h = hoverRef.current;
      if (!h) return;
      if (!h.pinned) {
        setHover(null);
        return;
      }
      // a pinned card follows its marker: moved straight in the DOM, no React render per frame
      const pt = map.project(h.lonlat);
      h.x = pt.x;
      h.y = pt.y;
      if (cardRef.current) placeHoverCard(cardRef.current, pt.x, pt.y);
    });
  }, []);

  /** Props for <MapHoverCard>: keep it open while the pointer is on it. */
  const cardHandlers = {
    onEnter: () => clearTimeout(timers.current.hide),
    onLeave: () => {
      if (!hoverRef.current?.pinned) timers.current.hide = setTimeout(() => setHover(null), HIDE_MS);
    },
    onClose: () => setHover(null),
    elRef: cardRef,
  };

  return { hover, setHover, bind, cardHandlers };
}
