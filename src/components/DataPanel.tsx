import { useEffect, useRef, useState } from "react";
import { BookOpen, Bus, ChevronDown, Download, FileText, Footprints, FolderOpen, Globe, Map, MapPin, Mountain, PackageOpen, PackagePlus, Pause, Play, RefreshCw, Route, Satellite, SlidersHorizontal, X } from "lucide-react";
import clsx from "clsx";
import { revealItemInDir, openPath } from "@tauri-apps/plugin-opener";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { InfoBar } from "./InfoBar";
import { FirstRunCard } from "./FirstRunCard";
import { PackageProgress, packageActive, type PackageDl } from "./PackageProgress";
import { apiX, DATA_COMPONENTS, type ComponentStatus, type DataComponent, type ProgressPhase, type UpdateCheck } from "../lib/api";
import { useStore } from "../lib/store";
import { useTauriEvent } from "../lib/useTauriEvent";
import { useT, type I18nKey } from "../lib/i18n";
import { fmtBytes, fmtDateTime } from "../lib/format";
import { Toggle } from "./RoutePanel";
import { RegionsSection } from "./RegionsSection";

const META: Record<DataComponent, { icon: typeof Map; label: I18nKey; desc: I18nKey }> = {
  tiles: { icon: Map, label: "compTiles", desc: "compTilesDesc" },
  world: { icon: Globe, label: "compWorld", desc: "compWorldDesc" },
  satellite: { icon: Satellite, label: "compSatellite", desc: "compSatelliteDesc" },
  terrain: { icon: Mountain, label: "compTerrain", desc: "compTerrainDesc" },
  routing: { icon: Route, label: "compRouting", desc: "compRoutingDesc" },
  geocode: { icon: MapPin, label: "compGeocode", desc: "compGeocodeDesc" },
  transit: { icon: Bus, label: "compTransit", desc: "compTransitDesc" },
  trails: { icon: Footprints, label: "compTrails", desc: "compTrailsDesc" },
  minyanim: { icon: BookOpen, label: "compMinyanim", desc: "compMinyanimDesc" },
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
  trails: 6e6,
  minyanim: 1e6,
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

/** A hint shown as its first sentence with "עוד…" for the rest (long explanations no longer
 *  push the controls down on every visit). */
export function MoreText({ text, className }: { text: string; className?: string }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const m = /^(.+?[.!?])\s+(.+)$/s.exec(text);
  if (!m) return <p className={clsx("m-0 text-xs text-muted", className)}>{text}</p>;
  return (
    <p className={clsx("m-0 text-xs text-muted", className)}>
      {open ? text : m[1]}{" "}
      <button className="text-accent hover:underline underline-offset-2" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        {open ? t("showLess") : t("showMore")}
      </button>
    </p>
  );
}

/** The components the update checks offer (settings.wantedComponents; null = all). */
export function isWanted(c: DataComponent, wanted: DataComponent[] | null | undefined): boolean {
  return !wanted || wanted.includes(c);
}

export function DataPanel() {
  const t = useT();
  const status = useStore((s) => s.dataStatus);
  const progress = useStore((s) => s.progress);
  const lang = useStore((s) => s.settings.language);
  const check = useStore((s) => s.dataCheck);
  const checking = useStore((s) => s.checkingUpdates);
  const wanted = useStore((s) => s.settings.wantedComponents);
  const { refreshDataStatus, checkUpdates, startUpdate, cancelUpdate, pauseUpdate, exportData, importData, downloadPackage, toastError, updateSettings } = useStore.getState();
  const satelliteQuality = useStore((s) => s.settings.satelliteQuality);
  const usePrebuilt = useStore((s) => s.settings.usePrebuiltData ?? true);
  const satelliteSource = useStore((s) => s.settings.satelliteSource ?? "govmap");
  const ortho = satelliteSource !== "sentinel";
  const updating = !!status?.updating;
  const paused = !!status?.paused;
  // component updates run side by side: a component is busy while it runs or waits; only a
  // package job (export / import / full download) blocks everything
  const busyComps = new Set<DataComponent>([...(status?.active ?? []), ...(status?.queued ?? [])]);
  const queuedComps = new Set<DataComponent>(status?.queued ?? []);
  const present = DATA_COMPONENTS.filter((c) => status?.[c]?.present);
  const anyPresent = present.length > 0;
  const [advanced, setAdvanced] = useState(false);
  // Components excluded from the next export (everything installed is included by default).
  const [excluded, setExcluded] = useState<DataComponent[]>([]);
  const selected = present.filter((c) => !excluded.includes(c));
  // Package size estimate: the PMTiles archives are stored as-is, the binary components deflate
  // to roughly these fractions (measured on real data).
  const PACK_RATIO: Record<DataComponent, number> = { tiles: 1, world: 1, satellite: 1, terrain: 1, routing: 0.5, geocode: 0.45, transit: 0.35, trails: 0.3, minyanim: 0.2 };
  const rawBytes = selected.reduce((n, c) => n + (status?.[c]?.sizeBytes ?? 0), 0);
  const estimatedBytes = selected.reduce((n, c) => n + (status?.[c]?.sizeBytes ?? 0) * PACK_RATIO[c], 0);
  // the map / imagery / elevation archives are already compressed inside and go in as they are
  const compresses = estimatedBytes < rawBytes * 0.98;

  const doExport = async () => {
    if (selected.length === 0) {
      useStore.getState().toast("error", t("nothingSelected"));
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
  // full ready-made package download: live progress and the saved file (to install / show),
  // shown where it was started – at the top (first run) or in "transfer to an offline computer"
  const [pkg, setPkg] = useState<PackageDl | null>(null);
  const [pkgFile, setPkgFile] = useState<string | null>(null);
  const [pkgOrigin, setPkgOrigin] = useState<"top" | "transfer">("transfer");
  const pkgTopRef = useRef<HTMLDivElement>(null);
  const pkgTransferRef = useRef<HTMLDivElement>(null);
  useTauriEvent<PackageDl>("package-download", setPkg);
  const pkgActive = packageActive(pkg);
  // a started download scrolls its progress into view (the panel may be scrolled anywhere)
  const pkgStarted = pkgActive && pkg?.phase === "queued";
  useEffect(() => {
    if (!pkgStarted) return;
    const h = window.setTimeout(() => (pkgOrigin === "top" ? pkgTopRef : pkgTransferRef).current?.scrollIntoView({ block: "nearest", behavior: "smooth" }), 50);
    return () => window.clearTimeout(h);
  }, [pkgStarted, pkgOrigin]);
  /** Download the ready-made package; with `install`, install it right away (first run). */
  const doDownloadPackage = async (install = false) => {
    try {
      const dest = await saveDialog({
        title: t("downloadPackage"),
        defaultPath: "MapsOffline-data.mapsdata",
        filters: [{ name: t("packageFilter"), extensions: ["mapsdata"] }],
      });
      if (!dest) return;
      setPkgFile(null);
      setPkgOrigin(install ? "top" : "transfer");
      setPkg({ phase: "queued", done: 0, total: 0, message: "" });
      if (await downloadPackage(dest)) {
        setPkgFile(dest);
        if (install) await importData(dest);
      }
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

  useEffect(() => {
    if (!updating) return;
    const h = window.setInterval(() => void refreshDataStatus(), 2000);
    return () => window.clearInterval(h);
  }, [updating, refreshDataStatus]);

  // After an update finishes, the check result is stale: re-check so the pills flip to "up to date".
  // Only on the true -> false edge: on mount `updating` is already false, and re-checking there
  // would hit the network on every opening of the panel (the 10-minute throttle covers that).
  const wasUpdating = useRef(updating);
  useEffect(() => {
    if (wasUpdating.current && !updating && check) void checkUpdates(true);
    wasUpdating.current = updating;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [updating]);

  const available = DATA_COMPONENTS.filter((c) => check?.[c]?.available && isWanted(c, wanted));
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
  const pending = DATA_COMPONENTS.filter((c) => !busyComps.has(c) && isWanted(c, wanted) && ((status && !status[c]?.present) || check?.[c]?.available));
  // a package job (not a component update) holds the data folder
  const exclusiveBusy = pkgActive || (updating && busyComps.size === 0);
  const checkErrors = DATA_COMPONENTS.filter((c) => check?.[c]?.error);
  const openPlan = () => setPlan(pending.filter((c) => !(OPTIONAL_EXTRAS.includes(c) && !status?.[c]?.present)));

  const openDir = async (dir: string | undefined) => {
    if (!dir) return;
    try {
      await openPath(dir);
    } catch {
      try {
        await revealItemInDir(dir);
      } catch (e) {
        toastError(e);
      }
    }
  };
  const openLogs = async () => {
    try {
      await openDir(await apiX.logsDir());
    } catch (e) {
      toastError(e);
    }
  };

  const firstRun = !!status && !anyPresent && !updating && !pkgActive && !(pkgOrigin === "top" && pkgFile);
  const pkgAtTop = pkgOrigin === "top" && (pkgActive || !!pkgFile || pkg?.phase === "failed");

  return (
    <div className="px-4 pb-5 flex flex-col gap-4">
      {firstRun && <FirstRunCard onPackage={() => void doDownloadPackage(true)} onChoose={openPlan} />}
      {pkgAtTop && (
        <section className="card p-4 flex flex-col gap-2 fade-in" aria-label={t("downloadPackage")}>
          <h2 className="text-base font-semibold m-0">{t("downloadPackage")}</h2>
          <PackageProgress ref={pkgTopRef} pkg={pkg} file={pkgFile} onInstall={pkgFile ? () => void importData(pkgFile) : undefined} />
        </section>
      )}
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
          >
            <Icon as={RefreshCw} size={14} className={clsx((updating || checking) && "animate-spin")} />
            {checking ? t("checking") : t("checkUpdates")}
          </button>
        </div>
        <div className="flex flex-col gap-2 mb-1">
          {updating && (
            <button className="btn btn-danger w-full" onClick={() => void cancelUpdate()}>
              <Icon as={X} /> {t("cancelUpdate")}
            </button>
          )}
          {exclusiveBusy ? null : pending.length > 0 ? (
            // only when there is something to do: an available update or a missing component
            plan ? (
              <div className="card p-3 flex flex-col gap-2 fade-in">
                <div className="font-semibold">{t("updatePlanTitle")}</div>
                <div className="flex flex-col">
                  {pending.map((c) => (
                    <label key={c} className="flex items-center gap-2.5 min-h-9 cursor-pointer">
                      <input type="checkbox" className="w-4 h-4 accent-[var(--accent)]" checked={plan.includes(c)} onChange={(e) => setPlan(e.target.checked ? [...plan, c] : plan.filter((x) => x !== c))} />
                      <span className="flex-1 min-w-0 truncate">{t(META[c].label)}</span>
                      <bdi dir="ltr" className="text-xs text-muted num shrink-0">
                        ~{fmtBytes(estimate(c))}
                      </bdi>
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
                    <Icon as={Download} /> {t("updatePlanStart")}{" "}
                    <bdi dir="ltr" className="num font-normal">
                      (~{fmtBytes(plan.reduce((n, c) => n + estimate(c), 0))})
                    </bdi>
                  </button>
                  <button className="btn" onClick={() => setPlan(null)}>
                    {t("cancel")}
                  </button>
                </div>
              </div>
            ) : (
              !firstRun && (
                <button className={clsx("btn w-full", !updating && "btn-primary")} onClick={openPlan}>
                  <Icon as={Download} /> {updating ? t("updateAlsoRest") : t("updateAll")}
                </button>
              )
            )
          ) : null}
          <p className="m-0 text-xs text-muted px-0.5">{t("dataNeedsInternet")}</p>
          <div className="group-list">
            <Toggle label={t("prebuiltData")} checked={usePrebuilt} onChange={(v) => void updateSettings({ usePrebuiltData: v })} />
          </div>
          <MoreText text={t("prebuiltDataHint")} className="px-0.5" />
        </div>
        <div className="group-list">
          {DATA_COMPONENTS.map((c) => (
            <ComponentCard
              key={c}
              id={c}
              st={status?.[c]}
              chk={check?.[c]}
              prog={progress[c]}
              updating={exclusiveBusy || busyComps.has(c)}
              waiting={queuedComps.has(c)}
              paused={paused}
              lang={lang}
              wanted={isWanted(c, wanted)}
              onUpdate={() => void startUpdate([c])}
              onCancel={() => void cancelUpdate(c)}
              onPause={(p) => void pauseUpdate(p)}
              extra={
                c === "satellite" ? (
                  <div className="flex flex-col gap-1.5 mt-1.5">
                    <label className="flex flex-col gap-1 text-xs text-muted">
                      <span>{t("satelliteSource")}</span>
                      <select
                        className="input h-9 w-full min-w-0"
                        value={satelliteSource}
                        disabled={exclusiveBusy || busyComps.has("satellite")}
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
                        className="input h-9 w-full min-w-0"
                        value={!ortho && satelliteQuality === "max" ? "high" : satelliteQuality}
                        disabled={exclusiveBusy || busyComps.has("satellite")}
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
                    {ortho && <MoreText text={t("satOrthoNote")} />}
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

      {/* advanced: transfer to an offline computer, data folder, logs, which components to offer */}
      <div className="flex flex-col gap-2">
        <button className="w-full flex items-center gap-2 h-9 px-1 rounded text-muted hover:text-fg transition-colors" aria-expanded={advanced} aria-controls="data-advanced" onClick={() => setAdvanced((v) => !v)}>
          <Icon as={SlidersHorizontal} />
          <span className="flex-1 text-start section-label p-0">{t("advanced")}</span>
          <Icon as={ChevronDown} className={clsx("transition-transform duration-150", advanced && "rotate-180")} />
        </button>
        {(advanced || (pkgOrigin === "transfer" && (pkgActive || !!pkgFile))) && (
          <div id="data-advanced" className="flex flex-col gap-4 fade-in">
            <div className="flex flex-col gap-2">
              <h2 className="section-label m-0">{t("offlineTransfer")}</h2>
              <div className="group-list">
                <div className="px-3 py-2.5 flex flex-col gap-2">
                  <button className="btn btn-sm btn-primary" disabled={updating || pkgActive} onClick={() => void doDownloadPackage()}>
                    <Icon as={Download} size={14} /> {t("downloadPackage")}
                  </button>
                  {pkgOrigin === "transfer" && <PackageProgress ref={pkgTransferRef} pkg={pkg} file={pkgFile} onInstall={pkgFile ? () => void importData(pkgFile) : undefined} />}
                  <MoreText text={t("downloadPackageHint")} />
                </div>
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
                          <button key={c} type="button" className="choice" aria-pressed={on} disabled={updating} onClick={() => setExcluded((x) => (on ? [...x, c] : x.filter((k) => k !== c)))}>
                            <Icon as={META[c].icon} size={14} />
                            {t(META[c].label)}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}
                <div className="row gap-2">
                  <button className="btn btn-sm flex-1" disabled={updating || selected.length === 0} onClick={() => void doExport()}>
                    <Icon as={PackagePlus} size={14} /> {t("exportPackage")}
                  </button>
                  <button className="btn btn-sm flex-1" disabled={updating} onClick={() => void doImport()}>
                    <Icon as={PackageOpen} size={14} /> {t("importPackage")}
                  </button>
                </div>
              </div>
              <MoreText text={t("offlineTransferHint")} className="px-0.5" />
            </div>

            <div className="flex flex-col gap-2">
              <h2 className="section-label m-0">{t("wantedComponents")}</h2>
              <div className="flex flex-wrap gap-1.5" role="group" aria-label={t("wantedComponents")}>
                {DATA_COMPONENTS.map((c) => {
                  const on = isWanted(c, wanted);
                  return (
                    <button
                      key={c}
                      type="button"
                      className="choice"
                      aria-pressed={on}
                      onClick={() => {
                        const cur = wanted ?? [...DATA_COMPONENTS];
                        const next = on ? cur.filter((x) => x !== c) : [...cur, c];
                        void updateSettings({ wantedComponents: next.length === DATA_COMPONENTS.length ? null : next });
                      }}
                    >
                      <Icon as={META[c].icon} size={14} />
                      {t(META[c].label)}
                    </button>
                  );
                })}
              </div>
              <p className="m-0 text-xs text-muted px-0.5">{t("wantedComponentsHint")}</p>
            </div>

            {status?.dir && (
              <div className="flex flex-col gap-2">
                <h2 className="section-label m-0">{t("dataDir")}</h2>
                <div className="group-list">
                  <div className="row">
                    <div className="flex-1 min-w-0 text-xs text-muted truncate selectable" dir="ltr" style={{ textAlign: "start" }}>
                      {status.dir}
                    </div>
                    <button className="btn btn-sm shrink-0" onClick={() => void openDir(status.dir)}>
                      <Icon as={FolderOpen} size={14} /> {t("openFolder")}
                    </button>
                  </div>
                  <div className="row">
                    <div className="flex-1 min-w-0 text-xs text-muted">{t("logsHint")}</div>
                    <button className="btn btn-sm shrink-0" onClick={() => void openLogs()}>
                      <Icon as={FileText} size={14} /> {t("openLogs")}
                    </button>
                  </div>
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function ComponentCard({
  id,
  st,
  chk,
  prog,
  updating,
  waiting,
  paused,
  lang,
  wanted,
  onUpdate,
  onCancel,
  onPause,
  extra,
}: {
  id: DataComponent;
  st?: ComponentStatus;
  chk?: UpdateCheck;
  prog?: { phase: ProgressPhase; done: number; total: number; message: string };
  /** this component cannot start now (it runs / waits, or a package job holds the data folder) */
  updating: boolean;
  /** waits for a running job that shares its source */
  waiting?: boolean;
  paused: boolean;
  lang: string;
  wanted: boolean;
  onUpdate: () => void;
  onCancel: () => void;
  onPause: (paused: boolean) => void;
  /** Extra controls under the description (satellite source and quality), behind "options". */
  extra?: React.ReactNode;
}) {
  const t = useT();
  const m = META[id];
  const [showExtra, setShowExtra] = useState(false);
  const active = prog && !["done", "failed", "cancelled"].includes(prog.phase);
  const running = active && prog!.phase !== "queued";
  const showPaused = !!running && paused;
  const pct = prog && prog.total > 0 ? Math.min(100, Math.round((prog.done / prog.total) * 100)) : null;
  return (
    <div className="flex flex-col gap-2 px-3 py-3">
      <div className="flex items-start gap-3">
        <span className={clsx("tile", st?.present && "text-fg")}>
          <Icon as={m.icon} />
        </span>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="font-semibold">{t(m.label)}</span>
            {active ? (
              <span className={clsx("pill", showPaused ? "" : "pill-accent")}>{showPaused ? t("phasePaused") : waiting ? t("phaseWaitingShared") : t(PHASE_KEY[prog!.phase])}</span>
            ) : !st?.present ? (
              <span className="pill">{t("missing")}</span>
            ) : chk?.available && wanted ? (
              <span className="pill pill-accent">{t("updateAvailable")}</span>
            ) : chk && !chk.error ? (
              <span className="pill pill-success">{t("upToDate")}</span>
            ) : (
              <span className="pill pill-success">{st?.loaded && id !== "tiles" ? t("loaded") : t("present")}</span>
            )}
          </div>
          <div className="text-xs text-muted mt-0.5">{t(m.desc)}</div>
          {extra && (
            <button className="text-xs text-accent hover:underline underline-offset-2 mt-1" aria-expanded={showExtra} onClick={() => setShowExtra((v) => !v)}>
              {showExtra ? t("hideOptions") : t("showOptions")}
            </button>
          )}
          {extra && showExtra && extra}
          {st?.present && (
            <div className="text-xs text-subtle mt-1 truncate num">
              <bdi dir="ltr">{fmtBytes(st.sizeBytes)}</bdi>
              {st.updatedIso && (
                <>
                  {" · "}
                  {t("updated")} <bdi dir="ltr">{fmtDateTime(st.updatedIso, lang)}</bdi>
                </>
              )}
              {st.version && <span className="sr-only">{` · ${t("version")}: ${st.version}`}</span>}
            </div>
          )}
        </div>
        {active ? (
          <div className="flex items-center shrink-0 -me-1.5">
            {running && (
              <Tip text={paused ? t("resumeUpdate") : t("pauseUpdate")}>
                <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={() => onPause(!paused)} aria-label={`${paused ? t("resumeUpdate") : t("pauseUpdate")}: ${t(m.label)}`}>
                  <Icon as={paused ? Play : Pause} size={14} />
                </button>
              </Tip>
            )}
            <Tip text={t("cancelComponent")}>
              <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={onCancel} aria-label={`${t("cancelComponent")}: ${t(m.label)}`}>
                <Icon as={X} size={15} />
              </button>
            </Tip>
          </div>
        ) : (
          <button
            className={clsx("btn btn-sm shrink-0", chk?.available && wanted && "btn-primary")}
            disabled={updating || (!!st?.present && !!chk && !chk.available && !chk.error)}
            onClick={onUpdate}
            aria-label={`${t("update")}: ${t(m.label)}`}
          >
            {t("update")}
          </button>
        )}
      </div>
      {prog && (
        <div className="fade-in ps-11">
          {prog.phase === "failed" ? (
            <InfoBar severity="error" title={t("phaseFailed")}>
              <span className="selectable break-words">{prog.message}</span>
            </InfoBar>
          ) : (
            <>
              <div className="flex justify-between gap-2 text-xs mb-1.5">
                <span className={clsx("truncate", prog.phase === "done" ? "text-success" : "text-muted")}>{active ? (showPaused ? t("phasePaused") : waiting ? t("phaseWaitingSharedHint") : prog.message || t(PHASE_KEY[prog.phase])) : t(PHASE_KEY[prog.phase])}</span>
                {pct !== null && active && <span className="text-muted num shrink-0">{pct}%</span>}
              </div>
              {active && (
                <div className="h-1 rounded-full bg-surface-2 overflow-hidden" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined} aria-label={t(m.label)}>
                  <div
                    className={clsx("h-full bg-accent rounded-full transition-[width] duration-300 ease-out", pct === null && !showPaused && "animate-pulse w-1/3", pct === null && showPaused && "w-1/3 opacity-50")}
                    style={pct !== null ? { width: `${pct}%` } : undefined}
                  />
                </div>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
