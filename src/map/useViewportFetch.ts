import { useEffect, useRef, type RefObject } from "react";
import type * as maplibregl from "maplibre-gl";
import type { Feature } from "geojson";
import { setFeatures } from "./geojson";
import { containsBbox, padBbox, type Bbox } from "./bbox";

export type { Bbox } from "./bbox";

export interface ViewportFetch {
  /** GeoJSON source the features go to */
  sourceId: string;
  /** off: the source is emptied and nothing is fetched */
  enabled: boolean;
  /** below this zoom the source is emptied instead of fetching */
  minZoom: number;
  debounceMs?: number;
  /** Features for the view, or "stop" to empty the source and stop fetching until the inputs
   *  change (e.g. the data is not installed). Errors are logged with `label`. */
  load: (bbox: Bbox, zoom: number) => Promise<Feature[] | "stop">;
  label: string;
  /** Share of the view added on every side of the fetched box (default 0.25: a box 1.5× the view),
   *  so small pans stay inside it and need no new query. */
  pad?: number;
}

/**
 * Keep a GeoJSON source filled with what is in view: fetched (debounced) for a padded box around
 * the view, refetched only when the view leaves that box or the whole zoom level changes (report
 * ד-3); stale answers dropped. Restarts when `ready`, `enabled` or any of `deps` change.
 */
export function useViewportFetch(mapRef: RefObject<maplibregl.Map | null>, ready: boolean, opts: ViewportFetch, deps: unknown[]): void {
  const optsRef = useRef(opts);
  optsRef.current = opts;
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !ready) return;
    const { sourceId } = optsRef.current;
    if (!optsRef.current.enabled) {
      setFeatures(map, sourceId, []);
      return;
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    let seq = 0;
    let stopped = false;
    /** box and whole zoom of the features now in the source */
    let have: { box: Bbox; zoom: number } | null = null;
    const run = async () => {
      if (stopped) return;
      const o = optsRef.current;
      if (map.getZoom() < o.minZoom) {
        have = null;
        setFeatures(map, sourceId, []);
        return;
      }
      const b = map.getBounds();
      const view: Bbox = [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
      const zoom = Math.floor(map.getZoom());
      if (have && have.zoom === zoom && containsBbox(have.box, view)) return;
      const box = padBbox(view, o.pad ?? 0.25);
      const id = ++seq;
      try {
        const feats = await o.load(box, map.getZoom());
        if (id !== seq) return;
        if (feats === "stop") {
          stopped = true;
          have = null;
          setFeatures(map, sourceId, []);
          return;
        }
        setFeatures(map, sourceId, feats);
        have = { box, zoom };
      } catch (e) {
        console.warn(`${o.label} failed`, e);
      }
    };
    const schedule = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => void run(), optsRef.current.debounceMs ?? 250);
    };
    map.on("moveend", schedule);
    schedule();
    return () => {
      map.off("moveend", schedule);
      if (timer) clearTimeout(timer);
      seq++;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, opts.enabled, ...deps]);
}
