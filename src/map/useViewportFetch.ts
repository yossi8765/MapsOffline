import { useEffect, useRef, type RefObject } from "react";
import type * as maplibregl from "maplibre-gl";
import type { Feature } from "geojson";
import { setFeatures } from "./geojson";

export type Bbox = [number, number, number, number];

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
}

/**
 * Keep a GeoJSON source filled with what is in view: refetched (debounced) after every move,
 * stale answers dropped. Restarts when `ready`, `enabled` or any of `deps` change.
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
    const run = async () => {
      if (stopped) return;
      const o = optsRef.current;
      if (map.getZoom() < o.minZoom) {
        setFeatures(map, sourceId, []);
        return;
      }
      const b = map.getBounds();
      const id = ++seq;
      try {
        const feats = await o.load([b.getWest(), b.getSouth(), b.getEast(), b.getNorth()], map.getZoom());
        if (id !== seq) return;
        if (feats === "stop") {
          stopped = true;
          setFeatures(map, sourceId, []);
          return;
        }
        setFeatures(map, sourceId, feats);
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
