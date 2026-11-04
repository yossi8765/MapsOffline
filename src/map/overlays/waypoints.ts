import * as maplibregl from "maplibre-gl";
import type { RoutePoint } from "../../lib/store";

/**
 * Draggable pins of the trip's points (A, numbered stops, B). Replaces `old` and returns the new
 * markers; `onDrag(i, lon, lat)` reports a pin dropped somewhere else.
 */
export function sync(map: maplibregl.Map, old: maplibregl.Marker[], waypoints: (RoutePoint | null)[], onDrag: (index: number, lon: number, lat: number) => void): maplibregl.Marker[] {
  old.forEach((m) => m.remove());
  const out: maplibregl.Marker[] = [];
  const n = waypoints.length;
  waypoints.forEach((wp, i) => {
    if (!wp) return;
    const el = document.createElement("div");
    const isEnd = i === n - 1;
    const isVia = i > 0 && i < n - 1;
    el.className = `wp-marker ${isEnd ? "wp-end" : isVia ? "wp-via" : ""}`;
    const span = document.createElement("span");
    span.textContent = i === 0 ? "A" : isEnd ? "B" : String(i);
    el.appendChild(span);
    const m = new maplibregl.Marker({ element: el, draggable: true, anchor: "bottom", offset: [0, 4] }).setLngLat([wp.lon, wp.lat]).addTo(map);
    m.on("dragend", () => {
      const ll = m.getLngLat();
      onDrag(i, ll.lng, ll.lat);
    });
    out.push(m);
  });
  return out;
}
