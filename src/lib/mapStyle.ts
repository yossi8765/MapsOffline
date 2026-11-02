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
  /** Raster PMTiles with Sentinel-2 imagery; shown under roads and labels when `satellite` is on. */
  satelliteUrl: string | null;
  satellite: boolean;
  /** Raster-DEM PMTiles (Terrarium); hillshade and/or 3D terrain when installed. */
  terrainUrl: string | null;
  hillshade: boolean;
  terrain3d: boolean;
  flavor: FlavorName;
  lang: string;
  poi: boolean;
  buildings3d: boolean;
}

function origin(): string {
  return window.location.origin;
}

export const SATELLITE_MAX_ZOOM = 14;
export const TERRAIN_MAX_ZOOM = 12;
const TERRAIN_ATTRIBUTION = 'Elevation: <a href="https://registry.opendata.aws/terrain-tiles/">Mapzen terrain tiles</a>';
const SATELLITE_ATTRIBUTION =
  '<a href="https://s2maps.eu">Sentinel-2 cloudless</a> by <a href="https://eox.at">EOX</a> (CC BY-NC-SA 4.0), contains modified Copernicus Sentinel data';

function satelliteLayer(): LayerSpecification {
  return { id: "satellite", type: "raster", source: "satellite", paint: { "raster-resampling": "linear", "raster-fade-duration": 150 } };
}

export function flavorFor(name: FlavorName): Flavor {
  const f = namedFlavor(name);
  // design: the stock light-flavor cyan sea is loud next to the neutral UI; use a calmer blue.
  return name === "light" ? { ...f, water: "#aacbe0" } : f;
}

/** Build the full MapLibre style. Without tiles the map is only a background. */
export function buildStyle(o: StyleOptions): StyleSpecification {
  const flavor = flavorFor(o.flavor);
  const base: StyleSpecification = {
    version: 8,
    glyphs: `${origin()}/fonts/{fontstack}/{range}.pbf`,
    sprite: `${origin()}/sprites/v4/${o.flavor === "dark" ? "dark" : "light"}`,
    sources: {},
    layers: [{ id: "background", type: "background", paint: { "background-color": flavor.background } }],
  };
  const sat = o.satellite && !!o.satelliteUrl;
  if (sat) {
    base.sources.satellite = { type: "raster", url: `pmtiles://${o.satelliteUrl}`, tileSize: 256, maxzoom: SATELLITE_MAX_ZOOM, attribution: SATELLITE_ATTRIBUTION };
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
  if (!o.tilesUrl) {
    if (sat) base.layers.push(satelliteLayer());
    if (dem && o.hillshade) base.layers.push(hillshadeLayer());
    return base;
  }

  base.sources.protomaps = {
    type: "vector",
    url: `pmtiles://${o.tilesUrl}`,
    attribution: '<a href="https://protomaps.com">Protomaps</a> © <a href="https://openstreetmap.org">OpenStreetMap</a>',
  };
  let ls: LayerSpecification[] = layers("protomaps", flavor, { lang: o.lang });
  if (!o.poi) ls = ls.filter((l) => l.id !== "pois");
  // Road numbers: the stock shields are tiny (8 px) and sparse; make them readable at every
  // zoom and place them before the street names so they win collisions.
  // Israeli colour code by road class: 1-digit national roads blue, 2-digit regional red,
  // 3-digit local green, longer numbers grey. The shield images are drawn on demand by
  // MapView (`styleimagemissing`), see `shieldImage()`.
  const shields = ls.find((l) => l.id === "roads_shields");
  if (shields && shields.type === "symbol") {
    shields.minzoom = 7;
    const n: ExpressionSpecification = ["length", ["get", "shield_text"]];
    shields.layout = {
      ...shields.layout,
      "icon-image": ["concat", "shield-", ["case", ["<=", n, 1], "blue", ["<=", n, 2], "red", ["<=", n, 3], "green", "grey"], "-", n],
      "text-field": ["get", "shield_text"],
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
    base.layers = ls;
  } else {
    base.layers = [base.layers[0], ...ls];
  }
  return base;
}

/** First symbol layer id – route layers are inserted below labels. */
export function firstSymbolLayer(style: StyleSpecification): string | undefined {
  return style.layers.find((l) => l.type === "symbol")?.id;
}
