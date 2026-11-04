import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import * as maplibregl from "maplibre-gl";
import { Database, Wifi } from "lucide-react";
import { useStore, resolveTheme } from "../lib/store";
import { useT } from "../lib/i18n";
import { buildStyle, MAX_BOUNDS, isMergedTiles, type FlavorName } from "../lib/mapStyle";
import { POI_CATEGORIES } from "../lib/poiCategories";
import { CategoryPicker, LayersButton, LocateButton } from "./CategoryPicker";
import { fmtCoords } from "../lib/format";
import { api, errorCode } from "../lib/api";
import { ContextMenu } from "./ContextMenu";
import { MapHoverCard } from "./MapHoverCard";
import { NavigationView } from "./NavigationView";
import { contourTilesUrl } from "../lib/contours";
import { renderRouteImage } from "../lib/mapImage";
import { setPrintMapProvider } from "../lib/print";
import { mapColors } from "../map/colors";
import { BUS_ICON, drawBusIcon } from "../map/icons";
import { addAll, linePreview as lineOverlay, myLocation as meOverlay, pois as poiOverlay, routes as routeOverlay, transitStops as stopsOverlay, waypoints as wpOverlay } from "../map/overlays";
import { bindTrailPopup } from "../map/popups";
import { useHoverCard } from "../map/useHoverCard";
import { useMapInstance } from "../map/useMapInstance";
import { useViewportFetch } from "../map/useViewportFetch";
import { useCamera } from "../map/useCamera";

/**
 * The map: composition of the map instance (map/useMapInstance), the overlays (map/overlays/*),
 * the viewport-fed layers (POIs, transit stops), the hover card and the camera reactions to the
 * store (fly-to, route fit, navigation follow).
 */
export function MapView() {
  const t = useT();
  const containerRef = useRef<HTMLDivElement>(null);
  const markersRef = useRef<maplibregl.Marker[]>([]);
  const poiMarkerRef = useRef<maplibregl.Marker | null>(null);
  const popupRef = useRef<maplibregl.Popup | null>(null);
  const { hover, setHover, bind: bindHover, cardHandlers } = useHoverCard();

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
  const highlight = useStore((s) => s.highlight);
  const waypoints = useStore((s) => s.waypoints);
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
    api
      .trailsPath()
      .then((p) => setTrailsFile(p ? convertFileSrc(p) : null))
      .catch(() => setTrailsFile(null));
  }, [tilesTick]);
  const trailsUrl = layers.trails || topo ? trailsFile : null;
  const flavorRef = useRef(flavor);
  flavorRef.current = flavor;

  const styleOptions = () => {
    return { tilesUrl, onlineTilesUrl, satelliteUrl, satellite: layers.satellite, satelliteOrtho, terrainUrl, worldUrl, hillshade: layers.hillshade, terrain3d: layers.terrain3d, roadShields: layers.roadShields, flavor, lang: layers.labelLang, poi: layers.poi, buildings3d: layers.buildings3d, theme: layers.theme, contours: layers.contours, contourTiles, trails: layers.trails, trailsUrl };
  };

  /** Show a bubble on the map (one at a time). */
  const showPopup = (map: maplibregl.Map, p: maplibregl.Popup) => {
    popupRef.current?.remove();
    popupRef.current = p.addTo(map);
  };

  // ---- create map
  const [poiLayerTick, setPoiLayerTick] = useState(0);
  const { mapRef, mapReady, setMapReady, cursor } = useMapInstance(containerRef, {
    lang,
    initialStyle: () => {
      // the store's current URLs (the props above may still be the first render's)
      const st = useStore.getState();
      const o = styleOptions();
      return buildStyle({ ...o, tilesUrl: st.tilesUrl, onlineTilesUrl: st.onlineTilesUrl, satelliteUrl: st.satelliteUrl, satelliteOrtho: st.settings.satelliteSource !== "sentinel" || !!st.dataStatus?.satellite?.version?.startsWith("govmap-"), terrainUrl: st.terrainUrl, worldUrl: st.worldUrl });
    },
    onCreate: (map) => {
      routeOverlay.bindClicks(map, { onSelect: (id) => useStore.getState().selectRoute(id), showPopup: (p) => showPopup(map, p) });
      bindHover(map, () => popupRef.current?.remove());
      bindTrailPopup(map, (p) => showPopup(map, p));
    },
    onStyleLoad: (map) => {
      addAll(map, flavorRef.current === "dark", () => setPoiLayerTick((n) => n + 1));
      const s = useStore.getState();
      routeOverlay.sync(map, s.routes, s.selectedRouteId);
    },
  });

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
    setMapReady(false);
    map.setStyle(buildStyle(styleOptions()), { diff: false });
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
      if (map.hasImage(BUS_ICON)) map.updateImage(BUS_ICON, drawBusIcon(dark));
      const s = useStore.getState();
      routeOverlay.sync(map, s.routes, s.selectedRouteId);
    });
    return () => cancelAnimationFrame(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accentSetting, theme, mapReady]);

  // ---- routes
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    routeOverlay.sync(map, routes, selectedRouteId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [routes, selectedRouteId, mapReady]);

  // ---- favorite line preview
  const linePreview = useStore((s) => s.linePreview);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    lineOverlay.sync(map, linePreview?.shape ?? null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [linePreview, mapReady]);

  // ---- my location marker
  const myLocation = useStore((s) => s.myLocation);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    meOverlay.sync(map, myLocation);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [myLocation, mapReady]);

  // ---- POI category layer (viewport fetch, debounced)
  const poiCatsKey = layers.poiCats.join(",");
  useViewportFetch(
    mapRef,
    mapReady,
    {
      sourceId: poiOverlay.SRC_POIS,
      enabled: !!poiCatsKey,
      minZoom: poiOverlay.POIS_MIN_ZOOM,
      label: "poi_layer",
      load: async (bbox, zoom) => {
        const selectedCats = poiCatsKey.split(",").flatMap((id) => POI_CATEGORIES.filter((c) => c.id === id));
        const cats = [...new Set(selectedCats.flatMap((c) => c.cats))];
        const pois = await api.poiLayer(cats, bbox, poiOverlay.POIS_LIMIT, Math.floor(zoom));
        return poiOverlay.poiFeatures(pois, selectedCats, useStore.getState().t);
      },
    },
    [poiCatsKey, poiLayerTick],
  );

  // ---- transit stops layer (viewport fetch, debounced); the public-transport map always shows them
  const transitStopsOn = layers.transitStops || layers.theme === "transit";
  useViewportFetch(
    mapRef,
    mapReady,
    {
      sourceId: stopsOverlay.SRC_TSTOPS,
      enabled: transitStopsOn,
      minZoom: stopsOverlay.TSTOPS_MIN_ZOOM,
      label: "transit_stops",
      load: async (bbox) => {
        try {
          const stops = await api.transitStops(bbox, stopsOverlay.TSTOPS_LIMIT);
          useStore.getState().setTransitStopsUnavailable(false);
          return stopsOverlay.stopFeatures(stops);
        } catch (e) {
          if (errorCode(e) === "NO_TRANSIT_DATA") {
            useStore.getState().setTransitStopsUnavailable(true);
            return "stop";
          }
          throw e;
        }
      },
    },
    [],
  );

  // ---- highlight
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    routeOverlay.syncHighlight(map, routes, highlight);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [highlight, routes, mapReady]);

  // ---- waypoint markers
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    markersRef.current = wpOverlay.sync(map, markersRef.current, waypoints, (i, lon, lat) => useStore.getState().setWaypoint(i, { lon, lat, label: fmtCoords(lon, lat) }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [marker]);

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

  useCamera(mapRef, mapReady, lang);

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
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
      {hover && <MapHoverCard target={hover} {...cardHandlers} />}
      <CategoryPicker />
      <LocateButton />
      <LayersButton />
      {contextMenu && <ContextMenu state={contextMenu} />}
      <NavigationView />
    </div>
  );
}
