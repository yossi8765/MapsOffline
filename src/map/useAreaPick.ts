import { useEffect, type RefObject } from "react";
import type * as maplibregl from "maplibre-gl";
import { useStore } from "../lib/store";
import { ensureSource, setFeatures } from "./geojson";

const SRC = "tool-area-pick";

/** The box of two corners, [w, s, e, n]. */
export function boxOf(a: [number, number], b: [number, number]): [number, number, number, number] {
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
}

function draw(map: maplibregl.Map, box: [number, number, number, number] | null): void {
  if (!map.getSource(SRC)) {
    ensureSource(map, SRC);
    const accent = getComputedStyle(document.documentElement).getPropertyValue("--accent").trim() || "#1a66d9";
    map.addLayer({ id: "area-pick-fill", type: "fill", source: SRC, paint: { "fill-color": accent, "fill-opacity": 0.12 } });
    map.addLayer({ id: "area-pick-line", type: "line", source: SRC, paint: { "line-color": accent, "line-width": 2, "line-dasharray": [2, 1.5] } });
  }
  if (!box) {
    setFeatures(map, SRC, []);
    return;
  }
  const [w, s, e, n] = box;
  setFeatures(map, SRC, [
    { type: "Feature", properties: {}, geometry: { type: "Polygon", coordinates: [[[w, s], [e, s], [e, n], [w, n], [w, s]]] } },
  ]);
}

/**
 * Rectangle picking on the map (sharp-imagery areas): while `store.areaPick` is set, a drag draws
 * a box instead of panning; releasing hands the box to the caller. Escape cancels.
 */
export function useAreaPick(mapRef: RefObject<maplibregl.Map | null>, mapReady: boolean): void {
  const areaPick = useStore((s) => s.areaPick);
  useEffect(() => {
    const map = mapRef.current;
    if (!map || !mapReady || !areaPick) return;
    let start: [number, number] | null = null;
    map.dragPan.disable();
    map.boxZoom.disable();
    map.getCanvas().style.cursor = "crosshair";
    const down = (e: maplibregl.MapMouseEvent) => {
      if (e.originalEvent.button !== 0) return;
      start = [e.lngLat.lng, e.lngLat.lat];
    };
    const move = (e: maplibregl.MapMouseEvent) => {
      if (start) draw(map, boxOf(start, [e.lngLat.lng, e.lngLat.lat]));
    };
    const up = (e: maplibregl.MapMouseEvent) => {
      if (!start) return;
      const box = boxOf(start, [e.lngLat.lng, e.lngLat.lat]);
      start = null;
      // a click without a drag is not an area
      if (box[2] - box[0] < 1e-4 || box[3] - box[1] < 1e-4) return;
      useStore.setState({ areaPick: null });
      areaPick.onPick(box);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        useStore.getState().cancelAreaPick();
      }
    };
    map.on("mousedown", down);
    map.on("mousemove", move);
    map.on("mouseup", up);
    window.addEventListener("keydown", key, true);
    return () => {
      map.off("mousedown", down);
      map.off("mousemove", move);
      map.off("mouseup", up);
      window.removeEventListener("keydown", key, true);
      map.dragPan.enable();
      map.boxZoom.enable();
      map.getCanvas().style.cursor = "";
      if (map.getStyle()) draw(map, null);
    };
  }, [areaPick, mapReady, mapRef]);
}
