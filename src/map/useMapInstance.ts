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
export function ensureProtocol() {
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
  /** UI language: control texts and the side the controls go to (followed at runtime). */
  lang: string;
  /** "metric" | "imperial": the scale bar (followed at runtime). */
  units: string;
  /** Create the map only once this is true (report ד-7: after the tiles URL is known, so the
   *  first style is already the right one and is not rebuilt right away). */
  enabled: boolean;
  /** Extra event wiring, once, right after the map is created. */
  onCreate: (map: maplibregl.Map) => void;
  /** After every full style (re)load: add the overlays. */
  onStyleLoad: (map: maplibregl.Map) => void;
}

/** Press-and-hold on a touch screen opens the context menu (report ג-30). */
const LONG_PRESS_MS = 550;

/**
 * Create the MapLibre map once: start view (last view when allowed), controls, gestures,
 * context menu, view tracking, road-shield images. `mapReady` turns true after the first style
 * load (style updates are diffed afterwards and keep it true).
 */
export function useMapInstance(containerRef: RefObject<HTMLDivElement | null>, opts: MapInstanceOptions) {
  const mapRef = useRef<maplibregl.Map | null>(null);
  const [mapReady, setMapReady] = useState(false);
  const optsRef = useRef(opts);
  optsRef.current = opts;
  const controls = useRef<{ nav?: maplibregl.NavigationControl; attrib?: maplibregl.AttributionControl; scale?: maplibregl.ScaleControl; side?: string; lang?: string; defaultLocale?: Record<string, string> }>({});

  useEffect(() => {
    if (!opts.enabled || !containerRef.current || mapRef.current) return;
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
      // control texts: set per UI language by the controls effect below
    });
    mapRef.current = map;
    const detachGestures = attachWheelGestures(map, getScrollMode);
    // two-finger rotation on touch screens (pinch zoom stays)
    map.touchZoomRotate.enableRotation();
    map.on("moveend", () => useStore.getState().setMapView([map.getCenter().lng, map.getCenter().lat], map.getZoom()));
    map.on("contextmenu", (e) => {
      e.preventDefault();
      useStore.getState().setContextMenu({ x: e.point.x, y: e.point.y, lon: e.lngLat.lng, lat: e.lngLat.lat });
    });
    map.on("click", () => {
      useStore.getState().setContextMenu(null);
    });
    // long press (touch): the context menu, as a right click does with a mouse
    let press: ReturnType<typeof setTimeout> | undefined;
    const cancelPress = () => press && clearTimeout(press);
    map.on("touchstart", (e) => {
      cancelPress();
      if (e.originalEvent.touches.length !== 1) return;
      press = setTimeout(() => {
        useStore.getState().setContextMenu({ x: e.point.x, y: e.point.y, lon: e.lngLat.lng, lat: e.lngLat.lat });
      }, LONG_PRESS_MS);
    });
    map.on("touchend", cancelPress);
    map.on("touchcancel", cancelPress);
    map.on("touchmove", cancelPress);
    map.on("movestart", cancelPress);
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
      cancelPress();
      detachGestures();
      map.remove();
      mapRef.current = null;
      controls.current = {};
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts.enabled]);

  // ---- controls: on the side the sidebar leaves free, in the UI language and units (report א-11;
  // they used to be fixed at creation, so switching to English hid them under the panel)
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    const c = controls.current;
    const side = opts.lang === "he" ? "left" : "right";
    if (c.side !== side || c.lang !== opts.lang) {
      if (c.nav) map.removeControl(c.nav);
      if (c.attrib) map.removeControl(c.attrib);
      if (c.scale) map.removeControl(c.scale);
      // MapLibre's own control texts (zoom in / out, compass) in the UI language: the map reads
      // them from its (private) `_locale` when a control is built
      const m = map as unknown as { _locale: Record<string, string> };
      c.defaultLocale ??= { ...m._locale };
      m._locale = opts.lang === "he" ? { ...c.defaultLocale, ...MAPLIBRE_HE } : { ...c.defaultLocale };
      c.nav = new maplibregl.NavigationControl({ visualizePitch: true });
      // compact = the "i" button; the full credits open on click (the short credit line in
      // MapView links to the About section), so it never covers the map (report א-4)
      c.attrib = new maplibregl.AttributionControl({ compact: true });
      c.scale = new maplibregl.ScaleControl({ maxWidth: 120, unit: opts.units === "imperial" ? "imperial" : "metric" });
      map.addControl(c.nav, `top-${side}`);
      map.addControl(c.attrib, `bottom-${side}`);
      map.addControl(c.scale, `bottom-${side}`);
      c.side = side;
      c.lang = opts.lang;
      // start collapsed (MapLibre opens a compact attribution on wide maps)
      requestAnimationFrame(() => map.getContainer().querySelector(".maplibregl-ctrl-attrib")?.classList.remove("maplibregl-compact-show"));
    }
    c.scale?.setUnit(opts.units === "imperial" ? "imperial" : "metric");
  }, [opts.lang, opts.units, mapReady]);

  return { mapRef, mapReady, setMapReady };
}
