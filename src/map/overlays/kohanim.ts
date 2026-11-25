import * as maplibregl from "maplibre-gl";
import type { Feature } from "geojson";
import type { KohanimArea } from "../../lib/api";
import { useStore } from "../../lib/store";
import { beforeIfPresent, ensureSource, setFeatures } from "../geojson";

export const SRC_KOHANIM = "kohanim-areas";
/** One dot per area: most areas are a road strip of ~500 × 100 m, a few pixels until street zoom. */
export const SRC_KOHANIM_POINTS = "kohanim-points";
/** The dots give way to the areas themselves from this zoom on. */
const AREA_ZOOM = 13;
export const KOHANIM_LAYERS = ["kohanim-fill", "kohanim-line", "kohanim-points"];

/** Colours of the association's map: level א pink, ב brown, ג orange. */
const LEVEL_COLOR: maplibregl.ExpressionSpecification = ["match", ["get", "level"], 1, "#FF1493", 2, "#8B4513", 3, "#FF8C00", "#FF1493"];

/** Kohanim areas, under the routes and labels. */
export function add(map: maplibregl.Map, beforeId: string | undefined): void {
  ensureSource(map, SRC_KOHANIM);
  ensureSource(map, SRC_KOHANIM_POINTS);
  const before = beforeIfPresent(map, beforeId);
  if (!map.getLayer("kohanim-fill")) {
    map.addLayer({ id: "kohanim-fill", type: "fill", source: SRC_KOHANIM, minzoom: AREA_ZOOM - 2, paint: { "fill-color": LEVEL_COLOR, "fill-opacity": ["interpolate", ["linear"], ["zoom"], AREA_ZOOM - 2, 0.25, AREA_ZOOM, 0.4] } }, before);
  }
  if (!map.getLayer("kohanim-line")) {
    map.addLayer(
      {
        id: "kohanim-line",
        type: "line",
        source: SRC_KOHANIM,
        minzoom: AREA_ZOOM - 2,
        paint: { "line-color": LEVEL_COLOR, "line-width": ["interpolate", ["linear"], ["zoom"], AREA_ZOOM - 2, 1.5, AREA_ZOOM, 2.5, 17, 4], "line-opacity": 0.9 },
      },
      before,
    );
  }
  if (!map.getLayer("kohanim-points")) {
    map.addLayer(
      {
        id: "kohanim-points",
        type: "circle",
        source: SRC_KOHANIM_POINTS,
        maxzoom: AREA_ZOOM,
        paint: {
          "circle-color": LEVEL_COLOR,
          "circle-radius": ["interpolate", ["linear"], ["zoom"], 7, 3, 10, 5, AREA_ZOOM, 6],
          "circle-stroke-color": "#ffffff",
          "circle-stroke-width": 1.5,
          "circle-opacity": 0.9,
        },
      },
      before,
    );
  }
}

/** Middle of a ring's bounding box (good enough for a dot on a road strip). */
function centre(ring: [number, number][]): [number, number] {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const [x, y] of ring) {
    x0 = Math.min(x0, x); y0 = Math.min(y0, y);
    x1 = Math.max(x1, x); y1 = Math.max(y1, y);
  }
  return [(x0 + x1) / 2, (y0 + y1) / 2];
}

/** Show the areas of `level` and the milder levels (null / 0 = none). */
export function sync(map: maplibregl.Map, areas: KohanimArea[] | null, level: number): void {
  if (!map.getSource(SRC_KOHANIM)) return;
  const shown = areas && level > 0 ? areas.filter((a) => a.level <= level) : [];
  const props = (a: KohanimArea) => ({ level: a.level, name: a.name, city: a.city, street: a.street, description: a.description });
  setFeatures(map, SRC_KOHANIM, shown.map((a): Feature => ({ type: "Feature", properties: props(a), geometry: { type: "Polygon", coordinates: [a.ring] } })));
  setFeatures(map, SRC_KOHANIM_POINTS, shown.map((a): Feature => ({ type: "Feature", properties: props(a), geometry: { type: "Point", coordinates: centre(a.ring as [number, number][]) } })));
}

/** An area clicked on the map: its name, place, level and the association's description. A dot
 *  (zoomed out) also brings the map in to the area. */
export function bindPopup(map: maplibregl.Map, show: (p: maplibregl.Popup) => void, skip: (e: maplibregl.MapMouseEvent) => boolean): void {
  map.on("click", (e) => {
    const layers = KOHANIM_LAYERS.filter((l) => map.getLayer(l));
    if (!layers.length || skip(e)) return;
    const f = map.queryRenderedFeatures(e.point, { layers })[0];
    if (!f) return;
    const st = useStore.getState();
    const p = f.properties ?? {};
    const level = Number(p.level);
    const root = document.createElement("div");
    root.className = "flex flex-col gap-1 min-w-[200px]";
    const title = document.createElement("div");
    title.className = "font-semibold text-base pe-4 selectable";
    title.textContent = String(p.name || st.t("kohanimArea"));
    root.appendChild(title);
    const place = [p.street, p.city].filter((s) => s && String(s).trim()).join(", ");
    const lines: string[] = [];
    if (place) lines.push(place);
    lines.push(`${st.t("kohanimLevelOf")}: ${st.t(level === 1 ? "kohanimBasic" : level === 2 ? "kohanimAssociation" : "kohanimStrict")}`);
    for (const l of lines) {
      const r = document.createElement("div");
      r.className = "text-xs text-muted selectable";
      r.textContent = l;
      root.appendChild(r);
    }
    if (p.description && String(p.description).trim()) {
      const d = document.createElement("div");
      d.className = "text-sm selectable";
      d.style.textAlign = "justify";
      d.textContent = String(p.description);
      root.appendChild(d);
    }
    show(new maplibregl.Popup({ offset: 8, maxWidth: "300px" }).setLngLat(e.lngLat).setDOMContent(root));
    if (f.layer.id === "kohanim-points") map.easeTo({ center: e.lngLat, zoom: Math.max(map.getZoom(), AREA_ZOOM + 2) });
  });
}
