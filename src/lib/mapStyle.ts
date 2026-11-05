import { layers, namedFlavor, type Flavor } from "@protomaps/basemaps";
import type { ExpressionSpecification, LayerSpecification, StyleSpecification } from "maplibre-gl";
import type { CustomLayer } from "./api";

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
  /** With the imagery on: draw the base map's roads / paths / rail / road numbers over it (default true). */
  satelliteRoads?: boolean;
  /** With the imagery on: draw the base map's names and labels over it (default true). */
  satelliteLabels?: boolean;
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
  /** Thematic look: cycleways or rail / transit brought forward, the rest of the roads muted;
   *  "topo" = hiking map (relief, contour lines and marked trails). */
  theme?: MapTheme;
  /** Contour vector tiles (lib/contours.ts) – drawn when `contours` is on or in the topo theme. */
  contourTiles?: string | null;
  contours?: boolean;
  /** Marked trails (GeoJSON written by the data update) – drawn when `trails` or topo. */
  trailsUrl?: string | null;
  trails?: boolean;
  /** Online elevation (AWS Terrain Tiles) when no DEM is installed and the internet may be used. */
  onlineTerrain?: boolean;
  /** Online trail tiles (Waymarked Trails) when the trails file is not installed and online is allowed. */
  onlineTrails?: boolean;
  /** User tile layers (Settings.customLayers), served by the backend at mo-user.localhost. */
  customLayers?: CustomLayer[];
}

export type MapTheme = "standard" | "cycle" | "transit" | "topo";

/** osmc:symbol colour names of Israeli marked trails → the paint colour (Israel Trail = orange). */
const TRAIL_COLORS: Record<string, string> = {
  red: "#d6332b",
  blue: "#1d5bbf",
  green: "#178246",
  black: "#1f2328",
  orange: "#f08a00",
  purple: "#7b3fb5",
  yellow: "#d9a400",
  brown: "#8a5a2b",
  white: "#8f96a3",
};

/** Contour lines (minor / major) with metre labels on the major ones. */
function contourLayers(dark: boolean, lang: string): LayerSpecification[] {
  const ink = dark ? "#b9a27c" : "#8a6a3d";
  const unit = lang === "he" ? " מ׳" : " m";
  return [
    {
      id: "contours",
      type: "line",
      source: "contours",
      "source-layer": "contours",
      minzoom: 10,
      paint: {
        "line-color": ink,
        "line-opacity": ["case", [">", ["get", "level"], 0], 0.55, 0.3],
        "line-width": ["case", [">", ["get", "level"], 0], 1, 0.5],
      },
    },
    {
      id: "contours-label",
      type: "symbol",
      source: "contours",
      "source-layer": "contours",
      minzoom: 12,
      filter: [">", ["get", "level"], 0],
      layout: {
        "symbol-placement": "line",
        "text-field": ["concat", ["number-format", ["get", "ele"], {}], unit],
        "text-font": ["Noto Sans Regular"],
        "text-size": 10,
        "symbol-spacing": 320,
      },
      paint: { "text-color": ink, "text-halo-color": dark ? "#111214" : "#ffffff", "text-halo-width": 1.2 },
    },
  ];
}

/** Trail networks drawn already at zoom 9–11 (national / regional); every trail from zoom 11.
 *  Below that the ~2,000 local trails covered the hills like scribbles (report א-13). */
export const TRAILS_LONG_NETWORKS = ["nwn", "ncn", "iwn", "icn", "rwn", "rcn"];
export const TRAILS_MINZOOM = 11;

/** Marked trails in their sign colour: white casing, dashed for cycling routes, name in colour. */
export function trailLayers(): LayerSpecification[] {
  const color: ExpressionSpecification = ["match", ["get", "colour"], ...Object.entries(TRAIL_COLORS).flat(), "#d6332b"] as unknown as ExpressionSpecification;
  const bike: ExpressionSpecification = ["in", ["get", "route"], ["literal", ["bicycle", "mtb"]]];
  const long: ExpressionSpecification = ["any", ["in", ["get", "network"], ["literal", TRAILS_LONG_NETWORKS]], ["in", "שביל ישראל", ["to-string", ["get", "name"]]]];
  const width = (a: number, b: number, c: number): ExpressionSpecification => ["interpolate", ["linear"], ["zoom"], 9, a, 14, b, 17, c];
  return [
    {
      id: "trails-long",
      type: "line",
      source: "trails",
      minzoom: 9,
      maxzoom: TRAILS_MINZOOM,
      filter: long,
      layout: { "line-cap": "round", "line-join": "round" },
      paint: { "line-color": color, "line-width": width(1.2, 2.5, 4), "line-opacity": 0.5 },
    },
    {
      id: "trails-casing",
      type: "line",
      source: "trails",
      minzoom: TRAILS_MINZOOM,
      layout: { "line-cap": "round", "line-join": "round" },
      paint: { "line-color": "#ffffff", "line-opacity": 0.85, "line-width": width(2.5, 5, 7) },
    },
    {
      id: "trails",
      type: "line",
      source: "trails",
      minzoom: TRAILS_MINZOOM,
      filter: ["!", bike],
      layout: { "line-cap": "round", "line-join": "round" },
      paint: { "line-color": color, "line-width": width(1.2, 2.5, 4) },
    },
    {
      id: "trails-bike",
      type: "line",
      source: "trails",
      minzoom: TRAILS_MINZOOM,
      filter: bike,
      paint: { "line-color": color, "line-width": width(1.2, 2.5, 4), "line-dasharray": [2, 1.5] },
    },
    {
      id: "trails-label",
      type: "symbol",
      source: "trails",
      minzoom: 12,
      filter: ["has", "name"],
      layout: { "symbol-placement": "line", "text-field": ["get", "name"], "text-font": ["Noto Sans Medium"], "text-size": 11, "symbol-spacing": 400 },
      paint: { "text-color": color, "text-halo-color": "#ffffff", "text-halo-width": 1.5 },
    },
  ];
}

const CYCLE_COLOR = "#16a34a";
const RAIL_COLOR = "#334155";

/** `c` blended toward `bg` (t = share of `c`); hex / rgb() colours, anything else unchanged. */
function blend(c: string, bg: string, t: number): string {
  const parse = (x: string): number[] | null => {
    const h = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(x.trim());
    if (h) {
      const v = h[1].length === 3 ? [...h[1]].map((d) => d + d).join("") : h[1];
      return [0, 2, 4].map((i) => parseInt(v.slice(i, i + 2), 16));
    }
    const r = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/i.exec(x.trim());
    return r ? [Number(r[1]), Number(r[2]), Number(r[3])] : null;
  };
  const a = parse(c);
  const b = parse(bg);
  if (!a || !b) return c;
  return "#" + a.map((v, i) => Math.round(v * t + b[i] * (1 - t)).toString(16).padStart(2, "0")).join("");
}

/** Every colour literal inside a paint value (plain or a zoom expression) blended toward `bg`. */
function blendPaint(v: unknown, bg: string, t: number): unknown {
  if (typeof v === "string") return blend(v, bg, t);
  if (Array.isArray(v)) return v.map((x) => blendPaint(x, bg, t));
  return v;
}

/** Restyle the Protomaps layer list for the cycling / public-transport map. */
function applyTheme(ls: LayerSpecification[], theme: MapTheme, dark: boolean, earth: string): LayerSpecification[] {
  if (theme === "standard") return ls;
  const muted = new Set(["roads_minor", "roads_minor_service", "roads_major", "roads_highway", "roads_link", "roads_bridges_minor", "roads_bridges_major", "roads_bridges_highway", "roads_bridges_link", "roads_tunnels_minor", "roads_tunnels_major", "roads_tunnels_highway", "roads_tunnels_link"]);
  const out: LayerSpecification[] = [];
  for (const l of ls) {
    if (l.type === "line" && muted.has(l.id)) {
      // quieter roads as a solid colour nearer the land: a translucent line shows every
      // segment overlap at junctions and ramps as a brighter seam
      const paint = l.paint as Record<string, unknown> | undefined;
      out.push({ ...l, paint: { ...paint, "line-color": blendPaint(paint?.["line-color"], earth, 0.55) } } as LayerSpecification);
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

/** Public AWS Terrain Tiles (Terrarium PNG, 256 px): the online stand-in for the elevation
 *  component (hillshade, 3D terrain, contours) when it is not installed. */
export const ONLINE_TERRAIN_URL = "https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png";
export const ONLINE_TERRAIN_MAX_ZOOM = 15;
const ONLINE_TERRAIN_ATTRIBUTION = "Terrain Tiles: Mapzen / AWS Open Data (various sources incl. SRTM)";
/** Waymarked Trails raster overlay: the online stand-in for the marked-trails component. */
export function onlineTrailsUrl(theme?: MapTheme): string {
  return `https://tile.waymarkedtrails.org/${theme === "cycle" ? "cycling" : "hiking"}/{z}/{x}/{y}.png`;
}
const ONLINE_TRAILS_ATTRIBUTION = '© <a href="https://waymarkedtrails.org">waymarkedtrails.org</a>, <a href="https://openstreetmap.org">OpenStreetMap</a>';
/** The previous DEM (256 px PNG), still in use until the terrain data is updated. */
const OLD_TERRAIN_ATTRIBUTION = 'Elevation: <a href="https://registry.opendata.aws/terrain-tiles/">Mapzen terrain tiles</a>';
const TERRAIN_ATTRIBUTION = 'Elevation: <a href="https://mapterhorn.com">Mapterhorn</a> – 10 m DEM of Israel (ASTER GDEM, METI/NASA, processed by 4cast), Copernicus GLO-30 (© DLR/Airbus, provided under COPERNICUS by the EU and ESA)';
const SENTINEL_ATTRIBUTION =
  '<a href="https://s2maps.eu">Sentinel-2 cloudless</a> by <a href="https://eox.at">EOX</a> (CC BY-NC-SA 4.0), contains modified Copernicus Sentinel data';

export const ORTHOPHOTO_ATTRIBUTION =
  'תצלומי אוויר © <a href="https://www.govmap.gov.il">המרכז למיפוי ישראל</a> (לשימוש אישי) · ' + SENTINEL_ATTRIBUTION;

/** Road-network layers of the base map: road lines (incl. bridges / tunnels / casings), rail
 *  and the road-number shields. */
export function isRoadLayer(l: LayerSpecification): boolean {
  const id = l.id.replace(/^world-/, "");
  if (id === "roads_shields" || id === "roads_oneway") return true;
  return l.type === "line" && (id.startsWith("roads") || id.startsWith("physical_line"));
}

/** Base-map text and point labels (place, street, water, POI names); road shields count as roads. */
export function isLabelLayer(l: LayerSpecification): boolean {
  return l.type === "symbol" && !isRoadLayer(l);
}

function hiddenOverImagery(l: LayerSpecification, o: StyleOptions): boolean {
  return (o.satelliteRoads === false && isRoadLayer(l)) || (o.satelliteLabels === false && isLabelLayer(l));
}

function withVisibility(l: LayerSpecification, v: "visible" | "none"): LayerSpecification {
  if (l.type === "background") return l;
  return { ...l, layout: { ...((l as { layout?: object }).layout ?? {}), visibility: v } } as LayerSpecification;
}

function satelliteLayer(): LayerSpecification {
  return { id: "satellite", type: "raster", source: "satellite", paint: { "raster-resampling": "linear", "raster-fade-duration": 150 } };
}

/** Template of the merged local map source (Israel + downloaded countries), served by the backend. */
export const MERGED_TILES_PREFIX = "http://mo-map.localhost/";
export function isMergedTiles(url: string | null | undefined): boolean {
  return !!url && url.startsWith(MERGED_TILES_PREFIX);
}

/** Dark-flavour road colours with more contrast against the land (#1f1f1f) and the buildings: the
 *  stock minor roads (#333–#3d3d3d) were hardly visible at street zoom (report א-15). */
export const DARK_ROADS = {
  other: "#41444a",
  minor_service: "#43464c",
  minor_a: "#50535a",
  minor_b: "#474a50",
  link: "#50535a",
  major: "#555860",
  highway: "#62656d",
  bridges_other: "#41444a",
  bridges_minor: "#474a50",
  bridges_link: "#50535a",
  bridges_major: "#555860",
  bridges_highway: "#62656d",
  buildings: "#18191b",
} as const;

export function flavorFor(name: FlavorName): Flavor {
  const f = namedFlavor(name);
  // design: the stock light-flavor cyan sea is loud next to the neutral UI; use a calmer blue.
  if (name === "light") return { ...f, water: "#aacbe0" };
  if (name === "dark") return { ...f, ...DARK_ROADS };
  return f;
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

const HEBREW_RANGE = ["U+0590-05FF", "U+FB1D-FB4F"];

export function buildStyle(o: StyleOptions): StyleSpecification {
  const flavor = flavorFor(o.flavor);
  const base: StyleSpecification = {
    version: 8,
    glyphs: `${origin()}/fonts/{fontstack}/{range}.pbf`,
    // Hebrew is drawn from real OpenType files (MapLibre 6 font-faces: correct niqqud and
    // final forms); every other script falls back to the PBF glyph ranges above. Arabic stays on
    // PBF on purpose: font-faces rasterises one grapheme cluster at a time, which would lose the
    // joining between letters.
    "font-faces": {
      "Noto Sans Regular": [{ url: `${origin()}/fonts/NotoSansHebrew-Regular.ttf`, "unicode-range": HEBREW_RANGE }],
      "Noto Sans Medium": [{ url: `${origin()}/fonts/NotoSansHebrew-Medium.ttf`, "unicode-range": HEBREW_RANGE }],
    },
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
  const topo = o.theme === "topo";
  const hillshade = o.hillshade || topo;
  const demOnline = !o.terrainUrl && !!o.onlineTerrain;
  const dem = (!!o.terrainUrl || demOnline) && (hillshade || o.terrain3d);
  const demAttribution = demOnline ? ONLINE_TERRAIN_ATTRIBUTION : o.terrainUrl?.includes("mapterhorn") ? TERRAIN_ATTRIBUTION : OLD_TERRAIN_ATTRIBUTION;
  if (dem) {
    base.sources.dem = demOnline
      ? { type: "raster-dem", tiles: [ONLINE_TERRAIN_URL], tileSize: 256, encoding: "terrarium", maxzoom: ONLINE_TERRAIN_MAX_ZOOM, attribution: demAttribution }
      : { type: "raster-dem", url: `pmtiles://${o.terrainUrl}`, tileSize: o.terrainUrl!.includes("mapterhorn") ? 512 : 256, encoding: "terrarium", maxzoom: TERRAIN_MAX_ZOOM, attribution: demAttribution };
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
    if (dem && hillshade) base.layers.push(hillshadeLayer());
    base.layers.push(...customRasterLayers((o.customLayers ?? []).filter((c) => c.visible), base));
    return base;
  }

  // Several countries installed: the backend merges every local archive into one tile URL.
  const merged = isMergedTiles(tilesUrl);
  base.sources.protomaps = {
    type: "vector",
    ...(merged ? { tiles: [tilesUrl], minzoom: 0, maxzoom: 15 } : { url: `pmtiles://${tilesUrl}` }),
    attribution: '<a href="https://protomaps.com">Protomaps</a> © <a href="https://openstreetmap.org">OpenStreetMap</a>',
  };
  let ls: LayerSpecification[] = applyTheme(withoutItalic(layers("protomaps", flavor, { lang: o.lang })), o.theme ?? "standard", o.flavor === "dark", flavor.earth);
  if (!o.poi) ls = ls.filter((l) => l.id !== "pois");
  // House numbers: the basemap draws them at 12 px from zoom 18 only, hard to make out – larger,
  // medium weight, a wider halo, and from zoom 17
  const addr = ls.find((l) => l.id === "address_label");
  if (addr && addr.type === "symbol") {
    addr.minzoom = 17;
    addr.layout = {
      ...addr.layout,
      "text-font": ["Noto Sans Medium"],
      "text-size": ["interpolate", ["linear"], ["zoom"], 17, 12, 18, 14, 20, 17],
      "text-padding": 1,
    };
    addr.paint = { ...addr.paint, "text-halo-width": 1.6 };
  }
  // Road numbers as Israeli signs (as in hebrew-maps): freeways blue, 1-2 digit roads red,
  // 3 digit green, 4 digit white with a black frame; a rounded badge with the inner white frame
  // of the real signs, the number drawn into the image. MapView's missing-image resolver draws
  // `shield:<kind>:<number>` on demand (shieldImage()). A road that carries several numbers
  // ("1;20") shows the first one.
  if (o.roadShields === false) ls = ls.filter((l) => l.id !== "roads_shields");
  const shields = ls.find((l) => l.id === "roads_shields");
  if (shields && shields.type === "symbol") {
    const raw: ExpressionSpecification = ["to-string", ["coalesce", ["get", "shield_text"], ["get", "ref"], ""]];
    const semi: ExpressionSpecification = ["index-of", ";", raw];
    const ref: ExpressionSpecification = ["case", [">", semi, 0], ["slice", raw, 0, semi], raw];
    const n: ExpressionSpecification = ["length", ref];
    const kind: ExpressionSpecification = ["case", ["==", ["get", "kind"], "highway"], "fw", ["<=", n, 2], "nat", ["==", n, 3], "reg", "loc"];
    shields.minzoom = 7;
    shields.filter = ["all", ["in", ["get", "kind"], ["literal", ["highway", "major_road", "medium_road"]]], [">", n, 0], ["<=", n, 4]];
    shields.layout = {
      "icon-image": ["concat", "shield:", kind, ":", ref],
      "icon-size": ["interpolate", ["linear"], ["zoom"], 7, 0.85, 12, 1, 16, 1.1],
      "icon-rotation-alignment": "viewport",
      "icon-padding": 6,
      "symbol-placement": "line",
      "symbol-spacing": ["interpolate", ["linear"], ["zoom"], 8, 420, 14, 546],
      // the most important roads win when shields compete for space
      "symbol-sort-key": ["match", ["get", "kind"], "highway", 0, "major_road", 1, 2],
    };
    shields.paint = {};
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
    // "imagery only": the roads and / or the names can be switched off over the photo. Hidden
    // with `visibility` (not removed) so switching them is a layout change, not a rebuild.
    ls = ls.map((l) => (hiddenOverImagery(l, o) ? withVisibility(l, "none") : l));
    worldLayers = worldLayers.map((l) => (hiddenOverImagery(l, o) ? withVisibility(l, "none") : l));
    const bgIdx = ls.findIndex((l) => l.id === "background");
    ls.splice(bgIdx + 1, 0, satelliteLayer());
  }
  if (dem && hillshade) {
    // relief shading over the ground fills (earth / landuse / water), under roads and labels
    const idx = ls.findIndex((l) => l.type === "line" || l.type === "symbol");
    ls.splice(idx < 0 ? ls.length : idx, 0, hillshadeLayer());
  }
  // contours and trails: lines under the first label layer, their labels on top
  const firstLabel = () => {
    const i = ls.findIndex((l) => l.type === "symbol");
    return i < 0 ? ls.length : i;
  };
  const putOverlay = (layers: LayerSpecification[]) => {
    const lines = layers.filter((l) => l.type !== "symbol");
    const labels = layers.filter((l) => l.type === "symbol");
    ls.splice(firstLabel(), 0, ...lines);
    ls.push(...labels);
  };
  if (o.contourTiles && (o.contours || topo)) {
    base.sources.contours = { type: "vector", tiles: [o.contourTiles], maxzoom: 15, attribution: demAttribution };
    putOverlay(contourLayers(o.flavor === "dark", o.lang));
  }
  if (o.trailsUrl && (o.trails || topo)) {
    // coarser tiling of the 5–6 MB file: fewer vertices per tile, no tiles beyond zoom 14
    base.sources.trails = { type: "geojson", data: o.trailsUrl, tolerance: 0.6, buffer: 32, maxzoom: 14, attribution: '© <a href="https://openstreetmap.org">OpenStreetMap</a>' };
    putOverlay(trailLayers());
  } else if (o.onlineTrails && (o.trails || topo)) {
    // no trails file installed: the Waymarked Trails raster overlay where the trail lines go
    base.sources["trails-online"] = { type: "raster", tiles: [onlineTrailsUrl(o.theme)], tileSize: 256, maxzoom: 18, attribution: ONLINE_TRAILS_ATTRIBUTION };
    putOverlay([{ id: "trails-online", type: "raster", source: "trails-online", minzoom: 8, paint: { "raster-opacity": 0.85 } }]);
  }
  // User tile layers: over the imagery and the land, under the roads and labels, in list order
  const custom = (o.customLayers ?? []).filter((c) => c.visible);
  if (custom.length) {
    const firstLine = ls.findIndex((l) => l.type === "line" || l.type === "symbol");
    ls.splice(firstLine < 0 ? ls.length : firstLine, 0, ...customRasterLayers(custom, base));
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

/** Tile URL template of a user layer, served (and cached) by the backend's mo-user scheme. */
export function customLayerTiles(id: string): string {
  return `http://mo-user.localhost/${encodeURIComponent(id)}/{z}/{x}/{y}`;
}

/** Raster sources (added to `style`) and layers of the visible user tile layers. */
function customRasterLayers(list: CustomLayer[], style: StyleSpecification): LayerSpecification[] {
  return list.map((c) => {
    style.sources[`custom-${c.id}`] = {
      type: "raster",
      tiles: [customLayerTiles(c.id)],
      tileSize: 256,
      minzoom: c.minzoom ?? 0,
      maxzoom: c.maxzoom ?? 19,
      ...(c.attribution ? { attribution: c.attribution } : {}),
    };
    return { id: `custom-${c.id}`, type: "raster", source: `custom-${c.id}`, paint: { "raster-opacity": Math.max(0, Math.min(1, c.opacity)), "raster-fade-duration": 150 } } as LayerSpecification;
  });
}

/** First symbol layer id – route layers are inserted below labels. */
export function firstSymbolLayer(style: StyleSpecification): string | undefined {
  // the world basemap sits under everything and has its own labels: skip them, or overlays
  // inserted "below the labels" would end up under the detailed map and the imagery
  return style.layers.find((l) => l.type === "symbol" && !l.id.startsWith("world-"))?.id;
}
