/**
 * The app store: one zustand store combined from slices (one file per area, so parallel work
 * on different areas does not collide):
 *   ui.ts        panels, toasts, context menu, links, translation
 *   settings.ts  settings.json (load / save), defaults
 *   map.ts       layers, view, tiles URLs, my location, map popovers and map tools
 *   route.ts     route planning, results, live navigation entry points
 *   data.ts      data status, updates, import / export
 *   saved.ts     favorites, line preview, history
 *   appUpdate.ts program updates
 * Every slice sees the whole store through `get()`.
 */
import { create } from "zustand";
import { createAppUpdateSlice } from "./appUpdate";
import { createDataSlice } from "./data";
import { createMapSlice } from "./map";
import { createRouteSlice } from "./route";
import { createSavedSlice } from "./saved";
import { createSettingsSlice } from "./settings";
import type { Store } from "./types";
import { createUiSlice } from "./ui";

export const useStore = create<Store>()((...a) => ({
  ...createUiSlice(...a),
  ...createSettingsSlice(...a),
  ...createMapSlice(...a),
  ...createRouteSlice(...a),
  ...createDataSlice(...a),
  ...createSavedSlice(...a),
  ...createAppUpdateSlice(...a),
}));

export type { Store, Panel, RoutePoint, ContextMenuState, Toast, MapLayers, HighlightState } from "./types";
export { DEFAULT_SETTINGS, avoidDefaults } from "./settings";
export { DEFAULT_LAYERS, loadSavedLayers, validateLayers, type IsochroneMode, type MapPopover, type CompareMode } from "./map";
export { findFavorite, findLineFavorite } from "./saved";
