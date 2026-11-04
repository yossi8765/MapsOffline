import { api, apiX, type GeoResult } from "../api";
import { describeError } from "../errors";
import { translate, type I18nKey } from "../i18n";
import { parseMapLink } from "../links";
import type { ContextMenuState, Panel, ShareTarget, SliceCreator, Toast, ToastOptions } from "./types";

export interface UiSlice {
  panel: Panel;
  sidebarCollapsed: boolean;
  contextMenu: ContextMenuState | null;
  toasts: Toast[];
  searchFocusTick: number;
  /** Keyboard-shortcut table dialog (App). */
  shortcutsOpen: boolean;
  /** Command palette (Ctrl+K). */
  paletteOpen: boolean;
  /** Share dialog (link + QR code). */
  share: ShareTarget | null;
  /** CSV address-list dialog. */
  batchOpen: boolean;
  /** Bumped by "route here / from here": the route panel focuses its first empty field. */
  routeFocusTick: number;
  /** A place to show in the search panel's card, set from outside the panel (links, nearby list). */
  searchPick: { lon: number; lat: number; label: string; result?: GeoResult; tick: number } | null;
  /** Internet is reachable: navigator.onLine as a quick hint, confirmed by the backend's
   *  `connectivity` probe (WebView2 reports onLine for any network adapter). */
  online: boolean;

  setPanel: (p: Panel) => void;
  toggleSidebar: () => void;
  /** Open a place in the search panel card (marker + fly-to). */
  pickPlace: (p: { lon: number; lat: number; label: string; result?: GeoResult }, zoom?: number) => void;
  /** Handle a `mapsoffline://map=<z>/<lat>/<lon>&q=<label>` link (also `geo:<lat>,<lon>`). */
  openLink: (url: string) => boolean;
  setContextMenu: (c: ContextMenuState | null) => void;
  /** Show a toast. Identical toasts merge (×N); errors stay until closed. */
  toast: (kind: Toast["kind"], text: string, opts?: ToastOptions) => void;
  /** Toast for any rejection: the code's translated message, developer text as details. */
  toastError: (e: unknown) => void;
  dismissToast: (id: number) => void;
  /** Hovering a toast pauses its timer; leaving restarts it. */
  holdToast: (id: number, hold: boolean) => void;
  focusSearch: () => void;
  setShortcutsOpen: (open: boolean) => void;
  setPaletteOpen: (open: boolean) => void;
  setShare: (s: ShareTarget | null) => void;
  setBatchOpen: (open: boolean) => void;
  /** Open the route panel with the keyboard focus in the first empty origin / destination field. */
  focusRouteField: () => void;
  /** Ctrl+P / print buttons: prints the selected route (nothing to print without one). */
  requestPrint: () => void;
  /** "N": north up, flat. */
  bumpNorthUp: () => void;
  /** Ask the backend whether the internet is reachable (falls back to navigator.onLine). */
  checkConnectivity: () => Promise<boolean>;
  t: (k: I18nKey) => string;
}

let toastSeq = 1;
const toastTimers = new Map<number, number>();

function fmtCoordsShort(lon: number, lat: number) {
  return `${lat.toFixed(5)}, ${lon.toFixed(5)}`;
}

export const createUiSlice: SliceCreator<UiSlice> = (set, get) => {
  const arm = (t: Toast) => {
    const old = toastTimers.get(t.id);
    if (old) window.clearTimeout(old);
    toastTimers.delete(t.id);
    const ms = t.duration ?? (t.kind === "error" ? 0 : t.action ? 6000 : 3500);
    if (ms > 0) toastTimers.set(t.id, window.setTimeout(() => get().dismissToast(t.id), ms));
  };

  return {
    // `?panel=search` opens on another panel (used by the headless UI screenshots)
    panel: ((typeof location !== "undefined" && (new URLSearchParams(location.search).get("panel") as Panel | null)) || "route") as Panel,
    sidebarCollapsed: false,
    contextMenu: null,
    toasts: [],
    searchFocusTick: 0,
    shortcutsOpen: false,
    paletteOpen: false,
    share: null,
    batchOpen: false,
    routeFocusTick: 0,
    searchPick: null,
    online: typeof navigator !== "undefined" ? navigator.onLine : false,

    setPanel: (panel) => set({ panel, sidebarCollapsed: false }),
    toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
    pickPlace: (p, zoom) => {
      const s = get();
      set((st) => ({ searchPick: { ...p, tick: (st.searchPick?.tick ?? 0) + 1 }, panel: "search", sidebarCollapsed: false }));
      s.setMarker({ lonlat: [p.lon, p.lat], label: p.label });
      s.flyToPoint([p.lon, p.lat], zoom ?? (p.result?.kind === "place" ? 13 : 17));
    },
    openLink: (url) => {
      const parsed = parseMapLink(url);
      if (!parsed) return false;
      // `geo:0,0?q=<address text>`: search for the text and open the best result
      if (parsed.query) {
        const q = parsed.query;
        void api
          .geocodeSearch(q, get().mapCenter, 1)
          .then((res) => {
            const r = res[0];
            if (r) get().pickPlace({ lon: r.lon, lat: r.lat, label: r.name, result: r });
            else get().toast("info", `${translate(get().settings.language, "searchNoResults")}: ${q}`);
          })
          .catch((e) => get().toastError(e));
        return true;
      }
      get().pickPlace({ lon: parsed.lon, lat: parsed.lat, label: parsed.label || fmtCoordsShort(parsed.lon, parsed.lat) }, parsed.zoom);
      return true;
    },
    setContextMenu: (contextMenu) => set({ contextMenu }),
    toast: (kind, text, opts = {}) => {
      // the same message again: count it on the existing toast instead of stacking a copy
      const same = get().toasts.find((x) => x.kind === kind && x.text === text && !x.action && !opts.action);
      if (same) {
        const merged = { ...same, count: (same.count ?? 1) + 1, details: opts.details ?? same.details };
        set((s) => ({ toasts: s.toasts.map((x) => (x.id === same.id ? merged : x)) }));
        arm(merged);
        return;
      }
      const t: Toast = { id: toastSeq++, kind, text, details: opts.details, action: opts.action, duration: opts.duration };
      // at most 4 on screen: the oldest non-error one goes first
      set((s) => {
        let list = [...s.toasts, t];
        while (list.length > 4) {
          const i = list.findIndex((x) => x.kind !== "error");
          list = list.filter((_, j) => j !== (i >= 0 ? i : 0));
        }
        return { toasts: list };
      });
      arm(t);
    },
    toastError: (e) => {
      const d = describeError(e, get().t);
      get().toast("error", d.title, { details: d.details });
    },
    dismissToast: (id) => {
      const old = toastTimers.get(id);
      if (old) window.clearTimeout(old);
      toastTimers.delete(id);
      set((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) }));
    },
    holdToast: (id, hold) => {
      const t = get().toasts.find((x) => x.id === id);
      if (!t) return;
      if (hold) {
        const old = toastTimers.get(id);
        if (old) window.clearTimeout(old);
        toastTimers.delete(id);
      } else arm(t);
    },
    focusSearch: () => set((s) => ({ panel: "search", sidebarCollapsed: false, searchFocusTick: s.searchFocusTick + 1 })),
    setShortcutsOpen: (shortcutsOpen) => set({ shortcutsOpen }),
    setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
    setShare: (share) => set({ share }),
    setBatchOpen: (batchOpen) => set({ batchOpen }),
    focusRouteField: () => set((s) => ({ panel: "route", sidebarCollapsed: false, routeFocusTick: s.routeFocusTick + 1 })),
    requestPrint: () => {
      // print through printPage (light theme, map overview); without a route there is nothing to
      // print, and the browser would print an empty page
      if (!get().routes.length) {
        get().toast("info", get().t("printNeedsRoute"));
        return;
      }
      void import("../print").then((m) => m.printPage());
    },
    bumpNorthUp: () => set((s) => ({ northUpTick: s.northUpTick + 1 })),
    checkConnectivity: async () => {
      const hint = typeof navigator !== "undefined" ? navigator.onLine : true;
      if (!hint) {
        set({ online: false });
        return false;
      }
      try {
        const ok = await apiX.connectivity();
        set({ online: ok });
        return ok;
      } catch {
        // backend without the probe (older build): trust the OS hint
        set({ online: hint });
        return hint;
      }
    },
    t: (k) => translate(get().settings.language, k),
  };
};
