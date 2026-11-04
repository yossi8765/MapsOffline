import type { Route } from "../lib/api";

/** A colour token from src/styles.css (the active theme and colour scheme), with a fallback. */
export function cssVar(name: string, fallback: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

/** Every colour the map overlays use, read from the CSS tokens, so the theme and the colour
 *  scheme reach the route, my location, stops and labels too. `dark` = the map is dark (the map
 *  flavour can differ from the panel theme). */
export function mapColors(dark: boolean): MapColors {
  // getComputedStyle is slow and this runs per route leg / stop: cache per theme state
  const root = typeof document !== "undefined" ? document.documentElement : null;
  const key = `${dark}|${root?.getAttribute("data-theme") ?? ""}|${root?.getAttribute("data-accent") ?? ""}|${root?.getAttribute("style") ?? ""}`;
  const hit = colorCache.get(key);
  if (hit) return hit;
  const c = readMapColors(dark);
  if (colorCache.size > 8) colorCache.clear();
  colorCache.set(key, c);
  return c;
}

const colorCache = new Map<string, MapColors>();

function readMapColors(dark: boolean) {
  const n = dark ? "-night" : "";
  return {
    me: cssVar("--map-me", "#1a66d9"),
    casing: cssVar(`--map-casing${n}`, dark ? "#0b1220" : "#ffffff"),
    halo: cssVar(`--map-halo${n}`, dark ? "#0b1220" : "#ffffff"),
    label: cssVar(`--map-label${n}`, dark ? "#e2e8f0" : "#1e293b"),
    alt: cssVar(`--map-alt${n}`, dark ? "#94a3b8" : "#64748b"),
    stop: cssVar(`--map-stop${n}`, dark ? "#5c7a94" : "#4a6276"),
    stopLabel: cssVar(`--map-stop-label${n}`, dark ? "#c3d0dc" : "#3b4f60"),
    poiCard: cssVar(`--map-poi-card${n}`, dark ? "#1e293b" : "#ffffff"),
    highlight: cssVar("--map-highlight", "#f59e0b"),
    rail: cssVar("--map-rail", "#7c3aed"),
    lightRail: cssVar("--map-light-rail", "#dc2626"),
    bus: cssVar("--map-bus", "#0891b2"),
    walk: cssVar("--map-walk", "#64748b"),
  };
}

export type MapColors = ReturnType<typeof readMapColors>;

export function modeColor(mode: string): string {
  return cssVar(mode === "foot" ? "--route-foot" : mode === "bike" ? "--route-bike" : "--route-car", "#1a66d9");
}

/** Colour of one leg of a route: the line's own colour, the vehicle's, walking grey or the mode's. */
export function legColor(route: Route, legIdx: number): string {
  const leg = route.legs[legIdx];
  if (leg?.transit) {
    const c = leg.transit.color;
    if (c) return c.startsWith("#") ? c : `#${c}`;
    const mc = mapColors(false);
    return leg.transit.vehicle === "rail" ? mc.rail : leg.transit.vehicle === "light_rail" ? mc.lightRail : mc.bus;
  }
  if (leg?.kind === "walk" && route.mode === "transit") return mapColors(false).walk;
  return modeColor(route.mode);
}

/** "#rrggbb" for a GTFS colour with or without the "#". */
export function hexColor(c: string): string {
  return c.startsWith("#") ? c : `#${c}`;
}
