// Wheel / trackpad gestures for the map.
//
// MapLibre's built-in scroll-zoom treats every wheel event as a zoom request and damps it through
// a sigmoid, which makes a precision touchpad feel sluggish: a pinch barely moves the zoom and a
// two-finger swipe zooms instead of travelling. This module replaces it with three distinct
// gestures:
//
//   * pinch (Chromium reports it as a wheel event with ctrlKey) -> fast zoom around the cursor
//   * mouse wheel notch                                            -> stepped, animated zoom
//   * two-finger scroll                                            -> pan (default) or zoom
//
// The two-finger behaviour is a per-device preference stored in localStorage because it depends on
// the hardware in front of the user, not on the account.
import type { LngLat, Map as MLMap } from "maplibre-gl";
import { getPrefString, setPrefString } from "./prefs";

export type ScrollMode = "pan" | "zoom";

export function getScrollMode(): ScrollMode {
  return getPrefString("mapsoffline.scrollMode") === "zoom" ? "zoom" : "pan";
}

export function setScrollMode(m: ScrollMode): void {
  setPrefString("mapsoffline.scrollMode", m);
}

/** Zoom levels per pinch delta unit. Chromium encodes a pinch as deltaY ≈ -100·ln(scale). */
const PINCH_RATE = 0.022;
/** Zoom levels per pixel of two-finger scroll when the preference is "zoom". */
const SCROLL_ZOOM_RATE = 0.006;
/** Zoom levels per mouse-wheel notch. */
const WHEEL_STEP = 0.8;
const WHEEL_DURATION = 200;
const MAX_STEP = 1.5;
/** macOS Chromium reports mouse notches as multiples of this value. */
const MAC_NOTCH = 4.000244140625;

/** Heuristic: a physical mouse wheel vs. a precision touchpad. */
export function isMouseWheel(e: WheelEvent): boolean {
  if (e.deltaMode !== WheelEvent.DOM_DELTA_PIXEL) return true;
  if (e.deltaX !== 0) return false;
  const d = Math.abs(e.deltaY);
  if (d < 40) return false;
  return d % 100 === 0 || d % 120 === 0 || d % MAC_NOTCH === 0;
}

function easeOut(t: number): number {
  return 1 - Math.pow(1 - t, 3);
}

/**
 * WebView2 only forwards touchpad pinches to the page when its own zoom control is enabled
 * (Tauri `zoomHotkeysEnabled`), so the page must veto browser zoom itself: ctrl + wheel / pinch
 * anywhere, and the Ctrl+plus / minus / 0 shortcuts.
 */
export function installPageZoomGuard(): () => void {
  const onWheel = (e: WheelEvent) => {
    if (e.ctrlKey) e.preventDefault();
  };
  const onKey = (e: KeyboardEvent) => {
    if ((e.ctrlKey || e.metaKey) && ["=", "+", "-", "_", "0"].includes(e.key)) e.preventDefault();
  };
  document.addEventListener("wheel", onWheel, { passive: false });
  document.addEventListener("keydown", onKey);
  return () => {
    document.removeEventListener("wheel", onWheel);
    document.removeEventListener("keydown", onKey);
  };
}

/**
 * Replace MapLibre's scroll zoom with gesture-aware handling. Returns a detach function.
 */
export function attachWheelGestures(map: MLMap, mode: () => ScrollMode): () => void {
  map.scrollZoom.disable();
  const el = map.getCanvasContainer();
  let wheelTarget: number | null = null;
  let wheelTimer: ReturnType<typeof setTimeout> | undefined;

  const around = (e: WheelEvent): LngLat => {
    const r = el.getBoundingClientRect();
    return map.unproject([e.clientX - r.left, e.clientY - r.top]);
  };
  const clampZoom = (z: number) => Math.min(map.getMaxZoom(), Math.max(map.getMinZoom(), z));

  let logged = 0;
  const onWheel = (e: WheelEvent) => {
    if (e.defaultPrevented) return;
    e.preventDefault();
    if (import.meta.env.DEV && e.ctrlKey && logged < 5) {
      logged++;
      console.warn(`wheel ctrl=${e.ctrlKey} mode=${e.deltaMode} dx=${e.deltaX} dy=${e.deltaY}`);
    }
    let dy = e.deltaMode === WheelEvent.DOM_DELTA_LINE ? e.deltaY * 40 : e.deltaMode === WheelEvent.DOM_DELTA_PAGE ? e.deltaY * 400 : e.deltaY;
    let dx = e.deltaMode === WheelEvent.DOM_DELTA_LINE ? e.deltaX * 40 : e.deltaX;
    if (dy === 0 && dx === 0) return;

    const mouse = isMouseWheel(e);

    // Pinch on a touchpad: Chromium reports it as ctrl + wheel with small deltas. Zoom at once,
    // around the fingers. (Ctrl + a real mouse notch falls through to the stepped path below.)
    if (e.ctrlKey && !mouse) {
      const step = Math.max(-MAX_STEP, Math.min(MAX_STEP, -dy * PINCH_RATE));
      wheelTarget = null;
      map.stop();
      map.zoomTo(clampZoom(map.getZoom() + step), { around: around(e), duration: 0 });
      return;
    }

    // Mouse wheel: stepped zoom, chained so a fast spin keeps accelerating instead of waiting
    // for each animation to finish.
    if (mouse) {
      const dir = dy > 0 ? -1 : 1;
      const base = wheelTarget ?? map.getZoom();
      const step = e.ctrlKey ? 1 : e.shiftKey ? 0.25 : 1;
      wheelTarget = clampZoom(base + dir * WHEEL_STEP * step);
      map.zoomTo(wheelTarget, { around: around(e), duration: WHEEL_DURATION, easing: easeOut });
      clearTimeout(wheelTimer);
      wheelTimer = setTimeout(() => (wheelTarget = null), WHEEL_DURATION + 50);
      return;
    }

    // Two-finger scroll on a touchpad.
    wheelTarget = null;
    if (mode() === "zoom") {
      const step = Math.max(-MAX_STEP, Math.min(MAX_STEP, -dy * SCROLL_ZOOM_RATE));
      map.zoomTo(clampZoom(map.getZoom() + step), { around: around(e), duration: 0 });
      return;
    }
    if (e.shiftKey && dx === 0) {
      dx = dy;
      dy = 0;
    }
    map.panBy([dx, dy], { duration: 0 });
  };

  el.addEventListener("wheel", onWheel, { passive: false });
  return () => {
    el.removeEventListener("wheel", onWheel);
    clearTimeout(wheelTimer);
  };
}
