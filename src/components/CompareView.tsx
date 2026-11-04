/**
 * Compare two looks of the same place (report ג-29): a second map, synced with the main one.
 *  - "swipe": the second map lies over the whole main map and is cut at a draggable divider;
 *  - "split": side by side – the second map takes the free half, the main map's centre moves into
 *    its own half (map padding), so both halves show the same area.
 * The second map shows aerial imagery, the standard map, the topographic map or the dark map.
 */
import { useEffect, useRef, useState, type RefObject } from "react";
import * as maplibregl from "maplibre-gl";
import { Columns2, GripVertical, SplitSquareHorizontal, X } from "lucide-react";
import { useStore, resolveTheme } from "../lib/store";
import { useT } from "../lib/i18n";
import { buildStyle, type StyleOptions } from "../lib/mapStyle";
import { ensureProtocol } from "../map/useMapInstance";

type Look = "satellite" | "standard" | "topo" | "dark";

export function CompareView({ mainRef }: { mainRef: RefObject<maplibregl.Map | null> }) {
  const t = useT();
  const mode = useStore((s) => s.compare);
  const lang = useStore((s) => s.settings.language);
  const satelliteUrl = useStore((s) => s.satelliteUrl);
  const mainSat = useStore((s) => s.layers.satellite);
  const [look, setLook] = useState<Look>(() => (!mainSat && satelliteUrl ? "satellite" : "standard"));
  const [pos, setPos] = useState(0.5);
  const hostRef = useRef<HTMLDivElement>(null);
  const mapRef = useRef<maplibregl.Map | null>(null);
  const free = lang === "he" ? "left" : "right";

  // ---- the second map
  useEffect(() => {
    const main = mainRef.current;
    if (!main || !hostRef.current) return;
    ensureProtocol();
    const st = useStore.getState();
    const theme = resolveTheme(st.settings.theme);
    const o: StyleOptions = {
      tilesUrl: st.tilesUrl,
      onlineTilesUrl: st.onlineTilesUrl,
      satelliteUrl: st.satelliteUrl,
      satellite: look === "satellite",
      satelliteOrtho: st.settings.satelliteSource !== "sentinel",
      terrainUrl: st.terrainUrl,
      worldUrl: st.worldUrl,
      hillshade: look === "topo",
      terrain3d: false,
      roadShields: st.layers.roadShields,
      flavor: look === "dark" ? "dark" : theme === "dark" && look !== "standard" ? "dark" : "light",
      lang: st.layers.labelLang,
      poi: st.layers.poi,
      buildings3d: false,
      theme: look === "topo" ? "topo" : "standard",
    };
    const map = new maplibregl.Map({
      container: hostRef.current,
      style: buildStyle(o),
      center: main.getCenter(),
      zoom: main.getZoom(),
      bearing: main.getBearing(),
      pitch: main.getPitch(),
      attributionControl: false,
      maxZoom: 20,
      canvasContextAttributes: { antialias: true },
    });
    mapRef.current = map;
    return () => {
      map.remove();
      mapRef.current = null;
    };
  }, [mainRef, look]);

  // ---- keep the two cameras together (either map can be moved in split mode)
  useEffect(() => {
    const main = mainRef.current;
    const other = mapRef.current;
    if (!main || !other) return;
    let syncing = false;
    const copy = (from: maplibregl.Map, to: maplibregl.Map) => {
      if (syncing) return;
      syncing = true;
      to.jumpTo({ center: from.getCenter(), zoom: from.getZoom(), bearing: from.getBearing(), pitch: from.getPitch() });
      syncing = false;
    };
    const a = () => copy(main, other);
    const b = () => copy(other, main);
    main.on("move", a);
    other.on("move", b);
    a();
    return () => {
      main.off("move", a);
      other.off("move", b);
    };
  }, [mainRef, look, mode]);

  // ---- split: the main map's centre moves into the half it still shows
  useEffect(() => {
    const main = mainRef.current;
    if (!main) return;
    if (mode !== "split") return;
    const half = Math.round(main.getContainer().clientWidth / 2);
    const prev = main.getPadding();
    main.setPadding({ ...prev, [free]: half });
    mapRef.current?.resize();
    return () => {
      main.setPadding({ ...main.getPadding(), [free]: prev[free as "left" | "right"] ?? 0 });
    };
  }, [mainRef, mode, free]);

  useEffect(() => {
    mapRef.current?.resize();
  }, [mode]);

  // ---- swipe: drag the divider
  const dragging = useRef(false);
  useEffect(() => {
    const move = (e: PointerEvent) => {
      if (!dragging.current || !hostRef.current?.parentElement) return;
      const r = hostRef.current.parentElement.getBoundingClientRect();
      setPos(Math.min(0.95, Math.max(0.05, (e.clientX - r.left) / r.width)));
    };
    const up = () => (dragging.current = false);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
  }, []);

  if (mode === "off") return null;
  const swipe = mode === "swipe";
  // the second map shows on the free side: left of the divider in Hebrew, right of it in English
  const clip = free === "left" ? `inset(0 ${(1 - pos) * 100}% 0 0)` : `inset(0 0 0 ${pos * 100}%)`;
  const st = useStore.getState();
  const looks: Look[] = satelliteUrl ? ["satellite", "standard", "topo", "dark"] : ["standard", "topo", "dark"];
  return (
    <>
      <div
        ref={hostRef}
        // z 1: under the main map's own controls (z 2), which stay usable; while swiping the
        // gestures go through to the main map (the two are synced)
        className="compare-map absolute top-0 bottom-0 z-[1]"
        style={swipe ? { left: 0, right: 0, clipPath: clip, pointerEvents: "none" } : { [free]: 0, width: "50%" }}
        aria-label={t("compareTitle")}
      />
      {swipe ? (
        <div className="absolute top-0 bottom-0 z-[6] w-0 no-print" style={{ left: `${pos * 100}%` }}>
          <div className="absolute inset-y-0 -translate-x-1/2 w-[3px] bg-white shadow" />
          <button
            className="absolute top-1/2 -translate-y-1/2 -translate-x-1/2 w-8 h-12 rounded popover flex items-center justify-center cursor-ew-resize"
            aria-label={t("compareDivider")}
            onPointerDown={(e) => {
              e.preventDefault();
              dragging.current = true;
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowLeft") setPos((p) => Math.max(0.05, p - 0.05));
              if (e.key === "ArrowRight") setPos((p) => Math.min(0.95, p + 0.05));
            }}
          >
            <GripVertical size={16} />
          </button>
        </div>
      ) : (
        <div className="absolute top-0 bottom-0 z-[6] w-[3px] -translate-x-1/2 bg-white shadow pointer-events-none" style={{ left: "50%" }} />
      )}
      <div role="toolbar" aria-label={t("compareTitle")} className="absolute top-3 start-1/2 -translate-x-1/2 rtl:translate-x-1/2 z-30 popover rounded-[var(--radius)] px-2 h-10 flex items-center gap-2 no-print fade-in">
        <span className="text-sm font-semibold ps-1">{t("compareTitle")}</span>
        <div className="seg seg-sm" role="group" aria-label={t("compareMode")}>
          <button aria-pressed={mode === "split"} onClick={() => st.setCompare("split")} title={t("compareSplit")}>
            <span className="flex items-center gap-1">
              <Columns2 size={14} /> {t("compareSplit")}
            </span>
          </button>
          <button aria-pressed={mode === "swipe"} onClick={() => st.setCompare("swipe")} title={t("compareSwipe")}>
            <span className="flex items-center gap-1">
              <SplitSquareHorizontal size={14} /> {t("compareSwipe")}
            </span>
          </button>
        </div>
        <select className="input h-8 w-auto text-sm" value={look} onChange={(e) => setLook(e.target.value as Look)} aria-label={t("compareLook")}>
          {looks.map((l) => (
            <option key={l} value={l}>
              {t(l === "satellite" ? "layerSatellite" : l === "standard" ? "themeStandard" : l === "topo" ? "themeTopo" : "compareDark")}
            </option>
          ))}
        </select>
        <button className="btn btn-ghost btn-icon btn-sm" onClick={() => st.setCompare("off")} aria-label={t("close")} title={`${t("close")} (Esc)`}>
          <X size={15} />
        </button>
      </div>
    </>
  );
}
