import { layers, namedFlavor, type Flavor } from "@protomaps/basemaps";
import type { ExpressionSpecification, LayerSpecification, StyleSpecification } from "maplibre-gl";

export const ISRAEL_BBOX: [number, number, number, number] = [34.2, 29.45, 35.95, 33.35];
export const MAX_BOUNDS: [[number, number], [number, number]] = [
  [ISRAEL_BBOX[0] - 1.5, ISRAEL_BBOX[1] - 1.0],
  [ISRAEL_BBOX[2] + 1.5, ISRAEL_BBOX[3] + 1.0],
];
export const DEFAULT_CENTER: [number, number] = [34.78, 32.08];
export const DEFAULT_ZOOM = 8;

export type FlavorName = "light" | "dark" | "grayscale";

export interface StyleOptions {
  tilesUrl: string | null;
  /** Planet-wide online build (through the range proxy) used when `tilesUrl` is null. */
  onlineTilesUrl?: string | null;
  /** Raster PMTiles with Sentinel-2 imagery; shown under roads and labels when `satellite` is on. */
  satelliteUrl: string | null;
  satellite: boolean;
  /** Installed imagery is the govmap orthophoto (else Sentinel-2): picks the attribution. */
  satelliteOrtho?: boolean;
  /** Raster-DEM PMTiles (Terrarium); hillshade and/or 3D terrain when installed. */
  terrainUrl: string | null;
  /** Minimal world basemap (zoom 0–6) drawn under the Israel map for context outside it. */
  worldUrl?: string | null;
  hillshade: boolean;
  terrain3d: boolean;
  /** Road-number shields (default on). */
  roadShields?: boolean;
  flavor: FlavorName;
  lang: string;
  poi: boolean;
  buildings3d: boolean;
  /** Thematic look: cycleways or rail / transit brought forward, the rest of the roads muted. */
  theme?: MapTheme;
}

export type MapTheme = "standard" | "cycle" | "transit";

const CYCLE_COLOR = "#16a34a";
const RAIL_COLOR = "#334155";

/** Restyle the Protomaps layer list for the cycling / public-transport map. */
function applyTheme(ls: LayerSpecification[], theme: MapTheme, dark: boolean): LayerSpecification[] {
  if (theme === "standard") return ls;
  const muted = new Set(["roads_minor", "roads_minor_service", "roads_major", "roads_highway", "roads_link", "roads_bridges_minor", "roads_bridges_major", "roads_bridges_highway", "roads_bridges_link", "roads_tunnels_minor", "roads_tunnels_major", "roads_tunnels_highway", "roads_tunnels_link"]);
  const out: LayerSpecification[] = [];
  for (const l of ls) {
    if (l.type === "line" && muted.has(l.id)) {
      out.push({ ...l, paint: { ...l.paint, "line-opacity": 0.55 } } as LayerSpecification);
      continue;
    }
    if (theme === "cycle" && l.type === "line" && (l.id === "roads_other" || l.id === "roads_bridges_other" || l.id === "roads_tunnels_other")) {
      // footways stay thin and quiet; cycleways get a white-cased green band on top
      out.push(l);
      // the stock filters use the legacy filter syntax, so the added test must too
      const isCycle = ["==", "kind_detail", "cycleway"] as unknown as ExpressionSpecification;
      const base = (l as { filter?: ExpressionSpecification }).filter;
      const filter = (base ? ["all", base, isCycle] : isCycle) as ExpressionSpecification;
      out.push({
        id: `${l.id}-cycle-casing`, type: "line", source: l.source, "source-layer": l["source-layer"], filter,
        minzoom: 11,
        layout: { "line-cap": "round", "line-join": "round" },
        paint: { "line-color": dark ? "#0b1220" : "#ffffff", "line-width": ["interpolate", ["exponential", 1.5], ["zoom"], 11, 2, 14, 4, 18, 10] },
      } as LayerSpecification);
      out.push({
        id: `${l.id}-cycle`, type: "line", source: l.source, "source-layer": l["source-layer"], filter,
        minzoom: 11,
        layout: { "line-cap": "round", "line-join": "round" },
        paint: { "line-color": CYCLE_COLOR, "line-width": ["interpolate", ["exponential", 1.5], ["zoom"], 11, 1, 14, 2.5, 18, 6] },
      } as LayerSpecification);
      continue;
    }
    if (theme === "transit" && l.type === "line" && l.id === "roads_rail") {
      // classic railway: dark band with a light dash inside
      const railWidth: ExpressionSpecification = ["interpolate", ["exponential", 1.5], ["zoom"], 6, 1, 12, 2.5, 16, 5, 19, 8];
      out.push({ ...l, paint: { "line-color": dark ? "#cbd5e1" : RAIL_COLOR, "line-width": railWidth } } as LayerSpecification);
      out.push({
        ...l, id: "roads_rail-dash",
        paint: { "line-color": dark ? "#1e293b" : "#ffffff", "line-width": ["interpolate", ["exponential", 1.5], ["zoom"], 6, 0.3, 12, 1, 16, 2, 19, 3.5], "line-dasharray": [3, 3] },
      } as LayerSpecification);
      continue;
    }
    out.push(l);
  }
  return out;
}

function origin(): string {
  return window.location.origin;
}

export const WORLD_MAX_ZOOM = 6;
export const TERRAIN_MAX_ZOOM = 12;
const TERRAIN_ATTRIBUTION = 'Elevation: <a href="https://registry.opendata.aws/terrain-tiles/">Mapzen terrain tiles</a>';
const SENTINEL_ATTRIBUTION =
  '<a href="https://s2maps.eu">Sentinel-2 cloudless</a> by <a href="https://eox.at">EOX</a> (CC BY-NC-SA 4.0), contains modified Copernicus Sentinel data';

export const ORTHOPHOTO_ATTRIBUTION =
  'תצלומי אוויר © <a href="https://www.govmap.gov.il">המרכז למיפוי ישראל</a> (לשימוש אישי) · ' + SENTINEL_ATTRIBUTION;

function satelliteLayer(): LayerSpecification {
  return { id: "satellite", type: "raster", source: "satellite", paint: { "raster-resampling": "linear", "raster-fade-duration": 150 } };
}

/** Template of the merged local map source (Israel + downloaded countries), served by the backend. */
export const MERGED_TILES_PREFIX = "http://mo-map.localhost/";
export function isMergedTiles(url: string | null | undefined): boolean {
  return !!url && url.startsWith(MERGED_TILES_PREFIX);
}

export function flavorFor(name: FlavorName): Flavor {
  const f = namedFlavor(name);
  // design: the stock light-flavor cyan sea is loud next to the neutral UI; use a calmer blue.
  return name === "light" ? { ...f, water: "#aacbe0" } : f;
}

/** Build the full MapLibre style. Without tiles the map is only a background. */
/** Protomaps sets sea, lake, river and island names in "Noto Sans Italic", whose glyph ranges
 *  have no Hebrew or Arabic: MapLibre then draws nothing (no "הים התיכון" at zoom 8). Use the
 *  regular face, which covers every script we ship. */
function withoutItalic(ls: LayerSpecification[]): LayerSpecification[] {
  return ls.map((l) => {
    if (l.type !== "symbol" || !l.layout) return l;
    const font = l.layout["text-font"];
    if (!font || !JSON.stringify(font).includes("Italic")) return l;
    const fixed = JSON.parse(JSON.stringify(font).replace(/Noto Sans Italic/g, "Noto Sans Regular"));
    return { ...l, layout: { ...l.layout, "text-font": fixed } } as LayerSpecification;
  });
}

export function buildStyle(o: StyleOptions): StyleSpecification {
  const flavor = flavorFor(o.flavor);
  const base: StyleSpecification = {
    version: 8,
    glyphs: `${origin()}/fonts/{fontstack}/{range}.pbf`,
    sprite: `${origin()}/sprites/v4/${o.flavor === "dark" ? "dark" : "light"}`,
    sources: {},
    layers: [{ id: "background", type: "background", paint: { "background-color": flavor.background } }],
  };
  // World basemap: its own source and a copy of the layer list, under the Israel layers; its
  // labels stop at zoom 7 so they never duplicate the detailed map's labels.
  let worldLayers: LayerSpecification[] = [];
  if (o.worldUrl) {
    base.sources.world = { type: "vector", url: `pmtiles://${o.worldUrl}`, maxzoom: WORLD_MAX_ZOOM, attribution: "" };
    worldLayers = withoutItalic(layers("world", flavor, { lang: o.lang }))
      .filter((l) => l.id !== "background" && l.id !== "pois")
      .map((l) => {
        const c = { ...l, id: `world-${l.id}` } as LayerSpecification;
        if (c.type === "symbol") c.maxzoom = 7;
        return c;
      });
  }
  const sat = o.satellite && !!o.satelliteUrl;
  if (sat) {
    const live = o.satelliteUrl!.startsWith("http://mo-sat.localhost/");
    base.sources.satellite = { type: "raster", ...(live ? { tiles: [o.satelliteUrl!], minzoom: 0, maxzoom: 19 } : { url: `pmtiles://${o.satelliteUrl}` }), tileSize: 256, attribution: o.satelliteOrtho ? ORTHOPHOTO_ATTRIBUTION : SENTINEL_ATTRIBUTION };
  }
  const dem = !!o.terrainUrl && (o.hillshade || o.terrain3d);
  if (dem) {
    base.sources.dem = { type: "raster-dem", url: `pmtiles://${o.terrainUrl}`, tileSize: 256, encoding: "terrarium", maxzoom: TERRAIN_MAX_ZOOM, attribution: TERRAIN_ATTRIBUTION };
    if (o.terrain3d) base.terrain = { source: "dem", exaggeration: 1.2 };
  }
  const hillshadeLayer = (): LayerSpecification => ({
    id: "hillshade",
    type: "hillshade",
    source: "dem",
    paint: {
      "hillshade-exaggeration": o.satellite ? 0.25 : 0.4,
      "hillshade-shadow-color": o.flavor === "dark" ? "#000000" : "#4a3f2f",
      "hillshade-highlight-color": o.flavor === "dark" ? "#334155" : "#ffffff",
      "hillshade-accent-color": o.flavor === "dark" ? "#0b1220" : "#8a7a60",
      "hillshade-illumination-direction": 335,
    },
  });
  const tilesUrl = o.tilesUrl ?? o.onlineTilesUrl ?? null;
  if (!tilesUrl) {
    base.layers.push(...worldLayers);
    if (sat) base.layers.push(satelliteLayer());
    if (dem && o.hillshade) base.layers.push(hillshadeLayer());
    return base;
  }

  // Several countries installed: the backend merges every local archive into one tile URL.
  const merged = isMergedTiles(tilesUrl);
  base.sources.protomaps = {
    type: "vector",
    ...(merged ? { tiles: [tilesUrl], minzoom: 0, maxzoom: 15 } : { url: `pmtiles://${tilesUrl}` }),
    attribution: '<a href="https://protomaps.com">Protomaps</a> © <a href="https://openstreetmap.org">OpenStreetMap</a>',
  };
  let ls: LayerSpecification[] = applyTheme(withoutItalic(layers("protomaps", flavor, { lang: o.lang })), o.theme ?? "standard", o.flavor === "dark");
  if (!o.poi) ls = ls.filter((l) => l.id !== "pois");
  // Road numbers: the stock shields are tiny (8 px) and sparse; make them readable at every
  // zoom and place them before the street names so they win collisions.
  // Israeli colour code by road class: 1-digit national roads blue, 2-digit regional red,
  // 3-digit local green, longer numbers grey. The shield images are drawn on demand by
  // MapView (`styleimagemissing`), see `shieldImage()`.
  // The current Protomaps builds carry the number in `ref` (no `shield_text`), and a road can
  // list several refs separated by ";": the first one is the shield.
  if (o.roadShields === false) ls = ls.filter((l) => l.id !== "roads_shields");
  const shields = ls.find((l) => l.id === "roads_shields");
  if (shields && shields.type === "symbol") {
    shields.minzoom = 7;
    const refText: ExpressionSpecification = ["coalesce", ["get", "shield_text"], ["get", "ref"], ""];
    const n: ExpressionSpecification = ["length", refText];
    // (no split expression in the style spec: roads with several numbers "1;20" get no shield)
    shields.filter = ["all", ["in", ["get", "kind"], ["literal", ["highway", "major_road"]]], [">", n, 0], ["<=", n, 5], ["==", ["index-of", ";", refText], -1]];
    shields.layout = {
      ...shields.layout,
      "icon-image": ["concat", "shield-", ["case", ["<=", n, 1], "blue", ["<=", n, 2], "red", ["<=", n, 3], "green", "grey"], "-", n],
      "text-field": refText,
      "text-font": ["Noto Sans Medium"],
      "text-size": ["interpolate", ["linear"], ["zoom"], 7, 10, 11, 12, 15, 14],
      "icon-size": ["interpolate", ["linear"], ["zoom"], 7, 0.83, 11, 1, 15, 1.17],
      "text-offset": [0, 0.05],
      "symbol-spacing": 160,
      "text-padding": 1,
      "icon-allow-overlap": false,
    };
    shields.paint = { "text-color": "#ffffff", "text-halo-width": 0 };
  }
  if (o.buildings3d) {
    const idx = ls.findIndex((l) => l.id === "buildings");
    const b3d: LayerSpecification = {
      id: "buildings-3d",
      type: "fill-extrusion",
      source: "protomaps",
      "source-layer": "buildings",
      minzoom: 14,
      paint: {
        "fill-extrusion-color": flavor.buildings,
        "fill-extrusion-height": ["coalesce", ["get", "height"], 8],
        "fill-extrusion-base": ["coalesce", ["get", "min_height"], 0],
        "fill-extrusion-opacity": 0.85,
      },
    };
    if (idx >= 0) {
      ls.splice(idx + 1, 0, b3d);
    } else {
      ls.push(b3d);
    }
  }
  if (sat) {
    // Imagery replaces every area fill (land, water, landuse, buildings); roads, boundaries and
    // labels stay on top. 3D buildings are kept when the user asked for them.
    ls = ls.filter((l) => l.type !== "fill" && (l.type !== "fill-extrusion" || o.buildings3d));
    // Over a photo, footways / sidewalks / paths and service roads drawn as wide solid white
    // bands hide what they run through: draw them as thin dashed lines instead (the photo shows
    // the actual pavement). Real streets keep their normal look.
    const THIN = new Set(["roads_other", "roads_bridges_other", "roads_tunnels_other", "roads_minor_service", "roads_pier"]);
    const DROP = new Set(["roads_tunnels_other_casing", "roads_bridges_other_casing", "roads_minor_service_casing"]);
    ls = ls
      .filter((l) => !DROP.has(l.id))
      .map((l) => {
        if (!THIN.has(l.id) || l.type !== "line") return l;
        const service = l.id === "roads_minor_service";
        return {
          ...l,
          paint: {
            "line-color": "#ffffff",
            "line-opacity": 0.85,
            "line-width": ["interpolate", ["exponential", 1.4], ["zoom"], 14, 0.6, 17, 1.4, 20, 2.6],
            "line-dasharray": service ? [4, 2] : [2, 2],
          },
        } as LayerSpecification;
      });
    const bgIdx = ls.findIndex((l) => l.id === "background");
    ls.splice(bgIdx + 1, 0, satelliteLayer());
  }
  if (dem && o.hillshade) {
    // relief shading over the ground fills (earth / landuse / water), under roads and labels
    const idx = ls.findIndex((l) => l.type === "line" || l.type === "symbol");
    ls.splice(idx < 0 ? ls.length : idx, 0, hillshadeLayer());
  }
  // Protomaps' layer list already starts with its own "background" layer. Outside the extracted
  // bbox there are no water polygons, so paint the background as sea instead of bare land.
  const bg = ls.find((l) => l.id === "background");
  if (bg && bg.type === "background") {
    bg.paint = { ...(bg.paint ?? {}), "background-color": flavor.water };
    base.layers = [bg, ...worldLayers, ...ls.filter((l) => l !== bg)];
  } else {
    base.layers = [base.layers[0], ...worldLayers, ...ls];
  }
  return base;
}

/** First symbol layer id – route layers are inserted below labels. */
export function firstSymbolLayer(style: StyleSpecification): string | undefined {
  // the world basemap sits under everything and has its own labels: skip them, or overlays
  // inserted "below the labels" would end up under the detailed map and the imagery
  return style.layers.find((l) => l.type === "symbol" && !l.id.startsWith("world-"))?.id;
}
