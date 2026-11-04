import { useEffect, useRef, type RefObject } from "react";
import type * as maplibregl from "maplibre-gl";
import type { LonLat } from "../lib/api";
import { useStore } from "../lib/store";
import { flatSteps } from "../lib/nav";
import { myLocation as meOverlay, routes as routeOverlay } from "./overlays";

/** Width the side panel takes from the map (route fit padding): measured, 480 as a fallback. */
function panelPad(): number {
  const el = document.querySelector<HTMLElement>("[data-sidebar]");
  const w = el?.getBoundingClientRect().width;
  return w && w > 0 ? Math.round(w) + 24 : 480;
}

/**
 * Camera reactions to the store: fly-to requests, fitting new routes, the "N" shortcut, tilting
 * for 3D buildings, and following the vehicle while navigating (heading-up, tilted; back to
 * north-up and flat when the session ends).
 */
export function useCamera(mapRef: RefObject<maplibregl.Map | null>, mapReady: boolean, lang: string): void {
  // ---- fly to
  const flyTo = useStore((s) => s.flyTo);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !flyTo) return;
    map.flyTo({ center: flyTo.lonlat, zoom: flyTo.zoom ?? Math.max(map.getZoom(), 15), duration: 800 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [flyTo]);

  // ---- fit bounds on new routes
  const fitRouteTick = useStore((s) => s.fitRouteTick);
  useEffect(() => {
    const map = mapRef.current;
    const s = useStore.getState();
    if (!map || !s.routes.length) return;
    routeOverlay.fit(map, s.routes, s.sidebarCollapsed ? 0 : panelPad(), lang === "he" ? "right" : "left");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fitRouteTick]);

  // ---- "N": north up, flat
  const northUpTick = useStore((s) => s.northUpTick);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !northUpTick) return;
    map.easeTo({ bearing: 0, pitch: 0, duration: 500 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [northUpTick]);

  // ---- 3D buildings: tilt the camera when switched on (extrusions are invisible from straight
  // above and below zoom 14), level it again when switched off.
  const buildings3d = useStore((s) => s.layers.buildings3d);
  const first3d = useRef(true);
  useEffect(() => {
    const map = mapRef.current;
    if (first3d.current) {
      first3d.current = false;
      return;
    }
    if (!map) return;
    if (buildings3d) {
      const z = map.getZoom();
      map.easeTo({ pitch: Math.max(map.getPitch(), 55), zoom: z < 15 ? 15.5 : z, duration: 900 });
    } else if (map.getPitch() > 0) {
      map.easeTo({ pitch: 0, duration: 500 });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [buildings3d]);

  // ---- live navigation
  const navUpdate = useStore((s) => s.navigation?.update ?? null);
  const navigating = useStore((s) => !!s.navigation);
  const manualStep = useStore((s) => s.navigation?.manualStep ?? null);
  // manual step browsing: show that maneuver, heading-up, instead of following the GPS
  useEffect(() => {
    const map = mapRef.current;
    const n = useStore.getState().navigation;
    if (!map || !mapReady || !n || manualStep === null) return;
    const st = flatSteps(n.route)[manualStep];
    if (!st) return;
    map.easeTo({ center: st.at, bearing: st.bearing, pitch: 50, zoom: 17, offset: [0, Math.round(map.getContainer().clientHeight * 0.15)], duration: 700 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [manualStep, mapReady]);
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
    // browsing steps by hand: the GPS marker still moves, the camera stays on the shown step
    const browsing = useStore.getState().navigation?.manualStep != null;
    const at: LonLat = [navUpdate.snappedLon, navUpdate.snappedLat];
    meOverlay.showSnapped(map, at);
    if (browsing) return;
    // keep the vehicle in the lower third of the view, as navigation apps do
    map.easeTo({
      center: at,
      bearing: navUpdate.bearing,
      pitch: 50,
      zoom: 17,
      offset: [0, Math.round(map.getContainer().clientHeight * 0.2)],
      duration: 900,
      easing: (x) => x,
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [navUpdate, navigating, mapReady, manualStep]);
}
