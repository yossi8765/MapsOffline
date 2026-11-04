import { useEffect, useRef, type RefObject } from "react";
import type * as maplibregl from "maplibre-gl";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { formatCoord } from "../lib/coords";

/**
 * Pointer coordinates in the chosen format (Settings: decimal / DMS / ITM / UTM). Listens to the
 * map itself and writes the text straight into the DOM: the big MapView component no longer
 * re-renders on every mouse move (report ד-4). Sits on the free side of the map, above the scale.
 */
export function CursorReadout({ mapRef, ready }: { mapRef: RefObject<maplibregl.Map | null>; ready: boolean }) {
  const textRef = useRef<HTMLElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const lastRef = useRef<[number, number] | null>(null);
  const format = useStore((s) => s.settings.coordFormat ?? "dd");
  const formatRef = useRef(format);
  formatRef.current = format;

  const paint = () => {
    const last = lastRef.current;
    if (!last || !textRef.current || !boxRef.current) return;
    textRef.current.textContent = formatCoord(last[0], last[1], formatRef.current);
    boxRef.current.style.visibility = "visible";
  };

  useEffect(() => {
    const map = mapRef.current;
    if (!map || !ready) return;
    let frame = 0;
    const onMove = (e: maplibregl.MapMouseEvent) => {
      lastRef.current = [e.lngLat.lng, e.lngLat.lat];
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          paint();
        });
    };
    const onOut = () => {
      if (boxRef.current) boxRef.current.style.visibility = "hidden";
    };
    map.on("mousemove", onMove);
    map.on("mouseout", onOut);
    return () => {
      map.off("mousemove", onMove);
      map.off("mouseout", onOut);
      if (frame) cancelAnimationFrame(frame);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mapRef, ready]);

  // a format change (Settings) repaints the last position at once
  useEffect(() => {
    if (boxRef.current?.style.visibility === "visible") paint();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [format]);

  return (
    <div
      ref={boxRef}
      style={{ visibility: "hidden" }}
      className="map-readout absolute bottom-[64px] end-3 z-10 rounded px-2 h-5 flex items-center text-[12px] text-muted num bg-[color-mix(in_srgb,var(--surface-raised)_85%,transparent)] pointer-events-none no-print"
    >
      <bdi dir="ltr" ref={textRef} />
    </div>
  );
}

/**
 * Short credit line at the bottom of the map (report א-4). The full attributions stay behind the
 * MapLibre "i" button (licence requirement); this line opens Settings, where About lists every
 * source. MapView hides it while navigating (the trip bar sits there).
 */
export function MapCredits() {
  const t = useT();
  const sat = useStore((s) => s.layers.satellite && !!s.satelliteUrl);
  const ortho = useStore((s) => s.settings.satelliteSource !== "sentinel");
  const dem = useStore((s) => !!s.terrainUrl && (s.layers.hillshade || s.layers.terrain3d || s.layers.contours || s.layers.theme === "topo"));
  const parts = ["© OpenStreetMap", "Protomaps"];
  if (sat) parts.push(ortho ? t("creditMapi") : "Sentinel-2 / EOX");
  if (dem) parts.push("Mapterhorn");
  return (
    <button
      type="button"
      className="map-credits absolute bottom-1.5 start-1/2 -translate-x-1/2 rtl:translate-x-1/2 z-10 rounded px-2 h-5 text-[11px] text-muted bg-[color-mix(in_srgb,var(--surface-raised)_80%,transparent)] hover:text-fg no-print"
      title={t("creditsOpen")}
      onClick={() => useStore.getState().setPanel("settings")}
    >
      <bdi>{parts.join(" · ")}</bdi>
    </button>
  );
}
