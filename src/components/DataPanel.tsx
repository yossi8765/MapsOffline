import { useEffect, useState } from "react";
import { Bus, Download, FolderOpen, Map, MapPin, Mountain, PackageOpen, PackagePlus, Pause, Play, RefreshCw, Route, Satellite, X } from "lucide-react";
import clsx from "clsx";
import { revealItemInDir, openPath } from "@tauri-apps/plugin-opener";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { DATA_COMPONENTS, type ComponentStatus, type DataComponent, type ProgressPhase, type UpdateCheck } from "../lib/api";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { fmtBytes, fmtDateTime } from "../lib/format";

const META: Record<DataComponent, { icon: React.ReactNode; label: I18nKey; desc: I18nKey }> = {
  tiles: { icon: <Map size={16} strokeWidth={1.75} />, label: "compTiles", desc: "compTilesDesc" },
  satellite: { icon: <Satellite size={16} strokeWidth={1.75} />, label: "compSatellite", desc: "compSatelliteDesc" },
  terrain: { icon: <Mountain size={16} strokeWidth={1.75} />, label: "compTerrain", desc: "compTerrainDesc" },
  routing: { icon: <Route size={16} strokeWidth={1.75} />, label: "compRouting", desc: "compRoutingDesc" },
  geocode: { icon: <MapPin size={16} strokeWidth={1.75} />, label: "compGeocode", desc: "compGeocodeDesc" },
  transit: { icon: <Bus size={16} strokeWidth={1.75} />, label: "compTransit", desc: "compTransitDesc" },
};

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
  const { refreshDataStatus, checkUpdates, startUpdate, cancelUpdate, pauseUpdate, exportData, importData, toastError } = useStore.getState();
  const updating = !!status?.updating;
  const paused = !!status?.paused;
  const present = DATA_COMPONENTS.filter((c) => status?.[c]?.present);
  const anyPresent = present.length > 0;
  // Components excluded from the next export (everything installed is included by default).
  const [excluded, setExcluded] = useState<DataComponent[]>([]);
  const selected = present.filter((c) => !excluded.includes(c));
  // Package size estimate: the PMTiles archives are stored as-is, the binary components deflate
  // to roughly these fractions (measured on real data).
  const PACK_RATIO: Record<DataComponent, number> = { tiles: 1, satellite: 1, terrain: 1, routing: 0.5, geocode: 0.45, transit: 0.35 };
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
  useEffect(() => {
    if (!updating && check) void checkUpdates(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [updating]);

  const available = DATA_COMPONENTS.filter((c) => check?.[c]?.available);
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
        ) : (
          <button className="btn btn-primary w-full" onClick={() => void startUpdate(DATA_COMPONENTS)}>
            <Download size={16} strokeWidth={2} /> {t("updateAll")}
          </button>
        )}
        <p className="m-0 text-xs text-muted px-0.5">{t("dataNeedsInternet")}</p>
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
            disabled={updating}
            onClick={onUpdate}
            title={chk && !chk.available && !chk.error ? t("forceUpdateTitle") : undefined}
          >
            {t("update")}
          </button>
        )}
      </div>
      {prog && (
        <div className="fade-in ps-11">
          <div className="flex justify-between gap-2 text-xs mb-1.5">
            <span
              className={clsx(prog.phase === "failed" ? "text-danger whitespace-normal break-words justify" : "truncate", prog.phase === "done" ? "text-success" : prog.phase !== "failed" && "text-muted")}
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
