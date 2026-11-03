import { useEffect, useRef, useState } from "react";
import { Bus, Download, FolderOpen, Globe, Map, MapPin, Mountain, PackageOpen, PackagePlus, Pause, Play, RefreshCw, Route, Satellite, X } from "lucide-react";
import clsx from "clsx";
import { revealItemInDir, openPath } from "@tauri-apps/plugin-opener";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { DATA_COMPONENTS, type ComponentStatus, type DataComponent, type ProgressPhase, type UpdateCheck } from "../lib/api";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { fmtBytes, fmtDateTime } from "../lib/format";
import { Toggle } from "./RoutePanel";
import { RegionsSection } from "./RegionsSection";

const META: Record<DataComponent, { icon: React.ReactNode; label: I18nKey; desc: I18nKey }> = {
  tiles: { icon: <Map size={16} strokeWidth={1.75} />, label: "compTiles", desc: "compTilesDesc" },
  world: { icon: <Globe size={16} strokeWidth={1.75} />, label: "compWorld", desc: "compWorldDesc" },
  satellite: { icon: <Satellite size={16} strokeWidth={1.75} />, label: "compSatellite", desc: "compSatelliteDesc" },
  terrain: { icon: <Mountain size={16} strokeWidth={1.75} />, label: "compTerrain", desc: "compTerrainDesc" },
  routing: { icon: <Route size={16} strokeWidth={1.75} />, label: "compRouting", desc: "compRoutingDesc" },
  geocode: { icon: <MapPin size={16} strokeWidth={1.75} />, label: "compGeocode", desc: "compGeocodeDesc" },
  transit: { icon: <Bus size={16} strokeWidth={1.75} />, label: "compTransit", desc: "compTransitDesc" },
};

/** Rough download sizes of a first install (prebuilt packages), for the "update all" summary. */
const NEW_BYTES: Record<DataComponent, number> = {
  tiles: 190e6,
  world: 45e6,
  satellite: 2.5e9,
  terrain: 55e6,
  routing: 50e6,
  geocode: 22e6,
  transit: 37e6,
};
const SAT_ORTHO_BYTES: Record<string, number> = { low: 150e6, medium: 600e6, high: 2.5e9, max: 10e9 };
const SAT_SENTINEL_BYTES: Record<string, number> = { low: 15e6, medium: 45e6, high: 120e6, max: 120e6 };
/** Big or optional components: not ticked by default when they are not installed yet. */
const OPTIONAL_EXTRAS: DataComponent[] = ["satellite", "world"];

const PHASE_KEY: Record<ProgressPhase, I18nKey> = {
  queued: "phaseQueued",
  downloading: "phaseDownloading",
  processing: "phaseProcessing",
  finalizing: "phaseFinalizing",
  done: "phaseDone",
  failed: "phaseFailed",
  cancelled: "phaseCancelled",
};

export function DataPanel() {
  const t = useT();
  const status = useStore((s) => s.dataStatus);
  const progress = useStore((s) => s.progress);
  const lang = useStore((s) => s.settings.language);
  const check = useStore((s) => s.dataCheck);
  const checking = useStore((s) => s.checkingUpdates);
  const { refreshDataStatus, checkUpdates, startUpdate, cancelUpdate, pauseUpdate, exportData, importData, toastError, updateSettings } = useStore.getState();
  const satelliteQuality = useStore((s) => s.settings.satelliteQuality);
  const usePrebuilt = useStore((s) => s.settings.usePrebuiltData ?? true);
  const satelliteSource = useStore((s) => s.settings.satelliteSource ?? "govmap");
  const ortho = satelliteSource !== "sentinel";
  const updating = !!status?.updating;
  const paused = !!status?.paused;
  const present = DATA_COMPONENTS.filter((c) => status?.[c]?.present);
  const anyPresent = present.length > 0;
  // Components excluded from the next export (everything installed is included by default).
  const [excluded, setExcluded] = useState<DataComponent[]>([]);
  const selected = present.filter((c) => !excluded.includes(c));
  // Package size estimate: the PMTiles archives are stored as-is, the binary components deflate
  // to roughly these fractions (measured on real data).
  const PACK_RATIO: Record<DataComponent, number> = { tiles: 1, world: 1, satellite: 1, terrain: 1, routing: 0.5, geocode: 0.45, transit: 0.35 };
  const rawBytes = selected.reduce((n, c) => n + (status?.[c]?.sizeBytes ?? 0), 0);
  const estimatedBytes = selected.reduce((n, c) => n + (status?.[c]?.sizeBytes ?? 0) * PACK_RATIO[c], 0);
  // the map / imagery / elevation archives are already compressed inside and go in as they are
  const compresses = estimatedBytes < rawBytes * 0.98;

  const doExport = async () => {
    if (selected.length === 0) {
      toastError(t("nothingSelected"));
      return;
    }
    try {
      const d = new Date();
      const stamp = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      const part = selected.length === present.length ? "" : `-${selected.join("+")}`;
      const dest = await saveDialog({
        title: t("exportPackage"),
        defaultPath: `MapsOffline${part}-${stamp}.mapsdata`,
        filters: [{ name: t("packageFilter"), extensions: ["mapsdata"] }],
      });
      if (dest) await exportData(dest, selected.length === present.length ? undefined : selected);
    } catch (e) {
      toastError(e);
    }
  };
  const doImport = async () => {
    try {
      const src = await openDialog({
        title: t("importPackage"),
        multiple: false,
        directory: false,
        filters: [{ name: t("packageFilter"), extensions: ["mapsdata"] }],
      });
      if (typeof src === "string" && src) await importData(src);
    } catch (e) {
      toastError(e);
    }
  };

  useEffect(() => {
    void refreshDataStatus();
    void checkUpdates();
  }, [refreshDataStatus, checkUpdates]);

  // After an update finishes, the check result is stale: re-check so the pills flip to "up to date".
  // Only on the true -> false edge: on mount `updating` is already false, and re-checking there
  // would hit the network on every opening of the panel (the 10-minute throttle covers that).
  const wasUpdating = useRef(updating);
  useEffect(() => {
    if (wasUpdating.current && !updating && check) void checkUpdates(true);
    wasUpdating.current = updating;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [updating]);

  const available = DATA_COMPONENTS.filter((c) => check?.[c]?.available);
  // "Update all" first shows what it is about to download and how big it is: on a fresh install
  // it would otherwise start the satellite imagery (up to many GB, hours) and the world map
  // without a word. Optional extras that are not installed yet start unticked.
  const [plan, setPlan] = useState<DataComponent[] | null>(null);
  const estimate = (c: DataComponent): number => {
    const st = status?.[c];
    if (st?.present && st.sizeBytes) return st.sizeBytes;
    if (c === "satellite") return (ortho ? SAT_ORTHO_BYTES : SAT_SENTINEL_BYTES)[satelliteQuality] ?? SAT_ORTHO_BYTES.high;
    return NEW_BYTES[c];
  };
  const pending = DATA_COMPONENTS.filter((c) => (status && !status[c]?.present) || check?.[c]?.available);
  const checkErrors = DATA_COMPONENTS.filter((c) => check?.[c]?.error);

  const openDir = async () => {
    if (!status?.dir) return;
    try {
      await openPath(status.dir);
    } catch {
      try {
        await revealItemInDir(status.dir);
      } catch (e) {
        toastError(e);
      }
    }
  };

  return (
    <div className="px-4 pb-5 flex flex-col gap-4">
      <div className="flex flex-col gap-2">
        <div className="flex items-center justify-between px-0.5">
          <h2 className="section-label m-0 p-0">{t("dataTitle")}</h2>
          <button
            className="btn btn-ghost btn-sm text-muted -me-1"
            disabled={checking}
            onClick={() => {
              void refreshDataStatus();
              void checkUpdates(true);
            }}
            title={t("checkUpdates")}
          >
            <RefreshCw size={14} strokeWidth={1.75} className={clsx((updating || checking) && "animate-spin")} />
            {checking ? t("checking") : t("checkUpdates")}
          </button>
        </div>
        <div className="flex flex-col gap-2 mb-1">
        {updating ? (
          <button className="btn btn-danger w-full !h-10" onClick={() => void cancelUpdate()}>
            <X size={16} strokeWidth={1.75} /> {t("cancelUpdate")}
          </button>
        ) : pending.length > 0 ? (
          // only when there is something to do: an available update or a missing component
          plan ? (
            <div className="card p-3 flex flex-col gap-2 fade-in">
              <div className="text-sm font-semibold">{t("updatePlanTitle")}</div>
              <div className="flex flex-col">
                {pending.map((c) => (
                  <label key={c} className="flex items-center gap-2.5 min-h-9 cursor-pointer">
                    <input
                      type="checkbox"
                      className="w-4 h-4 accent-[var(--accent)]"
                      checked={plan.includes(c)}
                      onChange={(e) => setPlan(e.target.checked ? [...plan, c] : plan.filter((x) => x !== c))}
                    />
                    <span className="flex-1 min-w-0 truncate">{t(META[c].label)}</span>
                    <bdi dir="ltr" className="text-xs text-muted num shrink-0">~{fmtBytes(estimate(c))}</bdi>
                  </label>
                ))}
              </div>
              <div className="flex items-center gap-2 pt-1">
                <button
                  className="btn btn-primary flex-1"
                  disabled={plan.length === 0}
                  onClick={() => {
                    const comps = pending.filter((c) => plan.includes(c));
                    setPlan(null);
                    void startUpdate(comps);
                  }}
                >
                  <Download size={16} strokeWidth={2} /> {t("updatePlanStart")} <bdi dir="ltr" className="num font-normal">(~{fmtBytes(plan.reduce((n, c) => n + estimate(c), 0))})</bdi>
                </button>
                <button className="btn" onClick={() => setPlan(null)}>
                  {t("cancel")}
                </button>
              </div>
            </div>
          ) : (
            <button className="btn btn-primary w-full" onClick={() => setPlan(pending.filter((c) => !(OPTIONAL_EXTRAS.includes(c) && !status?.[c]?.present)))}>
              <Download size={16} strokeWidth={2} /> {t("updateAll")}
            </button>
          )
        ) : null}
        <p className="m-0 text-xs text-muted px-0.5">{t("dataNeedsInternet")}</p>
        <div className="group-list">
          <Toggle label={t("prebuiltData")} checked={usePrebuilt} onChange={(v) => void updateSettings({ usePrebuiltData: v })} />
        </div>
        <p className="m-0 text-xs text-muted px-0.5">{t("prebuiltDataHint")}</p>
        </div>
        <div className="group-list">
          {DATA_COMPONENTS.map((c) => (
            <ComponentCard
              key={c}
              id={c}
              st={status?.[c]}
              chk={check?.[c]}
              prog={progress[c]}
              updating={updating}
              paused={paused}
              lang={lang}
              onUpdate={() => void startUpdate([c])}
              onCancel={() => void cancelUpdate(c)}
              onPause={(p) => void pauseUpdate(p)}
              extra={
                c === "satellite" ? (
                  <div className="flex flex-col gap-1.5 mt-1.5">
                    <label className="flex flex-col gap-1 text-xs text-muted">
                      <span>{t("satelliteSource")}</span>
                      <select
                        className="input h-9 text-sm w-full min-w-0"
                        value={satelliteSource}
                        disabled={updating}
                        onChange={(e) => {
                          void updateSettings({ satelliteSource: e.target.value as "govmap" | "sentinel" }).then(() => checkUpdates(true));
                        }}
                      >
                        <option value="govmap">{t("satSourceGovmap")}</option>
                        <option value="sentinel">{t("satSourceSentinel")}</option>
                      </select>
                    </label>
                    <label className="flex flex-col gap-1 text-xs text-muted">
                      <span>{t("satelliteQuality")}</span>
                      <select
                        className="input h-9 text-sm w-full min-w-0"
                        value={!ortho && satelliteQuality === "max" ? "high" : satelliteQuality}
                        disabled={updating}
                        onChange={(e) => {
                          void updateSettings({ satelliteQuality: e.target.value as "low" | "medium" | "high" | "max" }).then(() => checkUpdates(true));
                        }}
                      >
                        {ortho ? (
                          <>
                            <option value="low">{t("satOrthoLowOpt")}</option>
                            <option value="medium">{t("satOrthoMediumOpt")}</option>
                            <option value="high">{t("satOrthoHighOpt")}</option>
                            <option value="max">{t("satOrthoMaxOpt")}</option>
                          </>
                        ) : (
                          <>
                            <option value="low">{t("satQualityLowOpt")}</option>
                            <option value="medium">{t("satQualityMediumOpt")}</option>
                            <option value="high">{t("satQualityHighOpt")}</option>
                          </>
                        )}
                      </select>
                    </label>
                    {ortho && <p className="m-0 text-[11px] leading-snug text-muted">{t("satOrthoNote")}</p>}
                  </div>
                ) : undefined
              }
            />
          ))}
        </div>
        {check && !checking && (
          <p className={clsx("m-0 text-xs px-0.5 fade-in", available.length ? "text-fg" : "text-muted")}>
            {available.length
              ? `${t("updatesFor")} ${available.map((c) => t(META[c].label)).join(", ")}`
              : checkErrors.length === DATA_COMPONENTS.length
                ? `${t("checkFailed")}: ${check[checkErrors[0]].error}`
                : t("allUpToDate")}
            {" · "}
            {t("lastChecked")} <bdi dir="ltr">{fmtDateTime(check.checkedIso, lang)}</bdi>
          </p>
        )}
      </div>

      <RegionsSection />

      <div className="flex flex-col gap-2">
        <h2 className="section-label m-0">{t("offlineTransfer")}</h2>
        <div className="group-list">
          {anyPresent && (
            <div className="px-3 py-2.5 flex flex-col gap-2">
              <div className="flex items-center justify-between gap-2 text-xs text-muted">
                <span>{t("exportSelectHint")}</span>
                <span className="num">
                  {compresses ? (
                    <>
                      {t("exportSizeEstimate")} <bdi dir="ltr">~{fmtBytes(estimatedBytes)}</bdi> ({t("exportSizeRaw")} <bdi dir="ltr">{fmtBytes(rawBytes)}</bdi>)
                    </>
                  ) : (
                    <>
                      {t("exportSize")} <bdi dir="ltr">{fmtBytes(rawBytes)}</bdi>
                    </>
                  )}
                </span>
              </div>
              <div className="flex flex-wrap gap-1.5">
                {present.map((c) => {
                  const on = !excluded.includes(c);
                  return (
                    <button
                      key={c}
                      type="button"
                      className="choice"
                      aria-pressed={on}
                      disabled={updating}
                      onClick={() => setExcluded((x) => (on ? [...x, c] : x.filter((k) => k !== c)))}
                    >
                      {META[c].icon}
                      {t(META[c].label)}
                    </button>
                  );
                })}
              </div>
            </div>
          )}
          <div className="row gap-2">
            <button className="btn btn-sm flex-1" disabled={updating || selected.length === 0} onClick={() => void doExport()}>
              <PackagePlus size={14} strokeWidth={1.75} /> {t("exportPackage")}
            </button>
            <button className="btn btn-sm flex-1" disabled={updating} onClick={() => void doImport()}>
              <PackageOpen size={14} strokeWidth={1.75} /> {t("importPackage")}
            </button>
          </div>
        </div>
        <p className="m-0 text-xs text-muted px-0.5">{t("offlineTransferHint")}</p>
      </div>

      {status?.dir && (
        <div className="flex flex-col gap-2">
          <h2 className="section-label m-0">{t("dataDir")}</h2>
          <div className="group-list">
            <div className="row">
              <div className="flex-1 min-w-0 text-xs text-muted truncate" dir="ltr" style={{ textAlign: "start" }} title={status.dir}>
                {status.dir}
              </div>
              <button className="btn btn-sm shrink-0" onClick={() => void openDir()}>
                <FolderOpen size={14} strokeWidth={1.75} /> {t("openFolder")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function ComponentCard({
  id,
  st,
  chk,
  prog,
  updating,
  paused,
  lang,
  onUpdate,
  onCancel,
  onPause,
  extra,
}: {
  id: DataComponent;
  st?: ComponentStatus;
  chk?: UpdateCheck;
  prog?: { phase: ProgressPhase; done: number; total: number; message: string };
  updating: boolean;
  paused: boolean;
  lang: string;
  onUpdate: () => void;
  onCancel: () => void;
  onPause: (paused: boolean) => void;
  /** Extra controls under the description (satellite quality). */
  extra?: React.ReactNode;
}) {
  const t = useT();
  const m = META[id];
  const active = prog && !["done", "failed", "cancelled"].includes(prog.phase);
  const running = active && prog!.phase !== "queued";
  const showPaused = !!running && paused;
  const pct = prog && prog.total > 0 ? Math.min(100, Math.round((prog.done / prog.total) * 100)) : null;
  return (
    <div className="flex flex-col gap-2 px-3 py-3">
      <div className="flex items-start gap-3">
        <span className={clsx("tile", st?.present && "text-fg")}>{m.icon}</span>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="text-base font-semibold">{t(m.label)}</span>
            {active ? (
              <span className={clsx("pill", showPaused ? "" : "pill-accent")}>{showPaused ? t("phasePaused") : t(PHASE_KEY[prog!.phase])}</span>
            ) : !st?.present ? (
              <span className="pill">{t("missing")}</span>
            ) : chk?.available ? (
              <span className="pill pill-accent" title={chk.remoteVersion}>
                {t("updateAvailable")}
              </span>
            ) : chk && !chk.error ? (
              <span className="pill pill-success" title={chk.remoteVersion}>
                {t("upToDate")}
              </span>
            ) : (
              <span className="pill pill-success" title={chk?.error}>
                {st?.loaded && id !== "tiles" ? t("loaded") : t("present")}
              </span>
            )}
          </div>
          <div className="text-xs text-muted mt-0.5">{t(m.desc)}</div>
          {extra}
          {st?.present && (
            <div className="text-xs text-subtle mt-1 truncate num" title={st.version ? `${t("version")}: ${st.version}` : undefined}>
              <bdi dir="ltr">{fmtBytes(st.sizeBytes)}</bdi>
              {st.updatedIso && (
                <>
                  {" · "}
                  {t("updated")} <bdi dir="ltr">{fmtDateTime(st.updatedIso, lang)}</bdi>
                </>
              )}
            </div>
          )}
        </div>
        {active ? (
          <div className="flex items-center shrink-0 -me-1.5">
            {running && (
              <button
                className="btn btn-ghost btn-icon btn-sm text-muted"
                onClick={() => onPause(!paused)}
                title={paused ? t("resumeUpdate") : t("pauseUpdate")}
                aria-label={paused ? t("resumeUpdate") : t("pauseUpdate")}
              >
                {paused ? <Play size={14} strokeWidth={2} /> : <Pause size={14} strokeWidth={2} />}
              </button>
            )}
            <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={onCancel} title={t("cancelComponent")} aria-label={t("cancelComponent")}>
              <X size={15} strokeWidth={2} />
            </button>
          </div>
        ) : (
          <button
            className={clsx("btn btn-sm shrink-0", chk?.available && "btn-primary")}
            disabled={updating || (!!st?.present && !!chk && !chk.available && !chk.error)}
            onClick={onUpdate}
            title={st?.present && chk && !chk.available && !chk.error ? t("upToDate") : undefined}
          >
            {t("update")}
          </button>
        )}
      </div>
      {prog && (
        <div className="fade-in ps-11">
          <div className="flex justify-between gap-2 text-xs mb-1.5">
            <span
              className={clsx(prog.phase === "failed" ? "text-danger whitespace-normal break-words" : "truncate", prog.phase === "done" ? "text-success" : prog.phase !== "failed" && "text-muted")}
              title={prog.phase === "failed" ? prog.message : undefined}
            >
              {active ? (showPaused ? t("phasePaused") : prog.message || t(PHASE_KEY[prog.phase])) : prog.phase === "failed" && prog.message ? `${t("phaseFailed")}: ${prog.message}` : t(PHASE_KEY[prog.phase])}
            </span>
            {pct !== null && active && <span className="text-muted num shrink-0">{pct}%</span>}
          </div>
          {active && (
            <div className="h-1 rounded-full bg-surface-2 overflow-hidden" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined}>
              <div
                className={clsx("h-full bg-accent rounded-full transition-[width] duration-300 ease-out", pct === null && !showPaused && "animate-pulse w-1/3", pct === null && showPaused && "w-1/3 opacity-50")}
                style={pct !== null ? { width: `${pct}%` } : undefined}
              />
            </div>
          )}
        </div>
      )}
    </div>
  );
}
