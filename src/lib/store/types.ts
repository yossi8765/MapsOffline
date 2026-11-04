import type { StateCreator } from "zustand";
import type { GeoResult } from "../api";
import type { UiSlice } from "./ui";
import type { SettingsSlice } from "./settings";
import type { MapSlice } from "./map";
import type { RouteSlice } from "./route";
import type { DataSlice } from "./data";
import type { SavedSlice } from "./saved";
import type { AppUpdateSlice } from "./appUpdate";

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
  /** Developer text for errors: shown folded with a copy button. */
  details?: string;
  /** How many identical toasts were merged into this one. */
  count?: number;
  /** One action button ("בטל" after a delete). */
  action?: { label: string; run: () => void };
  /** ms; 0 = stays until closed (errors). */
  duration?: number;
}

export interface ToastOptions {
  details?: string;
  action?: Toast["action"];
  duration?: number;
}

/** What the share dialog shows: a place or the selected route. */
export type ShareTarget = { kind: "place"; lon: number; lat: number; label: string } | { kind: "route"; routeId: string };

export interface MapLayers {
  satellite: boolean;
  /** Over the satellite imagery: the base map's roads, paths, rail and road numbers (default on). */
  satelliteRoads: boolean;
  /** Over the satellite imagery: the base map's names and labels (default on). */
  satelliteLabels: boolean;
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
  /** Thematic map: standard, cycling, public transport, topographic (hiking). */
  theme: "standard" | "cycle" | "transit" | "topo";
  /** Contour lines from the elevation data (always on in the topographic theme). */
  contours: boolean;
  /** Marked hiking / cycling trails (always on in the topographic theme; needs trails data). */
  trails: boolean;
}

export interface HighlightState {
  routeId: string;
  legIndex: number;
  stepIndex?: number;
  /** The highlighted stretch spans this many steps from `stepIndex` (a merged roundabout entry +
   *  exit counts 2). Default 1. */
  stepCount?: number;
  /** One point on the route (elevation-profile hover): drawn as a dot. */
  point?: [number, number];
}

/** The whole store: one zustand store built from the slices in this folder. */
export type Store = UiSlice & SettingsSlice & MapSlice & RouteSlice & DataSlice & SavedSlice & AppUpdateSlice;

/** A slice creator with access to the whole store through `get()`. */
export type SliceCreator<T> = StateCreator<Store, [], [], T>;
