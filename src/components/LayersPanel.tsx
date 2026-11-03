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
    <div className="px-4 pb-5 flex flex-col gap-5">
      <section className="flex flex-col gap-2">
        <div className="group-list">
          <Toggle label={t("layerSatellite")} checked={layers.satellite} onChange={(v) => setLayers({ satellite: v })} />
          <Toggle label={t("layerHillshade")} checked={layers.hillshade} onChange={(v) => setLayers({ hillshade: v })} />
          <Toggle label={t("layerTerrain3d")} checked={layers.terrain3d} onChange={(v) => setLayers({ terrain3d: v })} />
          <Toggle label={t("layer3d")} checked={layers.buildings3d} onChange={(v) => setLayers({ buildings3d: v })} />
          <Toggle label={t("layerPoi")} checked={layers.poi} onChange={(v) => setLayers({ poi: v })} />
          <Toggle label={t("layerTransit")} checked={layers.transitStops} onChange={(v) => setLayers({ transitStops: v })} />
          <Toggle label={t("layerRoadShields")} checked={layers.roadShields} onChange={(v) => setLayers({ roadShields: v })} />
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
        <div className="seg">
          {(["standard", "cycle", "transit"] as MapLayers["theme"][]).map((th) => (
            <button key={th} aria-pressed={layers.theme === th} onClick={() => setLayers({ theme: th })}>
              <span>{t(th === "standard" ? "themeStandard" : th === "cycle" ? "themeCycle" : "themeTransit")}</span>
            </button>
          ))}
        </div>
        {layers.theme !== "standard" && (
          <p className="m-0 text-xs text-muted px-0.5 fade-in">{t(layers.theme === "cycle" ? "themeCycleHint" : "themeTransitHint")}</p>
        )}
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="section-label m-0">{t("mapFlavor")}</h2>
        <div className="seg">
          {(["auto", "light", "dark", "grayscale"] as MapLayers["flavor"][]).map((f) => (
            <button key={f} aria-pressed={layers.flavor === f} onClick={() => setLayers({ flavor: f })}>
              <span>{t(f === "auto" ? "flavorAuto" : f === "light" ? "flavorLight" : f === "dark" ? "flavorDark" : "flavorGrayscale")}</span>
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
