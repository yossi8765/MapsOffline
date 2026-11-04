import * as maplibregl from "maplibre-gl";
import type { RoutePoint } from "../../lib/store";

/** Glyph of a trip pin, the same in every language (no Latin A / B in the Hebrew UI, report א-12):
 *  origin = ring, destination = flag, stops = their number. Also used by the print sheet. */
export type PinKind = "start" | "end" | "via";

const FLAG_SVG =
  '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 21V4"/><path d="M5 4h11l-2 4 2 4H5"/></svg>';
const RING_SVG = '<svg viewBox="0 0 24 24" width="12" height="12" aria-hidden="true"><circle cx="12" cy="12" r="7" fill="none" stroke="currentColor" stroke-width="4"/></svg>';

export function pinKind(index: number, count: number): PinKind {
  return index === 0 ? "start" : index === count - 1 ? "end" : "via";
}

/** Inner HTML of a pin (trusted, static markup). */
export function pinGlyph(kind: PinKind, index: number): string {
  return kind === "start" ? RING_SVG : kind === "end" ? FLAG_SVG : String(index);
}

function fillPin(el: HTMLElement, kind: PinKind, index: number): void {
  // classList, not className: MapLibre keeps its own classes on the marker element
  el.classList.remove("wp-start", "wp-end", "wp-via");
  el.classList.add("wp-marker", `wp-${kind}`);
  el.dataset.glyph = `${kind}:${index}`;
  const span = el.firstElementChild as HTMLElement | null;
  if (span) span.innerHTML = pinGlyph(kind, index);
}

/**
 * Draggable pins of the trip's points. Existing markers are moved and relabelled in place
 * (report ד-8: no DOM rebuild on every drag); only a changed count adds or removes markers.
 * `onDrag(i, lon, lat)` reports a pin dropped somewhere else.
 */
export function sync(map: maplibregl.Map, old: (maplibregl.Marker | null)[], waypoints: (RoutePoint | null)[], onDrag: (index: number, lon: number, lat: number) => void): (maplibregl.Marker | null)[] {
  const n = waypoints.length;
  const out: (maplibregl.Marker | null)[] = [];
  for (let i = 0; i < Math.max(n, old.length); i++) {
    const wp = i < n ? waypoints[i] : null;
    let m = old[i] ?? null;
    if (!wp) {
      m?.remove();
      if (i < n) out.push(null);
      continue;
    }
    const kind = pinKind(i, n);
    if (!m) {
      const el = document.createElement("div");
      el.appendChild(document.createElement("span"));
      m = new maplibregl.Marker({ element: el, draggable: true, anchor: "bottom", offset: [0, 4] }).setLngLat([wp.lon, wp.lat]).addTo(map);
      const marker = m;
      // the index is read at drop time: the same marker may now stand for another point
      m.on("dragend", () => {
        const ll = marker.getLngLat();
        onDrag(Number(marker.getElement().dataset.index), ll.lng, ll.lat);
      });
    } else {
      const cur = m.getLngLat();
      if (cur.lng !== wp.lon || cur.lat !== wp.lat) m.setLngLat([wp.lon, wp.lat]);
    }
    const el = m.getElement();
    el.dataset.index = String(i);
    if (el.dataset.glyph !== `${kind}:${i}`) fillPin(el, kind, i);
    out.push(m);
  }
  return out;
}
