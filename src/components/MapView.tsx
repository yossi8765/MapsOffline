import { useEffect, useRef, useState } from "react";
import maplibregl, { type GeoJSONSource, type StyleSpecification } from "maplibre-gl";
import { Protocol } from "pmtiles";
import { Database, Wifi } from "lucide-react";
import { useStore, resolveTheme } from "../lib/store";
import { useT } from "../lib/i18n";
import { buildStyle, DEFAULT_CENTER, DEFAULT_ZOOM, MAX_BOUNDS, firstSymbolLayer, isMergedTiles, type FlavorName } from "../lib/mapStyle";
import { categoryOf, POI_CATEGORIES } from "../lib/poiCategories";
import { CategoryPicker, LocateButton } from "./CategoryPicker";
import { fmtCoords } from "../lib/format";
import { api, errorCode, type LonLat, type Route } from "../lib/api";
import type { Feature, FeatureCollection, Point } from "geojson";
import { ContextMenu } from "./ContextMenu";
import { attachWheelGestures, getScrollMode } from "../lib/gestures";

let protocolRegistered = false;
function ensureProtocol() {
  if (protocolRegistered) return;
  const protocol = new Protocol();
  maplibregl.addProtocol("pmtiles", protocol.tile);
  // Hebrew/Arabic label shaping – bundled locally so it works offline.
  maplibregl.setRTLTextPlugin(`${window.location.origin}/rtl-text.js`, false).catch((e) => console.warn("rtl plugin", e));
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

function modeColor(mode: string): string {
  const css = getComputedStyle(document.documentElement);
  const v = css.getPropertyValue(mode === "foot" ? "--route-foot" : mode === "bike" ? "--route-bike" : "--route-car").trim();
  return v || "#2563eb";
}

function legColor(route: Route, legIdx: number): string {
  const leg = route.legs[legIdx];
  if (leg?.transit) {
    const c = leg.transit.color;
    if (c) return c.startsWith("#") ? c : `#${c}`;
    return leg.transit.vehicle === "rail" ? "#7c3aed" : leg.transit.vehicle === "light_rail" ? "#dc2626" : "#0891b2";
  }
  if (leg?.kind === "walk" && route.mode === "transit") return "#64748b";
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
  // outline + blue square
  rr(2, 2, 28, 28, 8);
  c.fillStyle = dark ? "#0b1220" : "#ffffff";
  c.fill();
  rr(3.5, 3.5, 25, 25, 7);
  c.fillStyle = dark ? "#5c7a94" : "#4a6276";
  c.fill();
  // bus body
  c.fillStyle = "#ffffff";
  rr(9, 7.5, 14, 15, 3);
  c.fill();
  // window band
  c.fillStyle = dark ? "#5c7a94" : "#4a6276";
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
  const text = dark ? "#c3d0dc" : "#3b4f60";
  const halo = dark ? "#0b1220" : "#ffffff";
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

/** Popup body for a transit stop: name, code and from/to buttons. */
function stopPopupContent(name: string, code: string, lonlat: LonLat, sub = ""): HTMLElement {
  const s = useStore.getState();
  const root = document.createElement("div");
  root.className = "flex flex-col gap-2 min-w-[180px]";
  const title = document.createElement("div");
  title.className = "font-semibold text-base pe-4";
  title.textContent = name;
  root.appendChild(title);
  if (code || sub) {
    const c = document.createElement("div");
    c.className = "text-xs text-muted";
    c.textContent = code ? `${s.t("stopCode")}: ${code}` : sub;
    root.appendChild(c);
  }
  const row = document.createElement("div");
  row.className = "flex gap-1.5";
  const mk = (label: string, primary: boolean, onClick: () => void) => {
    const b = document.createElement("button");
    b.className = `btn btn-sm flex-1 ${primary ? "btn-primary" : ""}`;
    b.textContent = label;
    b.onclick = onClick;
    return b;
  };
  const point = { lon: lonlat[0], lat: lonlat[1], label: name };
  row.appendChild(
    mk(s.t("ctxFromHere"), false, () => {
      useStore.getState().setFrom(point);
      useStore.getState().setPanel("route");
    }),
  );
  row.appendChild(
    mk(s.t("ctxToHere"), true, () => {
      useStore.getState().setTo(point);
      useStore.getState().setPanel("route");
    }),
  );
  root.appendChild(row);
  return root;
}

const POI_ICON_PX = 36;

/** One marker style for every category: a rounded card with the lucide outline glyph in a
 * single ink colour (inverted in the dark theme), rasterised at the device pixel ratio. */
function poiIconSvg(lucideSvg: string, dark: boolean): string {
  const inner = lucideSvg.replace(/^[\s\S]*?<svg[^>]*>/, "").replace(/<\/svg>\s*$/, "");
  const card = dark ? "#1e293b" : "#ffffff";
  const edge = dark ? "#475569" : "#cbd5e1";
  const ink = dark ? "#e2e8f0" : "#334155";
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${POI_ICON_PX}" height="${POI_ICON_PX}" viewBox="0 0 36 36">` +
    `<rect x="2.5" y="2.5" width="31" height="31" rx="9" fill="${card}" stroke="${edge}" stroke-width="1"/>` +
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
          img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(poiIconSvg(c.svg, dark))}`;
        }),
    ),
  );
}

/** Road-number shield `shield-<colour>-<chars>`: rounded rectangle in the class colour with a
 * white border, sized for `chars` digits at the 12 px label (icon-size 1 = zoom 11). */
function shieldImage(name: string): ImageData | null {
  const m = /^shield-(blue|red|green|grey)-(\d+)$/.exec(name);
  if (!m) return null;
  const fill = { blue: "#1d5fd1", red: "#c62828", green: "#1e7f3c", grey: "#4b5563" }[m[1] as "blue" | "red" | "green" | "grey"];
  const chars = Math.max(1, Math.min(6, Number(m[2])));
  const w = 10 + 7.5 * chars;
  const h = 17;
  const dpr = Math.max(1, Math.min(4, Math.ceil(window.devicePixelRatio || 1)));
  const canvas = document.createElement("canvas");
  canvas.width = Math.ceil(w * dpr);
  canvas.height = Math.ceil(h * dpr);
  const c = canvas.getContext("2d")!;
  c.scale(dpr, dpr);
  c.beginPath();
  c.roundRect(0.5, 0.5, w - 1, h - 1, 3.5);
  c.fillStyle = fill;
  c.fill();
  c.lineWidth = 1.5;
  c.strokeStyle = "#ffffff";
  c.stroke();
  return c.getImageData(0, 0, canvas.width, canvas.height);
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
        "circle-color": "#2563eb",
        "circle-opacity": 0.12,
        "circle-stroke-color": "#2563eb",
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
      paint: { "circle-radius": 7, "circle-color": "#2563eb", "circle-stroke-color": "#ffffff", "circle-stroke-width": 2.5 },
    });
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
        layout: {
          "icon-image": ["concat", "poi-", ["get", "cat"]],
          "icon-size": ["interpolate", ["linear"], ["zoom"], 11, 0.55, 15, 0.75, 18, 0.9],
          "icon-allow-overlap": true,
          "icon-padding": 1,
          "text-field": ["step", ["zoom"], "", 14, ["get", "name"]],
          "text-font": ["Noto Sans Regular"],
          "text-size": 11,
          "text-offset": [0, 1.25],
          "text-anchor": "top",
          "text-optional": true,
          "text-max-width": 8,
        },
        paint: { "text-color": dark ? "#e2e8f0" : "#1e293b", "text-halo-color": dark ? "#0b1220" : "#ffffff", "text-halo-width": 1.3 },
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
  const casing = dark ? "#0b1220" : "#ffffff";
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
  const casing = dark ? "#0b1220" : "#ffffff";
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
    paint: { "line-color": dark ? "#94a3b8" : "#64748b", "line-width": 5, "line-opacity": 0.55 },
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
    paint: { "line-color": "#f59e0b", "line-width": 10, "line-opacity": 0.75 },
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
    paint: { "text-color": dark ? "#e2e8f0" : "#0f172a", "text-halo-color": casing, "text-halo-width": 1.5 },
  });
}

export function MapView() {
  const t = useT();
  const containerRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const markersRef = useRef<maplibregl.Marker[]>([]);
  const poiMarkerRef = useRef<maplibregl.Marker | null>(null);
  const popupRef = useRef<maplibregl.Popup | null>(null);
  const [cursor, setCursor] = useState<LonLat | null>(null);
  const [mapReady, setMapReady] = useState(false);

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
      style: buildStyle({ tilesUrl: st.tilesUrl, onlineTilesUrl: st.onlineTilesUrl, satelliteUrl: st.satelliteUrl, satellite: layers.satellite, satelliteOrtho: st.settings.satelliteSource !== "sentinel" || !!st.dataStatus?.satellite?.version?.startsWith("govmap-"), terrainUrl: st.terrainUrl, worldUrl: st.worldUrl, hillshade: layers.hillshade, terrain3d: layers.terrain3d, roadShields: layers.roadShields, flavor, lang: layers.labelLang, poi: layers.poi, buildings3d: layers.buildings3d, theme: layers.theme }),
      center: start.center,
      zoom: start.zoom,
      maxZoom: 20,
      minZoom: 5,
      maxBounds: MAX_BOUNDS,
      attributionControl: false,
      canvasContextAttributes: { antialias: true },
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
    map.on("click", "transit-stops", (e) => {
      const f = e.features?.[0];
      if (!f) return;
      const [lon, lat] = (f.geometry as Point).coordinates;
      const name = String(f.properties?.name ?? "");
      const code = f.properties?.code ? String(f.properties.code) : "";
      popupRef.current?.remove();
      popupRef.current = new maplibregl.Popup({ offset: 10, maxWidth: "260px" })
        .setLngLat([lon, lat])
        .setDOMContent(stopPopupContent(name, code, [lon, lat]))
        .addTo(map);
    });
    map.on("click", "poi-layer", (e) => {
      const f = e.features?.[0];
      if (!f) return;
      const [lon, lat] = (f.geometry as Point).coordinates;
      const name = String(f.properties?.name ?? "");
      const sub = String(f.properties?.label ?? "");
      popupRef.current?.remove();
      popupRef.current = new maplibregl.Popup({ offset: 10, maxWidth: "260px" })
        .setLngLat([lon, lat])
        .setDOMContent(stopPopupContent(name, "", [lon, lat], sub))
        .addTo(map);
    });
    map.on("mouseenter", "poi-layer", () => (map.getCanvas().style.cursor = "pointer"));
    map.on("mouseleave", "poi-layer", () => (map.getCanvas().style.cursor = ""));
    map.on("mouseenter", "transit-stops", () => (map.getCanvas().style.cursor = "pointer"));
    map.on("mouseleave", "transit-stops", () => (map.getCanvas().style.cursor = ""));
    map.on("style.load", () => {
      const firstSymbol = firstSymbolLayer(map.getStyle() as StyleSpecification);
      addTransitStopLayers(map, firstSymbol, flavorRef.current === "dark");
      addRouteLayers(map, firstSymbol, flavorRef.current === "dark");
      addLineLayers(map, firstSymbol, flavorRef.current === "dark");
      addMyLocationLayers(map);
      void loadPoiIcons(map, flavorRef.current === "dark").then(() => {
        if (!map.getStyle()) return;
        addPoiLayers(map, firstSymbol, flavorRef.current === "dark");
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
    map.on("styleimagemissing", (e) => {
      const img = shieldImage(e.id);
      if (img && !map.hasImage(e.id)) map.addImage(e.id, img, { pixelRatio: Math.max(1, Math.min(4, Math.ceil(window.devicePixelRatio || 1))) });
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
    const style = buildStyle({ tilesUrl, onlineTilesUrl, satelliteUrl, satellite: layers.satellite, satelliteOrtho, terrainUrl, worldUrl, hillshade: layers.hillshade, terrain3d: layers.terrain3d, roadShields: layers.roadShields, flavor, lang: layers.labelLang, poi: layers.poi, buildings3d: layers.buildings3d, theme: layers.theme });
    setMapReady(false);
    map.setStyle(style, { diff: false });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tilesUrl, onlineTilesUrl, satelliteUrl, satelliteOrtho, terrainUrl, worldUrl, tilesTick, flavor, layers.labelLang, layers.poi, layers.buildings3d, layers.satellite, layers.hillshade, layers.terrain3d, layers.roadShields, layers.theme]);

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
    const color = sh.color ?? "#0891b2";
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
        const pois = await api.poiLayer(cats, [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()], POIS_LIMIT);
        if (id !== seq) return;
        const feats = pois.flatMap((p) => {
          const c = categoryOf(p.category ?? "", p.name, selectedCats);
          if (!c) return [];
          return [{ type: "Feature" as const, properties: { name: p.name, cat: c.id, label: t(c.label) }, geometry: { type: "Point" as const, coordinates: [p.lon, p.lat] } }];
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
      <CategoryPicker />
      <LocateButton />
      {contextMenu && <ContextMenu state={contextMenu} />}
    </div>
  );
}
