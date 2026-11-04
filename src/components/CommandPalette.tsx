import { useEffect, useMemo, useRef, useState } from "react";
import {
  Box,
  Database,
  FileUp,
  History,
  Keyboard,
  Layers,
  LayoutGrid,
  ListPlus,
  LocateFixed,
  type LucideIcon,
  MapPin,
  Moon,
  PenLine,
  Printer,
  Radar,
  RefreshCw,
  Route,
  Ruler,
  Satellite,
  Search,
  Settings,
  Share2,
  SplitSquareHorizontal,
  Star,
  Sun,
  AppWindow,
  CircleDot,
} from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { api, type GeoResult } from "../lib/api";
import { useStore, type Store } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { matchRank, matchesAll } from "../lib/text";
import { shortcutHint } from "../lib/shortcuts";
import { useDialog } from "../lib/useDialog";
import { favIcon, kindIcon, resultLabel } from "./SearchBox";

/** One command of the palette. `when` hides it when it makes no sense now. */
export interface PaletteAction {
  id: string;
  label: I18nKey;
  icon: LucideIcon;
  /** extra words that find it ("dark" finds the theme switch) */
  keywords?: string;
  shortcut?: string;
  when?: (s: Store) => boolean;
  run: (s: Store) => void;
}

export const PALETTE_ACTIONS: PaletteAction[] = [
  { id: "search", label: "panelSearch", icon: Search, shortcut: "search", run: (s) => s.focusSearch() },
  {
    id: "route",
    label: "shortcutRoute",
    icon: Route,
    shortcut: "newRoute",
    run: (s) => {
      s.clearRoute();
      s.setPanel("route");
    },
  },
  { id: "favorites", label: "panelFavorites", icon: Star, shortcut: "favorites", run: (s) => s.setPanel("favorites") },
  { id: "history", label: "panelHistory", icon: History, shortcut: "history", run: (s) => s.setPanel("history") },
  { id: "data", label: "panelData", icon: Database, shortcut: "data", run: (s) => s.setPanel("data") },
  { id: "checkUpdates", label: "checkUpdates", icon: RefreshCw, keywords: "update download", run: (s) => {
    s.setPanel("data");
    void s.checkUpdates(true);
  } },
  { id: "settings", label: "panelSettings", icon: Settings, shortcut: "settings", run: (s) => s.setPanel("settings") },
  { id: "locate", label: "myLocation", icon: LocateFixed, shortcut: "locate", run: (s) => void s.locate(true) },
  { id: "layers", label: "layersButton", icon: Layers, shortcut: "layers", run: (s) => s.toggleLayers() },
  { id: "categories", label: "categoriesTitle", icon: LayoutGrid, shortcut: "categories", run: (s) => s.setMapPopover("categories") },
  { id: "nearby", label: "ctxNearby", icon: Radar, shortcut: "nearby", run: (s) => void s.showNearby(s.mapCenter[0], s.mapCenter[1]) },
  { id: "satellite", label: "layerSatellite", icon: Satellite, keywords: "aerial orthophoto", run: (s) => s.setLayers({ satellite: !s.layers.satellite }) },
  { id: "3d", label: "layer3d", icon: Box, shortcut: "3d", run: (s) => s.setLayers({ buildings3d: !s.layers.buildings3d }) },
  { id: "measure", label: "paletteMeasure", icon: Ruler, shortcut: "measure", keywords: "distance area", run: (s) => s.startMeasure() },
  { id: "draw", label: "paletteDraw", icon: PenLine, keywords: "drawing notes", run: (s) => s.startDrawing() },
  { id: "import", label: "paletteImport", icon: FileUp, keywords: "gpx kml geojson", run: (s) => s.openImport() },
  { id: "track", label: "paletteTrack", icon: CircleDot, keywords: "record gps", run: (s) => s.toggleTrackRecording() },
  { id: "isochrone", label: "paletteIsochrone", icon: Radar, keywords: "reach isochrone", run: (s) => s.showIsochrone(s.mapCenter, "car") },
  { id: "compare", label: "paletteCompare", icon: SplitSquareHorizontal, keywords: "split swipe", run: (s) => s.setCompare("split") },
  { id: "window", label: "paletteMapWindow", icon: AppWindow, keywords: "monitor window", run: (s) => s.openMapWindow() },
  { id: "batch", label: "batchTitle", icon: ListPlus, keywords: "csv excel addresses", run: (s) => s.setBatchOpen(true) },
  { id: "print", label: "print", icon: Printer, shortcut: "print", when: (s) => s.routes.length > 0, run: (s) => s.requestPrint() },
  {
    id: "share",
    label: "share",
    icon: Share2,
    when: (s) => !!s.selectedRouteId || !!s.searchPick,
    run: (s) => {
      if (s.selectedRouteId && s.routes.length) s.setShare({ kind: "route", routeId: s.selectedRouteId });
      else if (s.searchPick) s.setShare({ kind: "place", lon: s.searchPick.lon, lat: s.searchPick.lat, label: s.searchPick.label });
    },
  },
  { id: "dark", label: "paletteDark", icon: Moon, keywords: "theme night", when: (s) => s.settings.theme !== "dark", run: (s) => void s.updateSettings({ theme: "dark" }) },
  { id: "light", label: "paletteLight", icon: Sun, keywords: "theme day", when: (s) => s.settings.theme !== "light", run: (s) => void s.updateSettings({ theme: "light" }) },
  { id: "shortcuts", label: "shortcutsTitle", icon: Keyboard, shortcut: "help", run: (s) => s.setShortcutsOpen(true) },
];

type Item = { kind: "action"; a: PaletteAction } | { kind: "fav"; f: Store["favorites"][number] } | { kind: "place"; r: GeoResult };

/** Actions matching the query (label in the UI language, its English id and keywords). */
export function filterActions(q: string, t: (k: I18nKey) => string, s: Store): PaletteAction[] {
  const list = PALETTE_ACTIONS.filter((a) => !a.when || a.when(s));
  if (!q.trim()) return list.slice(0, 8);
  return list
    .filter((a) => matchesAll(`${t(a.label)} ${a.id} ${a.keywords ?? ""}`, q))
    .sort((a, b) => matchRank(t(a.label), q) - matchRank(t(b.label), q));
}

function Palette() {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const [places, setPlaces] = useState<GeoResult[]>([]);
  const favorites = useStore((s) => s.favorites);
  const close = () => useStore.getState().setPaletteOpen(false);
  useDialog(ref, close, { initialFocus: inputRef });

  // places from the address index (debounced, at least 2 letters)
  useEffect(() => {
    const text = q.trim();
    if (text.length < 2) {
      setPlaces([]);
      return;
    }
    let dead = false;
    const h = window.setTimeout(() => {
      api
        .geocodeSearch(text, useStore.getState().mapCenter, 6)
        .then((r) => !dead && setPlaces(r))
        .catch(() => !dead && setPlaces([]));
    }, 180);
    return () => {
      dead = true;
      window.clearTimeout(h);
    };
  }, [q]);

  const items: Item[] = useMemo(() => {
    const s = useStore.getState();
    const actions = filterActions(q, t, s).map((a): Item => ({ kind: "action", a }));
    const favs = q.trim()
      ? favorites
          .filter((f) => matchesAll(`${f.name} ${f.address ?? ""} ${f.group ?? ""}`, q))
          .slice(0, 5)
          .map((f): Item => ({ kind: "fav", f }))
      : [];
    return [...actions, ...favs, ...places.map((r): Item => ({ kind: "place", r }))];
  }, [q, t, favorites, places]);

  useEffect(() => setActive(0), [q]);

  const run = (it: Item) => {
    const s = useStore.getState();
    close();
    if (it.kind === "action") it.a.run(s);
    else if (it.kind === "fav") {
      const f = it.f;
      if (f.kind === "route" && f.data && "request" in f.data) s.loadRequest(f.data.request as never, (f.data as { labels?: string[] }).labels);
      else s.pickPlace({ lon: f.lon, lat: f.lat, label: f.name });
    } else s.pickPlace({ lon: it.r.lon, lat: it.r.lat, label: resultLabel(it.r), result: it.r });
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, items.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter" && items[active]) {
      e.preventDefault();
      run(items[active]);
    }
  };

  const header = (i: number) => {
    const k = items[i].kind;
    if (i > 0 && items[i - 1].kind === k) return null;
    return (
      <div className="px-3 pt-2 pb-1 section-label" role="presentation">
        {t(k === "action" ? "paletteActions" : k === "fav" ? "favorites" : "palettePlaces")}
      </div>
    );
  };

  return (
    <div className="absolute inset-0 z-50 flex items-start justify-center pt-[12vh] dialog-backdrop no-print" onClick={close}>
      <div ref={ref} role="dialog" aria-modal="true" aria-label={t("paletteTitle")} className="dialog w-[560px] max-w-[calc(100%-32px)] overflow-hidden fade-in" onClick={(e) => e.stopPropagation()}>
        <div className="relative border-b border-line">
          <Icon as={Search} className="absolute start-4 top-1/2 -translate-y-1/2 text-muted pointer-events-none" />
          <input
            ref={inputRef}
            className="w-full h-12 ps-11 pe-4 bg-transparent outline-none text-base"
            placeholder={t("palettePlaceholder")}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={onKey}
            role="combobox"
            aria-expanded
            aria-controls="palette-list"
            aria-activedescendant={items[active] ? `pal-${active}` : undefined}
            aria-autocomplete="list"
          />
        </div>
        <div id="palette-list" role="listbox" className="max-h-[50vh] scroll-y py-1">
          {items.length === 0 && <div className="px-4 py-3 text-muted">{t("searchNoResults")}</div>}
          {items.map((it, i) => {
            const label = it.kind === "action" ? t(it.a.label) : it.kind === "fav" ? it.f.name : resultLabel(it.r);
            const sub = it.kind === "fav" ? it.f.address : it.kind === "place" ? (it.r.categoryLabel ?? it.r.city) : undefined;
            const hint = it.kind === "action" && it.a.shortcut ? shortcutHint(it.a.shortcut) : undefined;
            return (
              <div key={`${it.kind}-${i}`}>
                {header(i)}
                <div
                  id={`pal-${i}`}
                  role="option"
                  aria-selected={i === active}
                  className={clsx("flex items-center gap-3 px-3 mx-1 min-h-10 rounded-[var(--radius-sm)] cursor-pointer", i === active && "bg-hover")}
                  onMouseEnter={() => setActive(i)}
                  onClick={() => run(it)}
                >
                  <span className="w-5 flex justify-center text-muted shrink-0">
                    {it.kind === "action" ? <Icon as={it.a.icon} /> : it.kind === "fav" ? favIcon(it.f.icon) : it.r ? kindIcon(it.r.kind) : <Icon as={MapPin} />}
                  </span>
                  <span className="flex-1 min-w-0">
                    <span className="block truncate-dir">{label}</span>
                    {sub && <span className="block truncate-dir text-xs text-muted">{sub}</span>}
                  </span>
                  {hint && (
                    <kbd className="kbd shrink-0" dir="ltr">
                      {hint}
                    </kbd>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

/** Ctrl+K: one box for commands, favorites and places. */
export function CommandPalette() {
  const open = useStore((s) => s.paletteOpen);
  return open ? <Palette /> : null;
}
