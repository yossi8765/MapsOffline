/**
 * The map alone, for a second window (report ג-29: a map on another screen). Opened by
 * `store.openMapWindow()` as `index.html?window=map`; main.tsx renders this instead of <App/>
 * for that URL. It loads the settings and the map files like App, then shows only the map with
 * its own layers and tools (no sidebar).
 */
import { useEffect } from "react";
import { useStore, resolveTheme } from "../lib/store";
import { MapView } from "./MapView";
import { LayersPopover } from "./LayersPopover";
import { Toasts } from "./Toast";

/** `?window=map` → the map-only window. */
export function isMapWindow(): boolean {
  return typeof location !== "undefined" && new URLSearchParams(location.search).get("window") === "map";
}

export function MapOnlyApp() {
  const settingsLoaded = useStore((s) => s.settingsLoaded);
  const theme = useStore((s) => s.settings.theme);
  const lang = useStore((s) => s.settings.language);
  useEffect(() => {
    const s = useStore.getState();
    void (async () => {
      await s.loadSettings();
      await Promise.all([s.refreshDataStatus(), s.refreshTiles(), s.loadFavorites()]);
    })();
  }, []);
  useEffect(() => {
    const root = document.documentElement;
    root.setAttribute("data-theme", resolveTheme(theme));
    root.setAttribute("dir", lang === "he" ? "rtl" : "ltr");
    root.setAttribute("lang", lang);
  }, [theme, lang]);
  return (
    <div className="fixed inset-0 overflow-hidden bg-bg">
      {settingsLoaded && <MapView />}
      <LayersPopover />
      <Toasts />
    </div>
  );
}
