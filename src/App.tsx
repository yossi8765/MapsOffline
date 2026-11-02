import { useEffect } from "react";
import { api } from "./lib/api";
import { resolveTheme, useStore } from "./lib/store";
import { isRtl } from "./lib/i18n";
import { MapView } from "./components/MapView";
import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toast";
import { PrintSheet } from "./components/PrintSheet";

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
    // program update: once a day, 15 s after start, only when the setting allows it
    const timer = window.setTimeout(() => {
      const s = useStore.getState();
      if (!s.settings.autoCheckAppUpdates || !navigator.onLine) return;
      let last = 0;
      try {
        last = Number(localStorage.getItem("mapsoffline.lastUpdateCheck") || 0);
      } catch {
        /* ignore */
      }
      if (Date.now() - last < 24 * 3600_000) return;
      try {
        localStorage.setItem("mapsoffline.lastUpdateCheck", String(Date.now()));
      } catch {
        /* ignore */
      }
      void s.checkAppUpdate(true);
    }, 15_000);
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
    const sync = () => useStore.setState({ online: navigator.onLine });
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
    document.documentElement.setAttribute("data-accent", accent || "blue");
  }, [accent]);

  // direction / lang
  useEffect(() => {
    document.documentElement.setAttribute("lang", lang);
    document.documentElement.setAttribute("dir", isRtl(lang) ? "rtl" : "ltr");
  }, [lang]);

  // global shortcuts
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useStore.getState();
      if (e.ctrlKey && !e.shiftKey && !e.altKey) {
        const k = e.key.toLowerCase();
        if (k === "f") {
          e.preventDefault();
          s.focusSearch();
        } else if (k === "r") {
          e.preventDefault();
          s.clearRoute();
          s.setPanel("route");
        } else if (k === "h") {
          e.preventDefault();
          s.setPanel("history");
        }
      } else if (e.key === "Escape") {
        s.setContextMenu(null);
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
        <Toasts />
      </div>
      <PrintSheet />
    </>
  );
}
