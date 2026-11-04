import { useStore, type MapLayers } from "../lib/store";
import { useT } from "../lib/i18n";
import { Toggle } from "./RoutePanel";

export function LayersPanel() {
  const t = useT();
  const layers = useStore((s) => s.layers);
  const setLayers = useStore((s) => s.setLayers);
  const stopsUnavailable = useStore((s) => s.transitStopsUnavailable);
  const satelliteUrl = useStore((s) => s.satelliteUrl);
  const terrainUrl = useStore((s) => s.terrainUrl);

  return (
    <div className="px-4 pb-4 flex flex-col gap-5">
      <section className="flex flex-col gap-2">
        <div className="group-list">
          <Toggle label={t("layerSatellite")} checked={layers.satellite} onChange={(v) => setLayers({ satellite: v })} />
          <Toggle label={t("layerHillshade")} checked={layers.hillshade} onChange={(v) => setLayers({ hillshade: v })} />
          <Toggle label={t("layerTerrain3d")} checked={layers.terrain3d} onChange={(v) => setLayers({ terrain3d: v })} />
          <Toggle label={t("layer3d")} checked={layers.buildings3d} onChange={(v) => setLayers({ buildings3d: v })} />
          <Toggle label={t("layerPoi")} checked={layers.poi} onChange={(v) => setLayers({ poi: v })} />
          <Toggle label={t("layerTransit")} checked={layers.transitStops} onChange={(v) => setLayers({ transitStops: v })} />
          <Toggle label={t("layerRoadShields")} checked={layers.roadShields} onChange={(v) => setLayers({ roadShields: v })} />
          <Toggle label={t("contours")} checked={layers.contours} onChange={(v) => setLayers({ contours: v })} />
          <Toggle label={t("trails")} checked={layers.trails} onChange={(v) => setLayers({ trails: v })} />
        </div>
        {layers.satellite && (
          <p className="m-0 text-xs text-muted px-0.5 fade-in">{satelliteUrl ? t("layerSatelliteHint") : t("layerSatelliteMissing")}</p>
        )}
        {(layers.hillshade || layers.terrain3d) && (
          <p className="m-0 text-xs text-muted px-0.5 fade-in">{terrainUrl ? t("layerTerrainHint") : t("layerTerrainMissing")}</p>
        )}
        {layers.buildings3d && <p className="m-0 text-xs text-muted px-0.5 fade-in">{t("layer3dHint")}</p>}
        {layers.transitStops && (
          <p className="m-0 text-xs text-muted px-0.5 fade-in">{stopsUnavailable ? t("layerTransitHint") : t("layerTransitZoomHint")}</p>
        )}
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="section-label m-0">{t("mapTheme")}</h2>
        {/* four long names do not fit in one row of the 340 px popover: two rows of two */}
        <div className="seg grid grid-cols-2">
          {(["standard", "cycle", "transit", "topo"] as MapLayers["theme"][]).map((th) => (
            <button key={th} aria-pressed={layers.theme === th} onClick={() => setLayers({ theme: th })}>
              <span>{t(th === "standard" ? "themeStandard" : th === "cycle" ? "themeCycle" : th === "transit" ? "themeTransit" : "themeTopo")}</span>
            </button>
          ))}
        </div>
        {layers.theme !== "standard" && (
          <p className="m-0 text-xs text-muted px-0.5 fade-in">{t(layers.theme === "cycle" ? "themeCycleHint" : layers.theme === "topo" ? "themeTopoHint" : "themeTransitHint")}</p>
        )}
      </section>

      <section className="flex flex-col gap-2">
        {/* light / dark follow the app theme (Settings); the map only adds a grey variant */}
        <h2 className="section-label m-0">{t("mapFlavor")}</h2>
        <div className="seg">
          {(["auto", "grayscale"] as MapLayers["flavor"][]).map((f) => (
            <button key={f} aria-pressed={f === "grayscale" ? layers.flavor === "grayscale" : layers.flavor !== "grayscale"} onClick={() => setLayers({ flavor: f })}>
              <span>{t(f === "auto" ? "mapFlavorNormal" : "flavorGrayscale")}</span>
            </button>
          ))}
        </div>
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="section-label m-0">{t("labelLang")}</h2>
        <div className="seg">
          {(["he", "en", "ar"] as MapLayers["labelLang"][]).map((l) => (
            <button key={l} aria-pressed={layers.labelLang === l} onClick={() => setLayers({ labelLang: l })}>
              <span>{t(l === "he" ? "langHe" : l === "en" ? "langEn" : "langAr")}</span>
            </button>
          ))}
        </div>
      </section>
    </div>
  );
}
