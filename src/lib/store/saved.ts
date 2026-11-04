import { api, apiX, type Favorite, type FavoriteKind, type GeoResult, type HistoryItem, type LineRef, type LineShape, type LonLat } from "../api";
import { routeArrow } from "../format";
import { getPrefJson, setPrefJson } from "../prefs";
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
  /** Group names in use (favorites panel sections / picker). */
  favoriteGroups: string[];
  loadFavoriteGroups: () => Promise<void>;
  /** Change name / icon / group / colour / note; null clears a field. */
  patchFavorite: (id: number, patch: { name?: string; icon?: string; group?: string | null; color?: string | null; note?: string | null; address?: string | null }) => Promise<void>;
  /** Delete with a 5 s "undo" toast: the row disappears at once, the database delete waits. */
  removeFavorite: (f: Favorite) => void;
  /** Save the current route request (points, mode, avoid options) as a favorite. */
  saveRouteFavorite: (name?: string) => Promise<void>;
  /** Favorite groups drawn on the map ("" = favorites without a group); remembered per device. */
  visibleFavoriteGroups: string[];
  toggleFavoriteGroupOnMap: (group: string) => void;
}

const loadVisibleGroups = (): string[] =>
  getPrefJson("mapsoffline.favGroupsOnMap", (v) => (Array.isArray(v) && v.every((x) => typeof x === "string") ? (v as string[]) : undefined), []);

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
  favoriteGroups: [],
  visibleFavoriteGroups: loadVisibleGroups(),
  toggleFavoriteGroupOnMap: (group) => {
    const cur = get().visibleFavoriteGroups;
    const next = cur.includes(group) ? cur.filter((g) => g !== group) : [...cur, group];
    setPrefJson("mapsoffline.favGroupsOnMap", next);
    set({ visibleFavoriteGroups: next });
  },

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
      const favorites = await api.favoritesList();
      // groups are derived locally too, so an older backend without favorites_groups still works
      const groups = [...new Set(favorites.map((f) => f.group).filter((g): g is string => !!g))];
      set({ favorites, favoriteGroups: groups.sort((a, b) => a.localeCompare(b, "he")) });
    } catch (e) {
      get().toastError(e);
    }
  },
  loadFavoriteGroups: async () => {
    try {
      const groups = await apiX.favoritesGroups();
      set({ favoriteGroups: groups });
    } catch {
      /* derived in loadFavorites */
    }
  },
  patchFavorite: async (id, patch) => {
    try {
      await apiX.favoritesPatch(id, patch);
    } catch (e) {
      // older backend: name / icon through the old command; the new fields need the update
      if (patch.group === undefined && patch.color === undefined && patch.note === undefined) {
        const f = get().favorites.find((x) => x.id === id);
        try {
          await api.favoritesUpdate(id, patch.name ?? f?.name ?? "", patch.icon ?? f?.icon ?? "star");
        } catch (e2) {
          get().toastError(e2);
        }
      } else get().toastError(e);
    }
    await get().loadFavorites();
  },
  removeFavorite: (f) => {
    const t = get().t;
    set((s) => ({ favorites: s.favorites.filter((x) => x.id !== f.id) }));
    if (get().linePreview?.favId === f.id) set({ linePreview: null });
    let undone = false;
    const timer = window.setTimeout(async () => {
      if (undone) return;
      try {
        await api.favoritesDelete(f.id);
      } catch (e) {
        get().toastError(e);
      }
      await get().loadFavorites();
    }, 5000);
    get().toast("info", `${t("favoriteRemoved")}: ${f.name}`, {
      duration: 5000,
      action: {
        label: t("undo"),
        run: () => {
          undone = true;
          window.clearTimeout(timer);
          void get().loadFavorites();
        },
      },
    });
  },
  saveRouteFavorite: async (name) => {
    const s = get();
    const req = s.lastRequest;
    const pts = s.waypoints;
    if (!req || pts.some((p) => !p)) return;
    const labels = pts.map((p) => p!.label);
    const title = name?.trim() || `${labels[0]} ${routeArrow(s.settings.language)} ${labels[labels.length - 1]}`;
    const icon = req.mode === "transit" ? "bus" : "route";
    try {
      const fav = await api.favoritesAdd(title, icon, pts[0]!.lon, pts[0]!.lat, labels.join(" · "), "route", { request: req, labels });
      await get().loadFavorites();
      s.toast("success", `${s.t("favoriteAdded")}: ${fav.name}`);
    } catch (e) {
      s.toastError(e);
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
      const item = await api.historyAdd(kind, label, payload);
      // insert locally (same label replaces the older entry, like the database does) instead of
      // reloading 200 rows after every search and route
      if (item && typeof item === "object" && "id" in item) {
        set((s) => ({ history: [item, ...s.history.filter((h) => h.id !== item.id && !(h.kind === item.kind && h.label === item.label))].slice(0, 200) }));
      } else void get().loadHistory();
    } catch (e) {
      console.warn("history_add failed", e);
    }
  },
});
