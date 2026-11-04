import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import * as maplibregl from "maplibre-gl";
import type { FeatureCollection } from "geojson";
import { InfoBar } from "./InfoBar";
import { useStore, resolveTheme } from "../lib/store";
import { useT } from "../lib/i18n";
import { buildStyle, firstSymbolLayer, MAX_BOUNDS, isMergedTiles, ONLINE_TERRAIN_URL, type FlavorName, type StyleOptions } from "../lib/mapStyle";
import { POI_CATEGORIES } from "../lib/poiCategories";
import { CategoryPicker, LayersButton, LocateButton } from "./CategoryPicker";
import { fmtCoords, fmtDistance } from "../lib/format";
import type { StyleSpecification } from "maplibre-gl";
import { api, apiX, errorCode } from "../lib/api";
import { ContextMenu } from "./ContextMenu";
import { MapHoverCard } from "./MapHoverCard";
import { NavigationView } from "./NavigationView";
import { routeImage } from "../lib/mapImage";
import { setPrintMapProvider } from "../lib/print";
import { hexColor, mapColors } from "../map/colors";
import { BUS_ICON, drawBusIcon, loadPoiIcons } from "../map/icons";
import { addAll, favorites as favOverlay, linePreview as lineOverlay, myLocation as meOverlay, pois as poiOverlay, routes as routeOverlay, transitStops as stopsOverlay, tools, waypoints as wpOverlay } from "../map/overlays";
import { bindTrailPopup } from "../map/popups";
import { toolActive, useHoverCard } from "../map/useHoverCard";
import { useMapInstance } from "../map/useMapInstance";
import { useViewportFetch } from "../map/useViewportFetch";
import { useCamera } from "../map/useCamera";
import { keepOverlays } from "../map/styleDiff";
import { CursorReadout, MapCredits } from "./MapFooter";
import { DrawToolbar, IsochroneLegend, MeasureBar, TrackRecorder } from "./MapTools";
import { useWindowFileDrop, userLayerData } from "../map/userLayerData";

// dialogs and the compare map load on first use (report ד-7)
const UserLayersPanel = lazy(() => import("./UserLayersPanel").then((m) => ({ default: m.UserLayersPanel })));
const ImportDialog = lazy(() => import("./UserLayersPanel").then((m) => ({ default: m.ImportDialog })));
const CustomLayersDialog = lazy(() => import("./CustomLayersDialog").then((m) => ({ default: m.CustomLayersDialog })));
const CompareView = lazy(() => import("./CompareView").then((m) => ({ default: m.CompareView })));

/** Live vehicles of a previewed line: refreshed this often while online (report ג-25). */
const VEHICLES_MS = 30_000;

/**
 * The map: composition of the map instance (map/useMapInstance), the overlays (map/overlays/*),
 * the viewport-fed layers (POIs, transit stops), the hover card, the map tools (measure, draw,
 * isochrone, import / user layers, track, compare) and the camera reactions to the store.
 */
export function MapView() {
  const t = useT();
  const containerRef = useRef<HTMLDivElement>(null);
  const markersRef = useRef<(maplibregl.Marker | null)[]>([]);
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
  const tilesChecked = useStore((s) => s.tilesChecked);
  const layers = useStore((s) => s.layers);
  const theme = useStore((s) => s.settings.theme);
  const customLayers = useStore((s) => s.settings.customLayers);
  const routes = useStore((s) => s.routes);
  const selectedRouteId = useStore((s) => s.selectedRouteId);
  const highlight = useStore((s) => s.highlight);
  const waypoints = useStore((s) => s.waypoints);
  const marker = useStore((s) => s.marker);
  const contextMenu = useStore((s) => s.contextMenu);
  const lang = useStore((s) => s.settings.language);
  const units = useStore((s) => s.settings.units);
  const navigating = useStore((s) => !!s.navigation);
  // online stand-ins for elevation and trails when their data is not installed
  const onlineOk = useStore((s) => s.online && s.settings.routingMode !== "offline");

  const flavor: FlavorName = layers.flavor === "auto" ? resolveTheme(theme) : layers.flavor;
  const topo = layers.theme === "topo";
  // contour lines: computed in the browser from the elevation archive (lib/contours.ts, loaded
  // with maplibre-contour only once contours or the topo theme are switched on)
  const [contourTiles, setContourTiles] = useState<string | null>(null);
  useEffect(() => {
    const demUrl = terrainUrl ?? (onlineOk ? ONLINE_TERRAIN_URL : null);
    if (!demUrl || !(layers.contours || topo)) {
      setContourTiles(null);
      return;
    }
    let dead = false;
    void import("../lib/contours").then((m) => !dead && setContourTiles(m.contourTilesUrl(demUrl)));
    return () => {
      dead = true;
    };
  }, [terrainUrl, onlineOk, layers.contours, topo]);
  // marked trails file (built by the data update; absent until installed). Asked for only when the
  // trails or the topo map are on: the 5–6 MB GeoJSON is not parsed otherwise.
  const trailsOn = layers.trails || topo;
  const [trailsFile, setTrailsFile] = useState<string | null>(null);
  useEffect(() => {
    if (!trailsOn) return;
    api
      .trailsPath()
      .then((p) => setTrailsFile(p ? convertFileSrc(p) : null))
      .catch(() => setTrailsFile(null));
  }, [tilesTick, trailsOn]);
  const trailsUrl = trailsOn ? trailsFile : null;
  const flavorRef = useRef(flavor);
  flavorRef.current = flavor;

  const styleOptions = (): StyleOptions => {
    return { tilesUrl, onlineTilesUrl, satelliteUrl, satellite: layers.satellite, satelliteRoads: layers.satelliteRoads, satelliteLabels: layers.satelliteLabels, satelliteOrtho, terrainUrl, worldUrl, hillshade: layers.hillshade, terrain3d: layers.terrain3d, roadShields: layers.roadShields, flavor, lang: layers.labelLang, poi: layers.poi, buildings3d: layers.buildings3d, theme: layers.theme, contours: layers.contours, contourTiles, trails: layers.trails, trailsUrl, customLayers, onlineTerrain: onlineOk, onlineTrails: onlineOk };
  };

  /** Show a bubble on the map (one at a time). */
  const showPopup = (map: maplibregl.Map, p: maplibregl.Popup) => {
    popupRef.current?.remove();
    popupRef.current = p.addTo(map);
  };

  /** The style the map shows now (JSON), to skip identical updates. */
  const lastStyle = useRef<string | null>(null);

  // ---- create map (once the tiles URL is known: the first style is then the right one)
  const [poiLayerTick, setPoiLayerTick] = useState(0);
  const { mapRef, mapReady } = useMapInstance(containerRef, {
    lang,
    units,
    enabled: tilesChecked,
    initialStyle: () => {
      const style = buildStyle(styleOptions());
      lastStyle.current = JSON.stringify(style);
      return style;
    },
    onCreate: (map) => {
      routeOverlay.bindClicks(map, { onSelect: (id) => !toolActive() && useStore.getState().selectRoute(id), showPopup: (p) => !toolActive() && showPopup(map, p) });
      bindHover(map, () => popupRef.current?.remove());
      bindTrailPopup(map, (p) => !toolActive() && showPopup(map, p));
      // map tools take the clicks: a measure point, a drawing vertex
      map.on("click", (e) => {
        const s = useStore.getState();
        const at: [number, number] = [e.lngLat.lng, e.lngLat.lat];
        if (s.measure) s.addMeasurePoint(at);
        else if (s.drawing) s.addDrawPoint(at);
      });
      map.on("dblclick", (e) => {
        const s = useStore.getState();
        if (s.drawing) {
          e.preventDefault();
          s.finishDrawFeature();
        } else if (s.measure) e.preventDefault();
      });
    },
    onStyleLoad: (map) => {
      addAll(map, flavorRef.current === "dark", () => setPoiLayerTick((n) => n + 1));
      const s = useStore.getState();
      routeOverlay.sync(map, s.routes, s.selectedRouteId);
      setStyleLoads((n) => n + 1);
    },
  });
  /** Bumped by every full style load (overlays re-added: their data is synced again). */
  const [styleLoads, setStyleLoads] = useState(0);

  // ---- style updates: diffed (report ד-1). A layer switch changes only that layer's visibility
  // or source; the overlays are carried over (keepOverlays) and keep their data; MapLibre falls
  // back to a full reload only for changes it cannot diff (then style.load re-adds the overlays).
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    // with a world basemap the map may leave Israel and zoom out to the planet
    const planet = !!worldUrl || isMergedTiles(tilesUrl) || (!tilesUrl && !!onlineTilesUrl);
    map.setMaxBounds(planet ? null : MAX_BOUNDS);
    map.setMinZoom(planet ? 1 : 5);
    const next = buildStyle(styleOptions());
    const key = JSON.stringify(next);
    if (key === lastStyle.current) return;
    lastStyle.current = key;
    map.setStyle(next, { diff: true, transformStyle: (prev, nextStyle) => keepOverlays(prev as StyleSpecification | undefined, nextStyle) });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mapRef.current, tilesUrl, onlineTilesUrl, satelliteUrl, satelliteOrtho, terrainUrl, worldUrl, tilesTick, flavor, layers.labelLang, layers.poi, layers.buildings3d, layers.satellite, layers.satelliteRoads, layers.satelliteLabels, layers.hillshade, layers.terrain3d, layers.roadShields, layers.theme, layers.contours, layers.trails, contourTiles, trailsUrl, customLayers, onlineOk]);

  // ---- colour scheme / theme / map flavour: the overlays take their colours from the CSS tokens,
  // which change without a style rebuild. Re-read them after App has set data-accent / data-theme.
  const accentSetting = useStore((s) => s.settings.accent);
  const vehicleColorSetting = useStore((s) => s.settings.navVehicleColor);
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
      // the vehicle markers are drawn in the chosen body colour (or the accent): redraw them
      meOverlay.setVehicleColor(useStore.getState().settings.navVehicleColor);
      meOverlay.addVehicleImages(map);
      paint("route-alts", "line-color", mc.alt);
      paint("route-hl", "line-color", mc.highlight);
      paint("route-alts-casing", "line-color", mc.casing);
      paint("route-sel-casing", "line-color", mc.casing);
      paint("route-stops-label", "text-color", mc.label);
      paint("transit-stops-label", "text-color", mc.stopLabel);
      paint("transit-stops-label", "text-halo-color", mc.halo);
      paint("poi-layer", "text-halo-color", mc.halo);
      paint("line-preview-casing", "line-color", mc.casing);
      if (map.hasImage(BUS_ICON)) map.updateImage(BUS_ICON, drawBusIcon(dark));
      // the category badges are drawn per flavour: redraw them for the new one
      for (const c of POI_CATEGORIES) if (map.hasImage(`poi-${c.id}`)) map.removeImage(`poi-${c.id}`);
      void loadPoiIcons(map, dark);
      const s = useStore.getState();
      routeOverlay.sync(map, s.routes, s.selectedRouteId);
    });
    return () => cancelAnimationFrame(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accentSetting, theme, flavor, mapReady, vehicleColorSetting]);

  // ---- routes
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    routeOverlay.sync(map, routes, selectedRouteId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [routes, selectedRouteId, mapReady, styleLoads]);

  // ---- favorite line preview (+ its live vehicles when online)
  const linePreview = useStore((s) => s.linePreview);
  const online = useStore((s) => s.online);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    lineOverlay.sync(map, linePreview?.shape ?? null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [linePreview, mapReady]);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    const shape = linePreview?.shape;
    if (!shape || !online) {
      tools.syncVehicles(map, [], "#000");
      return;
    }
    const color = shape.color ? hexColor(shape.color) : mapColors(false).bus;
    let dead = false;
    const load = () =>
      apiX
        .transitVehicles(shape.shortName, shape.agency)
        .then((v) => !dead && tools.syncVehicles(map, v, color))
        .catch(() => !dead && tools.syncVehicles(map, [], color));
    void load();
    const timer = setInterval(load, VEHICLES_MS);
    return () => {
      dead = true;
      clearInterval(timer);
      if (map.getStyle()) tools.syncVehicles(map, [], color);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [linePreview, online, mapReady]);

  // ---- favorite groups shown on the map (favorites panel eye toggle)
  const favorites = useStore((s) => s.favorites);
  const visibleFavGroups = useStore((s) => s.visibleFavoriteGroups);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    favOverlay.sync(map, favorites, visibleFavGroups);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [favorites, visibleFavGroups, mapReady, styleLoads]);

  // ---- my location marker
  const myLocation = useStore((s) => s.myLocation);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    meOverlay.sync(map, myLocation);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [myLocation, mapReady, styleLoads]);

  // ---- POI category layer (viewport fetch, debounced, padded box)
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
    [styleLoads],
  );

  // ---- highlight
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    routeOverlay.syncHighlight(map, routes, highlight);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [highlight, routes, mapReady]);

  // ---- waypoint pins (moved in place, report ד-8)
  useEffect(() => {
    const map = mapRef.current;
    if (!map) return;
    markersRef.current = wpOverlay.sync(map, markersRef.current, waypoints, (i, lon, lat) => useStore.getState().setWaypoint(i, { lon, lat, label: fmtCoords(lon, lat) }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [waypoints, mapReady]);

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
  }, [marker, mapReady]);

  // ---- map tools: measure, drawing, isochrone, recorded track
  const measure = useStore((s) => s.measure);
  const drawing = useStore((s) => s.drawing);
  const isochrone = useStore((s) => s.isochrone);
  const trackPoints = useStore((s) => s.track.update?.points ?? null);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    tools.syncMeasure(map, measure, (m) => fmtDistance(m, useStore.getState().t, units));
    tools.syncDraw(map, drawing);
    tools.syncIsochrone(map, isochrone?.at ?? null, isochrone?.bands ?? null);
    tools.syncTrack(map, trackPoints);
    // a tool takes the clicks: crosshair cursor, no double-click zoom
    const active = !!(measure || drawing);
    map.getCanvas().style.cursor = active ? "crosshair" : "";
    if (active) map.doubleClickZoom.disable();
    else map.doubleClickZoom.enable();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [measure, drawing, isochrone, trackPoints, mapReady, styleLoads, units]);
  // areas the route avoids (drawn with "avoid this area"), while a trip is being planned
  const avoidPolygons = useStore((s) => (s.waypoints.some(Boolean) ? s.avoid.polygons : undefined));
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    tools.syncAvoid(map, avoidPolygons);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [avoidPolygons, mapReady, styleLoads]);
  // the isochrone's area in view
  useEffect(() => {
    const map = mapRef.current;
    const b = isochrone?.bands;
    if (!map || !b?.length) return;
    const pts = b.flatMap((x) => x.rings.flat());
    if (!pts.length) return;
    const lons = pts.map((p) => p[0]);
    const lats = pts.map((p) => p[1]);
    map.fitBounds([[Math.min(...lons), Math.min(...lats)], [Math.max(...lons), Math.max(...lats)]], { padding: 80, duration: 600, maxZoom: 15 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isochrone?.bands]);

  // ---- user layers (imported files, drawings, tracks)
  const userLayers = useStore((s) => s.userLayers);
  const userLayersTick = useStore((s) => s.userLayersTick);
  const userData = useRef(new Map<string, FeatureCollection>());
  useEffect(() => {
    if (!mapReady) return;
    void useStore.getState().loadUserLayers();
    // the address-list dialog (and other non-map code) saves layers and announces it
    const reload = () => void useStore.getState().loadUserLayers();
    window.addEventListener("user-layers-changed", reload);
    return () => window.removeEventListener("user-layers-changed", reload);
  }, [mapReady]);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    let dead = false;
    void (async () => {
      const visible = userLayers.filter((l) => l.visible);
      await Promise.all(
        visible.map(async (l) => {
          try {
            userData.current.set(l.id, await userLayerData(l.id, userLayersTick));
          } catch (e) {
            console.warn("user layer", l.id, e);
          }
        }),
      );
      if (dead || !map.getStyle()) return;
      tools.syncUserLayers(map, userLayers, userData.current, firstSymbolLayer(map.getStyle() as StyleSpecification));
    })();
    return () => {
      dead = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [userLayers, userLayersTick, mapReady, styleLoads]);
  // fit to an imported file / a user layer
  const fitBox = useStore((s) => s.fitBox);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !fitBox) return;
    const [w, s, e, n] = fitBox.box;
    map.fitBounds([[w, s], [e, n]], { padding: 80, duration: 700, maxZoom: 16 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fitBox]);
  useWindowFileDrop();

  // ---- printout: a light, north-up picture of the selected route (lib/mapImage.ts); drawn in
  // the background as soon as a route is selected, so Print rarely has to wait (report ד-23)
  useEffect(() => {
    setPrintMapProvider(async (opts) => {
      const s = useStore.getState();
      const r = s.routes.find((x) => x.id === s.selectedRouteId) ?? s.routes[0];
      if (!r || r.geometry.length < 2) return null;
      try {
        return await routeImage(r, opts);
      } catch (e) {
        console.warn("print map image", e);
        return null;
      }
    });
    return () => setPrintMapProvider(null);
  }, []);
  useEffect(() => {
    const r = routes.find((x) => x.id === selectedRouteId);
    if (!r || r.geometry.length < 2 || navigating) return;
    const idle = (window as unknown as { requestIdleCallback?: (cb: () => void, o?: { timeout: number }) => number }).requestIdleCallback;
    const run = () => void routeImage(r, { scale: "fit", grid: false }).catch(() => undefined);
    const id = idle ? idle(run, { timeout: 4000 }) : window.setTimeout(run, 1500);
    return () => {
      const cancel = (window as unknown as { cancelIdleCallback?: (id: number) => void }).cancelIdleCallback;
      if (idle && cancel) cancel(id);
      else clearTimeout(id);
    };
  }, [routes, selectedRouteId, navigating]);

  useCamera(mapRef, mapReady, lang);

  // ---- keyboard: zoom, Escape, tool keys, full screen
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const map = mapRef.current;
      if (!map) return;
      if (e.key === "F11") {
        e.preventDefault();
        void (async () => {
          try {
            const { getCurrentWindow } = await import("@tauri-apps/api/window");
            const w = getCurrentWindow();
            await w.setFullscreen(!(await w.isFullscreen()));
          } catch (err) {
            console.warn("fullscreen", err);
          }
        })();
        return;
      }
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      const s = useStore.getState();
      if (e.key === "+" || e.key === "=") map.zoomIn();
      else if (e.key === "-") map.zoomOut();
      else if (e.key === "Backspace" && (s.measure || s.drawing)) {
        e.preventDefault();
        if (s.measure) s.undoMeasurePoint();
        else s.undoDrawPoint();
      } else if (e.key === "Enter" && s.drawing) {
        e.preventDefault();
        s.finishDrawFeature();
      } else if (e.key === "Escape") {
        // one thing at a time: a tool, then menus and bubbles
        if (s.measure) return s.stopMeasure();
        if (s.drawing) return s.cancelDrawing();
        if (s.isochrone) return s.clearIsochrone();
        if (s.compare !== "off") return s.setCompare("off");
        s.setContextMenu(null);
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
  const compare = useStore((s) => s.compare);
  const importOpen = useStore((s) => s.importOpen);
  const mapPopover = useStore((s) => s.mapPopover);
  // The "no map yet" card points at the Data panel, so it hides while that panel is open.
  const noTiles = tilesChecked && !tilesUrl && !onlineTilesUrl && !(panel === "data" && !sidebarCollapsed);
  const onlineMap = tilesChecked && !tilesUrl && !!onlineTilesUrl;
  const setPanel = useStore((s) => s.setPanel);

  return (
    <div className={navigating ? "map-root navigating absolute inset-0" : "map-root absolute inset-0"}>
      <div ref={containerRef} className="absolute inset-0" />
      {!navigating && <CursorReadout mapRef={mapRef} ready={mapReady} />}
      {!navigating && !onlineMap && <MapCredits />}
      {noTiles && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <InfoBar
            severity="info"
            className="w-[360px] max-w-[calc(100%-32px)] pointer-events-auto fade-in shadow-[var(--shadow-16)] !bg-[var(--surface)]"
            title={t("noTilesTitle")}
            action={
              <button className="btn btn-primary btn-sm" onClick={() => setPanel("data")}>
                {t("goToData")}
              </button>
            }
          >
            {t("noTilesBody")}
          </InfoBar>
        </div>
      )}
      {onlineMap && (
        <div className="absolute bottom-10 start-1/2 -translate-x-1/2 rtl:translate-x-1/2 w-[420px] max-w-[calc(100%-32px)] no-print fade-in">
          <InfoBar
            severity="info"
            className="shadow-[var(--shadow-8)] !bg-[var(--surface)]"
            action={
              <button className="btn btn-primary btn-sm" onClick={() => setPanel("data")}>
                {t("onlineMapDownload")}
              </button>
            }
          >
            {t("onlineMapBanner")}
          </InfoBar>
        </div>
      )}
      {hover && <MapHoverCard target={hover} {...cardHandlers} />}
      <CategoryPicker />
      <LocateButton />
      <LayersButton />
      {contextMenu && <ContextMenu state={contextMenu} />}
      <MeasureBar />
      <DrawToolbar />
      <IsochroneLegend />
      <TrackRecorder />
      <Suspense fallback={null}>
        {compare !== "off" && mapReady && <CompareView mainRef={mapRef} />}
        {mapPopover === "userLayers" && <UserLayersPanel />}
        {mapPopover === "customLayers" && <CustomLayersDialog />}
        {importOpen && <ImportDialog />}
      </Suspense>
      <NavigationView />
    </div>
  );
}
