import { api, type DataCheck, type DataComponent, type DataProgress, type DataStatus } from "../api";
import type { SliceCreator } from "./types";

export interface DataSlice {
  dataStatus: DataStatus | null;
  dataCheck: DataCheck | null;
  checkingUpdates: boolean;
  progress: Partial<Record<DataComponent, DataProgress>>;

  refreshDataStatus: () => Promise<void>;
  /** Ask the servers for newer data; `force` re-checks even if checked recently. */
  checkUpdates: (force?: boolean) => Promise<void>;
  setProgress: (p: DataProgress) => void;
  startUpdate: (components: DataComponent[]) => Promise<void>;
  /** `components` missing = all installed, [] = none; plus other countries / sharp areas by id */
  exportData: (dest: string, components?: DataComponent[], regions?: string[], areas?: string[]) => Promise<void>;
  importData: (src: string) => Promise<void>;
  /** Download the full ready-made data package to `dest`; resolves to true when it finished. */
  downloadPackage: (dest: string) => Promise<boolean>;
  cancelUpdate: (component?: DataComponent) => Promise<void>;
  pauseUpdate: (paused: boolean) => Promise<void>;
}

let statusErrorShown = false;

export const createDataSlice: SliceCreator<DataSlice> = (set, get) => ({
  dataStatus: null,
  dataCheck: null,
  checkingUpdates: false,
  progress: {},

  refreshDataStatus: async () => {
    try {
      const dataStatus = await api.dataStatus();
      set({ dataStatus });
    } catch (e) {
      // once per session: the panel would otherwise look empty without a word
      if (!statusErrorShown) {
        statusErrorShown = true;
        get().toastError(e);
      }
    }
  },
  checkUpdates: async (force = false) => {
    const s = get();
    if (s.checkingUpdates) return;
    if (!force && s.dataCheck && Date.now() - Date.parse(s.dataCheck.checkedIso) < 10 * 60_000) return;
    set({ checkingUpdates: true });
    try {
      const dataCheck = await api.dataCheck();
      set({ dataCheck });
    } catch (e) {
      console.warn("data_check failed", e);
    } finally {
      set({ checkingUpdates: false });
    }
  },
  setProgress: (p) => {
    set((s) => ({ progress: { ...s.progress, [p.component]: p } }));
    if (p.phase === "done" || p.phase === "failed" || p.phase === "cancelled") void get().refreshDataStatus();
  },
  startUpdate: async (components) => {
    const t = get().t;
    const queued: Partial<Record<DataComponent, DataProgress>> = {};
    // components already running keep their progress (the backend ignores them)
    const busy = new Set([...(get().dataStatus?.active ?? []), ...(get().dataStatus?.queued ?? [])]);
    for (const c of components) if (!busy.has(c)) queued[c] = { component: c, phase: "queued", done: 0, total: 0, message: t("phaseQueued") };
    set((s) => ({ progress: { ...s.progress, ...queued }, dataStatus: s.dataStatus && { ...s.dataStatus, updating: true } }));
    // the backend decides what starts now and what waits (shared sources): show it at once
    window.setTimeout(() => void get().refreshDataStatus(), 300);
    try {
      await api.dataUpdate(components);
      get().toast("success", t("phaseDone"));
    } catch (e) {
      get().toastError(e);
    } finally {
      await get().refreshDataStatus();
      await get().refreshTiles();
    }
  },
  cancelUpdate: async (component) => {
    try {
      await api.dataCancel(component);
    } catch (e) {
      get().toastError(e);
    }
  },
  pauseUpdate: async (paused) => {
    set((s) => ({ dataStatus: s.dataStatus && { ...s.dataStatus, paused } }));
    try {
      await api.dataPause(paused);
    } catch (e) {
      get().toastError(e);
    }
    await get().refreshDataStatus();
  },
  exportData: async (dest, components, regions, areas) => {
    const t = get().t;
    set((s) => ({ progress: {}, dataStatus: s.dataStatus && { ...s.dataStatus, updating: true } }));
    try {
      await api.dataExport(dest, components, regions, areas);
      get().toast("success", t("exportDone"));
    } catch (e) {
      get().toastError(e);
    } finally {
      await get().refreshDataStatus();
    }
  },
  downloadPackage: async (dest) => {
    const t = get().t;
    set((s) => ({ dataStatus: s.dataStatus && { ...s.dataStatus, updating: true } }));
    try {
      await api.dataDownloadPackage(dest);
      get().toast("success", t("downloadPackageDone"));
      return true;
    } catch (e) {
      get().toastError(e);
      return false;
    } finally {
      await get().refreshDataStatus();
    }
  },
  importData: async (src) => {
    const t = get().t;
    set((s) => ({ progress: {}, dataStatus: s.dataStatus && { ...s.dataStatus, updating: true } }));
    try {
      const comps = await api.dataImport(src);
      get().toast("success", `${t("importDone")} (${comps.length})`);
      api.dataPreload(true).catch(() => {});
    } catch (e) {
      get().toastError(e);
    } finally {
      // a package may add sharp-imagery areas to Settings (only that field: a full reload would
      // also reset the map layers)
      try {
        const fresh = await api.settingsGet();
        set((s) => ({ settings: { ...s.settings, satelliteAreas: fresh.satelliteAreas } }));
      } catch {
        /* keep the current settings */
      }
      await get().refreshDataStatus();
      await get().refreshTiles();
      void get().checkUpdates(true);
    }
  },
});
