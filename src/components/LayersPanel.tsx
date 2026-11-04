import clsx from "clsx";
import { AppWindow, Columns2, FileUp, Grid2x2, Layers3, Pencil, Ruler, Route as RouteIcon } from "lucide-react";
import { Icon } from "./Icon";
import { useStore, type MapLayers } from "../lib/store";
import { useT } from "../lib/i18n";
import { InfoTip } from "./InfoTip";

/** A layer switch; disabled (with a link to the Data panel) while its data is not installed
 *  (report ב-23: the switch used to turn on and show nothing). */
function LayerToggle({ label, checked, onChange, missing, info, sub }: { label: string; checked: boolean; onChange: (v: boolean) => void; missing?: boolean; info?: string; /** a sub-option of the row above (indented, smaller) */ sub?: boolean }) {
  const t = useT();
  return (
    <div className={clsx("row justify-between", missing && "opacity-80", sub && "ps-8 fade-in")}>
      <span className="flex flex-col min-w-0">
        <span className={clsx(sub ? "text-sm" : "text-base", "inline-flex items-center gap-1.5", missing && "text-muted")}>
          {label}
          {info && <InfoTip text={info} />}
        </span>
        {missing && (
          <button
            type="button"
            className="text-xs text-accent text-start hover:underline w-fit"
            onClick={() => {
              const s = useStore.getState();
              s.setMapPopover(null);
              s.setPanel("data");
            }}
          >
            {t("layerNeedsData")}
          </button>
        )}
      </span>
      <button type="button" role="switch" aria-checked={checked && !missing} aria-label={label} aria-disabled={missing} disabled={missing} className="switch" onClick={() => onChange(!checked)} />
    </div>
  );
}

export function LayersPanel() {
  const t = useT();
  const layers = useStore((s) => s.layers);
  const setLayers = useStore((s) => s.setLayers);
  const stopsUnavailable = useStore((s) => s.transitStopsUnavailable);
  const satelliteUrl = useStore((s) => s.satelliteUrl);
  const terrainUrl = useStore((s) => s.terrainUrl);
  const trailsInstalled = useStore((s) => s.dataStatus?.trails?.present ?? true);
  const transitInstalled = useStore((s) => s.dataStatus?.transit?.present ?? true);
  const userCount = useStore((s) => s.userLayers.length);
  const customCount = useStore((s) => (s.settings.customLayers ?? []).filter((c) => c.visible).length);
  const recording = useStore((s) => s.track.recording);
  const compare = useStore((s) => s.compare);
  // without the data, the online stand-ins (AWS elevation tiles, Waymarked Trails) serve them
  const onlineOk = useStore((s) => s.online && s.settings.routingMode !== "offline");
  const noTerrain = !terrainUrl && !onlineOk;
  const st = useStore.getState();
  const tool = (run: () => void) => () => {
    st.setMapPopover(null);
    run();
  };

  return (
    <div className="px-4 pb-4 flex flex-col gap-5">
      <section className="flex flex-col gap-2">
        <div className="group-list">
          <LayerToggle label={t("layerSatellite")} info={t("layerSatelliteHint")} checked={layers.satellite} missing={!satelliteUrl} onChange={(v) => setLayers({ satellite: v })} />
          {layers.satellite && satelliteUrl && (
            <>
              <LayerToggle label={t("satRoads")} sub checked={layers.satelliteRoads ?? true} onChange={(v) => setLayers({ satelliteRoads: v })} />
              <LayerToggle label={t("satLabels")} sub checked={layers.satelliteLabels ?? true} onChange={(v) => setLayers({ satelliteLabels: v })} />
            </>
          )}
          <LayerToggle label={t("layerHillshade")} checked={layers.hillshade} missing={noTerrain} onChange={(v) => setLayers({ hillshade: v })} />
          <LayerToggle label={t("layerTerrain3d")} info={t("layerTerrainHint")} checked={layers.terrain3d} missing={noTerrain} onChange={(v) => setLayers({ terrain3d: v })} />
          <LayerToggle label={t("layer3d")} info={t("layer3dHint")} checked={layers.buildings3d} onChange={(v) => setLayers({ buildings3d: v })} />
          <LayerToggle label={t("layerPoi")} checked={layers.poi} onChange={(v) => setLayers({ poi: v })} />
          <LayerToggle label={t("layerTransit")} info={t("layerTransitZoomHint")} checked={layers.transitStops} missing={!transitInstalled} onChange={(v) => setLayers({ transitStops: v })} />
          <LayerToggle label={t("layerRoadShields")} checked={layers.roadShields} onChange={(v) => setLayers({ roadShields: v })} />
          <LayerToggle label={t("contours")} checked={layers.contours} missing={noTerrain} onChange={(v) => setLayers({ contours: v })} />
          <LayerToggle label={t("trails")} checked={layers.trails} missing={!trailsInstalled && !onlineOk} onChange={(v) => setLayers({ trails: v })} />
        </div>
        {/* a state, not an explanation: stays visible */}
        {layers.transitStops && transitInstalled && stopsUnavailable && <p className="m-0 text-xs text-muted px-0.5 fade-in">{t("layerTransitHint")}</p>}
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

      <section className="flex flex-col gap-2">
        <h2 className="section-label m-0">{t("myLayersSection")}</h2>
        <div className="grid grid-cols-2 gap-1.5">
          <button className="btn btn-sm justify-start" onClick={() => st.setMapPopover("userLayers")}>
            <Icon as={Layers3} size={14} /> {t("userLayersTitle")}
            {userCount > 0 && <span className="num text-xs text-muted ms-auto">{userCount}</span>}
          </button>
          <button className="btn btn-sm justify-start" onClick={() => st.setMapPopover("customLayers")}>
            <Icon as={Grid2x2} size={14} /> {t("customLayersTitle")}
            {customCount > 0 && <span className="num text-xs text-muted ms-auto">{customCount}</span>}
          </button>
          <button className="btn btn-sm justify-start" onClick={tool(() => st.openImport())}>
            <Icon as={FileUp} size={14} /> {t("importTitle")}
          </button>
        </div>
      </section>

      <section className="flex flex-col gap-2">
        <h2 className="section-label m-0">{t("toolsSection")}</h2>
        <div className="grid grid-cols-2 gap-1.5">
          <button className="btn btn-sm justify-start" onClick={tool(() => st.startMeasure())}>
            <Icon as={Ruler} size={14} /> {t("measureTitle")}
          </button>
          <button className="btn btn-sm justify-start" onClick={tool(() => st.startDrawing())}>
            <Icon as={Pencil} size={14} /> {t("drawTitle")}
          </button>
          <button className={clsx("btn btn-sm justify-start", recording && "text-danger")} onClick={tool(() => st.toggleTrackRecording())}>
            <Icon as={RouteIcon} size={14} /> {recording ? t("trackStop") : t("trackStart")}
          </button>
          <button className={clsx("btn btn-sm justify-start", compare !== "off" && "text-accent")} onClick={tool(() => st.setCompare(compare === "off" ? "swipe" : "off"))}>
            <Icon as={Columns2} size={14} /> {t("compareTitle")}
          </button>
          <button className="btn btn-sm justify-start" onClick={tool(() => st.openMapWindow())}>
            <Icon as={AppWindow} size={14} /> {t("mapWindowOpen")}
          </button>
        </div>
      </section>
    </div>
  );
}
