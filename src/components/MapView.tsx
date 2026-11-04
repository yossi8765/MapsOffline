import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import * as maplibregl from "maplibre-gl";
// MapLibre 6 is ESM with a separate worker module: let Vite emit it and tell MapLibre where it is
// (it otherwise looks next to its own module URL, which the bundle does not have).
import maplibreWorkerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import type { GeoJSONSource, StyleSpecification } from "maplibre-gl";
import { Protocol } from "pmtiles";
import { Database, Wifi } from "lucide-react";
import { useStore, resolveTheme } from "../lib/store";
import { useT } from "../lib/i18n";
import { buildStyle, DEFAULT_CENTER, DEFAULT_ZOOM, MAX_BOUNDS, firstSymbolLayer, isMergedTiles, type FlavorName } from "../lib/mapStyle";
import { categoryOf, POI_CATEGORIES } from "../lib/poiCategories";
import { CategoryPicker, LayersButton, LocateButton } from "./CategoryPicker";
import { fmtCoords, fmtDistance } from "../lib/format";
import { api, errorCode, type GeoResult, type LonLat, type Route } from "../lib/api";
import type { Feature, FeatureCollection, Point } from "geojson";
import { ContextMenu } from "./ContextMenu";
import { MapHoverCard, type HoverTarget } from "./MapHoverCard";
import { NavigationView } from "./NavigationView";
import { attachWheelGestures, getScrollMode } from "../lib/gestures";
import { contourTilesUrl } from "../lib/contours";
import { renderRouteImage } from "../lib/mapImage";
import { setPrintMapProvider } from "../lib/print";

let protocolRegistered = false;
function ensureProtocol() {
  if (protocolRegistered) return;
  maplibregl.setWorkerUrl(maplibreWorkerUrl);
  const protocol = new Protocol();
  maplibregl.addProtocol("pmtiles", protocol.tile);
  // MapLibre 6 draws right-to-left text itself: no RTL plugin any more.
  protocolRegistered = true;
}

const SRC_ALTS = "route-alts";
const SRC_SEL = "route-selected";
const SRC_HL = "route-highlight";
const SRC_STOPS = "route-stops";
const SRC_LINE = "line-preview";
const SRC_POIS = "poi-layer";
const SRC_ME = "my-location";
const POIS_MIN_ZOOM = 11;
const POIS_LIMIT = 1500;

/** A colour token from src/styles.css (the active theme and colour scheme), with a fallback. */
function cssVar(name: string, fallback: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

/** Every colour the map overlays use, read from the CSS tokens, so the theme and the colour
 *  scheme reach the route, my location, stops and labels too. `dark` = the map is dark (the map
 *  flavour can differ from the panel theme). */
function mapColors(dark: boolean) {
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

function modeColor(mode: string): string {
  return cssVar(mode === "foot" ? "--route-foot" : mode === "bike" ? "--route-bike" : "--route-car", "#1a66d9");
}

function legColor(route: Route, legIdx: number): string {
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

function routeFeatures(routes: Route[], selectedId: string | null) {
  const alts: Feature[] = [];
  const sel: Feature[] = [];
  const stops: Feature[] = [];
  for (const r of routes) {
    if (r.id !== selectedId) {
      alts.push({ type: "Feature", properties: { id: r.id }, geometry: { type: "LineString", coordinates: r.geometry } });
      continue;
    }
    r.legs.forEach((leg, i) => {
      sel.push({
        type: "Feature",
        properties: { id: r.id, leg: i, kind: leg.kind, color: legColor(r, i), walk: leg.kind === "walk" && r.mode === "transit" },
        geometry: { type: "LineString", coordinates: leg.geometry },
      });
      if (leg.transit) {
        const ts = leg.transit;
        ts.stops.forEach((s, j) => {
          const isEnd = j === 0 || j === ts.stops.length - 1;
          stops.push({
            type: "Feature",
            properties: { name: s.name, end: isEnd, color: legColor(r, i) },
            geometry: { type: "Point", coordinates: [s.lon, s.lat] },
          });
        });
      }
    });
  }
  return { alts, sel, stops };
}

const SRC_TSTOPS = "transit-stops";
const TSTOPS_MIN_ZOOM = 13;
const TSTOPS_LIMIT = 3000;

const BUS_ICON = "bus-stop-marker";
const BUS_ICON_PX = 32; // logical size; rendered at device pixel ratio so it stays crisp

/** Bus-stop marker in the Google Maps style: blue rounded square, white bus glyph, thin white outline. */
function drawBusIcon(dark: boolean): ImageData {
  const dpr = Math.max(1, Math.min(4, Math.ceil(window.devicePixelRatio || 1)));
  const px = BUS_ICON_PX * dpr;
  const canvas = document.createElement("canvas");
  canvas.width = px;
  canvas.height = px;
  const c = canvas.getContext("2d")!;
  c.scale(dpr, dpr);
  const rr = (x: number, y: number, w: number, h: number, r: number) => {
    c.beginPath();
    c.roundRect(x, y, w, h, r);
  };
  const mc = mapColors(dark);
  // outline + blue square
  rr(2, 2, 28, 28, 8);
  c.fillStyle = mc.casing;
  c.fill();
  rr(3.5, 3.5, 25, 25, 7);
  c.fillStyle = mc.stop;
  c.fill();
  // bus body
  c.fillStyle = "#ffffff";
  rr(9, 7.5, 14, 15, 3);
  c.fill();
  // window band
  c.fillStyle = mc.stop;
  rr(11, 10, 10, 5.5, 1.2);
  c.fill();
  // lights
  c.beginPath();
  c.arc(12.5, 19, 1.25, 0, Math.PI * 2);
  c.arc(19.5, 19, 1.25, 0, Math.PI * 2);
  c.fill();
  // wheels
  c.fillStyle = "#ffffff";
  rr(10.5, 22, 3.2, 2.6, 1);
  c.fill();
  rr(18.3, 22, 3.2, 2.6, 1);
  c.fill();
  return c.getImageData(0, 0, px, px);
}

function addTransitStopLayers(map: maplibregl.Map, beforeId: string | undefined, dark: boolean) {
  const empty = { type: "FeatureCollection", features: [] } as FeatureCollection;
  if (!map.getSource(SRC_TSTOPS)) map.addSource(SRC_TSTOPS, { type: "geojson", data: empty });
  const before = beforeId && map.getLayer(beforeId) ? beforeId : undefined;
  const text = mapColors(dark).stopLabel;
  const halo = mapColors(dark).halo;
  if (!map.hasImage(BUS_ICON)) {
    try {
      const img = drawBusIcon(dark);
      map.addImage(BUS_ICON, img, { pixelRatio: img.width / BUS_ICON_PX });
    } catch (e) {
      console.warn("bus icon", e);
    }
  }
  if (!map.getLayer("transit-stops")) {
    map.addLayer(
      {
        id: "transit-stops",
        type: "symbol",
        source: SRC_TSTOPS,
        minzoom: TSTOPS_MIN_ZOOM,
        layout: {
          "icon-image": BUS_ICON,
          "icon-size": ["interpolate", ["linear"], ["zoom"], 13, 0.42, 16, 0.62, 19, 0.8],
          "icon-allow-overlap": true,
          "icon-padding": 1,
        },
        paint: {
          "icon-opacity": ["interpolate", ["linear"], ["zoom"], 13, 0.85, 15, 1],
        },
      },
      before,
    );
  }
  if (!map.getLayer("transit-stops-label")) {
    map.addLayer(
      {
        id: "transit-stops-label",
        type: "symbol",
        source: SRC_TSTOPS,
        minzoom: 15,
        layout: {
          "text-field": ["get", "name"],
          "text-font": ["Noto Sans Regular"],
          "text-size": 11,
          "text-offset": ["interpolate", ["linear"], ["zoom"], 15, ["literal", [0, 1.0]], 19, ["literal", [0, 1.35]]],
          "text-anchor": "top",
          "text-optional": true,
          "text-max-width": 8,
        },
        paint: { "text-color": text, "text-halo-color": halo, "text-halo-width": 1.3 },
      },
      before,
    );
  }
}

const POI_ICON_PX = 36;

/** Category marker: a badge in the category's colour (the same colour as its chip and its
 * label) with the white lucide glyph and a thin outline in the map's casing colour. */
function poiIconSvg(lucideSvg: string, dark: boolean, color: string): string {
  const inner = lucideSvg.replace(/^[\s\S]*?<svg[^>]*>/, "").replace(/<\/svg>\s*$/, "");
  const card = color;
  const edge = mapColors(dark).casing;
  const ink = "#ffffff";
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${POI_ICON_PX}" height="${POI_ICON_PX}" viewBox="0 0 36 36">` +
    `<rect x="2.5" y="2.5" width="31" height="31" rx="9" fill="${card}" stroke="${edge}" stroke-width="1.5"/>` +
    `<g transform="translate(9 9) scale(0.75)" fill="none" stroke="${ink}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${inner}</g>` +
    `</svg>`
  );
}

async function loadPoiIcons(map: maplibregl.Map, dark: boolean): Promise<void> {
  const dpr = Math.max(1, Math.min(4, Math.ceil(window.devicePixelRatio || 1)));
  await Promise.all(
    POI_CATEGORIES.map(
      (c) =>
        new Promise<void>((resolve) => {
          const name = `poi-${c.id}`;
          if (map.hasImage(name)) return resolve();
          const img = new Image(POI_ICON_PX * dpr, POI_ICON_PX * dpr);
          img.onload = () => {
            try {
              const canvas = document.createElement("canvas");
              canvas.width = POI_ICON_PX * dpr;
              canvas.height = POI_ICON_PX * dpr;
              const ctx = canvas.getContext("2d")!;
              ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
              if (!map.hasImage(name)) map.addImage(name, ctx.getImageData(0, 0, canvas.width, canvas.height), { pixelRatio: dpr });
            } catch (e) {
              console.warn("poi icon", c.id, e);
            }
            resolve();
          };
          img.onerror = () => resolve();
          img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(poiIconSvg(c.svg, dark, c.color))}`;
        }),
    ),
  );
}

/** Israeli road-sign colours (hebrew-maps' palette). */
const SHIELD_STYLES = {
  fw: { bg: "#1D5BBF", fg: "#FFFFFF", frame: "#FFFFFF", edge: "#123C80" },
  nat: { bg: "#D6332B", fg: "#FFFFFF", frame: "#FFFFFF", edge: "#8F1D17" },
  reg: { bg: "#178246", fg: "#FFFFFF", frame: "#FFFFFF", edge: "#0D5A2F" },
  loc: { bg: "#FFFFFF", fg: "#1F2328", frame: "#1F2328", edge: "#6B7280" },
} as const;

/** Road-number shield `shield:<fw|nat|reg|loc>:<number>`: a rounded badge with the inner frame
 *  of the real signs and the number, drawn at the device pixel ratio. */
function shieldImage(name: string): { data: ImageData; pixelRatio: number } | null {
  const m = /^shield:(fw|nat|reg|loc):(.+)$/.exec(name);
  if (!m) return null;
  const st = SHIELD_STYLES[m[1] as keyof typeof SHIELD_STYLES];
  const text = m[2].trim();
  const dpr = Math.max(1, Math.min(4, Math.ceil(window.devicePixelRatio || 1)));
  const height = 18;
  const font = `700 ${Math.round(height * 0.64)}px "Segoe UI", "Noto Sans", Arial, sans-serif`;
  const probe = document.createElement("canvas").getContext("2d")!;
  probe.font = font;
  const w = Math.max(height + 2, Math.ceil(probe.measureText(text).width) + 12);
  const h = height;
  const canvas = document.createElement("canvas");
  canvas.width = Math.round((w + 2) * dpr);
  canvas.height = Math.round((h + 3) * dpr);
  const c = canvas.getContext("2d")!;
  c.scale(dpr, dpr);
  // body with a soft drop shadow
  c.shadowColor = "rgba(0,0,0,.28)";
  c.shadowBlur = 1.5;
  c.shadowOffsetY = 0.8;
  c.beginPath();
  c.roundRect(1, 1, w, h, 3.5);
  c.fillStyle = st.bg;
  c.fill();
  c.shadowColor = "transparent";
  c.lineWidth = 1;
  c.strokeStyle = st.edge;
  c.stroke();
  // inner frame, as on the real signs
  c.beginPath();
  c.roundRect(2.6, 2.6, w - 3.2, h - 3.2, 2.2);
  c.lineWidth = 1.1;
  c.strokeStyle = st.frame;
  c.stroke();
  c.font = font;
  c.fillStyle = st.fg;
  c.textAlign = "center";
  c.textBaseline = "middle";
  c.fillText(text, 1 + w / 2, 1 + h / 2 + 0.5);
  return { data: c.getImageData(0, 0, canvas.width, canvas.height), pixelRatio: dpr };
}

/** "My location": accuracy disc (metres, scaled with zoom) + blue dot. */
function addMyLocationLayers(map: maplibregl.Map) {
  const empty = { type: "FeatureCollection", features: [] } as FeatureCollection;
  if (!map.getSource(SRC_ME)) map.addSource(SRC_ME, { type: "geojson", data: empty });
  if (!map.getLayer("my-location-accuracy")) {
    map.addLayer({
      id: "my-location-accuracy",
      type: "circle",
      source: SRC_ME,
      paint: {
        // radius in px = accuracy_m / metres-per-pixel(zoom); r0 is the value at zoom 0
        "circle-radius": ["interpolate", ["exponential", 2], ["zoom"], 0, ["get", "r0"], 22, ["*", ["get", "r0"], 4194304]],
        "circle-color": mapColors(false).me,
        "circle-opacity": 0.12,
        "circle-stroke-color": mapColors(false).me,
        "circle-stroke-opacity": 0.3,
        "circle-stroke-width": 1,
      },
    });
  }
  if (!map.getLayer("my-location-dot")) {
    map.addLayer({
      id: "my-location-dot",
      type: "circle",
      source: SRC_ME,
      paint: { "circle-radius": 7, "circle-color": mapColors(false).me, "circle-stroke-color": "#ffffff", "circle-stroke-width": 2.5 },
    });
  }
}

/** A POI marker's GeoResult (kept as JSON in the feature), its category as the picker's label. */
function parseResult(json: unknown, label: string): GeoResult | undefined {
  if (typeof json !== "string") return undefined;
  try {
    const r = JSON.parse(json) as GeoResult;
    return { ...r, category: label || r.category };
  } catch {
    return undefined;
  }
}

function addPoiLayers(map: maplibregl.Map, beforeId: string | undefined, dark: boolean) {
  const empty = { type: "FeatureCollection", features: [] } as FeatureCollection;
  if (!map.getSource(SRC_POIS)) map.addSource(SRC_POIS, { type: "geojson", data: empty });
  const before = beforeId && map.getLayer(beforeId) ? beforeId : undefined;
  if (!map.getLayer("poi-layer")) {
    map.addLayer(
      {
        id: "poi-layer",
        type: "symbol",
        source: SRC_POIS,
        minzoom: POIS_MIN_ZOOM,
        // Fewer, named markers (hebrew-maps / Google): from zoom 13 a marker is drawn only with
        // its name next to it (text-optional false), the name in the category's colour, placed on
        // whichever side has room. Server-side thinning (minzoom per POI) keeps the density even.
        layout: {
          "icon-image": ["concat", "poi-", ["get", "cat"]],
          "icon-size": ["interpolate", ["linear"], ["zoom"], 11, 0.55, 15, 0.75, 18, 0.9],
          "icon-allow-overlap": false,
          "icon-padding": 2,
          "text-field": ["step", ["zoom"], "", 13, ["get", "name"]],
          "text-font": ["Noto Sans Regular"],
          "text-size": 11,
          "text-variable-anchor": ["top", "right", "left", "bottom"],
          "text-radial-offset": 1.15,
          "text-justify": "auto",
          "text-optional": false,
          "text-max-width": 8,
          "symbol-sort-key": ["get", "rank"],
        },
        paint: { "text-color": ["get", "color"], "text-halo-color": mapColors(dark).halo, "text-halo-width": 1.4 },
      },
      before,
    );
  }
}

/** A favorite transit line (favorites panel): its shape and stops, under the labels. */
function addLineLayers(map: maplibregl.Map, beforeId: string | undefined, dark: boolean) {
  const empty = { type: "FeatureCollection", features: [] } as FeatureCollection;
  if (!map.getSource(SRC_LINE)) map.addSource(SRC_LINE, { type: "geojson", data: empty });
  const before = beforeId && map.getLayer(beforeId) ? beforeId : undefined;
  const casing = mapColors(dark).casing;
  if (!map.getLayer("line-preview-casing")) {
    map.addLayer(
      { id: "line-preview-casing", type: "line", source: SRC_LINE, filter: ["==", ["geometry-type"], "LineString"], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": casing, "line-width": 9, "line-opacity": 0.9 } },
      before,
    );
  }
  if (!map.getLayer("line-preview")) {
    map.addLayer(
      { id: "line-preview", type: "line", source: SRC_LINE, filter: ["==", ["geometry-type"], "LineString"], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": ["get", "color"], "line-width": 5 } },
      before,
    );
  }
  if (!map.getLayer("line-preview-stops")) {
    map.addLayer(
      {
        id: "line-preview-stops",
        type: "circle",
        source: SRC_LINE,
        filter: ["==", ["geometry-type"], "Point"],
        minzoom: 11,
        paint: { "circle-radius": ["interpolate", ["linear"], ["zoom"], 11, 2.5, 15, 5], "circle-color": casing, "circle-stroke-color": ["get", "color"], "circle-stroke-width": 2 },
      },
      before,
    );
  }
}

function addRouteLayers(map: maplibregl.Map, beforeId: string | undefined, dark: boolean) {
  const mc = mapColors(dark);
  const casing = mc.casing;
  const empty = { type: "FeatureCollection", features: [] } as FeatureCollection;
  if (!map.getSource(SRC_ALTS)) map.addSource(SRC_ALTS, { type: "geojson", data: empty });
  if (!map.getSource(SRC_SEL)) map.addSource(SRC_SEL, { type: "geojson", data: empty });
  if (!map.getSource(SRC_HL)) map.addSource(SRC_HL, { type: "geojson", data: empty });
  if (!map.getSource(SRC_STOPS)) map.addSource(SRC_STOPS, { type: "geojson", data: empty });
  const before = beforeId && map.getLayer(beforeId) ? beforeId : undefined;
  const add = (l: maplibregl.LayerSpecification) => {
    if (!map.getLayer(l.id)) map.addLayer(l, before);
  };
  add({
    id: "route-alts-casing",
    type: "line",
    source: SRC_ALTS,
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": casing, "line-width": 8, "line-opacity": 0.6 },
  });
  add({
    id: "route-alts",
    type: "line",
    source: SRC_ALTS,
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": mc.alt, "line-width": 5, "line-opacity": 0.55 },
  });
  add({
    id: "route-sel-casing",
    type: "line",
    source: SRC_SEL,
    filter: ["!", ["get", "walk"]],
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": casing, "line-width": 10, "line-opacity": 0.9 },
  });
  add({
    id: "route-sel",
    type: "line",
    source: SRC_SEL,
    filter: ["!", ["get", "walk"]],
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": ["get", "color"], "line-width": 6 },
  });
  add({
    id: "route-sel-walk",
    type: "line",
    source: SRC_SEL,
    filter: ["get", "walk"],
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": ["get", "color"], "line-width": 5, "line-dasharray": [0.2, 1.6] },
  });
  add({
    id: "route-hl",
    type: "line",
    source: SRC_HL,
    layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-color": mc.highlight, "line-width": 10, "line-opacity": 0.75 },
  });
  add({
    id: "route-stops",
    type: "circle",
    source: SRC_STOPS,
    paint: {
      "circle-radius": ["case", ["get", "end"], 6, 4],
      "circle-color": casing,
      "circle-stroke-color": ["get", "color"],
      "circle-stroke-width": ["case", ["get", "end"], 3, 2],
    },
  });
  add({
    id: "route-stops-label",
    type: "symbol",
    source: SRC_STOPS,
    minzoom: 13,
    filter: ["get", "end"],
    layout: {
      "text-field": ["get", "name"],
      "text-font": ["Noto Sans Regular"],
      "text-size": 11,
      "text-offset": [0, 1.1],
      "text-anchor": "top",
      "text-optional": true,
    },
    paint: { "text-color": mc.label, "text-halo-color": casing, "text-halo-width": 1.5 },
  });
}

/** Hebrew texts of MapLibre's built-in controls (navigation, attribution, scale). */
const MAPLIBRE_HE: Record<string, string> = {
  "NavigationControl.ZoomIn": "התקרבות",
  "NavigationControl.ZoomOut": "התרחקות",
  "NavigationControl.ResetBearing": "איפוס כיוון לצפון",
  "AttributionControl.ToggleAttribution": "הצגת מקורות המפה",
  "AttributionControl.MapFeedback": "דיווח על טעות במפה",
  "FullscreenControl.Enter": "מסך מלא",
  "FullscreenControl.Exit": "יציאה ממסך מלא",
  "GeolocateControl.FindMyLocation": "המיקום שלי",
  "GeolocateControl.LocationNotAvailable": "המיקום אינו זמין",
  "ScaleControl.Meters": "מ׳",
  "ScaleControl.Kilometers": "ק״מ",
};

export function MapView() {
  const t = useT();
  const containerRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const markersRef = useRef<maplibregl.Marker[]>([]);
  const poiMarkerRef = useRef<maplibregl.Marker | null>(null);
  const popupRef = useRef<maplibregl.Popup | null>(null);
  const [cursor, setCursor] = useState<LonLat | null>(null);
  const [mapReady, setMapReady] = useState(false);
  const [hover, setHover] = useState<HoverTarget | null>(null);
  const hoverRef = useRef<HoverTarget | null>(null);
  hoverRef.current = hover;
  const hoverTimers = useRef<{ show?: ReturnType<typeof setTimeout>; hide?: ReturnType<typeof setTimeout> }>({});

  const tilesUrl = useStore((s) => s.tilesUrl);
  const satelliteUrl = useStore((s) => s.satelliteUrl);
  const satelliteOrtho = useStore((s) => s.settings.satelliteSource !== "sentinel" || !!s.dataStatus?.satellite?.version?.startsWith("govmap-"));
  const terrainUrl = useStore((s) => s.terrainUrl);
  const worldUrl = useStore((s) => s.worldUrl);
  const onlineTilesUrl = useStore((s) => s.onlineTilesUrl);
  const tilesTick = useStore((s) => s.tilesTick);
  const layers = useStore((s) => s.layers);
  const theme = useStore((s) => s.settings.theme);
  const routes = useStore((s) => s.routes);
  const selectedRouteId = useStore((s) => s.selectedRouteId);
  const fitRouteTick = useStore((s) => s.fitRouteTick);
  const highlight = useStore((s) => s.highlight);
  const waypoints = useStore((s) => s.waypoints);
  const flyTo = useStore((s) => s.flyTo);
  const marker = useStore((s) => s.marker);
  const contextMenu = useStore((s) => s.contextMenu);
  const lang = useStore((s) => s.settings.language);

  const flavor: FlavorName = layers.flavor === "auto" ? resolveTheme(theme) : layers.flavor;
  const topo = layers.theme === "topo";
  // contour lines: computed in the browser from the elevation archive (lib/contours.ts)
  const contourTiles = useMemo(() => (terrainUrl && (layers.contours || topo) ? contourTilesUrl(terrainUrl) : null), [terrainUrl, layers.contours, topo]);
  // marked trails archive (built by the data update; absent until installed)
  const [trailsFile, setTrailsFile] = useState<string | null>(null);
  useEffect(() => {
    invoke<string | null>("trails_path")
      .then((p) => setTrailsFile(p ? convertFileSrc(p) : null))
      .catch(() => setTrailsFile(null));
  }, [tilesTick]);
  const trailsUrl = layers.trails || topo ? trailsFile : null;
  const contourRef = useRef({ contourTiles, trailsUrl });
  contourRef.current = { contourTiles, trailsUrl };
  const flavorRef = useRef(flavor);
  flavorRef.current = flavor;

  // ---- create map
  useEffect(() => {
    if (!containerRef.current || mapRef.current) return;
    ensureProtocol();
    const st = useStore.getState();
    // last map view (when the setting allows it; mirrored in localStorage for this early read)
    let start: { center: LonLat; zoom: number } = { center: DEFAULT_CENTER, zoom: DEFAULT_ZOOM };
    try {
      if (localStorage.getItem("mapsoffline.rememberView") !== "0") {
        const saved = JSON.parse(localStorage.getItem("mapsoffline.lastView") || "null") as { lon: number; lat: number; zoom: number } | null;
        if (saved && Number.isFinite(saved.lon) && Number.isFinite(saved.lat) && Number.isFinite(saved.zoom)) start = { center: [saved.lon, saved.lat], zoom: saved.zoom };
      }
    } catch {
      /* ignore */
    }
    const map = new maplibregl.Map({
      container: containerRef.current,
      style: buildStyle({ tilesUrl: st.tilesUrl, onlineTilesUrl: st.onlineTilesUrl, satelliteUrl: st.satelliteUrl, satellite: layers.satellite, satelliteOrtho: st.settings.satelliteSource !== "sentinel" || !!st.dataStatus?.satellite?.version?.startsWith("govmap-"), terrainUrl: st.terrainUrl, worldUrl: st.worldUrl, hillshade: layers.hillshade, terrain3d: layers.terrain3d, roadShields: layers.roadShields, flavor, lang: layers.labelLang, poi: layers.poi, buildings3d: layers.buildings3d, theme: layers.theme, contours: layers.contours, contourTiles: contourRef.current.contourTiles, trails: layers.trails, trailsUrl: contourRef.current.trailsUrl }),
      center: start.center,
      zoom: start.zoom,
      maxZoom: 20,
      minZoom: 5,
      maxBounds: MAX_BOUNDS,
      attributionControl: false,
      canvasContextAttributes: { antialias: true },
      // MapLibre's own control tooltips / aria labels in the UI language
      locale: lang === "he" ? MAPLIBRE_HE : {},
    });
    mapRef.current = map;
    const detachGestures = attachWheelGestures(map, getScrollMode);
    // The sidebar floats on the start side (right in Hebrew); every map control goes to the
    // other side so the panel never hides it (the OSM attribution is a licence requirement).
    const freeSide = lang === "he" ? "left" : "right";
    map.addControl(new maplibregl.NavigationControl({ visualizePitch: true }), `top-${freeSide}`);
    map.addControl(new maplibregl.AttributionControl({ compact: true }), `bottom-${freeSide}`);
    map.addControl(new maplibregl.ScaleControl({ maxWidth: 120, unit: "metric" }), `bottom-${freeSide}`);
    map.on("mousemove", (e) => setCursor([e.lngLat.lng, e.lngLat.lat]));
    map.on("moveend", () => useStore.getState().setMapView([map.getCenter().lng, map.getCenter().lat], map.getZoom()));
    map.on("contextmenu", (e) => {
      e.preventDefault();
      useStore.getState().setContextMenu({ x: e.point.x, y: e.point.y, lon: e.lngLat.lng, lat: e.lngLat.lat });
    });
    map.on("click", () => {
      useStore.getState().setContextMenu(null);
    });
    map.on("click", "route-alts", (e) => {
      const id = e.features?.[0]?.properties?.id as string | undefined;
      if (id) useStore.getState().selectRoute(id);
    });
    map.on("mouseenter", "route-alts", () => (map.getCanvas().style.cursor = "pointer"));
    map.on("mouseleave", "route-alts", () => (map.getCanvas().style.cursor = ""));
    map.on("click", "route-stops", (e) => {
      const f = e.features?.[0];
      if (!f) return;
      const [lon, lat] = (f.geometry as Point).coordinates;
      popupRef.current?.remove();
      popupRef.current = new maplibregl.Popup({ offset: 10 }).setLngLat([lon, lat]).setText(String(f.properties?.name ?? "")).addTo(map);
    });
    // Hover card (desktop): 200 ms before it opens, 260 ms of grace to move onto it; a click
    // pins it until the map is clicked elsewhere.
    const hoverLayers = ["poi-layer", "transit-stops"];
    const targetOf = (f: maplibregl.MapGeoJSONFeature, pinned: boolean): HoverTarget => {
      const [lon, lat] = (f.geometry as Point).coordinates as [number, number];
      const pt = map.project([lon, lat]);
      const stop = f.layer.id === "transit-stops";
      return {
        kind: stop ? "stop" : "poi",
        name: String(f.properties?.name ?? ""),
        sub: stop ? undefined : String(f.properties?.label ?? ""),
        code: stop && f.properties?.code ? String(f.properties.code) : undefined,
        color: stop ? undefined : (f.properties?.color as string | undefined),
        // the full search result of a POI marker: the place card needs its category (synagogue →
        // prayer times), opening hours and kosher flag
        result: stop ? undefined : parseResult(f.properties?.json, String(f.properties?.label ?? "")),
        lonlat: [lon, lat],
        x: pt.x,
        y: pt.y,
        pinned,
      };
    };
    const sameTarget = (a: HoverTarget | null, b: HoverTarget) => !!a && a.lonlat[0] === b.lonlat[0] && a.lonlat[1] === b.lonlat[1];
    let pending: HoverTarget | null = null;
    for (const layer of hoverLayers) {
      map.on("mousemove", layer, (e) => {
        map.getCanvas().style.cursor = "pointer";
        const f = e.features?.[0];
        if (!f || hoverRef.current?.pinned) return;
        const tgt = targetOf(f, false);
        clearTimeout(hoverTimers.current.hide);
        if (sameTarget(hoverRef.current, tgt) || sameTarget(pending, tgt)) return;
        pending = tgt;
        clearTimeout(hoverTimers.current.show);
        hoverTimers.current.show = setTimeout(() => {
          pending = null;
          setHover(tgt);
        }, 200);
      });
      map.on("mouseleave", layer, () => {
        map.getCanvas().style.cursor = "";
        pending = null;
        clearTimeout(hoverTimers.current.show);
        if (hoverRef.current?.pinned) return;
        hoverTimers.current.hide = setTimeout(() => setHover(null), 260);
      });
      map.on("click", layer, (e) => {
        const f = e.features?.[0];
        if (!f) return;
        clearTimeout(hoverTimers.current.show);
        clearTimeout(hoverTimers.current.hide);
        popupRef.current?.remove();
        setHover(targetOf(f, true));
      });
    }
    map.on("click", (e) => {
      const present = hoverLayers.filter((l) => map.getLayer(l));
      if (!present.length || !map.queryRenderedFeatures(e.point, { layers: present }).length) setHover(null);
    });
    // a marked trail: name, length and network in a small card
    map.on("click", (e) => {
      const trailLayers = ["trails", "trails-bike"].filter((l) => map.getLayer(l));
      if (!trailLayers.length) return;
      if (map.queryRenderedFeatures(e.point, { layers: ["poi-layer", "transit-stops"].filter((l) => map.getLayer(l)) }).length) return;
      const f = map.queryRenderedFeatures(e.point, { layers: trailLayers })[0];
      if (!f) return;
      const st = useStore.getState();
      const root = document.createElement("div");
      root.className = "flex flex-col gap-1 min-w-[180px]";
      const title = document.createElement("div");
      title.className = "font-semibold text-base pe-4 selectable";
      title.textContent = String(f.properties?.name ?? st.t("trails"));
      root.appendChild(title);
      const rows: [string, string][] = [];
      const len = Number(f.properties?.lengthM ?? f.properties?.length_m);
      if (Number.isFinite(len) && len > 0) rows.push([st.t("trailLength"), fmtDistance(len, st.t, st.settings.units)]);
      if (f.properties?.network) rows.push([st.t("trailNetwork"), String(f.properties.network)]);
      for (const [k, v] of rows) {
        const r = document.createElement("div");
        r.className = "text-xs text-muted";
        r.textContent = `${k}: ${v}`;
        root.appendChild(r);
      }
      popupRef.current?.remove();
      popupRef.current = new maplibregl.Popup({ offset: 8, maxWidth: "260px" }).setLngLat(e.lngLat).setDOMContent(root).addTo(map);
    });
    // a pinned card follows its marker; an unpinned one closes when the map moves
    map.on("move", () => {
      const h = hoverRef.current;
      if (!h) return;
      if (!h.pinned) {
        setHover(null);
        return;
      }
      const pt = map.project(h.lonlat);
      setHover({ ...h, x: pt.x, y: pt.y });
    });
    map.on("style.load", () => {
      const firstSymbol = firstSymbolLayer(map.getStyle() as StyleSpecification);
      addTransitStopLayers(map, firstSymbol, flavorRef.current === "dark");
      addRouteLayers(map, firstSymbol, flavorRef.current === "dark");
      addLineLayers(map, firstSymbol, flavorRef.current === "dark");
      addMyLocationLayers(map);
      void loadPoiIcons(map, flavorRef.current === "dark").then(() => {
        if (!map.getStyle()) return;
        // above the base map's labels (under the location dot): the markers the user asked for
        // are placed first, so a house number or street name never hides one of them
        addPoiLayers(map, "my-location-accuracy", flavorRef.current === "dark");
        setPoiLayerTick((n) => n + 1);
      });
      setMapReady(true);
      syncRoutes(map);
    });
    let viewTimer: ReturnType<typeof setTimeout> | undefined;
    map.on("moveend", () => {
      if (viewTimer) clearTimeout(viewTimer);
      viewTimer = setTimeout(() => {
        try {
          const c = map.getCenter();
          localStorage.setItem("mapsoffline.lastView", JSON.stringify({ lon: c.lng, lat: c.lat, zoom: map.getZoom() }));
        } catch {
          /* ignore */
        }
      }, 400);
    });
    // MapLibre 6: `styleimagemissing` only notifies; the resolver is awaited before an image
    // counts as missing, so road shields are drawn here on first use.
    map.setMissingStyleImageResolver((id) => {
      const img = shieldImage(id);
      if (img && !map.hasImage(id)) map.addImage(id, img.data, { pixelRatio: img.pixelRatio });
    });
    map.on("error", (e) => {
      // Missing glyph/sprite/tile errors should not spam the console.
      console.warn("map error", e?.error?.message ?? e);
    });
    return () => {
      detachGestures();
      map.remove();
      mapRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ---- style rebuild (not on mount: the map was just created with this very style, and a
  // second setStyle would load glyphs, sprites, first tiles and the POI icons twice)
  const firstStyle = useRef(true);
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    // with a world basemap the map may leave Israel and zoom out to the planet
    const planet = !!worldUrl || isMergedTiles(tilesUrl) || (!tilesUrl && !!onlineTilesUrl);
    map.setMaxBounds(planet ? null : MAX_BOUNDS);
    map.setMinZoom(planet ? 1 : 5);
    if (firstStyle.current) {
      firstStyle.current = false;
      return;
    }
    const style = buildStyle({ tilesUrl, onlineTilesUrl, satelliteUrl, satellite: layers.satellite, satelliteOrtho, terrainUrl, worldUrl, hillshade: layers.hillshade, terrain3d: layers.terrain3d, roadShields: layers.roadShields, flavor, lang: layers.labelLang, poi: layers.poi, buildings3d: layers.buildings3d, theme: layers.theme, contours: layers.contours, contourTiles, trails: layers.trails, trailsUrl });
    setMapReady(false);
    map.setStyle(style, { diff: false });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tilesUrl, onlineTilesUrl, satelliteUrl, satelliteOrtho, terrainUrl, worldUrl, tilesTick, flavor, layers.labelLang, layers.poi, layers.buildings3d, layers.satellite, layers.hillshade, layers.terrain3d, layers.roadShields, layers.theme, layers.contours, layers.trails, contourTiles, trailsUrl]);

  // ---- colour scheme / theme: the overlays take their colours from the CSS tokens, which change
  // without a style rebuild when only the accent changes. Re-read them after App has set
  // data-accent / data-theme (next frame).
  const accentSetting = useStore((s) => s.settings.accent);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    const id = requestAnimationFrame(() => {
      if (!map.getStyle()) return;
      const dark = flavorRef.current === "dark";
      const mc = mapColors(dark);
      const paint = (layer: string, prop: Parameters<typeof map.setPaintProperty>[1], value: string) => {
        if (map.getLayer(layer)) map.setPaintProperty(layer, prop, value);
      };
      paint("my-location-accuracy", "circle-color", mc.me);
      paint("my-location-accuracy", "circle-stroke-color", mc.me);
      paint("my-location-dot", "circle-color", mc.me);
      paint("route-alts", "line-color", mc.alt);
      paint("route-hl", "line-color", mc.highlight);
      paint("route-stops-label", "text-color", mc.label);
      paint("transit-stops-label", "text-color", mc.stopLabel);
      if (map.hasImage(BUS_ICON)) {
        const img = drawBusIcon(dark);
        map.updateImage(BUS_ICON, img);
      }
      syncRoutes(map);
    });
    return () => cancelAnimationFrame(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accentSetting, theme, mapReady]);

  // ---- 3D buildings: tilt the camera when switched on (extrusions are invisible from straight
  // above and below zoom 14), level it again when switched off.
  const first3d = useRef(true);
  useEffect(() => {
    const map = mapRef.current;
    if (first3d.current) {
      first3d.current = false;
      return;
    }
    if (!map) return;
    if (layers.buildings3d) {
      const z = map.getZoom();
      map.easeTo({ pitch: Math.max(map.getPitch(), 55), zoom: z < 15 ? 15.5 : z, duration: 900 });
    } else if (map.getPitch() > 0) {
      map.easeTo({ pitch: 0, duration: 500 });
    }
  }, [layers.buildings3d]);

  // ---- routes
  function syncRoutes(map: maplibregl.Map) {
    const s = useStore.getState();
    const { alts, sel, stops } = routeFeatures(s.routes, s.selectedRouteId);
    (map.getSource(SRC_ALTS) as GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features: alts });
    (map.getSource(SRC_SEL) as GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features: sel });
    (map.getSource(SRC_STOPS) as GeoJSONSource | undefined)?.setData({ type: "FeatureCollection", features: stops });
  }
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    syncRoutes(map);
  }, [routes, selectedRouteId, mapReady]);

  // ---- favorite line preview
  const linePreview = useStore((s) => s.linePreview);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    const src = map.getSource(SRC_LINE) as GeoJSONSource | undefined;
    if (!src) return;
    if (!linePreview) {
      src.setData({ type: "FeatureCollection", features: [] });
      return;
    }
    const sh = linePreview.shape;
    const color = sh.color ? (sh.color.startsWith("#") ? sh.color : `#${sh.color}`) : mapColors(false).bus;
    const features: Feature[] = [
      { type: "Feature", properties: { color }, geometry: { type: "LineString", coordinates: sh.geometry } },
      ...sh.stops.map((s): Feature => ({ type: "Feature", properties: { color, name: s.name }, geometry: { type: "Point", coordinates: [s.lon, s.lat] } })),
    ];
    src.setData({ type: "FeatureCollection", features });
    const pts = sh.geometry.length ? sh.geometry : sh.stops.map((s) => [s.lon, s.lat] as LonLat);
    if (pts.length) {
      const lons = pts.map((p) => p[0]);
      const lats = pts.map((p) => p[1]);
      map.fitBounds([[Math.min(...lons), Math.min(...lats)], [Math.max(...lons), Math.max(...lats)]], { padding: 70, duration: 700, maxZoom: 15 });
    }
  }, [linePreview, mapReady]);

  // ---- my location marker
  const myLocation = useStore((s) => s.myLocation);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    const src = map.getSource(SRC_ME) as GeoJSONSource | undefined;
    if (!src) return;
    if (!myLocation) {
      src.setData({ type: "FeatureCollection", features: [] });
      return;
    }
    const mpp0 = 156543.03392 * Math.cos((myLocation.lat * Math.PI) / 180); // metres per pixel at zoom 0
    src.setData({
      type: "FeatureCollection",
      features: [{ type: "Feature", properties: { r0: Math.max(myLocation.accuracyM, 5) / mpp0 }, geometry: { type: "Point", coordinates: [myLocation.lon, myLocation.lat] } }],
    });
  }, [myLocation, mapReady]);

  // ---- POI category layer (viewport fetch, debounced)
  const [poiLayerTick, setPoiLayerTick] = useState(0);
  const poiCatsKey = layers.poiCats.join(",");
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    const src = () => map.getSource(SRC_POIS) as GeoJSONSource | undefined;
    if (!poiCatsKey) {
      src()?.setData({ type: "FeatureCollection", features: [] });
      return;
    }
    const selectedCats = poiCatsKey.split(",").flatMap((id) => POI_CATEGORIES.filter((c) => c.id === id));
    const cats = [...new Set(selectedCats.flatMap((c) => c.cats))];
    let timer: ReturnType<typeof setTimeout> | undefined;
    let seq = 0;
    const t = useStore.getState().t;
    const fetchPois = async () => {
      if (map.getZoom() < POIS_MIN_ZOOM) {
        src()?.setData({ type: "FeatureCollection", features: [] });
        return;
      }
      const b = map.getBounds();
      const id = ++seq;
      try {
        const poiLayer = api.poiLayer as (cats: string[], bbox: [number, number, number, number], limit?: number, zoom?: number) => ReturnType<typeof api.poiLayer>;
        const pois = await poiLayer(cats, [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()], POIS_LIMIT, Math.floor(map.getZoom()));
        if (id !== seq) return;
        const feats = pois.flatMap((p) => {
          const c = categoryOf(p.category ?? "", p.name, selectedCats);
          if (!c) return [];
          return [{ type: "Feature" as const, properties: { name: p.name, cat: c.id, label: t(c.label), color: c.color, rank: -(p.score ?? 0), json: JSON.stringify(p) }, geometry: { type: "Point" as const, coordinates: [p.lon, p.lat] } }];
        });
        src()?.setData({ type: "FeatureCollection", features: feats });
      } catch (e) {
        console.warn("poi_layer failed", e);
      }
    };
    const schedule = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => void fetchPois(), 250);
    };
    map.on("moveend", schedule);
    schedule();
    return () => {
      map.off("moveend", schedule);
      if (timer) clearTimeout(timer);
      seq++;
    };
  }, [poiCatsKey, mapReady, poiLayerTick]);

  // ---- transit stops layer (viewport fetch, debounced)
  // the public-transport map always shows the stops
  const transitStopsOn = layers.transitStops || layers.theme === "transit";
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    const src = () => map.getSource(SRC_TSTOPS) as GeoJSONSource | undefined;
    if (!transitStopsOn) {
      src()?.setData({ type: "FeatureCollection", features: [] });
      return;
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    let seq = 0;
    let unavailable = false;
    const fetchStops = async () => {
      if (unavailable) return;
      if (map.getZoom() < TSTOPS_MIN_ZOOM) {
        src()?.setData({ type: "FeatureCollection", features: [] });
        return;
      }
      const b = map.getBounds();
      const id = ++seq;
      try {
        const stops = await api.transitStops([b.getWest(), b.getSouth(), b.getEast(), b.getNorth()], TSTOPS_LIMIT);
        if (id !== seq) return;
        useStore.getState().setTransitStopsUnavailable(false);
        src()?.setData({
          type: "FeatureCollection",
          features: stops.map((s) => ({
            type: "Feature",
            properties: { id: s.id, name: s.name, code: s.code ?? "" },
            geometry: { type: "Point", coordinates: [s.lon, s.lat] },
          })),
        });
      } catch (e) {
        if (errorCode(e) === "NO_TRANSIT_DATA") {
          unavailable = true;
          useStore.getState().setTransitStopsUnavailable(true);
          src()?.setData({ type: "FeatureCollection", features: [] });
        } else {
          console.warn("transit_stops failed", e);
        }
      }
    };
    const schedule = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => void fetchStops(), 250);
    };
    map.on("moveend", schedule);
    schedule();
    return () => {
      map.off("moveend", schedule);
      if (timer) clearTimeout(timer);
      seq++;
    };
  }, [transitStopsOn, mapReady]);

  // ---- fit bounds on new routes
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !routes.length) return;
    const b = new maplibregl.LngLatBounds();
    for (const r of routes) for (const p of r.geometry) b.extend(p);
    if (b.isEmpty()) return;
    const collapsed = useStore.getState().sidebarCollapsed;
    const pad = { top: 60, bottom: 60, left: 60, right: 60 };
    if (!collapsed) {
      if (lang === "he") pad.right = 480;
      else pad.left = 480;
    }
    map.fitBounds(b, { padding: pad, maxZoom: 16, duration: 600 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fitRouteTick]);

  // ---- highlight
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    const src = map.getSource(SRC_HL) as GeoJSONSource | undefined;
    if (!src) return;
    const r = routes.find((x) => x.id === highlight?.routeId);
    if (!r || !highlight) {
      src.setData({ type: "FeatureCollection", features: [] });
      return;
    }
    const leg = r.legs[highlight.legIndex];
    if (!leg) return;
    let coords = leg.geometry;
    if (highlight.stepIndex !== undefined && leg.steps?.length) {
      const st = leg.steps[highlight.stepIndex];
      const next = leg.steps[highlight.stepIndex + 1];
      const a = st?.geometryStart ?? 0;
      const b = next ? next.geometryStart + 1 : leg.geometry.length;
      coords = leg.geometry.slice(a, Math.max(b, a + 2));
    }
    src.setData({
      type: "FeatureCollection",
      features: coords.length > 1 ? [{ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: coords } }] : [],
    });
  }, [highlight, routes, mapReady]);

  // ---- waypoint markers
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    markersRef.current.forEach((m) => m.remove());
    markersRef.current = [];
    const n = waypoints.length;
    waypoints.forEach((wp, i) => {
      if (!wp) return;
      const el = document.createElement("div");
      const isEnd = i === n - 1;
      const isVia = i > 0 && i < n - 1;
      el.className = `wp-marker ${isEnd ? "wp-end" : isVia ? "wp-via" : ""}`;
      const span = document.createElement("span");
      span.textContent = i === 0 ? "A" : isEnd ? "B" : String(i);
      el.appendChild(span);
      const m = new maplibregl.Marker({ element: el, draggable: true, anchor: "bottom", offset: [0, 4] })
        .setLngLat([wp.lon, wp.lat])
        .addTo(map);
      m.on("dragend", () => {
        const ll = m.getLngLat();
        useStore.getState().setWaypoint(i, { lon: ll.lng, lat: ll.lat, label: fmtCoords(ll.lng, ll.lat) });
      });
      markersRef.current.push(m);
    });
  }, [waypoints]);

  // ---- a map bubble belongs to the moment it was opened: once the trip, the routes or the
  // panel change it would only cover the pins (declared before the marker effect, so a marker
  // set in the same render still gets its bubble)
  const panelNow = useStore((s) => s.panel);
  useEffect(() => {
    popupRef.current?.remove();
    popupRef.current = null;
  }, [waypoints, routes, panelNow]);

  // ---- search marker (the search panel shows the place card, so no bubble there)
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    poiMarkerRef.current?.remove();
    poiMarkerRef.current = null;
    if (!marker) return;
    const el = document.createElement("div");
    el.className = "poi-marker";
    poiMarkerRef.current = new maplibregl.Marker({ element: el }).setLngLat(marker.lonlat).addTo(map);
    popupRef.current?.remove();
    popupRef.current = null;
    if (useStore.getState().panel === "search" && !useStore.getState().sidebarCollapsed) return;
    popupRef.current = new maplibregl.Popup({ offset: 12, closeButton: true }).setLngLat(marker.lonlat).setText(marker.label).addTo(map);
  }, [marker]);

  // ---- "N": north up, flat
  const northUpTick = useStore((s) => s.northUpTick);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !northUpTick) return;
    map.easeTo({ bearing: 0, pitch: 0, duration: 500 });
  }, [northUpTick]);

  // ---- printout: a light, north-up picture of the selected route (lib/mapImage.ts)
  useEffect(() => {
    setPrintMapProvider(async () => {
      const s = useStore.getState();
      const r = s.routes.find((x) => x.id === s.selectedRouteId) ?? s.routes[0];
      if (!r || r.geometry.length < 2) return null;
      try {
        return await renderRouteImage(r);
      } catch (e) {
        console.warn("print map image", e);
        return null;
      }
    });
    return () => setPrintMapProvider(null);
  }, []);

  // ---- live navigation: follow the snapped position heading-up and tilted; the dot shows the
  // snapped position. Back to north-up and flat when the session ends.
  const navUpdate = useStore((s) => s.navigation?.update ?? null);
  const navigating = useStore((s) => !!s.navigation);
  const wasNavigating = useRef(false);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    if (!navigating) {
      if (wasNavigating.current) {
        wasNavigating.current = false;
        map.easeTo({ bearing: 0, pitch: 0, duration: 600 });
      }
      return;
    }
    wasNavigating.current = true;
    if (!navUpdate) return;
    const at: LonLat = [navUpdate.snappedLon, navUpdate.snappedLat];
    (map.getSource(SRC_ME) as GeoJSONSource | undefined)?.setData({
      type: "FeatureCollection",
      features: [{ type: "Feature", properties: { r0: 5 / (156543.03392 * Math.cos((at[1] * Math.PI) / 180)) }, geometry: { type: "Point", coordinates: at } }],
    });
    // keep the vehicle in the lower third of the view, as navigation apps do
    map.easeTo({
      center: at,
      bearing: navUpdate.bearing,
      pitch: 50,
      zoom: 17,
      offset: [0, Math.round(map.getContainer().clientHeight * 0.2)],
      duration: 900,
      easing: (t) => t,
    });
  }, [navUpdate, navigating, mapReady]);

  // ---- fly to
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !flyTo) return;
    map.flyTo({ center: flyTo.lonlat, zoom: flyTo.zoom ?? Math.max(map.getZoom(), 15), duration: 800 });
  }, [flyTo]);

  // ---- keyboard
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const map = mapRef.current;
      if (!map) return;
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      if (e.key === "+" || e.key === "=") map.zoomIn();
      else if (e.key === "-") map.zoomOut();
      else if (e.key === "Escape") {
        useStore.getState().setContextMenu(null);
        popupRef.current?.remove();
        setHover(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const panel = useStore((s) => s.panel);
  const sidebarCollapsed = useStore((s) => s.sidebarCollapsed);
  // The "no map yet" card points at the Data panel, so it hides while that panel is open.
  const tilesChecked = useStore((s) => s.tilesChecked);
  const noTiles = tilesChecked && !tilesUrl && !onlineTilesUrl && !(panel === "data" && !sidebarCollapsed);
  const onlineMap = tilesChecked && !tilesUrl && !!onlineTilesUrl;
  const setPanel = useStore((s) => s.setPanel);

  return (
    <div className="absolute inset-0">
      <div ref={containerRef} className="absolute inset-0" />
      {cursor && (
        <div className="absolute bottom-2 start-1/2 -translate-x-1/2 rtl:translate-x-1/2 rounded px-2 h-5 flex items-center text-[11px] text-muted num bg-[color-mix(in_srgb,var(--surface-raised)_80%,transparent)] pointer-events-none no-print">
          <bdi dir="ltr">{fmtCoords(cursor[0], cursor[1])}</bdi>
        </div>
      )}
      {noTiles && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <div className="popover rounded-[var(--radius)] p-6 w-[340px] max-w-[calc(100%-32px)] text-center pointer-events-auto fade-in">
            <div className="mx-auto mb-4 w-10 h-10 rounded-full bg-surface-2 text-muted flex items-center justify-center">
              <Database size={18} strokeWidth={1.75} />
            </div>
            <div className="text-lg font-semibold mb-1">{t("noTilesTitle")}</div>
            <div className="text-muted text-sm mb-5">{t("noTilesBody")}</div>
            <button className="btn btn-primary" onClick={() => setPanel("data")}>
              {t("goToData")}
            </button>
          </div>
        </div>
      )}
      {onlineMap && (
        <div className="absolute bottom-10 start-1/2 -translate-x-1/2 rtl:translate-x-1/2 popover rounded-[var(--radius)] ps-3 pe-1.5 h-9 flex items-center gap-2 text-xs no-print fade-in">
          <Wifi size={14} className="text-accent shrink-0" />
          <span>{t("onlineMapBanner")}</span>
          <button className="btn btn-primary btn-sm !h-6 !text-[12px] !px-2.5" onClick={() => setPanel("data")}>
            {t("onlineMapDownload")}
          </button>
        </div>
      )}
      {hover && (
        <MapHoverCard
          target={hover}
          onEnter={() => clearTimeout(hoverTimers.current.hide)}
          onLeave={() => {
            if (!hoverRef.current?.pinned) hoverTimers.current.hide = setTimeout(() => setHover(null), 260);
          }}
          onClose={() => setHover(null)}
        />
      )}
      <CategoryPicker />
      <LocateButton />
      <LayersButton />
      {contextMenu && <ContextMenu state={contextMenu} />}
      <NavigationView />
    </div>
  );
}
