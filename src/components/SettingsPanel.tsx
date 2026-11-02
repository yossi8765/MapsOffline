import { useState } from "react";
import { FolderOpen, RotateCcw } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { getScrollMode, setScrollMode, type ScrollMode } from "../lib/gestures";
import { Toggle } from "./RoutePanel";

export function SettingsPanel() {
  const t = useT();
  const [scrollMode, setScroll] = useState<ScrollMode>(getScrollMode);
  const pickScroll = (m: ScrollMode) => {
    setScrollMode(m);
    setScroll(m);
  };
  const settings = useStore((s) => s.settings);
  const version = useStore((s) => s.appVersion);
  const update = useStore((s) => s.updateSettings);

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

      <Section title={t("navigation")} hint={t("avoidAreasHint")}>
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
          <div className="flex-1 min-w-0 text-xs text-muted truncate" dir="ltr" style={{ textAlign: "start" }} title={settings.dataDir ?? ""}>
            {settings.dataDir || useStore.getState().dataStatus?.dir || "\u2014"}
          </div>
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

      <Section title={t("shortcuts")}>
        {[
          ["Ctrl+F", t("shortcutSearch")],
          ["Ctrl+R", t("shortcutRoute")],
          ["Ctrl+H", t("shortcutHistory")],
          ["Esc", t("shortcutEsc")],
          ["+ / \u2212", t("shortcutZoom")],
        ].map(([k, v]) => (
          <div key={k} className="row min-h-10 justify-between">
            <span className="text-base">{v}</span>
            <kbd className="kbd" dir="ltr">
              {k}
            </kbd>
          </div>
        ))}
      </Section>

      <div className="flex flex-col gap-1 px-0.5 text-sm">
        <h2 className="section-label m-0 p-0 mb-1">{t("about")}</h2>
        <div className="font-semibold">
          {t("appName")} {version && <span className="text-subtle font-normal num">v{version}</span>}
        </div>
        <p className="m-0 text-muted">{t("aboutText")}</p>
      </div>
    </div>
  );
}

function Section({ title, hint, children }: { title: string; hint?: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h2 className="section-label m-0">{title}</h2>
      <div className="group-list">{children}</div>
      {hint && <p className="m-0 text-xs text-muted px-0.5">{hint}</p>}
    </section>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="row justify-between">
      <span className="text-base">{label}</span>
      {children}
    </div>
  );
}
