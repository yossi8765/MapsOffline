// One table drives both the keyboard handler (App.tsx) and the shortcuts dialog, so the two can
// never drift apart again. Keys are physical (KeyboardEvent.code) so they work in the Hebrew
// layout too (D = ג, L = ך, N = מ, T = א, / = .).
import type { I18nKey } from "./i18n";
import type { Store } from "./store";
import { toggleFullscreen } from "./fullscreen";

export interface Shortcut {
  id: string;
  /** KeyboardEvent.code values that trigger it (any of them). */
  codes: string[];
  ctrl?: boolean;
  shift?: boolean;
  /** Also while a text field has the focus (Ctrl shortcuts). Plain keys never fire while typing. */
  whileTyping?: boolean;
  /** What the dialog shows, e.g. "Ctrl+F", "/". */
  keys: string[];
  label: I18nKey;
  /** Shown in the dialog only (MapLibre or the browser handles it). */
  displayOnly?: boolean;
  run?: (s: Store) => void;
}

export const SHORTCUTS: Shortcut[] = [
  { id: "search", codes: ["KeyF"], ctrl: true, whileTyping: true, keys: ["Ctrl+F", "/"], label: "shortcutSearch", run: (s) => s.focusSearch() },
  { id: "searchSlash", codes: ["Slash"], keys: [], label: "shortcutSearch", run: (s) => s.focusSearch() },
  { id: "palette", codes: ["KeyK"], ctrl: true, whileTyping: true, keys: ["Ctrl+K"], label: "shortcutPalette", run: (s) => s.setPaletteOpen(!s.paletteOpen) },
  {
    id: "newRoute",
    codes: ["KeyR"],
    ctrl: true,
    whileTyping: true,
    keys: ["Ctrl+R"],
    label: "shortcutRoute",
    run: (s) => {
      s.clearRoute();
      s.setPanel("route");
    },
  },
  { id: "routePanel", codes: ["KeyD"], keys: ["D"], label: "shortcutRoutePanel", run: (s) => s.setPanel("route") },
  { id: "history", codes: ["KeyH"], ctrl: true, whileTyping: true, keys: ["Ctrl+H"], label: "shortcutHistory", run: (s) => s.setPanel("history") },
  { id: "favorites", codes: ["KeyB"], ctrl: true, whileTyping: true, keys: ["Ctrl+B"], label: "shortcutFavorites", run: (s) => s.setPanel("favorites") },
  { id: "settings", codes: ["Comma"], ctrl: true, whileTyping: true, keys: ["Ctrl+,"], label: "shortcutSettings", run: (s) => s.setPanel("settings") },
  { id: "data", codes: ["KeyD"], ctrl: true, shift: true, whileTyping: true, keys: ["Ctrl+Shift+D"], label: "shortcutData", run: (s) => s.setPanel("data") },
  { id: "print", codes: ["KeyP"], ctrl: true, whileTyping: true, keys: ["Ctrl+P"], label: "shortcutPrint", run: (s) => s.requestPrint() },
  { id: "locate", codes: ["KeyL"], keys: ["L"], label: "shortcutLocate", run: (s) => void s.locate(true) },
  { id: "layers", codes: ["KeyM"], keys: ["M"], label: "shortcutLayers", run: (s) => s.toggleLayers() },
  { id: "categories", codes: ["KeyC"], keys: ["C"], label: "shortcutCategories", run: (s) => s.setMapPopover("categories") },
  { id: "nearby", codes: ["KeyB"], keys: ["B"], label: "shortcutNearby", run: (s) => void s.showNearby(s.mapCenter[0], s.mapCenter[1]) },
  { id: "measure", codes: ["KeyR"], keys: ["R"], label: "shortcutMeasure", run: (s) => s.startMeasure() },
  { id: "north", codes: ["KeyN"], keys: ["N"], label: "shortcutNorth", run: (s) => s.bumpNorthUp() },
  { id: "3d", codes: ["KeyT"], keys: ["T"], label: "shortcut3d", run: (s) => s.setLayers({ buildings3d: !s.layers.buildings3d }) },
  { id: "fullscreen", codes: ["F11"], keys: ["F11"], label: "shortcutFullscreen", run: () => void toggleFullscreen() },
  { id: "help", codes: ["Slash"], shift: true, keys: ["?"], label: "shortcutHelp", run: (s) => s.setShortcutsOpen(!s.shortcutsOpen) },
  { id: "zoom", codes: [], keys: ["+", "−"], label: "shortcutZoom", displayOnly: true },
  { id: "esc", codes: [], keys: ["Esc"], label: "shortcutEsc", displayOnly: true },
];

/** Keys the WebView would use for itself (reload, history navigation): always swallowed. */
const BROWSER_KEYS = new Set(["F5", "BrowserBack", "BrowserForward", "BrowserRefresh"]);

export function isBrowserKey(e: Pick<KeyboardEvent, "code" | "key" | "ctrlKey" | "shiftKey" | "altKey">): boolean {
  if (BROWSER_KEYS.has(e.code) || BROWSER_KEYS.has(e.key)) return true;
  if (e.ctrlKey && e.shiftKey && e.code === "KeyR") return true; // hard reload
  if (e.altKey && (e.code === "ArrowLeft" || e.code === "ArrowRight")) return true; // back / forward
  return false;
}

/** The shortcut this key press means, if any. `typing`: a text field has the focus. */
export function matchShortcut(e: Pick<KeyboardEvent, "code" | "ctrlKey" | "shiftKey" | "altKey" | "metaKey">, typing: boolean): Shortcut | undefined {
  if (e.altKey || e.metaKey) return undefined;
  return SHORTCUTS.find(
    (s) =>
      !s.displayOnly &&
      s.codes.includes(e.code) &&
      !!s.ctrl === e.ctrlKey &&
      !!s.shift === e.shiftKey &&
      (!typing || (s.whileTyping && s.ctrl)),
  );
}

/** Rows for the dialog: one per label, keys merged (Ctrl+F and / both search). */
export function shortcutRows(): { label: I18nKey; keys: string[] }[] {
  const rows: { label: I18nKey; keys: string[] }[] = [];
  for (const s of SHORTCUTS) {
    if (!s.keys.length) continue;
    const row = rows.find((r) => r.label === s.label);
    if (row) row.keys.push(...s.keys.filter((k) => !row.keys.includes(k)));
    else rows.push({ label: s.label, keys: [...s.keys] });
  }
  return rows;
}

/** "Ctrl+K" style hint for a shortcut id (tooltips). */
export function shortcutHint(id: string): string | undefined {
  return SHORTCUTS.find((s) => s.id === id)?.keys[0];
}
