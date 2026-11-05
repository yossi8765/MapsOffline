import { create } from "zustand";
import { api, errorText, type ConflictPolicy, type RegionPart } from "./api";
import { useStore } from "./store";

/** One country to download / import. `key` is the country id the backend reports progress under. */
export type QueueItem =
  | { kind: "geofabrik"; key: string; label: string; parts: RegionPart[]; policy: ConflictPolicy }
  | { kind: "osmand"; key: string; label: string; file: string; policy: ConflictPolicy }
  | { kind: "obfFile"; key: string; label: string; path: string; policy: ConflictPolicy };

interface QueueState {
  /** waiting, in order (the running one is `current`) */
  items: QueueItem[];
  current: QueueItem | null;
  /** add countries (already queued / running ones are ignored) and start the queue */
  enqueue: (items: QueueItem[]) => void;
  /** drop a waiting item */
  remove: (key: string) => void;
  /** drop every waiting item (the running one is cancelled with api.regionCancel) */
  clear: () => void;
}

/** Country id of a local OsmAnd file – the same rule as `obf_import::region_id` in Rust. */
export function obfRegionId(fileOrPath: string): string {
  let f = fileOrPath.split(/[\\/]/).pop() ?? fileOrPath;
  f = f.replace(/\.zip$/i, "").replace(/\.obf$/i, "").replace(/_2$/, "");
  const s = f.toLowerCase().replace(/[^a-z0-9_-]/g, "_");
  return `osmand/${s || "file"}`;
}

/** Downloads one country at a time: several big downloads side by side would only compete for
 *  the disk and the line. Lives outside the panels so closing the Data panel does not stop it. */
export const useRegionQueue = create<QueueState>((set, get) => {
  const run = async () => {
    if (get().current) return;
    const next = get().items[0];
    if (!next) return;
    set({ current: next, items: get().items.slice(1) });
    const st = useStore.getState();
    const t = st.t;
    try {
      if (next.kind === "geofabrik") {
        const r = await api.regionInstall(next.key, next.parts, next.policy);
        if (r.skipped) st.toast("info", `${t("regSkippedSameArea")}: ${next.label}`);
      } else {
        const r = await api.obfInstall(next.kind === "osmand" ? { file: next.file } : { path: next.path }, next.policy);
        if (r.skipped) st.toast("info", `${t("regSkippedSameArea")}: ${next.label}`);
      }
    } catch (e) {
      const msg = errorText(e);
      if (!/cancel/i.test(msg)) st.toast("error", `${next.label}: ${msg}`);
    } finally {
      set({ current: null });
      void useStore.getState().refreshTiles();
      void run();
    }
  };
  return {
    items: [],
    current: null,
    enqueue: (items) => {
      const busy = new Set([...get().items.map((i) => i.key), ...(get().current ? [get().current!.key] : [])]);
      const add = items.filter((i) => !busy.has(i.key));
      if (!add.length) return;
      set({ items: [...get().items, ...add] });
      void run();
    },
    remove: (key) => set({ items: get().items.filter((i) => i.key !== key) }),
    clear: () => {
      set({ items: [] });
      const cur = get().current;
      if (cur) api.regionCancel(cur.key).catch(() => {});
    },
  };
});
