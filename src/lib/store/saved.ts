import { api, type Favorite, type FavoriteKind, type GeoResult, type HistoryItem, type LineRef, type LineShape, type LonLat } from "../api";
import type { SliceCreator } from "./types";

/** Favorites, the favorite-line preview and the search / route history. */
export interface SavedSlice {
  favorites: Favorite[];
  /** A favorite transit line drawn on the map (favorites panel). */
  linePreview: { favId: number; shape: LineShape } | null;
  history: HistoryItem[];

  loadFavorites: () => Promise<void>;
  /** Save / remove a searched place as a favorite; resolves to the new saved state. */
  toggleFavorite: (p: { lon: number; lat: number; label: string; result?: GeoResult }) => Promise<boolean>;
  /** Save / remove a transit line as a favorite; resolves to the new saved state. */
  toggleLineFavorite: (line: LineRef, at: LonLat) => Promise<boolean>;
  showLine: (f: Favorite) => Promise<void>;
  clearLine: () => void;
  loadHistory: () => Promise<void>;
  addHistory: (kind: "search" | "route", label: string, payload: unknown) => Promise<void>;
}

/** The favorite saved at this spot (within ~10 cm), if any. */
export function findFavorite(favs: Favorite[], lon: number, lat: number): Favorite | undefined {
  return favs.find((f) => f.kind !== "line" && Math.abs(f.lon - lon) < 1e-6 && Math.abs(f.lat - lat) < 1e-6);
}

/** The favorite entry of this transit line (same number, agency and headsign), if any. */
export function findLineFavorite(favs: Favorite[], line: LineRef): Favorite | undefined {
  return favs.find((f) => {
    if (f.kind !== "line") return false;
    const d = (f.data ?? {}) as Partial<LineRef>;
    return d.routeShortName === line.routeShortName && (d.agency ?? "") === (line.agency ?? "") && (d.headsign ?? "") === (line.headsign ?? "");
  });
}

export const createSavedSlice: SliceCreator<SavedSlice> = (set, get) => ({
  favorites: [],
  linePreview: null,
  history: [],

  toggleFavorite: async (p) => {
    const t = get().t;
    const existing = findFavorite(get().favorites, p.lon, p.lat);
    try {
      if (existing) {
        await api.favoritesDelete(existing.id);
        await get().loadFavorites();
        get().toast("info", `${t("favoriteRemoved")}: ${existing.name}`);
        return false;
      }
      const k = p.result?.kind;
      const kind: FavoriteKind = k === "stop" ? "stop" : k === "poi" || k === "place" ? "place" : "address";
      const code = p.result?.category?.startsWith("stop:") ? p.result.category.slice(5) : undefined;
      await api.favoritesAdd(p.label, "star", p.lon, p.lat, p.label, kind, code ? { code } : undefined);
      await get().loadFavorites();
      get().toast("success", `${t("favoriteAdded")}: ${p.label}`);
      return true;
    } catch (e) {
      get().toastError(e);
      return !!existing;
    }
  },
  toggleLineFavorite: async (line, at) => {
    const t = get().t;
    const existing = findLineFavorite(get().favorites, line);
    try {
      if (existing) {
        if (get().linePreview?.favId === existing.id) set({ linePreview: null });
        await api.favoritesDelete(existing.id);
        await get().loadFavorites();
        get().toast("info", `${t("favoriteRemoved")}: ${existing.name}`);
        return false;
      }
      const name = `${t("lineLabel")} ${line.routeShortName}${line.agency ? ` · ${line.agency}` : ""}`;
      const data: Record<string, unknown> = { routeShortName: line.routeShortName, agency: line.agency };
      if (line.routeLongName) data.routeLongName = line.routeLongName;
      if (line.headsign) data.headsign = line.headsign;
      if (line.color) data.color = line.color;
      await api.favoritesAdd(name, "bus", at[0], at[1], line.headsign, "line", data);
      await get().loadFavorites();
      get().toast("success", `${t("favoriteAdded")}: ${name}`);
      return true;
    } catch (e) {
      get().toastError(e);
      return !!existing;
    }
  },
  showLine: async (f) => {
    const d = (f.data ?? {}) as Partial<LineRef>;
    if (!d.routeShortName) return;
    try {
      const shape = await api.transitLine(d.routeShortName, d.agency ?? "", d.headsign, d.routeLongName);
      set({ linePreview: { favId: f.id, shape } });
    } catch (e) {
      get().toastError(e);
    }
  },
  clearLine: () => set({ linePreview: null }),
  loadFavorites: async () => {
    try {
      set({ favorites: await api.favoritesList() });
    } catch (e) {
      console.warn("favorites_list failed", e);
    }
  },
  loadHistory: async () => {
    try {
      set({ history: await api.historyList(undefined, 200) });
    } catch (e) {
      console.warn("history_list failed", e);
    }
  },
  addHistory: async (kind, label, payload) => {
    try {
      await api.historyAdd(kind, label, payload);
      void get().loadHistory();
    } catch (e) {
      console.warn("history_add failed", e);
    }
  },
});
