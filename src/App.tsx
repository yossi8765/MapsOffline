import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, apiX, type DataProgress, type SystemAccent } from "./lib/api";
import { useTauriEvent } from "./lib/useTauriEvent";
import { resolveTheme, useStore } from "./lib/store";
import { isRtl, translate } from "./lib/i18n";
import { isBrowserKey, matchShortcut } from "./lib/shortcuts";
import { exitFullscreen, isFullscreenNow } from "./lib/fullscreen";
import { askConfirm } from "./lib/format";
import { MapView } from "./components/MapView";
import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toast";
import { PrintSheet } from "./components/PrintSheet";
import { LayersPopover } from "./components/LayersPopover";
import { ShortcutsDialog } from "./components/ShortcutsDialog";
import { NearbyList } from "./components/NearbyList";
import { CommandPalette } from "./components/CommandPalette";
import { ShareDialog } from "./components/ShareDialog";
import { BatchGeocodeDialog } from "./components/BatchGeocodeDialog";
import { TeachingTip } from "./components/TeachingTip";

/** Elements where the browser's own context menu stays (copy / paste in text fields). */
function wantsNativeMenu(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  return !!el.closest("input, textarea, [contenteditable=true], [contenteditable='']");
}

function applySystemAccent(a: SystemAccent | null) {
  const root = document.documentElement.style;
  if (!a) {
    for (const k of ["--sys-accent", "--sys-light1", "--sys-light2", "--sys-dark1"]) root.removeProperty(k);
    return;
  }
  root.setProperty("--sys-accent", a.accent);
  root.setProperty("--sys-light1", a.light1);
  root.setProperty("--sys-light2", a.light2);
  root.setProperty("--sys-dark1", a.dark1);
}

export default function App() {
  const theme = useStore((s) => s.settings.theme);
  const lang = useStore((s) => s.settings.language);
  const settingsLoaded = useStore((s) => s.settingsLoaded);

  // startup
  useEffect(() => {
    const s = useStore.getState();
    void (async () => {
      await s.loadSettings();
      await Promise.all([s.refreshDataStatus(), s.refreshTiles(), s.loadFavorites(), s.loadHistory()]);
      // transit too: origin/destination search offers bus stops once the model is in memory
      api.dataPreload(true).catch(() => {});
    })();
    // program update: on every start, a few seconds in (the window is drawn first), when the setting allows it
    const timer = window.setTimeout(async () => {
      const s = useStore.getState();
      if (!s.settings.autoCheckAppUpdates) return;
      if (!(await s.checkConnectivity())) return;
      void s.checkAppUpdate(true);
    }, 5_000);
    return () => window.clearTimeout(timer);
  }, []);

  useTauriEvent<DataProgress>("data-progress", (p) => useStore.getState().setProgress(p));
  useTauriEvent<DataProgress>("app-update-progress", (p) => useStore.setState({ appUpdateProgress: { done: p.done, total: p.total, message: p.message } }));
  useTauriEvent("data-updated", () => {
    const st = useStore.getState();
    void st.refreshDataStatus();
    void st.refreshTiles();
  });

  // connectivity: the OS events are a hint, the backend probe decides (gates "open in Google
  // Maps", websites, the online map). Re-checked every 2 minutes.
  useEffect(() => {
    const sync = async () => {
      const was = useStore.getState().online;
      const ok = await useStore.getState().checkConnectivity();
      // back online without a local map: fetch the online map now
      const s = useStore.getState();
      if (ok && !was && s.tilesChecked && !s.tilesUrl && !s.onlineTilesUrl) void s.refreshTiles();
    };
    const onChange = () => void sync();
    window.addEventListener("online", onChange);
    window.addEventListener("offline", onChange);
    void sync();
    const timer = window.setInterval(onChange, 120_000);
    return () => {
      window.removeEventListener("online", onChange);
      window.removeEventListener("offline", onChange);
      window.clearInterval(timer);
    };
  }, []);

  // theme – the page and the Windows title bar / frame together
  useEffect(() => {
    const apply = () => {
      const resolved = resolveTheme(theme);
      document.documentElement.setAttribute("data-theme", resolved);
      // "auto" leaves the frame to Windows; a forced theme paints it to match (needs the
      // core:window:allow-set-theme permission; an older build just keeps the system frame)
      try {
        getCurrentWindow()
          .setTheme(theme === "auto" ? null : resolved)
          .catch(() => {});
      } catch {
        /* not inside Tauri (browser preview, tests): there is no native frame */
      }
    };
    apply();
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", apply);
    // "night" follows the sun: re-evaluate every minute so the switch happens on time.
    const timer = theme === "night" ? window.setInterval(apply, 60_000) : undefined;
    return () => {
      mq.removeEventListener("change", apply);
      if (timer) window.clearInterval(timer);
    };
  }, [theme]);

  // colour scheme of the whole interface (a light colour row in Settings); "white" = plain
  const colorScheme = useStore((s) => s.settings.colorScheme);
  useEffect(() => {
    // `?scheme=mint` previews a scheme without touching the settings (headless screenshots)
    const preview = typeof location !== "undefined" ? new URLSearchParams(location.search).get("scheme") : null;
    document.documentElement.setAttribute("data-scheme", preview || colorScheme || "white");
  }, [colorScheme]);
  // the old "pastel" accent was a whole-panel scheme: it becomes the lavender scheme
  useEffect(() => {
    const st = useStore.getState();
    if (settingsLoaded && (st.settings.accent as string) === "pastel") void st.updateSettings({ accent: "system", colorScheme: "lavender" });
  }, [settingsLoaded]);

  // accent colour; "system" follows the Windows accent and its changes
  const accent = useStore((s) => s.settings.accent);
  useEffect(() => {
    // `?accent=pastel` previews a scheme without touching the settings (headless screenshots)
    const preview = typeof location !== "undefined" ? new URLSearchParams(location.search).get("accent") : null;
    const value = preview || accent || "blue";
    document.documentElement.setAttribute("data-accent", value);
    if (value !== "system") {
      applySystemAccent(null);
      return;
    }
    let dead = false;
    let un: (() => void) | undefined;
    apiX
      .systemAccent()
      .then((a) => !dead && applySystemAccent(a))
      .catch(() => {});
    apiX
      .onSystemAccent((a) => applySystemAccent(a))
      .then((f) => (dead ? f() : (un = f)))
      .catch(() => {});
    return () => {
      dead = true;
      un?.();
    };
  }, [accent]);

  // text size (Settings → appearance): a factor on every type / control token
  const uiScale = useStore((s) => s.settings.uiScale);
  useEffect(() => {
    const set = (f: number) => document.documentElement.style.setProperty("--ui-scale", String(Math.min(1.25, Math.max(1, f))));
    if (uiScale !== "system") {
      set(Number(uiScale) || 1);
      return;
    }
    let dead = false;
    const read = () =>
      apiX
        .systemTextScale()
        .then((f) => !dead && set(f))
        .catch(() => !dead && set(1));
    void read();
    // Windows has no event for the text-size slider here: look again when the window comes back
    window.addEventListener("focus", read);
    return () => {
      dead = true;
      window.removeEventListener("focus", read);
    };
  }, [uiScale]);

  // direction / lang
  useEffect(() => {
    document.documentElement.setAttribute("lang", lang);
    document.documentElement.setAttribute("dir", isRtl(lang) ? "rtl" : "ltr");
  }, [lang]);

  // The WebView's own context menu (back, reload, save as, print, devtools…) is not part of the
  // app: only text fields keep it for copy / paste. The map has its own menu.
  useEffect(() => {
    const onMenu = (e: MouseEvent) => {
      if (!wantsNativeMenu(e.target)) e.preventDefault();
    };
    document.addEventListener("contextmenu", onMenu);
    return () => document.removeEventListener("contextmenu", onMenu);
  }, []);

  // global shortcuts (lib/shortcuts.ts – the same table the shortcuts dialog shows)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // reload / history navigation would reset the whole app
      if (isBrowserKey(e)) {
        e.preventDefault();
        return;
      }
      const s = useStore.getState();
      const el = e.target as HTMLElement | null;
      const typing = !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
      if (e.key === "Escape") {
        if (s.contextMenu) {
          s.setContextMenu(null);
          return;
        }
        if (s.navigation && !typing) {
          e.preventDefault();
          const t = (k: Parameters<typeof translate>[1]) => translate(s.settings.language, k);
          void askConfirm(t("navEndConfirm"), t("navEnd"), t("cancel"), t("appName")).then((ok) => ok && void useStore.getState().stopNavigation());
          return;
        }
        // nothing else wanted Escape: leave full screen (F11)
        if (!typing && isFullscreenNow()) void exitFullscreen();
        return;
      }
      const sc = matchShortcut(e, typing);
      if (!sc?.run) return;
      e.preventDefault();
      sc.run(s);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <>
      <div className="app-shell relative w-full h-full bg-bg text-fg overflow-hidden">
        <MapView />
        {settingsLoaded && <Sidebar />}
        <LayersPopover />
        <NearbyList />
        <ShortcutsDialog />
        <CommandPalette />
        <ShareDialog />
        <BatchGeocodeDialog />
        <TeachingTip />
        <Toasts />
      </div>
      <PrintSheet />
    </>
  );
}
