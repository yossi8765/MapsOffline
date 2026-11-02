import { create } from "zustand";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  api,
  errorCode,
  errorText,
  type Avoid,
  type DataComponent,
  type DataProgress,
  type DataStatus,
  type DataCheck,
  type Favorite,
  type FavoriteKind,
  type LineRef,
  type LineShape,
  type MyLocation,
  type AppUpdate,
  type GeoResult,
  type HistoryItem,
  type LonLat,
  type Mode,
  type Route,
  type RouteRequest,
  type RouteSource,
  type Settings,
  type TimeSpec,
} from "./api";
import { translate, type I18nKey } from "./i18n";

export type Panel = "route" | "search" | "favorites" | "history" | "layers" | "data" | "settings";

export interface RoutePoint {
  lon: number;
  lat: number;
  label: string;
  result?: GeoResult;
}

export interface ContextMenuState {
  x: number;
  y: number;
  lon: number;
  lat: number;
}

export interface Toast {
  id: number;
  kind: "info" | "success" | "error";
  text: string;
}

export interface MapLayers {
  satellite: boolean;
  /** Hillshading from the elevation data. */
  hillshade: boolean;
  /** Road-number shields. */
  roadShields: boolean;
  /** 3D terrain (needs tilt). */
  terrain3d: boolean;
  /** Selected map-layer POI categories (ids from lib/poiCategories). */
  poiCats: string[];
  buildings3d: boolean;
  poi: boolean;
  transitStops: boolean;
  flavor: "auto" | "light" | "dark" | "grayscale";
  labelLang: "he" | "en" | "ar";
}

export interface HighlightState {
  routeId: string;
  legIndex: number;
  stepIndex?: number;
}

const DEFAULT_SETTINGS: Settings = {
  language: "he",
  theme: "auto",
  units: "metric",
  dataDir: null,
  showTransitLayer: true,
  showPoiLayer: true,
  mapLabelLanguage: "he",
  routingMode: "auto",
  osrmUrl: "https://routing.openstreetmap.de",
  transitApiUrl: "https://api.transitous.org",
  avoidTolls: true,
  autoCheckAppUpdates: true,
  avoidAreas: "ab",
  avoidMotorways: false,
  avoidFerries: false,
  unpaved: "avoidLong",
  avoidHardTurns: false,
};

/** Route-panel defaults derived from the settings. */
function avoidDefaults(s: Settings): Avoid {
  return {
    toll: s.avoidTolls,
    motorway: s.avoidMotorways,
    ferry: s.avoidFerries,
    unpaved: s.unpaved !== "allow",
    unpavedLongOnly: s.unpaved === "avoidLong",
    hardTurns: s.avoidHardTurns,
    areaA: s.avoidAreas !== "none",
    areaB: s.avoidAreas === "ab",
  };
}

const NAV_SETTING_KEYS: (keyof Settings)[] = ["avoidTolls", "avoidAreas", "avoidMotorways", "avoidFerries", "unpaved", "avoidHardTurns"];

interface State {
  // ui
  panel: Panel;
  sidebarCollapsed: boolean;
  contextMenu: ContextMenuState | null;
  toasts: Toast[];
  searchFocusTick: number;
  settingsLoaded: boolean;
  settings: Settings;
  layers: MapLayers;
  appVersion: string;
  transitStopsUnavailable: boolean;

  // map
  mapCenter: LonLat;
  mapZoom: number;
  flyTo: { lonlat: LonLat; zoom?: number; tick: number } | null;
  marker: { lonlat: LonLat; label: string } | null;
  fitRouteTick: number;

  // route planning
  waypoints: (RoutePoint | null)[]; // [from, ...via, to]
  mode: Mode;
  time: TimeSpec;
  avoid: Avoid;
  alternativesCount: number;
  routes: Route[];
  selectedRouteId: string | null;
  /** The selected route card is expanded to show its navigation steps. */
  detailsOpen: boolean;
  routeLoading: boolean;
  routeError: string | null;
  routeSource: RouteSource | null;
  routeProvider: string | null;
  routeTick: number;
  highlight: HighlightState | null;
  lastRequest: RouteRequest | null;

  // data
  dataStatus: DataStatus | null;
  dataCheck: DataCheck | null;
  checkingUpdates: boolean;
  progress: Partial<Record<DataComponent, DataProgress>>;
  tilesUrl: string | null;
  satelliteUrl: string | null;
  terrainUrl: string | null;
  /** The tiles path has been queried at least once (the "no map" card waits for it). */
  tilesChecked: boolean;
  tilesTick: number;

  // history
  favorites: Favorite[];
  /** A favorite transit line drawn on the map (favorites panel). */
  linePreview: { favId: number; shape: LineShape } | null;
  /** Last fix of "my location" (map dot + search option). */
  myLocation: (MyLocation & { at: number }) | null;
  locating: boolean;
  appUpdate: AppUpdate | null;
  appUpdateBusy: "check" | "download" | null;
  appUpdateProgress: { done: number; total: number; message: string } | null;
  /** The OS reports a network connection (navigator.onLine); gates online-only menu items. */
  online: boolean;
  history: HistoryItem[];
}

interface Actions {
  setPanel: (p: Panel) => void;
  toggleSidebar: () => void;
  setContextMenu: (c: ContextMenuState | null) => void;
  toast: (kind: Toast["kind"], text: string) => void;
  toastError: (e: unknown) => void;
  dismissToast: (id: number) => void;
  focusSearch: () => void;
  t: (k: I18nKey) => string;

  loadSettings: () => Promise<void>;
  updateSettings: (patch: Partial<Settings>) => Promise<void>;
  setLayers: (patch: Partial<MapLayers>) => void;
  setTransitStopsUnavailable: (v: boolean) => void;

  setMapView: (center: LonLat, zoom: number) => void;
  flyToPoint: (lonlat: LonLat, zoom?: number) => void;
  setMarker: (m: { lonlat: LonLat; label: string } | null) => void;

  setWaypoint: (index: number, p: RoutePoint | null) => void;
  setFrom: (p: RoutePoint) => void;
  setTo: (p: RoutePoint) => void;
  addVia: (p?: RoutePoint | null) => void;
  removeWaypoint: (index: number) => void;
  moveWaypoint: (index: number, dir: -1 | 1) => void;
  swapEnds: () => void;
  setMode: (m: Mode) => void;
  setTime: (t: TimeSpec) => void;
  setAvoid: (a: Partial<Avoid>) => void;
  setAlternativesCount: (n: number) => void;
  computeRoute: () => Promise<void>;
  selectRoute: (id: string | null) => void;
  /** Click on a route card: select it (opening its steps), or toggle the steps when already selected. */
  toggleRoute: (id: string) => void;
  clearRoute: () => void;
  setHighlight: (h: HighlightState | null) => void;
  loadRequest: (req: RouteRequest, labels?: string[]) => void;

  refreshDataStatus: () => Promise<void>;
  /** Ask the servers for newer data; `force` re-checks even if checked recently. */
  checkUpdates: (force?: boolean) => Promise<void>;
  refreshTiles: () => Promise<void>;
  setProgress: (p: DataProgress) => void;
  startUpdate: (components: DataComponent[]) => Promise<void>;
  exportData: (dest: string, components?: DataComponent[]) => Promise<void>;
  importData: (src: string) => Promise<void>;
  cancelUpdate: (component?: DataComponent) => Promise<void>;
  pauseUpdate: (paused: boolean) => Promise<void>;

  loadFavorites: () => Promise<void>;
  /** Save / remove a searched place as a favorite; resolves to the new saved state. */
  toggleFavorite: (p: { lon: number; lat: number; label: string; result?: GeoResult }) => Promise<boolean>;
  /** Save / remove a transit line as a favorite; resolves to the new saved state. */
  toggleLineFavorite: (line: LineRef, at: LonLat) => Promise<boolean>;
  showLine: (f: Favorite) => Promise<void>;
  /** Ask the OS for the position (system consent dialog on first use); flies there when `fly`. */
  locate: (fly?: boolean) => Promise<MyLocation | null>;
  /** Ask GitLab for a newer signed release; `quiet` = startup check (toast only when newer). */
  checkAppUpdate: (quiet?: boolean) => Promise<void>;
  /** Download, verify, start the installer and quit. */
  installAppUpdate: () => Promise<void>;
  /** Reorder the via stops for the shortest trip (origin fixed; destination fixed when `fixedEnd`). */
  optimizeOrder: (fixedEnd: boolean) => Promise<void>;
  clearLine: () => void;
  loadHistory: () => Promise<void>;
  addHistory: (kind: "search" | "route", label: string, payload: unknown) => Promise<void>;
}

let toastSeq = 1;
let routeSeq = 0;

export const useStore = create<State & Actions>((set, get) => ({
  // `?panel=search` opens on another panel (used by the headless UI screenshots)
  panel: ((typeof location !== "undefined" && (new URLSearchParams(location.search).get("panel") as Panel | null)) || "route") as Panel,
  sidebarCollapsed: false,
  contextMenu: null,
  toasts: [],
  searchFocusTick: 0,
  settingsLoaded: false,
  settings: DEFAULT_SETTINGS,
  layers: { satellite: false, hillshade: false, roadShields: true, terrain3d: false, poiCats: [], buildings3d: false, poi: true, transitStops: false, flavor: "auto", labelLang: "he" },
  appVersion: "",
  transitStopsUnavailable: false,

  mapCenter: [34.78, 32.08],
  mapZoom: 8,
  flyTo: null,
  marker: null,
  fitRouteTick: 0,

  waypoints: [null, null],
  mode: "car",
  time: { kind: "now" },
  avoid: avoidDefaults(DEFAULT_SETTINGS),
  alternativesCount: 3,
  routes: [],
  selectedRouteId: null,
  detailsOpen: false,
  routeLoading: false,
  routeError: null,
  routeSource: null,
  routeProvider: null,
  routeTick: 0,
  highlight: null,
  lastRequest: null,

  dataStatus: null,
  dataCheck: null,
  checkingUpdates: false,
  progress: {},
  tilesUrl: null,
  satelliteUrl: null,
  terrainUrl: null,
  tilesChecked: false,
  tilesTick: 0,

  favorites: [],
  linePreview: null,
  myLocation: null,
  locating: false,
  appUpdate: null,
  appUpdateBusy: null,
  appUpdateProgress: null,
  online: typeof navigator !== "undefined" ? navigator.onLine : false,
  history: [],

  // ---------------------------------------------------------------- ui
  setPanel: (panel) => set({ panel, sidebarCollapsed: false }),
  toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
  setContextMenu: (contextMenu) => set({ contextMenu }),
  toast: (kind, text) => {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts, { id, kind, text }] }));
    setTimeout(() => get().dismissToast(id), kind === "error" ? 6000 : 3000);
  },
  toastError: (e) => {
    const t = get().t;
    const code = errorCode(e);
    const text = errorText(e);
    const msg =
      code === "NO_ROUTING_DATA"
        ? t("errNoRouting")
        : code === "NO_TRANSIT_DATA"
          ? t("errNoTransit")
          : code === "NO_GEOCODE_DATA"
            ? t("errNoGeocode")
            : code === "UPDATE_RUNNING"
              ? t("errUpdateRunning")
              : code === "NEED_TWO_WAYPOINTS"
                ? t("errNeedTwo")
                : text.includes("not implemented")
                  ? t("errNotImplemented")
                  : `${t("errGeneric")}: ${text}`;
    get().toast("error", msg);
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) })),
  focusSearch: () => set((s) => ({ panel: "search", sidebarCollapsed: false, searchFocusTick: s.searchFocusTick + 1 })),
  t: (k) => translate(get().settings.language, k),

  // ---------------------------------------------------------------- settings
  loadSettings: async () => {
    try {
      const s = await api.settingsGet();
      const merged = { ...DEFAULT_SETTINGS, ...s };
      set({
        settings: merged,
        settingsLoaded: true,
        avoid: { ...get().avoid, ...avoidDefaults(merged) },
        layers: {
          ...get().layers,
          poi: merged.showPoiLayer,
          transitStops: merged.showTransitLayer,
          labelLang: (merged.mapLabelLanguage as MapLayers["labelLang"]) || "he",
        },
      });
    } catch (e) {
      console.warn("settings_get failed", e);
      set({ settingsLoaded: true });
    }
    try {
      const info = await api.appInfo();
      set({ appVersion: info.version });
    } catch {
      /* ignore */
    }
  },
  updateSettings: async (patch) => {
    const next = { ...get().settings, ...patch };
    set({ settings: next });
    if (NAV_SETTING_KEYS.some((k) => patch[k] !== undefined)) {
      set({ avoid: { ...get().avoid, ...avoidDefaults(next) } });
      void get().computeRoute();
    }
    try {
      const saved = await api.settingsSet(next);
      set({ settings: { ...DEFAULT_SETTINGS, ...saved } });
      if (patch.dataDir !== undefined) {
        await get().refreshDataStatus();
        await get().refreshTiles();
      }
    } catch (e) {
      get().toastError(e);
    }
  },
  setTransitStopsUnavailable: (transitStopsUnavailable) => {
    if (get().transitStopsUnavailable !== transitStopsUnavailable) set({ transitStopsUnavailable });
  },
  setLayers: (patch) => {
    const layers = { ...get().layers, ...patch };
    set({ layers });
    const s = get().settings;
    const settingsPatch: Partial<Settings> = {};
    if (patch.poi !== undefined && patch.poi !== s.showPoiLayer) settingsPatch.showPoiLayer = patch.poi;
    if (patch.transitStops !== undefined && patch.transitStops !== s.showTransitLayer)
      settingsPatch.showTransitLayer = patch.transitStops;
    if (patch.labelLang !== undefined && patch.labelLang !== s.mapLabelLanguage)
      settingsPatch.mapLabelLanguage = patch.labelLang;
    if (Object.keys(settingsPatch).length) void get().updateSettings(settingsPatch);
  },

  // ---------------------------------------------------------------- map
  setMapView: (mapCenter, mapZoom) => set({ mapCenter, mapZoom }),
  flyToPoint: (lonlat, zoom) => set((s) => ({ flyTo: { lonlat, zoom, tick: (s.flyTo?.tick ?? 0) + 1 } })),
  setMarker: (marker) => set({ marker }),

  // ---------------------------------------------------------------- route planning
  setWaypoint: (index, p) => {
    const wps = [...get().waypoints];
    wps[index] = p;
    set({ waypoints: wps });
    void get().computeRoute();
  },
  setFrom: (p) => get().setWaypoint(0, p),
  setTo: (p) => get().setWaypoint(get().waypoints.length - 1, p),
  addVia: (p) => {
    const wps = [...get().waypoints];
    if (wps.length >= 7) return;
    wps.splice(wps.length - 1, 0, p ?? null);
    set({ waypoints: wps });
    if (p) void get().computeRoute();
  },
  removeWaypoint: (index) => {
    const wps = [...get().waypoints];
    if (wps.length <= 2) {
      wps[index] = null;
    } else {
      wps.splice(index, 1);
    }
    set({ waypoints: wps });
    void get().computeRoute();
  },
  moveWaypoint: (index, dir) => {
    const wps = [...get().waypoints];
    const j = index + dir;
    if (j < 0 || j >= wps.length) return;
    [wps[index], wps[j]] = [wps[j], wps[index]];
    set({ waypoints: wps });
    void get().computeRoute();
  },
  swapEnds: () => {
    const wps = [...get().waypoints].reverse();
    set({ waypoints: wps });
    void get().computeRoute();
  },
  setMode: (mode) => {
    set({ mode });
    void get().computeRoute();
  },
  setTime: (time) => {
    set({ time });
    void get().computeRoute();
  },
  setAvoid: (a) => {
    set({ avoid: { ...get().avoid, ...a } });
    void get().computeRoute();
  },
  setAlternativesCount: (n) => {
    set({ alternativesCount: n });
    void get().computeRoute();
  },
  computeRoute: async () => {
    const s = get();
    const pts = s.waypoints;
    if (pts.some((p) => !p) || pts.length < 2) {
      set({ routes: [], selectedRouteId: null, routeError: null, highlight: null });
      return;
    }
    const req: RouteRequest = {
      waypoints: pts.map((p) => ({ lon: p!.lon, lat: p!.lat, name: p!.label })),
      mode: s.mode,
      time: s.time.kind === "now" ? { kind: "now" } : s.time,
      avoid: s.avoid,
      alternatives: s.mode === "transit" ? Math.max(s.alternativesCount, 3) : s.alternativesCount,
    };
    const seq = ++routeSeq;
    set({ routeLoading: true, routeError: null, lastRequest: req });
    try {
      const resp = await api.route(req);
      if (seq !== routeSeq) return;
      const routes = resp.routes;
      set({
        routes,
        routeSource: resp.source ?? "offline",
        routeProvider: resp.provider ?? null,
        selectedRouteId: routes[0]?.id ?? null,
        detailsOpen: false,
        routeLoading: false,
        routeError: routes.length ? null : get().t("noRoute"),
        routeTick: get().routeTick + 1,
        fitRouteTick: get().fitRouteTick + 1,
        highlight: null,
      });
      if (routes.length) {
        const label = `${pts[0]!.label} ← ${pts[pts.length - 1]!.label}`;
        const r = routes[0];
        void get().addHistory("route", label, {
          request: req,
          labels: pts.map((p) => p!.label),
          summary: { durationSec: r.durationSec, distanceM: r.distanceM, mode: r.mode, transfers: r.transfers },
        });
      }
    } catch (e) {
      if (seq !== routeSeq) return;
      const code = errorCode(e);
      const t = get().t;
      const msg =
        errorText(e).startsWith("ONLINE_FAILED")
          ? t("errOnlineFailed")
          : errorText(e).startsWith("NO_ROUTE_AVOID_AREAS")
            ? t("errNoRouteAreas")
            : code === "NO_ROUTING_DATA"
          ? t("errNoRouting")
          : code === "NO_TRANSIT_DATA"
            ? t("errNoTransit")
            : errorText(e).includes("not implemented")
              ? t("errNotImplemented")
              : errorText(e);
      set({ routes: [], selectedRouteId: null, routeLoading: false, routeError: msg });
    }
  },
  selectRoute: (selectedRouteId) => set({ selectedRouteId, highlight: null }),
  toggleRoute: (id) => {
    const s = get();
    if (s.selectedRouteId === id) set({ detailsOpen: !s.detailsOpen, highlight: null });
    else set({ selectedRouteId: id, detailsOpen: true, highlight: null });
  },
  clearRoute: () => {
    routeSeq++;
    set({
      waypoints: [null, null],
      routes: [],
      selectedRouteId: null,
      routeError: null,
      routeLoading: false,
      highlight: null,
      lastRequest: null,
    });
  },
  setHighlight: (highlight) => set({ highlight }),
  loadRequest: (req, labels) => {
    const wps: RoutePoint[] = req.waypoints.map((w, i) => ({
      lon: w.lon,
      lat: w.lat,
      label: labels?.[i] ?? w.name ?? `${w.lat.toFixed(5)}, ${w.lon.toFixed(5)}`,
    }));
    set({
      waypoints: wps,
      mode: req.mode,
      time: req.time ?? { kind: "now" },
      avoid: req.avoid,
      alternativesCount: req.alternatives || 3,
      panel: "route",
      sidebarCollapsed: false,
    });
    void get().computeRoute();
  },

  // ---------------------------------------------------------------- data
  refreshDataStatus: async () => {
    try {
      const dataStatus = await api.dataStatus();
      set({ dataStatus });
    } catch (e) {
      console.warn("data_status failed", e);
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
  refreshTiles: async () => {
    try {
      const [p, sp, tp] = await Promise.all([api.tilesPath(), api.satellitePath().catch(() => null), api.terrainPath().catch(() => null)]);
      // A fresh query string per refresh: the pmtiles protocol caches archive headers and
      // directories per URL, so a replaced file must get a new URL or stale tiles keep showing.
      const bust = `?v=${Date.now()}`;
      const url = p ? convertFileSrc(p) + bust : null;
      set((s) => ({ tilesUrl: url, satelliteUrl: sp ? convertFileSrc(sp) + bust : null, terrainUrl: tp ? convertFileSrc(tp) + bust : null, tilesChecked: true, tilesTick: s.tilesTick + 1 }));
    } catch (e) {
      console.warn("tiles_path failed", e);
      set({ tilesUrl: null, satelliteUrl: null, terrainUrl: null, tilesChecked: true });
    }
  },
  setProgress: (p) => {
    set((s) => ({ progress: { ...s.progress, [p.component]: p } }));
    if (p.phase === "done" || p.phase === "failed" || p.phase === "cancelled") void get().refreshDataStatus();
  },
  startUpdate: async (components) => {
    const t = get().t;
    const queued: Partial<Record<DataComponent, DataProgress>> = {};
    for (const c of components) queued[c] = { component: c, phase: "queued", done: 0, total: 0, message: t("phaseQueued") };
    set((s) => ({ progress: { ...s.progress, ...queued }, dataStatus: s.dataStatus && { ...s.dataStatus, updating: true } }));
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
  exportData: async (dest, components) => {
    const t = get().t;
    set((s) => ({ progress: {}, dataStatus: s.dataStatus && { ...s.dataStatus, updating: true } }));
    try {
      await api.dataExport(dest, components);
      get().toast("success", t("exportDone"));
    } catch (e) {
      get().toastError(e);
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
      await get().refreshDataStatus();
      await get().refreshTiles();
      void get().checkUpdates(true);
    }
  },

  // ---------------------------------------------------------------- history
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
      const shape = await api.transitLine(d.routeShortName, d.agency ?? "", d.headsign);
      set({ linePreview: { favId: f.id, shape } });
    } catch (e) {
      get().toastError(e);
    }
  },
  clearLine: () => set({ linePreview: null }),
  checkAppUpdate: async (quiet = false) => {
    const t = get().t;
    set({ appUpdateBusy: "check" });
    try {
      const u = await api.appUpdateCheck();
      set({ appUpdate: u });
      if (u.available && quiet) get().toast("info", `${t("updateAvailableToast")} ${u.latest}`);
    } catch (e) {
      const msg = errorText(e);
      set({ appUpdate: { current: get().appVersion ?? "", latest: "", available: false, pageUrl: "", notes: "", checkedIso: new Date().toISOString(), error: msg } });
      if (!quiet) get().toastError(e);
    } finally {
      set({ appUpdateBusy: null });
    }
  },
  installAppUpdate: async () => {
    const t = get().t;
    set({ appUpdateBusy: "download", appUpdateProgress: { done: 0, total: 0, message: "" } });
    try {
      const path = await api.appUpdateDownload();
      set({ appUpdateProgress: { done: 1, total: 1, message: t("updateStarting") } });
      await api.appUpdateInstall(path);
    } catch (e) {
      get().toastError(e);
      set({ appUpdateBusy: null, appUpdateProgress: null });
    }
  },
  optimizeOrder: async (fixedEnd) => {
    const s = get();
    const pts = s.waypoints;
    if (pts.some((p) => !p) || pts.length < 3) return;
    const req: RouteRequest = {
      waypoints: pts.map((p) => ({ lon: p!.lon, lat: p!.lat, name: p!.label })),
      mode: s.mode,
      time: { kind: "now" },
      avoid: s.avoid,
      alternatives: 0,
    };
    set({ routeLoading: true });
    try {
      const order = await api.optimizeOrder(req, fixedEnd);
      const changed = order.some((o, i) => o !== i);
      set({ waypoints: order.map((i) => pts[i]) });
      if (!changed) get().toast("info", get().t("orderAlreadyBest"));
      await get().computeRoute();
    } catch (e) {
      set({ routeLoading: false });
      get().toastError(e);
    }
  },
  locate: async (fly = true) => {
    const t = get().t;
    set({ locating: true });
    try {
      const loc = await api.locate();
      set({ myLocation: { ...loc, at: Date.now() } });
      if (fly) get().flyToPoint([loc.lon, loc.lat], loc.accuracyM > 500 ? 13 : 16);
      return loc;
    } catch (e) {
      const code = errorCode(e);
      get().toast("error", code === "LOCATION_DENIED" ? t("locationDenied") : t("locationUnavailable"));
      return null;
    } finally {
      set({ locating: false });
    }
  },
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
}));

export function resolveTheme(theme: string): "light" | "dark" {
  if (theme === "light" || theme === "dark") return theme;
  if (theme === "night") return isNightInIsrael() ? "dark" : "light";
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/**
 * Dark between sunset and sunrise, computed for the centre of Israel (31.8°N, 35.0°E) with the
 * standard NOAA approximation (accurate to a few minutes, plenty for a theme switch).
 */
export function isNightInIsrael(now = new Date()): boolean {
  const lat = 31.8;
  const lon = 35.0;
  const rad = Math.PI / 180;
  const start = Date.UTC(now.getUTCFullYear(), 0, 0);
  const doy = Math.floor((now.getTime() - start) / 86_400_000);
  const g = (2 * Math.PI / 365) * (doy - 1 + (now.getUTCHours() - 12) / 24);
  const eqTime = 229.18 * (0.000075 + 0.001868 * Math.cos(g) - 0.032077 * Math.sin(g) - 0.014615 * Math.cos(2 * g) - 0.040849 * Math.sin(2 * g));
  const decl = 0.006918 - 0.399912 * Math.cos(g) + 0.070257 * Math.sin(g) - 0.006758 * Math.cos(2 * g) + 0.000907 * Math.sin(2 * g) - 0.002697 * Math.cos(3 * g) + 0.00148 * Math.sin(3 * g);
  const cosHa = (Math.cos(90.833 * rad) - Math.sin(lat * rad) * Math.sin(decl)) / (Math.cos(lat * rad) * Math.cos(decl));
  if (cosHa >= 1) return true; // polar night (never at this latitude)
  if (cosHa <= -1) return false;
  const ha = Math.acos(cosHa) / rad; // degrees
  const solarNoonUtcMin = 720 - 4 * lon - eqTime;
  const sunriseUtcMin = solarNoonUtcMin - 4 * ha;
  const sunsetUtcMin = solarNoonUtcMin + 4 * ha;
  const nowUtcMin = now.getUTCHours() * 60 + now.getUTCMinutes();
  return nowUtcMin < sunriseUtcMin || nowUtcMin > sunsetUtcMin;
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
