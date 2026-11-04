import * as maplibregl from "maplibre-gl";
import { fmtDistance } from "../lib/format";
import { useStore } from "../lib/store";
import { HOVER_LAYERS } from "./useHoverCard";

/** A marked trail clicked on the map: name, length and network in a small bubble. */
export function bindTrailPopup(map: maplibregl.Map, show: (p: maplibregl.Popup) => void): void {
  map.on("click", (e) => {
    const trailLayers = ["trails", "trails-bike"].filter((l) => map.getLayer(l));
    if (!trailLayers.length) return;
    if (map.queryRenderedFeatures(e.point, { layers: HOVER_LAYERS.filter((l) => map.getLayer(l)) }).length) return;
    const f = map.queryRenderedFeatures(e.point, { layers: trailLayers })[0];
    if (!f) return;
    const st = useStore.getState();
    const root = document.createElement("div");
    root.className = "flex flex-col gap-1 min-w-[180px]";
    const title = document.createElement("div");
    title.className = "font-semibold text-base pe-4 selectable";
    title.textContent = String(f.properties?.name ?? st.t("trails"));
    root.appendChild(title);
    const rows: [string, string][] = [];
    const len = Number(f.properties?.lengthM ?? f.properties?.length_m);
    if (Number.isFinite(len) && len > 0) rows.push([st.t("trailLength"), fmtDistance(len, st.t, st.settings.units)]);
    if (f.properties?.network) rows.push([st.t("trailNetwork"), String(f.properties.network)]);
    for (const [k, v] of rows) {
      const r = document.createElement("div");
      r.className = "text-xs text-muted";
      r.textContent = `${k}: ${v}`;
      root.appendChild(r);
    }
    show(new maplibregl.Popup({ offset: 8, maxWidth: "260px" }).setLngLat(e.lngLat).setDOMContent(root));
  });
}
