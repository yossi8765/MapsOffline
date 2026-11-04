import { useEffect } from "react";
import { api } from "./lib/api";
import { resolveTheme, useStore } from "./lib/store";
import { isRtl } from "./lib/i18n";
import { MapView } from "./components/MapView";
import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toast";
import { PrintSheet } from "./components/PrintSheet";
import { LayersPopover } from "./components/LayersPopover";
import { ShortcutsDialog } from "./components/ShortcutsDialog";
import { NearbyList } from "./components/NearbyList";
import { printPage } from "./lib/print";

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
    let unProg: (() => void) | undefined;
    let unUpd: (() => void) | undefined;
    api.onDataProgress((p) => useStore.getState().setProgress(p)).then((u) => (unProg = u)).catch(() => {});
    let unAppProg: (() => void) | undefined;
    api
      .onAppUpdateProgress((p) => useStore.setState({ appUpdateProgress: { done: p.done, total: p.total, message: p.message } }))
      .then((u) => (unAppProg = u))
      .catch(() => {});
    // program update: on every start, a few seconds in (the window is drawn first), when the setting allows it
    const timer = window.setTimeout(() => {
      const s = useStore.getState();
      if (!s.settings.autoCheckAppUpdates || !navigator.onLine) return;
      void s.checkAppUpdate(true);
    }, 5_000);
    api
      .onDataUpdated(() => {
        const st = useStore.getState();
        void st.refreshDataStatus();
        void st.refreshTiles();
      })
      .then((u) => (unUpd = u))
      .catch(() => {});
    return () => {
      unProg?.();
      unUpd?.();
      unAppProg?.();
      window.clearTimeout(timer);
    };
  }, []);

  // connectivity (gates online-only actions such as "open in Google Maps")
  useEffect(() => {
    const sync = () => {
      useStore.setState({ online: navigator.onLine });
      // back online without a local map: fetch the online map now
      const s = useStore.getState();
      if (navigator.onLine && s.tilesChecked && !s.tilesUrl && !s.onlineTilesUrl) void s.refreshTiles();
    };
    window.addEventListener("online", sync);
    window.addEventListener("offline", sync);
    sync();
    return () => {
      window.removeEventListener("online", sync);
      window.removeEventListener("offline", sync);
    };
  }, []);

  // theme
  useEffect(() => {
    const apply = () => document.documentElement.setAttribute("data-theme", resolveTheme(theme));
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

  // accent colour (light theme only; the CSS ignores it in dark mode)
  const accent = useStore((s) => s.settings.accent);
  useEffect(() => {
    // `?accent=pastel` previews a scheme without touching the settings (headless screenshots)
    const preview = typeof location !== "undefined" ? new URLSearchParams(location.search).get("accent") : null;
    document.documentElement.setAttribute("data-accent", preview || accent || "blue");
  }, [accent]);

  // direction / lang
  useEffect(() => {
    document.documentElement.setAttribute("lang", lang);
    document.documentElement.setAttribute("dir", isRtl(lang) ? "rtl" : "ltr");
  }, [lang]);

  // global shortcuts – by physical key (e.code), so they work in the Hebrew layout too
  // (D = ג, L = ך, N = מ, T = א, / = .). Single keys only when no field has the focus.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useStore.getState();
      const el = e.target as HTMLElement | null;
      const typing = !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
      if (e.ctrlKey && !e.shiftKey && !e.altKey) {
        if (e.code === "KeyF" || e.code === "KeyK") {
          e.preventDefault();
          s.focusSearch();
        } else if (e.code === "KeyR") {
          e.preventDefault();
          s.clearRoute();
          s.setPanel("route");
        } else if (e.code === "KeyH") {
          e.preventDefault();
          s.setPanel("history");
        } else if (e.code === "KeyP") {
          // print through printPage (light theme, map overview); without a route there is
          // nothing to print, and the browser would print an empty page (5.62)
          e.preventDefault();
          if (s.routes.length) void printPage();
        }
        return;
      }
      if (e.key === "Escape") {
        s.setContextMenu(null);
        if (s.shortcutsOpen) useStore.setState({ shortcutsOpen: false });
        return;
      }
      if (typing || e.ctrlKey || e.altKey || e.metaKey) return;
      if (e.code === "Slash" && e.shiftKey) {
        e.preventDefault();
        useStore.setState({ shortcutsOpen: !s.shortcutsOpen });
      } else if (e.code === "Slash") {
        e.preventDefault();
        s.focusSearch();
      } else if (e.code === "KeyD") {
        s.setPanel("route");
      } else if (e.code === "KeyL") {
        void s.locate(true);
      } else if (e.code === "KeyN") {
        useStore.setState({ northUpTick: s.northUpTick + 1 });
      } else if (e.code === "KeyT") {
        s.setLayers({ buildings3d: !s.layers.buildings3d });
      }
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
        <Toasts />
      </div>
      <PrintSheet />
    </>
  );
}
