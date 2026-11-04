import clsx from "clsx";
import { useState } from "react";
import { ChevronDown, Download, ExternalLink, FileText, FolderOpen, RefreshCw, RotateCcw } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { openPath, openUrl } from "@tauri-apps/plugin-opener";
import { Icon } from "./Icon";
import { DEFAULT_SETTINGS, useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { getScrollMode, setScrollMode, type ScrollMode } from "../lib/gestures";
import { Toggle } from "./RoutePanel";
import { ShortcutList } from "./ShortcutsDialog";
import { apiX, COLOR_SCHEMES, DATA_COMPONENTS, type ColorScheme, type CoordFormat, type Settings } from "../lib/api";
import { askConfirm, fmtDate } from "../lib/format";
import { CREDITS } from "../lib/credits";
import { formatCoord } from "../lib/coords";

const ACCENTS = ["system", "blue", "green", "purple", "orange", "teal", "rose", "slate"] as const;

/** Every `mapsoffline.*` key in localStorage (layers, view, nearby, scroll mode…). */
function clearLocalPrefs() {
  try {
    const keys: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k?.startsWith("mapsoffline.")) keys.push(k);
    }
    keys.forEach((k) => localStorage.removeItem(k));
  } catch {
    /* storage unavailable */
  }
}

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
  const online = useStore((s) => s.online);
  const { checkAppUpdate, installAppUpdate } = useStore.getState();
  const update = useStore((s) => s.updateSettings);
  const statusDir = useStore((s) => s.dataStatus?.dir);
  const dataStatus = useStore((s) => s.dataStatus);
  const lang = settings.language;
  // "נתונים: OSM 2026-11-01 · עודכנו 03/11/2026": the source date of the map / routing data and
  // when data was last installed here
  const dataStamp = (() => {
    const osm = [dataStatus?.routing?.version, dataStatus?.geocode?.version].map((v) => /OSM (\d{4}-\d{2}-\d{2})/.exec(v ?? "")?.[1]).find(Boolean);
    const times = DATA_COMPONENTS.map((c) => dataStatus?.[c]?.updatedIso).filter((x): x is string => !!x).sort();
    const built = times.length ? fmtDate(times[times.length - 1], lang) : "";
    if (!osm && !built) return "";
    const parts = [osm ? t("dataStampOsm").replace("{d}", osm) : "", built ? t("dataStampBuilt").replace("{d}", built) : ""].filter(Boolean);
    return `${t("dataStamp")}: ${parts.join(" · ")}`;
  })();
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [creditsOpen, setCreditsOpen] = useState(false);

  const chooseDir = async () => {
    try {
      const dir = await open({ directory: true, multiple: false, title: t("chooseFolder") });
      if (typeof dir === "string" && dir) await update({ dataDir: dir });
    } catch (e) {
      useStore.getState().toastError(e);
    }
  };
  const openLogs = async () => {
    try {
      await openPath(await apiX.logsDir());
    } catch (e) {
      useStore.getState().toastError(e);
    }
  };
  const resetAll = async () => {
    if (!(await askConfirm(t("resetSettingsConfirm"), t("resetSettings"), t("cancel"), t("appName")))) return;
    clearLocalPrefs();
    // the data folder stays: resetting it would hide every installed component
    const keep: Partial<Settings> = { dataDir: settings.dataDir, teachingTipsSeen: settings.teachingTipsSeen };
    await update({ ...DEFAULT_SETTINGS, ...keep });
    useStore.getState().toast("success", t("resetSettingsDone"));
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
        <Row label={t("colorScheme")} hint={t("colorSchemeHint")}>
          <div className="flex gap-2 flex-wrap" role="radiogroup" aria-label={t("colorScheme")}>
            {COLOR_SCHEMES.map((c) => {
              const on = (settings.colorScheme ?? "white") === c;
              return (
                <button
                  key={c}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  title={t(`scheme_${c}` as I18nKey)}
                  aria-label={t(`scheme_${c}` as I18nKey)}
                  className={clsx(
                    "relative w-8 h-8 rounded-full border transition-transform duration-150 hover:scale-110 flex items-center justify-center",
                    on ? "border-fg border-2 scale-110" : "border-line-strong",
                  )}
                  style={{ background: SCHEME_SWATCH[c][0] }}
                  onClick={() => void update({ colorScheme: c })}
                >
                  {/* the scheme's accent as a dot: what buttons and highlights will look like */}
                  <span aria-hidden className="w-2.5 h-2.5 rounded-full" style={{ background: SCHEME_SWATCH[c][1] }} />
                </button>
              );
            })}
          </div>
        </Row>
        {(settings.colorScheme ?? "white") === "white" && (
        <Row label={t("accentColor")} hint={t("accentColorHint")}>
          <div className="flex gap-1.5 flex-wrap" role="radiogroup" aria-label={t("accentColor")}>
            {ACCENTS.map((a) => (
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
        )}
        <Row label={t("textSize")} hint={t("textSizeHint")}>
          <div className="seg seg-sm seg-fit">
            {(["system", 1, 1.12, 1.25] as const).map((v) => (
              <button key={String(v)} aria-pressed={settings.uiScale === v} onClick={() => void update({ uiScale: v })}>
                <span>{t(v === "system" ? "textSizeSystem" : v === 1 ? "textSizeNormal" : v === 1.12 ? "textSizeLarge" : "textSizeXLarge")}</span>
              </button>
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
        <Row label={t("coordFormat")} hint={formatCoord(34.78, 32.08, settings.coordFormat ?? "dd")}>
          <div className="seg seg-sm seg-fit">
            {(["dd", "dms", "itm", "utm"] as CoordFormat[]).map((f) => (
              <button key={f} aria-pressed={(settings.coordFormat ?? "dd") === f} onClick={() => void update({ coordFormat: f })}>
                <span>{t(`coord_${f}` as I18nKey)}</span>
              </button>
            ))}
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

      <section className="flex flex-col gap-2">
        <h2 className="section-label m-0">{t("shortcuts")}</h2>
        <ShortcutList />
        <p className="m-0 text-xs text-muted px-0.5">{t("shortcutsNote")}</p>
      </section>

      {/* rarely changed: data folder, touchpad behaviour, logs, reset */}
      <section className="flex flex-col gap-2">
        <button type="button" className="flex items-center gap-1 section-label m-0 self-start hover:text-fg transition-colors" aria-expanded={advancedOpen} onClick={() => setAdvancedOpen((v) => !v)}>
          {t("advanced")}
          <Icon as={ChevronDown} size={14} className={clsx("transition-transform duration-150", advancedOpen && "rotate-180")} />
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
                <bdi dir="ltr" className="flex-1 min-w-0 text-xs text-muted truncate selectable">
                  {settings.dataDir || statusDir || "—"}
                </bdi>
              </div>
              <div className="row gap-2">
                <button className="btn btn-sm flex-1" onClick={() => void chooseDir()}>
                  <Icon as={FolderOpen} size={14} /> {t("chooseFolder")}
                </button>
                <button className="btn btn-sm btn-ghost text-muted" disabled={!settings.dataDir} onClick={() => void update({ dataDir: null })}>
                  <Icon as={RotateCcw} size={14} /> {t("resetFolder")}
                </button>
              </div>
            </Section>

            <Section title={t("troubleshooting")} hint={t("logsHint")}>
              <div className="row gap-2 flex-wrap">
                <button className="btn btn-sm" onClick={() => void openLogs()}>
                  <Icon as={FileText} size={14} /> {t("openLogs")}
                </button>
                <button className="btn btn-sm btn-danger" onClick={() => void resetAll()}>
                  <Icon as={RotateCcw} size={14} /> {t("resetSettings")}
                </button>
              </div>
            </Section>
          </div>
        )}
      </section>

      <div className="flex flex-col gap-1 px-0.5">
        <h2 className="section-label m-0 p-0 mb-1">{t("about")}</h2>
        <div className="font-semibold">
          {t("appName")} {version && <span className="text-subtle font-normal num">v{version}</span>}
        </div>
        <p className="m-0 text-muted">{t("aboutText")}</p>
        {dataStamp && <p className="m-0 text-xs text-muted num">{dataStamp}</p>}
        <button className="self-start flex items-center gap-1 text-accent hover:underline underline-offset-2 mt-1" aria-expanded={creditsOpen} onClick={() => setCreditsOpen((v) => !v)}>
          {t("creditsTitle")}
          <Icon as={ChevronDown} size={14} className={clsx("transition-transform duration-150", creditsOpen && "rotate-180")} />
        </button>
        {creditsOpen && (
          <ul className="m-0 p-0 list-none group-list mt-1 fade-in">
            {CREDITS.map((c) => (
              <li key={c.id} className="px-3 py-2 flex flex-col gap-0.5">
                <span className="flex items-center gap-1.5">
                  <bdi className="font-medium">{c.name}</bdi>
                  {c.url && online && (
                    <button className="text-subtle hover:text-accent" aria-label={`${c.name} – ${t("openWebsite")}`} onClick={() => void openUrl(c.url).catch(() => {})}>
                      <Icon as={ExternalLink} size={12} />
                    </button>
                  )}
                </span>
                <span className="text-xs text-muted">
                  {t(c.use)} · <bdi>{c.licence}</bdi>
                </span>
              </li>
            ))}
          </ul>
        )}
        <div className="group-list mt-2">
          <Toggle label={t("autoCheckAppUpdates")} checked={settings.autoCheckAppUpdates} onChange={(v) => void update({ autoCheckAppUpdates: v })} />
          <div className="row gap-2 flex-wrap">
            <button className="btn btn-sm" disabled={!!updateBusy} onClick={() => void checkAppUpdate(false)}>
              <Icon as={RefreshCw} size={14} className={clsx(updateBusy === "check" && "animate-spin")} />
              {updateBusy === "check" ? t("checking") : t("checkAppUpdates")}
            </button>
            {appUpdate?.available && !updateBusy && (
              <button className="btn btn-sm btn-primary" onClick={() => void installAppUpdate()}>
                <Icon as={Download} size={14} /> {t("installUpdate")} {appUpdate.latest}
              </button>
            )}
            {appUpdate?.available && appUpdate.pageUrl && (
              <button className="text-xs text-accent underline-offset-2 hover:underline" onClick={() => void openUrl(appUpdate.pageUrl).catch(() => {})}>
                {t("releaseNotes")}
              </button>
            )}
          </div>
          {appUpdate && !updateBusy && (
            <div className="px-3 py-2 text-xs">
              {appUpdate.error ? (
                <span className="text-danger">
                  {t("checkFailed")}: {appUpdate.error}
                </span>
              ) : appUpdate.available ? (
                <span className="text-fg">
                  {t("updateAvailableToast")} <span className="num">{appUpdate.latest}</span> ({t("installedVersion")} <span className="num">{appUpdate.current}</span>)
                </span>
              ) : (
                <span className="text-muted">
                  {t("appUpToDate")} <span className="num">{appUpdate.current}</span>
                </span>
              )}
            </div>
          )}
          {updateBusy === "download" && updateProgress && (
            <div className="px-3 py-2 text-xs">
              <div className="flex justify-between gap-2 mb-1.5">
                <span className="truncate text-muted">{updateProgress.message || t("downloadingUpdate")}</span>
                {updateProgress.total > 0 && <span className="num shrink-0">{Math.min(100, Math.round((updateProgress.done / updateProgress.total) * 100))}%</span>}
              </div>
              <div className="h-1 rounded-full bg-surface-2 overflow-hidden" role="progressbar" aria-label={t("downloadingUpdate")}>
                <div
                  className={clsx("h-full bg-accent rounded-full transition-[width] duration-300", updateProgress.total === 0 && "animate-pulse w-1/3")}
                  style={updateProgress.total > 0 ? { width: `${Math.min(100, (updateProgress.done / updateProgress.total) * 100)}%` } : undefined}
                />
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** [surface tint, accent] of each colour scheme, as drawn on its swatch. */
const SCHEME_SWATCH: Record<ColorScheme, [string, string]> = {
  white: ["#ffffff", "var(--accent)"],
  sky: ["#cfe3fb", "#1f6fd1"],
  mint: ["#cdeedd", "#1d8352"],
  lavender: ["#ded6f7", "#6b4fd8"],
  peach: ["#fbdcc8", "#c4561a"],
  rose: ["#f8d3dd", "#c42d5a"],
  sand: ["#efe3c4", "#946a12"],
  aqua: ["#c9ecee", "#13808a"],
};

const ACCENT_SWATCH: Record<string, string> = {
  // the Windows accent: the swatch shows a Windows-blue / system gradient
  system: "linear-gradient(135deg, #0067c0 0 50%, #4cc2ff 50% 100%)",
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
        <span>{label}</span>
        {hint && (
          <span className="text-xs text-muted" dir="auto">
            {hint}
          </span>
        )}
      </span>
      {children}
    </div>
  );
}
