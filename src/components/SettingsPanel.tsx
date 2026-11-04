import clsx from "clsx";
import { useState } from "react";
import { ChevronDown, FolderOpen, RotateCcw, Download, RefreshCw } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { getScrollMode, setScrollMode, type ScrollMode } from "../lib/gestures";
import { Toggle } from "./RoutePanel";
import { SHORTCUTS } from "./ShortcutsDialog";
import { DATA_COMPONENTS } from "../lib/api";
import { fmtDateTime } from "../lib/format";

export function SettingsPanel() {
  const t = useT();
  const [scrollMode, setScroll] = useState<ScrollMode>(getScrollMode);
  const pickScroll = (m: ScrollMode) => {
    setScrollMode(m);
    setScroll(m);
  };
  const settings = useStore((s) => s.settings);
  const version = useStore((s) => s.appVersion);
  const appUpdate = useStore((s) => s.appUpdate);
  const updateBusy = useStore((s) => s.appUpdateBusy);
  const updateProgress = useStore((s) => s.appUpdateProgress);
  const { checkAppUpdate, installAppUpdate } = useStore.getState();
  const update = useStore((s) => s.updateSettings);
  const statusDir = useStore((s) => s.dataStatus?.dir);
  const dataStatus = useStore((s) => s.dataStatus);
  // "נתונים: OSM 2026-11-01 · עודכנו 03/11/2026": the source date of the map / routing data and
  // when data was last installed here
  const dataStamp = (() => {
    const osm = [dataStatus?.routing?.version, dataStatus?.geocode?.version].map((v) => /OSM (\d{4}-\d{2}-\d{2})/.exec(v ?? "")?.[1]).find(Boolean);
    const times = DATA_COMPONENTS.map((c) => dataStatus?.[c]?.updatedIso).filter((x): x is string => !!x).sort();
    const built = times.length ? fmtDateTime(times[times.length - 1]).split(" ")[0] : "";
    if (!osm && !built) return "";
    const parts = [osm ? t("dataStampOsm").replace("{d}", osm) : "", built ? t("dataStampBuilt").replace("{d}", built) : ""].filter(Boolean);
    return `${t("dataStamp")}: ${parts.join(" · ")}`;
  })();
  const [advancedOpen, setAdvancedOpen] = useState(false);

  const chooseDir = async () => {
    try {
      const dir = await open({ directory: true, multiple: false, title: t("chooseFolder") });
      if (typeof dir === "string" && dir) await update({ dataDir: dir });
    } catch (e) {
      useStore.getState().toastError(e);
    }
  };

  return (
    <div className="px-4 pb-5 flex flex-col gap-5">
      <Section title={t("appearance")}>
        <Row label={t("language")}>
          <div className="seg seg-sm seg-fit">
            <button aria-pressed={settings.language === "he"} onClick={() => void update({ language: "he" })}>
              <span>{t("langHe")}</span>
            </button>
            <button aria-pressed={settings.language === "en"} onClick={() => void update({ language: "en" })}>
              <span>{t("langEn")}</span>
            </button>
          </div>
        </Row>
        <Row label={t("theme")}>
          <div className="seg seg-sm seg-fit">
            {(["auto", "light", "dark", "night"] as const).map((th) => (
              <button key={th} aria-pressed={settings.theme === th} onClick={() => void update({ theme: th })}>
                <span>{t(th === "auto" ? "themeAuto" : th === "light" ? "themeLight" : th === "dark" ? "themeDark" : "themeNight")}</span>
              </button>
            ))}
          </div>
        </Row>
        <Row label={t("accentColor")} hint={t("accentColorHint")}>
          <div className="flex gap-1.5 flex-wrap" role="radiogroup" aria-label={t("accentColor")}>
            {(["blue", "green", "purple", "orange", "teal", "rose", "slate", "pastel"] as const).map((a) => (
              <button
                key={a}
                type="button"
                role="radio"
                aria-checked={settings.accent === a}
                title={t(`accent_${a}` as I18nKey)}
                aria-label={t(`accent_${a}` as I18nKey)}
                className={clsx("w-6 h-6 rounded-full border-2 transition-transform duration-150 hover:scale-110", settings.accent === a ? "border-fg scale-110" : "border-transparent")}
                style={{ background: ACCENT_SWATCH[a] }}
                onClick={() => void update({ accent: a })}
              />
            ))}
          </div>
        </Row>
        <Row label={t("units")}>
          <div className="seg seg-sm seg-fit">
            <button aria-pressed={settings.units === "metric"} onClick={() => void update({ units: "metric" })}>
              <span>{t("unitsMetric")}</span>
            </button>
            <button aria-pressed={settings.units === "imperial"} onClick={() => void update({ units: "imperial" })}>
              <span>{t("unitsImperial")}</span>
            </button>
          </div>
        </Row>
      </Section>

      <Section title={t("routingMode")} hint={t("routingModeHint")}>
        <div className="p-2">
          <div className="seg seg-fit">
            {(["auto", "online", "offline"] as const).map((m) => (
              <button key={m} aria-pressed={settings.routingMode === m} onClick={() => void update({ routingMode: m })}>
                <span>{t(m === "auto" ? "routingAuto" : m === "online" ? "routingOnline" : "routingOffline")}</span>
              </button>
            ))}
          </div>
        </div>
      </Section>

      <Section title={t("startup")}>
        <Toggle label={t("rememberMapView")} checked={settings.rememberMapView} onChange={(v) => void update({ rememberMapView: v })} />
        <Toggle label={t("navVoiceSetting")} checked={settings.navVoice ?? true} onChange={(v) => void update({ navVoice: v })} />
        <Toggle label={t("transitReliabilitySetting")} checked={settings.transitReliability ?? true} onChange={(v) => void update({ transitReliability: v })} />
      </Section>

      <Section title={t("navigationDefaults")} hint={`${t("navDefaultsHint")} ${t("avoidAreasHint")}`}>
        <Toggle label={t("avoidTollsDefault")} checked={settings.avoidTolls} onChange={(v) => void update({ avoidTolls: v })} />
        <Toggle label={t("avoidMotorwaysDefault")} checked={settings.avoidMotorways} onChange={(v) => void update({ avoidMotorways: v })} />
        <Toggle label={t("avoidFerriesDefault")} checked={settings.avoidFerries} onChange={(v) => void update({ avoidFerries: v })} />
        <Row label={t("unpavedRoads")}>
          <div className="seg seg-sm seg-fit">
            {(["allow", "avoidLong", "avoid"] as const).map((m) => (
              <button key={m} aria-pressed={settings.unpaved === m} onClick={() => void update({ unpaved: m })}>
                <span>{t(m === "allow" ? "unpavedAllow" : m === "avoidLong" ? "unpavedAvoidLong" : "unpavedAvoid")}</span>
              </button>
            ))}
          </div>
        </Row>
        <Toggle label={t("avoidHardTurnsDefault")} checked={settings.avoidHardTurns} onChange={(v) => void update({ avoidHardTurns: v })} />
        <Row label={t("avoidAreas")}>
          <div className="seg seg-sm seg-fit">
            {(["ab", "a", "none"] as const).map((m) => (
              <button key={m} aria-pressed={settings.avoidAreas === m} onClick={() => void update({ avoidAreas: m })}>
                <span>{t(m === "ab" ? "avoidAreasOptAB" : m === "a" ? "avoidAreasOptA" : "avoidAreasOptNone")}</span>
              </button>
            ))}
          </div>
        </Row>
      </Section>

      <Section title={t("shortcuts")} hint={t("shortcutsNote")}>
        {SHORTCUTS.map(([k, label]) => (
          <div key={k} className="row min-h-10 justify-between">
            <span className="text-base">{t(label)}</span>
            <kbd className="kbd" dir="ltr">
              {k}
            </kbd>
          </div>
        ))}
      </Section>

      {/* rarely changed: data folder, touchpad behaviour */}
      <section className="flex flex-col gap-2">
        <button
          type="button"
          className="flex items-center gap-1 section-label m-0 self-start hover:text-fg transition-colors"
          aria-expanded={advancedOpen}
          onClick={() => setAdvancedOpen((v) => !v)}
        >
          {t("advanced")}
          <ChevronDown size={14} className={clsx("transition-transform duration-150", advancedOpen && "rotate-180")} />
        </button>
        {advancedOpen && (
          <div className="flex flex-col gap-5 fade-in">
      <Section title={t("gestures")} hint={t("gesturesHint")}>
        <Row label={t("twoFingerScroll")}>
          <div className="seg seg-sm seg-fit">
            {(["pan", "zoom"] as const).map((m) => (
              <button key={m} aria-pressed={scrollMode === m} onClick={() => pickScroll(m)}>
                <span>{t(m === "pan" ? "scrollPan" : "scrollZoom")}</span>
              </button>
            ))}
          </div>
        </Row>
      </Section>

      <Section title={t("dataDir")}>
        <div className="row">
          <bdi dir="ltr" className="flex-1 min-w-0 text-xs text-muted truncate selectable" title={settings.dataDir || statusDir || ""}>
            {settings.dataDir || statusDir || "\u2014"}
          </bdi>
        </div>
        <div className="row gap-2">
          <button className="btn btn-sm flex-1" onClick={() => void chooseDir()}>
            <FolderOpen size={14} strokeWidth={1.75} /> {t("chooseFolder")}
          </button>
          <button className="btn btn-sm btn-ghost text-muted" disabled={!settings.dataDir} onClick={() => void update({ dataDir: null })}>
            <RotateCcw size={14} strokeWidth={1.75} /> {t("resetFolder")}
          </button>
        </div>
      </Section>
          </div>
        )}
      </section>

      <div className="flex flex-col gap-1 px-0.5 text-sm">
        <h2 className="section-label m-0 p-0 mb-1">{t("about")}</h2>
        <div className="font-semibold">
          {t("appName")} {version && <span className="text-subtle font-normal num">v{version}</span>}
        </div>
        <p className="m-0 text-muted">{t("aboutText")}</p>
        {dataStamp && <p className="m-0 text-xs text-muted num">{dataStamp}</p>}
        <div className="group-list mt-2">
          <Toggle label={t("autoCheckAppUpdates")} checked={settings.autoCheckAppUpdates} onChange={(v) => void update({ autoCheckAppUpdates: v })} />
          <div className="row gap-2 flex-wrap">
            <button className="btn btn-sm" disabled={!!updateBusy} onClick={() => void checkAppUpdate(false)}>
              <RefreshCw size={14} strokeWidth={1.75} className={clsx(updateBusy === "check" && "animate-spin")} />
              {updateBusy === "check" ? t("checking") : t("checkAppUpdates")}
            </button>
            {appUpdate?.available && !updateBusy && (
              <button className="btn btn-sm btn-primary" onClick={() => void installAppUpdate()}>
                <Download size={14} strokeWidth={2} /> {t("installUpdate")} {appUpdate.latest}
              </button>
            )}
            {appUpdate?.available && (
              <a className="text-xs text-accent underline-offset-2 hover:underline" href={appUpdate.pageUrl} target="_blank" rel="noreferrer">
                {t("releaseNotes")}
              </a>
            )}
          </div>
          {appUpdate && !updateBusy && (
            <div className="px-3 py-2 text-xs">
              {appUpdate.error ? (
                <span className="text-danger">{t("checkFailed")}: {appUpdate.error}</span>
              ) : appUpdate.available ? (
                <span className="text-fg">{t("updateAvailableToast")} <span className="num">{appUpdate.latest}</span> ({t("installedVersion")} <span className="num">{appUpdate.current}</span>)</span>
              ) : (
                <span className="text-muted">{t("appUpToDate")} <span className="num">{appUpdate.current}</span></span>
              )}
            </div>
          )}
          {updateBusy === "download" && updateProgress && (
            <div className="px-3 py-2 text-xs">
              <div className="flex justify-between gap-2 mb-1.5">
                <span className="truncate text-muted">{updateProgress.message || t("downloadingUpdate")}</span>
                {updateProgress.total > 0 && <span className="num shrink-0">{Math.min(100, Math.round((updateProgress.done / updateProgress.total) * 100))}%</span>}
              </div>
              <div className="h-1 rounded-full bg-surface-2 overflow-hidden">
                <div className={clsx("h-full bg-accent rounded-full transition-[width] duration-300", updateProgress.total === 0 && "animate-pulse w-1/3")} style={updateProgress.total > 0 ? { width: `${Math.min(100, (updateProgress.done / updateProgress.total) * 100)}%` } : undefined} />
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

const ACCENT_SWATCH: Record<string, string> = {
  blue: "#1a66d9",
  green: "#15803d",
  purple: "#6d28d9",
  orange: "#c2410c",
  teal: "#0f766e",
  rose: "#be123c",
  slate: "#334155",
  // the pastel scheme recolours the whole panel: show its palette on the swatch
  pastel: "conic-gradient(from 90deg, #b8c8f5, #f5c6d6, #c8ecd9, #fbe3b3, #d9ccf5, #b8c8f5)",
};

function Section({ title, hint, children }: { title: string; hint?: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h2 className="section-label m-0">{title}</h2>
      <div className="group-list">{children}</div>
      {hint && <p className="m-0 text-xs text-muted px-0.5">{hint}</p>}
    </section>
  );
}

/** Label above its control, the control full width: long Hebrew labels never wrap into a
 *  narrow column next to a segmented control. */
function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-2 px-3 py-2.5 [&>.seg]:w-full">
      <span className="flex flex-col">
        <span className="text-base">{label}</span>
        {hint && <span className="text-xs text-muted">{hint}</span>}
      </span>
      {children}
    </div>
  );
}
