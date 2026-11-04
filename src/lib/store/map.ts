import { convertFileSrc } from "@tauri-apps/api/core";
import { api, errorCode, type GeoResult, type LonLat, type MyLocation, type Settings } from "../api";
import { categoryOf, POI_CATEGORIES } from "../poiCategories";
import { getNearbyPref, getPrefJson, setNearbyPref, setPrefJson } from "../prefs";
import type { MapLayers, SliceCreator } from "./types";

/** Isochrone ("how far can I get") travel modes. */
export type IsochroneMode = "car" | "foot" | "bike" | "wheelchair" | "transit";
export type MapPopover = "layers" | "nearby" | "categories" | null;
export type CompareMode = "off" | "split" | "swipe";

export interface MapSlice {
  layers: MapLayers;
  transitStopsUnavailable: boolean;
  /** Map layers popover (opened from a map button; components/LayersPopover.tsx). */
  layersOpen: boolean;
  /** Bumped by the "N" shortcut: MapView turns the map north-up and flat (bearing 0, pitch 0). */
  northUpTick: number;
  /** "What's nearby" list (context menu): places within `radius` m of a point, nearest first,
   *  optionally of one picker category (`cat`, POI_CATEGORIES id). */
  nearby: { lon: number; lat: number; items: GeoResult[]; loading: boolean; radius: number; cat: string } | null;

  mapCenter: LonLat;
  mapZoom: number;
  flyTo: { lonlat: LonLat; zoom?: number; tick: number } | null;
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

  setLayers: (patch: Partial<MapLayers>) => void;
  setTransitStopsUnavailable: (v: boolean) => void;
  setLayersOpen: (open: boolean) => void;
  toggleLayers: () => void;
  showNearby: (lon: number, lat: number, opts?: { radius?: number; cat?: string }) => Promise<void>;
  closeNearby: () => void;
  setMapView: (center: LonLat, zoom: number) => void;
  flyToPoint: (lonlat: LonLat, zoom?: number) => void;
  setMarker: (m: { lonlat: LonLat; label: string } | null) => void;
  refreshTiles: () => Promise<void>;
  /** Ask the OS for the position (system consent dialog on first use); flies there when `fly`. */
  locate: (fly?: boolean) => Promise<MyLocation | null>;

  // ---- map tools (IMPROVEMENTS wave 1, stream F2). Declared here so other streams can call
  // them; until F2 implements them they do nothing.
  startMeasure: (at?: LonLat) => void;
  startDrawing: () => void;
  showIsochrone: (at: LonLat, mode: IsochroneMode) => void;
  clearIsochrone: () => void;
  openImport: () => void;
  toggleTrackRecording: () => void;
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

const noop = () => {};

export const createMapSlice: SliceCreator<MapSlice> = (set, get) => ({
  layers: loadSavedLayers(),
  transitStopsUnavailable: false,
  layersOpen: false,
  northUpTick: 0,
  nearby: null,

  mapCenter: [34.78, 32.08],
  mapZoom: 8,
  flyTo: null,
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
  setLayersOpen: (layersOpen) => set({ layersOpen }),
  // the layers popover and the nearby list share the spot next to the map buttons
  toggleLayers: () => set((s) => ({ layersOpen: !s.layersOpen, nearby: null })),
  showNearby: async (lon, lat, opts) => {
    // radius and category are remembered for the next time (per user, localStorage)
    const saved = getNearbyPref();
    const radius = opts?.radius ?? saved.radius ?? 600;
    const cat = opts?.cat ?? saved.cat ?? "";
    setNearbyPref({ radius, cat });
    set((s) => ({ nearby: { lon, lat, items: s.nearby?.lon === lon && s.nearby?.lat === lat ? s.nearby.items : [], loading: true, radius, cat }, layersOpen: false }));
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
      set({ nearby: null });
      get().toastError(e);
    }
  },
  closeNearby: () => set({ nearby: null }),
  setMapView: (mapCenter, mapZoom) => set({ mapCenter, mapZoom }),
  flyToPoint: (lonlat, zoom) => set((s) => ({ flyTo: { lonlat, zoom, tick: (s.flyTo?.tick ?? 0) + 1 } })),
  setMarker: (marker) => set({ marker }),

  refreshTiles: async () => {
    try {
      const [p, sp, tp, wp, merged] = await Promise.all([api.tilesPath(), api.satellitePath().catch(() => null), api.terrainPath().catch(() => null), api.worldPath().catch(() => null), api.mapTilesAvailable().catch(() => false)]);
      // A fresh query string per refresh: the pmtiles protocol caches archive headers and
      // directories per URL, so a replaced file must get a new URL or stale tiles keep showing.
      const bust = `?v=${Date.now()}`;
      // other countries installed: one merged source over every local map (see map_tiles.rs)
      const url = merged ? `http://mo-map.localhost/{z}/{x}/{y}.mvt${bust}` : p ? convertFileSrc(p) + bust : null;
      set((s) => ({
        tilesUrl: url,
        // satellite layer through the backend: the downloaded archive, tiles seen before, or live
        // orthophoto when online (see online_sat.rs)
        satelliteUrl: sp || navigator.onLine ? `http://mo-sat.localhost/{z}/{x}/{y}${bust}` : null,
        terrainUrl: tp ? convertFileSrc(tp) + bust : null,
        worldUrl: wp ? convertFileSrc(wp) + bust : null,
        tilesChecked: true,
        tilesTick: s.tilesTick + 1,
      }));
      if (!url && navigator.onLine) {
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

  startMeasure: noop,
  startDrawing: noop,
  showIsochrone: noop,
  clearIsochrone: noop,
  openImport: noop,
  toggleTrackRecording: noop,
  setCompare: noop,
  openMapWindow: noop,
  setMapPopover: noop,
});
