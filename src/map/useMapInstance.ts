import { useEffect, useRef, useState, type RefObject } from "react";
import * as maplibregl from "maplibre-gl";
// MapLibre 6 is ESM with a separate worker module: let Vite emit it and tell MapLibre where it is
// (it otherwise looks next to its own module URL, which the bundle does not have).
import maplibreWorkerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import type { StyleSpecification } from "maplibre-gl";
import { Protocol } from "pmtiles";
import type { LonLat } from "../lib/api";
import { attachWheelGestures, getScrollMode } from "../lib/gestures";
import { DEFAULT_CENTER, DEFAULT_ZOOM, MAX_BOUNDS } from "../lib/mapStyle";
import { getLastView, getRememberView, setLastView } from "../lib/prefs";
import { useStore } from "../lib/store";
import { shieldImage } from "./icons";
import { MAPLIBRE_HE } from "./locale";

let protocolRegistered = false;
function ensureProtocol() {
  if (protocolRegistered) return;
  maplibregl.setWorkerUrl(maplibreWorkerUrl);
  const protocol = new Protocol();
  maplibregl.addProtocol("pmtiles", protocol.tile);
  // MapLibre 6 draws right-to-left text itself: no RTL plugin any more.
  protocolRegistered = true;
}

export interface MapInstanceOptions {
  /** The style the map is created with (read once). */
  initialStyle: () => StyleSpecification;
  /** UI language at creation: control texts and the side the controls go to. */
  lang: string;
  /** Extra event wiring, once, right after the map is created. */
  onCreate: (map: maplibregl.Map) => void;
  /** After every style (re)load: add the overlays. */
  onStyleLoad: (map: maplibregl.Map) => void;
}

/**
 * Create the MapLibre map once: start view (last view when allowed), controls, gestures,
 * context menu, view tracking, road-shield images. `mapReady` turns true after each style load
 * (set it false before a setStyle); `cursor` is the pointer position for the coordinate readout.
 */
export function useMapInstance(containerRef: RefObject<HTMLDivElement | null>, opts: MapInstanceOptions) {
  const mapRef = useRef<maplibregl.Map | null>(null);
  const [mapReady, setMapReady] = useState(false);
  const [cursor, setCursor] = useState<LonLat | null>(null);
  const optsRef = useRef(opts);
  optsRef.current = opts;

  useEffect(() => {
    if (!containerRef.current || mapRef.current) return;
    ensureProtocol();
    const o = optsRef.current;
    // last map view (when the setting allows it; mirrored in localStorage for this early read)
    let start: { center: LonLat; zoom: number } = { center: DEFAULT_CENTER, zoom: DEFAULT_ZOOM };
    if (getRememberView()) {
      const saved = getLastView();
      if (saved) start = { center: [saved.lon, saved.lat], zoom: saved.zoom };
    }
    const map = new maplibregl.Map({
      container: containerRef.current,
      style: o.initialStyle(),
      center: start.center,
      zoom: start.zoom,
      maxZoom: 20,
      minZoom: 5,
      maxBounds: MAX_BOUNDS,
      attributionControl: false,
      canvasContextAttributes: { antialias: true },
      // MapLibre's own control tooltips / aria labels in the UI language
      locale: o.lang === "he" ? MAPLIBRE_HE : {},
    });
    mapRef.current = map;
    const detachGestures = attachWheelGestures(map, getScrollMode);
    // The sidebar floats on the start side (right in Hebrew); every map control goes to the
    // other side so the panel never hides it (the OSM attribution is a licence requirement).
    const freeSide = o.lang === "he" ? "left" : "right";
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
    o.onCreate(map);
    map.on("style.load", () => {
      optsRef.current.onStyleLoad(map);
      setMapReady(true);
    });
    let viewTimer: ReturnType<typeof setTimeout> | undefined;
    map.on("moveend", () => {
      if (viewTimer) clearTimeout(viewTimer);
      viewTimer = setTimeout(() => {
        const c = map.getCenter();
        setLastView({ lon: c.lng, lat: c.lat, zoom: map.getZoom() });
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

  return { mapRef, mapReady, setMapReady, cursor };
}
