import { api, type GeoResult } from "../api";
import { errorMessage } from "../errors";
import { translate, type I18nKey } from "../i18n";
import { parseMapLink } from "../links";
import type { ContextMenuState, Panel, SliceCreator, Toast } from "./types";

export interface UiSlice {
  panel: Panel;
  sidebarCollapsed: boolean;
  contextMenu: ContextMenuState | null;
  toasts: Toast[];
  searchFocusTick: number;
  /** Keyboard-shortcut table dialog (App). */
  shortcutsOpen: boolean;
  /** A place to show in the search panel's card, set from outside the panel (links, nearby list). */
  searchPick: { lon: number; lat: number; label: string; result?: GeoResult; tick: number } | null;
  /** The OS reports a network connection (navigator.onLine); gates online-only menu items. */
  online: boolean;

  setPanel: (p: Panel) => void;
  toggleSidebar: () => void;
  /** Open a place in the search panel card (marker + fly-to). */
  pickPlace: (p: { lon: number; lat: number; label: string; result?: GeoResult }, zoom?: number) => void;
  /** Handle a `mapsoffline://map=<z>/<lat>/<lon>&q=<label>` link (also `geo:<lat>,<lon>`). */
  openLink: (url: string) => boolean;
  setContextMenu: (c: ContextMenuState | null) => void;
  toast: (kind: Toast["kind"], text: string) => void;
  /** Toast for any rejection: the code's translated message (lib/errors.ts). */
  toastError: (e: unknown) => void;
  dismissToast: (id: number) => void;
  focusSearch: () => void;
  t: (k: I18nKey) => string;
}

let toastSeq = 1;

function fmtCoordsShort(lon: number, lat: number) {
  return `${lat.toFixed(5)}, ${lon.toFixed(5)}`;
}

export const createUiSlice: SliceCreator<UiSlice> = (set, get) => ({
  // `?panel=search` opens on another panel (used by the headless UI screenshots)
  panel: ((typeof location !== "undefined" && (new URLSearchParams(location.search).get("panel") as Panel | null)) || "route") as Panel,
  sidebarCollapsed: false,
  contextMenu: null,
  toasts: [],
  searchFocusTick: 0,
  shortcutsOpen: false,
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
  toast: (kind, text) => {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts, { id, kind, text }] }));
    setTimeout(() => get().dismissToast(id), kind === "error" ? 6000 : 3000);
  },
  toastError: (e) => get().toast("error", errorMessage(e, get().t)),
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) })),
  focusSearch: () => set((s) => ({ panel: "search", sidebarCollapsed: false, searchFocusTick: s.searchFocusTick + 1 })),
  t: (k) => translate(get().settings.language, k),
});
