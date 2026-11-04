import { Database, PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen, Search, Settings, Star } from "lucide-react";
import { useEffect, useRef } from "react";
import clsx from "clsx";
import { useStore, type Panel } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { RoutePanel } from "./RoutePanel";
import { SearchPanel } from "./SearchPanel";
import { HistoryPanel } from "./HistoryPanel";
import { FavoritesPanel } from "./FavoritesPanel";
import { DataPanel } from "./DataPanel";
import { SettingsPanel } from "./SettingsPanel";

/** Four rail items. "Search" holds the search card and the route form (two tabs), "Saved" holds
 *  favorites and history; map layers live in a popover on the map (LayersPopover). */
type Group = "find" | "saved" | "data" | "settings";

const GROUPS: { id: Group; icon: React.ReactNode; label: I18nKey; panels: Panel[] }[] = [
  { id: "find", icon: <Search size={20} strokeWidth={1.75} />, label: "railSearch", panels: ["search", "route"] },
  { id: "saved", icon: <Star size={20} strokeWidth={1.75} />, label: "railSaved", panels: ["favorites", "history"] },
  { id: "data", icon: <Database size={20} strokeWidth={1.75} />, label: "railData", panels: ["data"] },
  { id: "settings", icon: <Settings size={20} strokeWidth={1.75} />, label: "railSettings", panels: ["settings"] },
];

const TAB_LABEL: Partial<Record<Panel, I18nKey>> = {
  search: "panelSearch",
  route: "panelRoute",
  favorites: "panelFavorites",
  history: "panelHistory",
};

export function groupOf(panel: Panel): Group {
  return GROUPS.find((g) => g.panels.includes(panel))?.id ?? "find";
}

export function Sidebar() {
  const t = useT();
  const panel = useStore((s) => s.panel);
  const collapsed = useStore((s) => s.sidebarCollapsed);
  const setPanel = useStore((s) => s.setPanel);
  const toggle = useStore((s) => s.toggleSidebar);
  const lang = useStore((s) => s.settings.language);
  const rtl = lang === "he";
  const updating = useStore((s) => s.dataStatus?.updating);

  // the old "layers" panel moved to the map popover
  const shown: Panel = panel === "layers" ? "search" : panel;
  const group = GROUPS.find((g) => g.panels.includes(shown))!;
  // a group reopens on the tab used last in it
  const lastInGroup = useRef<Partial<Record<Group, Panel>>>({});
  useEffect(() => {
    lastInGroup.current[group.id] = shown;
  }, [group.id, shown]);

  // panel sits on the inline-end side of the rail: in RTL it opens to the left
  const CloseIcon = rtl ? PanelLeftClose : PanelRightClose;
  const OpenIcon = rtl ? PanelLeftOpen : PanelRightOpen;

  return (
    <div className="absolute top-3 bottom-3 start-3 z-20 flex items-stretch no-print pointer-events-none">
      <div className="panel-surface rounded-[var(--radius)] flex items-stretch overflow-hidden pointer-events-auto">
        {/* rail: icon + short label */}
        <nav className="w-16 shrink-0 flex flex-col items-center py-2 gap-1" aria-label={t("appName")}>
          {GROUPS.map((g) => {
            const active = g.id === group.id && !collapsed;
            return (
              <button
                key={g.id}
                aria-label={t(g.label)}
                aria-pressed={active}
                onClick={() => (g.id === group.id ? toggle() : setPanel(lastInGroup.current[g.id] ?? g.panels[0]))}
                className={clsx(
                  "relative w-14 h-14 rounded flex flex-col items-center justify-center gap-1 transition-colors duration-150",
                  active ? "bg-accent-soft text-accent" : "text-muted hover:text-fg hover:bg-hover",
                )}
              >
                {active && <span className="absolute -start-1 top-3 bottom-3 w-[3px] rounded-e-full bg-accent" />}
                {g.icon}
                <span className="text-[11px] leading-none font-medium">{t(g.label)}</span>
                {g.id === "data" && updating && (
                  <span className="absolute top-2 end-2.5 w-2 h-2 rounded-full bg-accent ring-2 ring-[var(--surface-solid)] animate-pulse" />
                )}
              </button>
            );
          })}
          <div className="flex-1" />
          <button
            className="w-10 h-10 rounded flex items-center justify-center text-muted hover:text-fg hover:bg-hover transition-colors duration-150"
            onClick={toggle}
            title={collapsed ? t("expand") : t("collapse")}
            aria-label={collapsed ? t("expand") : t("collapse")}
          >
            {collapsed ? <OpenIcon size={18} strokeWidth={1.75} /> : <CloseIcon size={18} strokeWidth={1.75} />}
          </button>
        </nav>

        {/* panel */}
        <div
          className={clsx(
            "flex flex-col min-w-0 border-s border-line overflow-hidden transition-[width,opacity] duration-150 ease-out print-area",
            collapsed ? "w-0 opacity-0 border-s-0 pointer-events-none" : "w-[380px] opacity-100",
          )}
        >
          <div className="flex items-center gap-3 px-4 h-14 shrink-0">
            {group.panels.length > 1 ? (
              <div className="seg seg-sm flex-1" role="tablist">
                {group.panels.map((p) => (
                  <button key={p} role="tab" aria-selected={shown === p} aria-pressed={shown === p} onClick={() => setPanel(p)}>
                    <span>{t(TAB_LABEL[p]!)}</span>
                  </button>
                ))}
              </div>
            ) : (
              <h1 className="text-lg font-semibold m-0">{t(group.label)}</h1>
            )}
          </div>
          <div key={shown} className="flex-1 min-h-0 scroll-y fade-in">
            {shown === "route" && <RoutePanel />}
            {shown === "search" && <SearchPanel />}
            {shown === "favorites" && <FavoritesPanel />}
            {shown === "history" && <HistoryPanel />}
            {shown === "data" && <DataPanel />}
            {shown === "settings" && <SettingsPanel />}
          </div>
        </div>
      </div>
    </div>
  );
}
