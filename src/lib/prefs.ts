/**
 * Per-device preferences kept in localStorage (`mapsoffline.*`). Every read is validated and
 * every access is wrapped: localStorage throws in some private / locked-down profiles.
 * Account-level settings live in settings.json (Rust), not here.
 */

/** Every key the app stores, so a typo is a type error. */
export type PrefKey =
  | "mapsoffline.layers" // map layers chosen last time (JSON, see store/map.ts)
  | "mapsoffline.nearby" // "what's nearby" radius + category (JSON)
  | "mapsoffline.rememberView" // "1" | "0": mirror of settings.rememberMapView for the early read
  | "mapsoffline.lastView" // last map view {lon, lat, zoom} (JSON)
  | "mapsoffline.scrollMode"; // two-finger scroll: "pan" | "zoom"

export function getPrefString(key: PrefKey): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function setPrefString(key: PrefKey, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* private window etc. */
  }
}

/** A JSON preference; `validate` returns the typed value or undefined (→ `fallback`). */
export function getPrefJson<T>(key: PrefKey, validate: (v: unknown) => T | undefined, fallback: T): T {
  const raw = getPrefString(key);
  if (raw == null) return fallback;
  try {
    const v = validate(JSON.parse(raw));
    return v === undefined ? fallback : v;
  } catch {
    return fallback;
  }
}

export function setPrefJson(key: PrefKey, value: unknown): void {
  try {
    setPrefString(key, JSON.stringify(value));
  } catch {
    /* unserialisable: ignore */
  }
}

export function removePref(key: PrefKey): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* ignore */
  }
}

/** Every `mapsoffline.*` key (settings reset). */
export const ALL_PREF_KEYS: PrefKey[] = ["mapsoffline.layers", "mapsoffline.nearby", "mapsoffline.rememberView", "mapsoffline.lastView", "mapsoffline.scrollMode"];

// ---------------------------------------------------------------- typed accessors

export interface LastView {
  lon: number;
  lat: number;
  zoom: number;
}

export function getLastView(): LastView | null {
  return getPrefJson<LastView | null>(
    "mapsoffline.lastView",
    (v) => {
      const o = v as Partial<LastView> | null;
      return o && Number.isFinite(o.lon) && Number.isFinite(o.lat) && Number.isFinite(o.zoom) ? { lon: o.lon!, lat: o.lat!, zoom: o.zoom! } : undefined;
    },
    null,
  );
}

export function setLastView(v: LastView): void {
  setPrefJson("mapsoffline.lastView", v);
}

/** The map starts at the last view unless the user switched "remember the map view" off. */
export function getRememberView(): boolean {
  return getPrefString("mapsoffline.rememberView") !== "0";
}

export function setRememberView(on: boolean): void {
  setPrefString("mapsoffline.rememberView", on ? "1" : "0");
}

export interface NearbyPref {
  radius?: number;
  cat?: string;
}

export function getNearbyPref(): NearbyPref {
  return getPrefJson<NearbyPref>(
    "mapsoffline.nearby",
    (v) => {
      if (!v || typeof v !== "object") return undefined;
      const o = v as Record<string, unknown>;
      const out: NearbyPref = {};
      if (typeof o.radius === "number" && Number.isFinite(o.radius)) out.radius = o.radius;
      if (typeof o.cat === "string") out.cat = o.cat;
      return out;
    },
    {},
  );
}

export function setNearbyPref(p: Required<NearbyPref>): void {
  setPrefJson("mapsoffline.nearby", p);
}
