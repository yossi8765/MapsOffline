import { convertFileSrc } from "@tauri-apps/api/core";
import type { Feature, FeatureCollection } from "geojson";
import { api, apiX, errorCode, type DataStatus, type GeoResult, type IsochroneBand, type LonLat, type MyLocation, type Route, type Settings, type TrackUpdate, type UserLayer } from "../api";
import { categoryOf, POI_CATEGORIES } from "../poiCategories";
import { getNearbyPref, getPrefJson, setNearbyPref, setPrefJson } from "../prefs";
import type { MapLayers, SliceCreator } from "./types";
import { dataTag } from "../dataTag";

export { dataTag };

/** Isochrone ("how far can I get") travel modes. */
export type IsochroneMode = "car" | "foot" | "bike" | "wheelchair" | "transit";
export type MapPopover = "layers" | "nearby" | "categories" | "userLayers" | "customLayers" | null;
export type CompareMode = "off" | "split" | "swipe";
export type DrawMode = "point" | "line" | "polygon" | "text";

/** Distance / area measuring on the map (MeasureBar + map/overlays/tools). */
export interface MeasureState {
  points: LonLat[];
  /** close the line into an area */
  area: boolean;
}

/** Drawing on the map (DrawToolbar). `purpose: "avoid"` = one polygon for route avoid.polygons. */
export interface DrawState {
  mode: DrawMode;
  color: string;
  purpose: "draw" | "avoid";
  /** finished features */
  features: Feature[];
  /** vertices of the feature being drawn */
  current: LonLat[];
  /** text placed by a click in "text" mode */
  text: string;
}

export interface IsochroneState {
  at: LonLat;
  mode: IsochroneMode;
  minutes: number[];
  bands: IsochroneBand[] | null;
  loading: boolean;
  error: string | null;
}

export interface TrackState {
  recording: boolean;
  starting: boolean;
  update: TrackUpdate | null;
}

export interface MapSlice {
  layers: MapLayers;
  transitStopsUnavailable: boolean;
  /** Map layers popover (opened from a map button; components/LayersPopover.tsx). Mirrors
   *  `mapPopover === "layers"` (kept for older callers). */
  layersOpen: boolean;
  /** The one map popover open next to the map buttons (report ב-12). */
  mapPopover: MapPopover;
  /** Bumped by the "N" shortcut: MapView turns the map north-up and flat (bearing 0, pitch 0). */
  northUpTick: number;
  /** "What's nearby" list (context menu): places within `radius` m of a point, nearest first,
   *  optionally of one picker category (`cat`, POI_CATEGORIES id). */
  nearby: { lon: number; lat: number; items: GeoResult[]; loading: boolean; radius: number; cat: string } | null;

  mapCenter: LonLat;
  mapZoom: number;
  flyTo: { lonlat: LonLat; zoom?: number; tick: number } | null;
  /** Fit the map to a box (user layer, imported file). */
  fitBox: { box: [number, number, number, number]; tick: number } | null;
  marker: { lonlat: LonLat; label: string } | null;

  tilesUrl: string | null;
  satelliteUrl: string | null;
  terrainUrl: string | null;
  worldUrl: string | null;
  /** Planet-wide Protomaps build read through the app's range proxy (no local map, online). */
  onlineTilesUrl: string | null;
  /** The tiles path has been queried at least once (the "no map" card waits for it). */
  tilesChecked: boolean;
  tilesTick: number;

  /** Last fix of "my location" (map dot + search option). */
  myLocation: (MyLocation & { at: number }) | null;
  locating: boolean;

  // ---- map tools
  measure: MeasureState | null;
  drawing: DrawState | null;
  isochrone: IsochroneState | null;
  importOpen: boolean;
  userLayers: UserLayer[];
  /** bumped when a user layer's features changed (the overlay reloads them) */
  userLayersTick: number;
  track: TrackState;
  compare: CompareMode;

  setLayers: (patch: Partial<MapLayers>) => void;
  setTransitStopsUnavailable: (v: boolean) => void;
  setLayersOpen: (open: boolean) => void;
  toggleLayers: () => void;
  showNearby: (lon: number, lat: number, opts?: { radius?: number; cat?: string }) => Promise<void>;
  closeNearby: () => void;
  setMapView: (center: LonLat, zoom: number) => void;
  flyToPoint: (lonlat: LonLat, zoom?: number) => void;
  fitToBox: (box: [number, number, number, number]) => void;
  setMarker: (m: { lonlat: LonLat; label: string } | null) => void;
  refreshTiles: () => Promise<void>;
  /** Ask the OS for the position (system consent dialog on first use); flies there when `fly`. */
  locate: (fly?: boolean) => Promise<MyLocation | null>;

  // ---- map tools (IMPROVEMENTS stream F2). Signatures shared with the other streams.
  startMeasure: (at?: LonLat) => void;
  addMeasurePoint: (p: LonLat) => void;
  undoMeasurePoint: () => void;
  setMeasureArea: (area: boolean) => void;
  stopMeasure: () => void;
  /** Drawing tools; `purpose: "avoid"` draws one area the route then avoids. */
  startDrawing: (purpose?: "draw" | "avoid") => void;
  setDrawMode: (m: DrawMode) => void;
  setDrawColor: (c: string) => void;
  setDrawText: (text: string) => void;
  /** A map click while drawing: a vertex, a point, or (text mode) the current text there. */
  addDrawPoint: (p: LonLat, text?: string) => void;
  undoDrawPoint: () => void;
  /** Close the feature being drawn (line / polygon). */
  finishDrawFeature: () => void;
  cancelDrawing: () => void;
  /** Save the drawing as a user layer (kind "drawing"); an avoid-area becomes route avoid.polygons. */
  saveDrawing: (name: string) => Promise<void>;
  showIsochrone: (at: LonLat, mode: IsochroneMode) => void;
  setIsochroneMinutes: (minutes: number[]) => void;
  clearIsochrone: () => void;
  openImport: () => void;
  closeImport: () => void;
  /** Parse and save files as user layers (GPX / KML / GeoJSON); returns the layers added. */
  importFiles: (files: { name: string; text: string }[]) => Promise<UserLayer[]>;
  loadUserLayers: () => Promise<void>;
  updateUserLayer: (id: string, patch: { name?: string; color?: string; visible?: boolean }) => Promise<void>;
  deleteUserLayer: (id: string) => Promise<void>;
  /** Route along the first line of a user layer (map matching) and show it as the route result. */
  routeAlongLayer: (id: string, mode: "car" | "foot" | "bike") => Promise<void>;
  /** Put a ready route (map-matched track) into the route results. */
  showRouteResult: (route: Route) => void;
  toggleTrackRecording: () => void;
  setTrackUpdate: (u: TrackUpdate | null) => void;
  setCompare: (mode: CompareMode) => void;
  openMapWindow: () => void;
  setMapPopover: (p: MapPopover) => void;
}

export const DEFAULT_LAYERS: MapLayers = { satellite: false, hillshade: false, roadShields: true, terrain3d: false, poiCats: [], buildings3d: false, poi: true, transitStops: false, flavor: "auto", labelLang: "he", theme: "standard", contours: false, trails: false };

/** Saved layers checked key by key against the defaults (type and array-ness); undefined when
 *  the value is not an object at all. */
export function validateLayers(saved: unknown): MapLayers | undefined {
  if (!saved || typeof saved !== "object") return undefined;
  const out: MapLayers = { ...DEFAULT_LAYERS };
  const rec = saved as Record<string, unknown>;
  for (const k of Object.keys(DEFAULT_LAYERS) as (keyof MapLayers)[]) {
    const v = rec[k];
    if (v !== undefined && typeof v === typeof DEFAULT_LAYERS[k] && Array.isArray(v) === Array.isArray(DEFAULT_LAYERS[k])) (out as unknown as Record<string, unknown>)[k] = v;
  }
  return out;
}

/** The map layers chosen last time. Read synchronously so the map is created with them (a later
 *  change would rebuild the style). poi / transitStops / labelLang live in settings.json too and
 *  are overwritten from there by loadSettings. */
export function loadSavedLayers(): MapLayers {
  return getPrefJson<MapLayers>("mapsoffline.layers", validateLayers, DEFAULT_LAYERS);
}

const compTag = (ds: DataStatus | null, c: keyof Pick<DataStatus, "tiles" | "satellite" | "terrain" | "world">) => {
  const s = ds?.[c];
  return s ? `${s.updatedIso ?? ""}${s.version ?? ""}${s.sizeBytes}` : "";
};

/** Last isochrone request (a newer one or clear drops the older answer). */
let isoSeq = 0;
let trackUnlisten: (() => void) | null = null;

export const DRAW_COLORS = ["#d6332b", "#1d5bbf", "#178246", "#f08a00", "#7b3fb5", "#1f2328"];

/** Draw state features as one FeatureCollection (the one being drawn included when drawable). */
export function drawingCollection(d: DrawState): FeatureCollection {
  return { type: "FeatureCollection", features: d.features };
}

export const createMapSlice: SliceCreator<MapSlice> = (set, get) => ({
  layers: loadSavedLayers(),
  transitStopsUnavailable: false,
  layersOpen: false,
  mapPopover: null,
  northUpTick: 0,
  nearby: null,

  mapCenter: [34.78, 32.08],
  mapZoom: 8,
  flyTo: null,
  fitBox: null,
  marker: null,

  tilesUrl: null,
  satelliteUrl: null,
  terrainUrl: null,
  worldUrl: null,
  onlineTilesUrl: null,
  tilesChecked: false,
  tilesTick: 0,

  myLocation: null,
  locating: false,

  measure: null,
  drawing: null,
  isochrone: null,
  importOpen: false,
  userLayers: [],
  userLayersTick: 0,
  track: { recording: false, starting: false, update: null },
  compare: "off",

  setLayers: (patch) => {
    const layers = { ...get().layers, ...patch };
    set({ layers });
    setPrefJson("mapsoffline.layers", layers);
    const s = get().settings;
    const settingsPatch: Partial<Settings> = {};
    if (patch.poi !== undefined && patch.poi !== s.showPoiLayer) settingsPatch.showPoiLayer = patch.poi;
    if (patch.transitStops !== undefined && patch.transitStops !== s.showTransitLayer) settingsPatch.showTransitLayer = patch.transitStops;
    if (patch.labelLang !== undefined && patch.labelLang !== s.mapLabelLanguage) settingsPatch.mapLabelLanguage = patch.labelLang;
    if (Object.keys(settingsPatch).length) void get().updateSettings(settingsPatch);
  },
  setTransitStopsUnavailable: (transitStopsUnavailable) => {
    if (get().transitStopsUnavailable !== transitStopsUnavailable) set({ transitStopsUnavailable });
  },
  setLayersOpen: (open) => get().setMapPopover(open ? "layers" : get().mapPopover === "layers" ? null : get().mapPopover),
  // the map popovers share the spot next to the map buttons: one at a time
  toggleLayers: () => get().setMapPopover(get().mapPopover === "layers" ? null : "layers"),
  setMapPopover: (p) => set((s) => ({ mapPopover: p, layersOpen: p === "layers", nearby: p === "nearby" ? s.nearby : null })),
  showNearby: async (lon, lat, opts) => {
    // radius and category are remembered for the next time (per user, localStorage)
    const saved = getNearbyPref();
    const radius = opts?.radius ?? saved.radius ?? 600;
    const cat = opts?.cat ?? saved.cat ?? "";
    setNearbyPref({ radius, cat });
    set((s) => ({ nearby: { lon, lat, items: s.nearby?.lon === lon && s.nearby?.lat === lat ? s.nearby.items : [], loading: true, radius, cat }, layersOpen: false, mapPopover: "nearby" }));
    const dLat = radius / 111_320;
    const dLon = radius / (111_320 * Math.cos((lat * Math.PI) / 180));
    const picked = POI_CATEGORIES.find((c) => c.id === cat);
    const cats = picked ? picked.cats : [...new Set(POI_CATEGORIES.flatMap((c) => c.cats))];
    try {
      const res = await api.poiLayer(cats, [lon - dLon, lat - dLat, lon + dLon, lat + dLat], 1000);
      const dist = (r: GeoResult) => Math.hypot((r.lon - lon) * Math.cos((lat * Math.PI) / 180), r.lat - lat);
      const items = res
        .filter((r) => r.name && dist(r) * 111_320 <= radius && (!picked || categoryOf(r.category ?? "", r.name, [picked])))
        .sort((a, b) => dist(a) - dist(b))
        .slice(0, 60);
      const cur = get().nearby;
      if (cur?.lon === lon && cur?.lat === lat && cur.radius === radius && cur.cat === cat) set({ nearby: { lon, lat, items, loading: false, radius, cat } });
    } catch (e) {
      set({ nearby: null, mapPopover: get().mapPopover === "nearby" ? null : get().mapPopover });
      get().toastError(e);
    }
  },
  closeNearby: () => set((s) => ({ nearby: null, mapPopover: s.mapPopover === "nearby" ? null : s.mapPopover })),
  setMapView: (mapCenter, mapZoom) => set({ mapCenter, mapZoom }),
  flyToPoint: (lonlat, zoom) => set((s) => ({ flyTo: { lonlat, zoom, tick: (s.flyTo?.tick ?? 0) + 1 } })),
  fitToBox: (box) => set((s) => ({ fitBox: { box, tick: (s.fitBox?.tick ?? 0) + 1 } })),
  setMarker: (marker) => set({ marker }),

  refreshTiles: async () => {
    try {
      const [p, sp, tp, wp, merged, ds, regions] = await Promise.all([
        api.tilesPath(),
        api.satellitePath().catch(() => null),
        api.terrainPath().catch(() => null),
        api.worldPath().catch(() => null),
        api.mapTilesAvailable().catch(() => false),
        api.dataStatus().catch(() => null),
        api.regionsInstalled().catch(() => []),
      ]);
      const online = get().online;
      // other countries installed: one merged source over every local map (see map_tiles.rs)
      const mapTag = dataTag(compTag(ds, "tiles"), ...regions.map((r) => `${r.id}${r.updatedIso ?? ""}${r.busy}`));
      const url = merged ? `http://mo-map.localhost/{z}/{x}/{y}.mvt?v=${mapTag}` : p ? `${convertFileSrc(p)}?v=${dataTag(compTag(ds, "tiles"))}` : null;
      // satellite layer through the backend: the downloaded archive, tiles seen before, or live
      // orthophoto when online (see online_sat.rs)
      const satelliteUrl = sp || online ? `http://mo-sat.localhost/{z}/{x}/{y}?v=${dataTag(compTag(ds, "satellite"), get().settings.satelliteSource, sp ? 1 : 0)}` : null;
      const terrainUrl = tp ? `${convertFileSrc(tp)}?v=${dataTag(compTag(ds, "terrain"))}` : null;
      const worldUrl = wp ? `${convertFileSrc(wp)}?v=${dataTag(compTag(ds, "world"))}` : null;
      const s = get();
      const changed = s.tilesUrl !== url || s.satelliteUrl !== satelliteUrl || s.terrainUrl !== terrainUrl || s.worldUrl !== worldUrl;
      set((st) => ({ tilesUrl: url, satelliteUrl, terrainUrl, worldUrl, tilesChecked: true, tilesTick: changed ? st.tilesTick + 1 : st.tilesTick }));
      if (!url && online) {
        // no local map: show the live Protomaps build until the user downloads one
        try {
          const key = await api.tilesOnlineKey();
          set({ onlineTilesUrl: `http://mo-tiles.localhost/${key}` });
        } catch (e) {
          console.warn("online map unavailable", e);
          set({ onlineTilesUrl: null });
        }
      } else if (url) {
        set({ onlineTilesUrl: null });
      }
    } catch (e) {
      console.warn("tiles_path failed", e);
      set({ tilesUrl: null, satelliteUrl: null, terrainUrl: null, worldUrl: null, tilesChecked: true });
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
      get().toast("error", errorCode(e) === "LOCATION_DENIED" ? t("err_LOCATION_DENIED") : t("err_LOCATION_UNAVAILABLE"));
      return null;
    } finally {
      set({ locating: false });
    }
  },

  // ---------------------------------------------------------------- measure
  startMeasure: (at) => set({ measure: { points: at ? [at] : [], area: false }, drawing: null, contextMenu: null }),
  addMeasurePoint: (p) => set((s) => (s.measure ? { measure: { ...s.measure, points: [...s.measure.points, p] } } : {})),
  undoMeasurePoint: () => set((s) => (s.measure ? { measure: { ...s.measure, points: s.measure.points.slice(0, -1) } } : {})),
  setMeasureArea: (area) => set((s) => (s.measure ? { measure: { ...s.measure, area } } : {})),
  stopMeasure: () => set({ measure: null }),

  // ---------------------------------------------------------------- drawing
  startDrawing: (purpose = "draw") =>
    set({ drawing: { mode: purpose === "avoid" ? "polygon" : "line", color: purpose === "avoid" ? "#c93636" : DRAW_COLORS[0], purpose, features: [], current: [], text: "" }, measure: null, contextMenu: null }),
  setDrawText: (text) => set((s) => (s.drawing ? { drawing: { ...s.drawing, text } } : {})),
  setDrawMode: (mode) => set((s) => (s.drawing ? { drawing: { ...s.drawing, mode, current: [] } } : {})),
  setDrawColor: (color) => set((s) => (s.drawing ? { drawing: { ...s.drawing, color } } : {})),
  addDrawPoint: (p, text) =>
    set((s) => {
      const d = s.drawing;
      if (!d) return {};
      if (d.mode === "point" || d.mode === "text") {
        text = d.mode === "text" ? (text ?? d.text).trim() : undefined;
        if (d.mode === "text" && !text) return {};
        const f: Feature = { type: "Feature", properties: { color: d.color, ...(text ? { text } : {}) }, geometry: { type: "Point", coordinates: p } };
        return { drawing: { ...d, features: [...d.features, f] } };
      }
      return { drawing: { ...d, current: [...d.current, p] } };
    }),
  undoDrawPoint: () =>
    set((s) => {
      const d = s.drawing;
      if (!d) return {};
      if (d.current.length) return { drawing: { ...d, current: d.current.slice(0, -1) } };
      return { drawing: { ...d, features: d.features.slice(0, -1) } };
    }),
  finishDrawFeature: () =>
    set((s) => {
      const d = s.drawing;
      if (!d) return {};
      const c = d.current;
      if (d.mode === "line" && c.length >= 2) {
        return { drawing: { ...d, current: [], features: [...d.features, { type: "Feature", properties: { color: d.color }, geometry: { type: "LineString", coordinates: c } }] } };
      }
      if (d.mode === "polygon" && c.length >= 3) {
        return { drawing: { ...d, current: [], features: [...d.features, { type: "Feature", properties: { color: d.color }, geometry: { type: "Polygon", coordinates: [[...c, c[0]]] } }] } };
      }
      return { drawing: { ...d, current: [] } };
    }),
  cancelDrawing: () => set({ drawing: null }),
  saveDrawing: async (name) => {
    get().finishDrawFeature();
    const d = get().drawing;
    if (!d) return;
    if (d.purpose === "avoid") {
      const rings = d.features.flatMap((f) => (f.geometry.type === "Polygon" ? [f.geometry.coordinates[0] as LonLat[]] : []));
      set({ drawing: null });
      if (!rings.length) return;
      const prev = get().avoid.polygons ?? [];
      get().setAvoid({ polygons: [...prev, ...rings] });
      get().toast("success", get().t("drawAvoidAdded"));
      return;
    }
    if (!d.features.length) {
      set({ drawing: null });
      return;
    }
    try {
      const layer = await apiX.userLayerSave(name || get().t("drawDefaultName"), "drawing", JSON.stringify(drawingCollection(d)), d.color);
      set((s) => ({ drawing: null, userLayers: [...s.userLayers.filter((l) => l.id !== layer.id), layer], userLayersTick: s.userLayersTick + 1 }));
      get().toast("success", get().t("drawSaved"));
    } catch (e) {
      get().toastError(e);
    }
  },

  // ---------------------------------------------------------------- isochrone
  showIsochrone: (at, mode) => {
    const minutes = get().isochrone?.minutes ?? [10, 20, 30];
    const seq = ++isoSeq;
    set({ isochrone: { at, mode, minutes, bands: null, loading: true, error: null }, contextMenu: null });
    const t = get().time;
    const p =
      mode === "transit"
        ? apiX.transitIsochrone(at[0], at[1], t.kind === "now" ? null : (t.iso ?? null), minutes)
        : apiX.isochrone(at[0], at[1], mode, minutes);
    p.then(
      (bands) => {
        if (seq === isoSeq) set((s) => (s.isochrone ? { isochrone: { ...s.isochrone, bands, loading: false } } : {}));
      },
      (e) => {
        if (seq !== isoSeq) return;
        set((s) => (s.isochrone ? { isochrone: { ...s.isochrone, loading: false, error: get().t("isoFailed") } } : {}));
        get().toastError(e);
      },
    );
  },
  setIsochroneMinutes: (minutes) => {
    const iso = get().isochrone;
    set({ isochrone: iso ? { ...iso, minutes } : null });
    if (iso) get().showIsochrone(iso.at, iso.mode);
  },
  clearIsochrone: () => {
    isoSeq++;
    set({ isochrone: null });
  },

  // ---------------------------------------------------------------- import / user layers
  openImport: () => set({ importOpen: true, contextMenu: null }),
  closeImport: () => set({ importOpen: false }),
  importFiles: async (files) => {
    const { parseGeoFile } = await import("../../map/geoFiles");
    const added: UserLayer[] = [];
    for (const f of files) {
      try {
        const fc = parseGeoFile(f.name, f.text);
        if (!fc.features.length) {
          get().toast("error", `${get().t("importEmpty")}: ${f.name}`);
          continue;
        }
        const name = f.name.replace(/\.(gpx|kml|geojson|json)$/i, "");
        const layer = await apiX.userLayerSave(name, "import", JSON.stringify(fc), DRAW_COLORS[(get().userLayers.length + added.length) % DRAW_COLORS.length]);
        added.push(layer);
      } catch (e) {
        if (e instanceof Error && e.message.startsWith("PARSE:")) get().toast("error", `${get().t("importBadFile")}: ${f.name}`);
        else get().toastError(e);
      }
    }
    if (added.length) {
      set((s) => ({ userLayers: [...s.userLayers.filter((l) => !added.some((a) => a.id === l.id)), ...added], userLayersTick: s.userLayersTick + 1, importOpen: false, mapPopover: "userLayers" }));
      const b = added[added.length - 1].bbox;
      if (b) get().fitToBox(b);
      get().toast("success", `${get().t("importLayerDone")}: ${added.map((a) => a.name).join(", ")}`);
    }
    return added;
  },
  loadUserLayers: async () => {
    try {
      const userLayers = await apiX.userLayersList();
      set((s) => ({ userLayers, userLayersTick: s.userLayersTick + 1 }));
    } catch (e) {
      // older backend without user layers: nothing to show
      console.warn("user layers", e);
    }
  },
  updateUserLayer: async (id, patch) => {
    // optimistic: the switch moves at once
    set((s) => ({ userLayers: s.userLayers.map((l) => (l.id === id ? { ...l, ...patch } : l)) }));
    try {
      const l = await apiX.userLayerUpdate(id, patch);
      set((s) => ({ userLayers: s.userLayers.map((x) => (x.id === id ? l : x)) }));
    } catch (e) {
      get().toastError(e);
      void get().loadUserLayers();
    }
  },
  deleteUserLayer: async (id) => {
    try {
      await apiX.userLayerDelete(id);
      set((s) => ({ userLayers: s.userLayers.filter((l) => l.id !== id), userLayersTick: s.userLayersTick + 1 }));
    } catch (e) {
      get().toastError(e);
    }
  },
  routeAlongLayer: async (id, mode) => {
    try {
      const text = await apiX.userLayerGet(id);
      const { firstLine } = await import("../../map/geoFiles");
      const line = firstLine(JSON.parse(text) as FeatureCollection);
      if (!line || line.length < 2) {
        get().toast("error", get().t("importNoLine"));
        return;
      }
      set({ routeLoading: true });
      const route = await apiX.matchTrack(line, mode);
      get().showRouteResult(route);
    } catch (e) {
      set({ routeLoading: false });
      get().toastError(e);
    }
  },
  showRouteResult: (route) => {
    const g = route.geometry;
    if (!g.length) return;
    const lbl = (p: LonLat) => `${p[1].toFixed(5)}, ${p[0].toFixed(5)}`;
    set((s) => ({
      waypoints: [
        { lon: g[0][0], lat: g[0][1], label: lbl(g[0]) },
        { lon: g[g.length - 1][0], lat: g[g.length - 1][1], label: lbl(g[g.length - 1]) },
      ],
      mode: route.mode,
      routes: [route],
      transitAll: [],
      selectedRouteId: route.id,
      detailsOpen: true,
      routeLoading: false,
      routeError: null,
      routeSource: "offline",
      routeProvider: null,
      lastRequest: null,
      highlight: null,
      routeTick: s.routeTick + 1,
      fitRouteTick: s.fitRouteTick + 1,
      panel: "route",
      sidebarCollapsed: false,
    }));
  },

  // ---------------------------------------------------------------- track recording
  toggleTrackRecording: () => {
    const tr = get().track;
    if (tr.starting) return;
    if (tr.recording) {
      set({ track: { ...tr, starting: true } });
      const name = `${get().t("trackDefaultName")} ${new Date().toLocaleString(get().settings.language === "he" ? "he-IL" : "en-GB", { dateStyle: "short", timeStyle: "short" })}`;
      apiX
        .trackStop(true, name)
        .then((layer) => {
          if (layer) {
            set((s) => ({ userLayers: [...s.userLayers.filter((l) => l.id !== layer.id), layer], userLayersTick: s.userLayersTick + 1 }));
            get().toast("success", `${get().t("trackSaved")}: ${layer.name}`);
          }
        })
        .catch((e) => get().toastError(e))
        .finally(() => {
          trackUnlisten?.();
          trackUnlisten = null;
          set({ track: { recording: false, starting: false, update: null } });
        });
      return;
    }
    set({ track: { recording: false, starting: true, update: null } });
    void (async () => {
      try {
        trackUnlisten?.();
        const un = await apiX.onTrackUpdate((u) => get().setTrackUpdate(u));
        trackUnlisten = un;
        await apiX.trackStart();
        set({ track: { recording: true, starting: false, update: null } });
      } catch (e) {
        trackUnlisten?.();
        trackUnlisten = null;
        set({ track: { recording: false, starting: false, update: null } });
        get().toastError(e);
      }
    })();
  },
  setTrackUpdate: (update) => set((s) => ({ track: { ...s.track, update } })),

  // ---------------------------------------------------------------- compare / window
  setCompare: (compare) => set({ compare }),
  openMapWindow: () => {
    void (async () => {
      try {
        const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
        const existing = await WebviewWindow.getByLabel("map");
        if (existing) {
          await existing.setFocus();
          return;
        }
        const w = new WebviewWindow("map", { url: "index.html?window=map", title: get().t("mapWindowTitle"), width: 1200, height: 800 });
        w.once("tauri://error", (e) => {
          console.warn("map window", e);
          get().toast("error", get().t("mapWindowFailed"));
        });
      } catch (e) {
        get().toastError(e);
      }
    })();
  },
});
