/** User layers (stream F2): their features, read once per layer and change tick, and files dropped
 *  on the window. Kept out of the lazily loaded UserLayersPanel so MapView can use them. */
import { useEffect } from "react";
import type { FeatureCollection } from "geojson";
import { apiX, readTextFile } from "../lib/api";
import { useStore } from "../lib/store";

const dataCache = new Map<string, Promise<FeatureCollection>>();

/** The features of a user layer (cached per id and `tick`, i.e. until the layers change). */
export function userLayerData(id: string, tick: number): Promise<FeatureCollection> {
  const key = `${id}@${tick}`;
  let p = dataCache.get(key);
  if (!p) {
    p = apiX.userLayerGet(id).then((s) => JSON.parse(s) as FeatureCollection);
    dataCache.set(key, p);
    p.catch(() => dataCache.delete(key));
    // older ticks of the same layer are not needed any more
    for (const k of dataCache.keys()) if (k.startsWith(`${id}@`) && k !== key) dataCache.delete(k);
  }
  return p;
}

export const IMPORT_EXT = /\.(gpx|kml|geojson|json)$/i;

/**
 * Files dropped anywhere on the window (Tauri's drag & drop gives paths; the backend reads them
 * through `read_text_file`). Without that command the import dialog opens (file picker).
 * Mounted once by MapView.
 */
export function useWindowFileDrop(): void {
  useEffect(() => {
    let un: (() => void) | undefined;
    let dead = false;
    void (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const u = await getCurrentWebview().onDragDropEvent(async (ev) => {
          if (ev.payload.type !== "drop") return;
          const paths = ev.payload.paths.filter((p) => IMPORT_EXT.test(p));
          if (!paths.length) return;
          const st = useStore.getState();
          try {
            const files = await Promise.all(paths.map(async (p) => ({ name: p.split(/[\\/]/).pop() ?? p, text: await readTextFile(p) })));
            await st.importFiles(files);
          } catch (e) {
            console.warn("drop import", e);
            st.openImport();
          }
        });
        if (dead) u();
        else un = u;
      } catch {
        /* not in Tauri (tests / plain browser) */
      }
    })();
    return () => {
      dead = true;
      un?.();
    };
  }, []);
}
