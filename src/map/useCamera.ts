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

/** Navigation: how long after the user's last pan / zoom the camera follows the vehicle again. */
const FOLLOW_RESUME_MS = 5000;
const RESUME_GLIDE_MS = 800;

/** Navigation camera padding: the vehicle sits in the lower part of the view. */
function followPadding(map: maplibregl.Map) {
  return { top: Math.round(map.getContainer().clientHeight * 0.4), bottom: 0, left: 0, right: 0 };
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
  // Smooth motion: fixes arrive about once a second (GPS or the simulator); between two of them
  // the vehicle and the camera glide on every animation frame instead of jumping.
  const shown = useRef<{ at: LonLat; bearing: number } | null>(null);
  const lastFixAt = useRef(0);
  const frame = useRef(0);
  // The user may pan / zoom / rotate while navigating: following pauses and comes back on its own
  // FOLLOW_RESUME_MS after the last touch of the map (with a short glide back to the vehicle).
  const pausedUntil = useRef(0);
  const resumeTimer = useRef(0);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady || !navigating) return;
    const follow = () => {
      const s = shown.current;
      if (!s || useStore.getState().navigation?.manualStep != null) return;
      // the glide back must not be cut by the per-frame jumpTo
      pausedUntil.current = performance.now() + RESUME_GLIDE_MS;
      map.easeTo({ center: s.at, bearing: s.bearing, pitch: 50, zoom: 17, padding: followPadding(map), duration: RESUME_GLIDE_MS });
    };
    const touched = () => {
      pausedUntil.current = performance.now() + FOLLOW_RESUME_MS;
      window.clearTimeout(resumeTimer.current);
      resumeTimer.current = window.setTimeout(follow, FOLLOW_RESUME_MS);
    };
    // the wheel / trackpad gestures (lib/gestures.ts) move the map without an originalEvent,
    // so listen on the canvas itself; MapLibre's own handlers and zoom buttons carry one
    const el = map.getCanvasContainer();
    const byUser = (e: { originalEvent?: unknown }) => {
      if (e.originalEvent) touched();
    };
    el.addEventListener("pointerdown", touched);
    el.addEventListener("wheel", touched, { passive: true });
    map.on("move", byUser);
    map.on("zoomstart", byUser);
    return () => {
      el.removeEventListener("pointerdown", touched);
      el.removeEventListener("wheel", touched);
      map.off("move", byUser);
      map.off("zoomstart", byUser);
      window.clearTimeout(resumeTimer.current);
      pausedUntil.current = 0;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [navigating, mapReady]);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady) return;
    if (!navigating) {
      cancelAnimationFrame(frame.current);
      shown.current = null;
      if (wasNavigating.current) {
        wasNavigating.current = false;
        map.easeTo({ bearing: 0, pitch: 0, padding: { top: 0, bottom: 0, left: 0, right: 0 }, duration: 600 });
      }
      return;
    }
    wasNavigating.current = true;
    if (!navUpdate) return;
    const target: LonLat = [navUpdate.snappedLon, navUpdate.snappedLat];
    // car / truck navigation draws the chosen vehicle (Settings); walking and cycling keep the dot
    const st = useStore.getState();
    const mode = st.navigation?.route.mode;
    const vehicle = mode === "car" || mode === "truck" ? (st.settings.navVehicle ?? "arrow") : null;
    const now = performance.now();
    // glide over about the time between fixes (the next one arrives as this glide ends)
    const duration = Math.min(1500, Math.max(250, lastFixAt.current ? now - lastFixAt.current : 900));
    lastFixAt.current = now;
    const from = shown.current ?? { at: target, bearing: navUpdate.bearing };
    // a big jump (first fix, a new route, a lost signal) is not animated
    const far = Math.abs(from.at[0] - target[0]) > 0.004 || Math.abs(from.at[1] - target[1]) > 0.004;
    const start = far ? { at: target, bearing: navUpdate.bearing } : from;
    // turn the short way round
    let dB = navUpdate.bearing - start.bearing;
    dB = ((dB + 540) % 360) - 180;
    cancelAnimationFrame(frame.current);
    const t0 = now;
    const step = () => {
      const k = Math.min(1, (performance.now() - t0) / duration);
      const at: LonLat = [start.at[0] + (target[0] - start.at[0]) * k, start.at[1] + (target[1] - start.at[1]) * k];
      const bearing = (start.bearing + dB * k + 360) % 360;
      shown.current = { at, bearing };
      meOverlay.showSnapped(map, at, vehicle, bearing);
      // browsing steps by hand: the vehicle still moves, the camera stays on the shown step
      // the user moving the map pauses following for a few seconds
      if (useStore.getState().navigation?.manualStep == null && performance.now() >= pausedUntil.current) {
        // keep the vehicle in the lower part of the view, as navigation apps do
        map.jumpTo({ center: at, bearing, pitch: 50, zoom: 17, padding: followPadding(map) });
      }
      if (k < 1) frame.current = requestAnimationFrame(step);
    };
    frame.current = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [navUpdate, navigating, mapReady, manualStep]);
}
