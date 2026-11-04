import { api, type Avoid, type Settings } from "../api";
import { setRememberView } from "../prefs";
import type { MapLayers, SliceCreator } from "./types";

export const DEFAULT_SETTINGS: Settings = {
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
  satelliteQuality: "high",
  satelliteSource: "govmap",
  rememberMapView: true,
  accent: "blue",
  avoidAreas: "ab",
  avoidMotorways: false,
  avoidFerries: false,
  unpaved: "avoidLong",
  avoidHardTurns: true,
  transitPreference: "fastest",
  usePrebuiltData: true,
  transitReliability: true,
  navVoice: true,
  coordFormat: "dd",
  colorScheme: "white",
  uiScale: "system",
  customLayers: [],
  wantedComponents: null,
  teachingTipsSeen: [],
};

/** Route-panel defaults derived from the settings. */
export function avoidDefaults(s: Settings): Avoid {
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

export interface SettingsSlice {
  settingsLoaded: boolean;
  settings: Settings;
  appVersion: string;
  loadSettings: () => Promise<void>;
  updateSettings: (patch: Partial<Settings>) => Promise<void>;
}

/** Bumped by every updateSettings: only the newest write's answer is applied (two quick
 *  toggles used to let the first answer overwrite the second change). Writes go out one after
 *  the other and each sends the newest settings, so the file never ends with an older state. */
let settingsSeq = 0;
let writeChain: Promise<unknown> = Promise.resolve();

export const createSettingsSlice: SliceCreator<SettingsSlice> = (set, get) => ({
  settingsLoaded: false,
  settings: DEFAULT_SETTINGS,
  appVersion: "",

  loadSettings: async () => {
    try {
      const s = await api.settingsGet();
      const merged = { ...DEFAULT_SETTINGS, ...s };
      setRememberView(merged.rememberMapView);
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
      // the window still opens with the defaults; say why the user's choices are missing
      set({ settingsLoaded: true });
      get().toastError(e);
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
    const seq = ++settingsSeq;
    set({ settings: next });
    if (patch.rememberMapView !== undefined) setRememberView(patch.rememberMapView);
    if (NAV_SETTING_KEYS.some((k) => patch[k] !== undefined)) {
      set({ avoid: { ...get().avoid, ...avoidDefaults(next) } });
      void get().computeRoute();
    }
    try {
      const write = writeChain.then(() => api.settingsSet(get().settings));
      writeChain = write.catch(() => {});
      const saved = await write;
      if (seq === settingsSeq) set({ settings: { ...DEFAULT_SETTINGS, ...saved } });
      if (patch.dataDir !== undefined) {
        await get().refreshDataStatus();
        await get().refreshTiles();
      }
    } catch (e) {
      get().toastError(e);
    }
  },
});
