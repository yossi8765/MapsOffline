import { Database, PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen, Search, Settings, Star } from "lucide-react";
import { useEffect, useRef } from "react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { useStore, type Panel } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { shortcutHint } from "../lib/shortcuts";
import { RoutePanel } from "./RoutePanel";
import { SearchPanel } from "./SearchPanel";
import { HistoryPanel } from "./HistoryPanel";
import { FavoritesPanel } from "./FavoritesPanel";
import { DataPanel } from "./DataPanel";
import { SettingsPanel } from "./SettingsPanel";

/** Four rail items. "Search" holds the search card and the route form (two tabs), "Saved" holds
 *  favorites and history; map layers live in a popover on the map (LayersPopover). */
type Group = "find" | "saved" | "data" | "settings";

const GROUPS: { id: Group; icon: typeof Search; label: I18nKey; panels: Panel[]; shortcut: string }[] = [
  { id: "find", icon: Search, label: "railSearch", panels: ["search", "route"], shortcut: "search" },
  { id: "saved", icon: Star, label: "railSaved", panels: ["favorites", "history"], shortcut: "favorites" },
  { id: "data", icon: Database, label: "railData", panels: ["data"], shortcut: "data" },
  { id: "settings", icon: Settings, label: "railSettings", panels: ["settings"], shortcut: "settings" },
];

const TAB_LABEL: Partial<Record<Panel, I18nKey>> = {
  search: "panelSearch",
  route: "panelRoute",
  favorites: "panelFavorites",
  history: "panelHistory",
};

/** Rail geometry (px at text scale 1): items are 56 high with a 4 gap after 8 of padding. */
const RAIL_PAD = 8;
const RAIL_STEP = 60;

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
  const groupIndex = GROUPS.indexOf(group);
  // a group reopens on the tab used last in it
  const lastInGroup = useRef<Partial<Record<Group, Panel>>>({});
  useEffect(() => {
    lastInGroup.current[group.id] = shown;
  }, [group.id, shown]);

  // panel sits on the inline-end side of the rail: in RTL it opens to the left
  const CloseIcon = rtl ? PanelLeftClose : PanelRightClose;
  const OpenIcon = rtl ? PanelLeftOpen : PanelRightOpen;

  // arrow keys move between the tabs of a tab list (WAI-ARIA tabs pattern)
  const onTabKeys = (e: React.KeyboardEvent) => {
    const panels = group.panels;
    const i = panels.indexOf(shown);
    const forward = rtl ? "ArrowLeft" : "ArrowRight";
    const back = rtl ? "ArrowRight" : "ArrowLeft";
    let next = -1;
    if (e.key === forward) next = (i + 1) % panels.length;
    else if (e.key === back) next = (i - 1 + panels.length) % panels.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = panels.length - 1;
    if (next < 0) return;
    e.preventDefault();
    setPanel(panels[next]);
    requestAnimationFrame(() => (e.currentTarget.querySelector(`[data-tab="${panels[next]}"]`) as HTMLElement | null)?.focus());
  };

  return (
    // docked to the window edge (Windows NavigationView): no strip of map behind the rail
    <div data-sidebar className="absolute top-0 bottom-0 start-0 z-20 flex items-stretch no-print pointer-events-none">
      <div className="panel-surface border-y-0 border-s-0 flex items-stretch overflow-hidden pointer-events-auto">
        {/* rail: icon + short label */}
        <nav className="relative w-16 shrink-0 flex flex-col items-center py-2 gap-1" aria-label={t("appName")}>
          {/* selection indicator: 3 × 16, slides between the items */}
          {!collapsed && (
            <span
              aria-hidden
              className="absolute start-0 w-[3px] h-4 rounded-e-full bg-accent transition-transform duration-150 ease-out"
              style={{ top: 0, transform: `translateY(${RAIL_PAD + groupIndex * RAIL_STEP + 20}px)` }}
            />
          )}
          {GROUPS.map((g) => {
            const active = g.id === group.id && !collapsed;
            return (
              <Tip key={g.id} text={t(g.label)} shortcut={shortcutHint(g.shortcut)} side="end">
                <button
                  aria-label={t(g.label)}
                  aria-current={active ? "page" : undefined}
                  data-tt={g.id === "find" ? "search" : undefined}
                  // a click on the open group folds the panel (the user's choice); a folded panel opens
                  // where it was; another group switches to its last tab
                  onClick={() => (g.id === group.id ? (collapsed ? setPanel(shown) : toggle()) : setPanel(lastInGroup.current[g.id] ?? g.panels[0]))}
                  className={clsx(
                    "relative w-14 h-14 rounded flex flex-col items-center justify-center gap-1 transition-colors duration-150",
                    active ? "bg-accent-soft text-accent" : "text-muted hover:text-fg hover:bg-hover",
                  )}
                >
                  <Icon as={g.icon} size={20} />
                  <span className="text-xs leading-none font-medium">{t(g.label)}</span>
                  {g.id === "data" && updating && <span className="absolute top-2 end-2.5 w-2 h-2 rounded-full bg-accent ring-2 ring-[var(--surface-solid)] animate-pulse" />}
                </button>
              </Tip>
            );
          })}
          <div className="flex-1" />
          <Tip text={collapsed ? t("expand") : t("collapse")} side="end">
            <button
              className="w-10 h-10 rounded flex items-center justify-center text-muted hover:text-fg hover:bg-hover transition-colors duration-150"
              onClick={toggle}
              aria-label={collapsed ? t("expand") : t("collapse")}
              aria-expanded={!collapsed}
            >
              <Icon as={collapsed ? OpenIcon : CloseIcon} size={18} />
            </button>
          </Tip>
        </nav>

        {/* panel */}
        <div
          className={clsx(
            "flex flex-col min-w-0 border-s border-line overflow-hidden transition-[width,opacity] duration-150 ease-out print-area",
            collapsed ? "w-0 opacity-0 border-s-0 pointer-events-none" : "w-[var(--sidebar-w)] opacity-100",
          )}
          aria-hidden={collapsed || undefined}
        >
          <div className="flex items-center gap-3 px-4 h-14 shrink-0 overflow-hidden [scrollbar-gutter:stable]">
            {group.panels.length > 1 ? (
              <div className="seg seg-sm flex-1" role="tablist" aria-label={t(group.label)} onKeyDown={onTabKeys}>
                {group.panels.map((p) => (
                  <button
                    key={p}
                    role="tab"
                    data-tab={p}
                    id={`tab-${p}`}
                    aria-selected={shown === p}
                    aria-controls="panel-body"
                    tabIndex={shown === p ? 0 : -1}
                    onClick={() => setPanel(p)}
                  >
                    <span>{t(TAB_LABEL[p]!)}</span>
                  </button>
                ))}
              </div>
            ) : (
              <h1 className="text-xl font-semibold m-0">{t(group.label)}</h1>
            )}
          </div>
          <div
            key={shown}
            id="panel-body"
            role={group.panels.length > 1 ? "tabpanel" : undefined}
            aria-labelledby={group.panels.length > 1 ? `tab-${shown}` : undefined}
            className="flex-1 min-h-0 scroll-y fade-in"
          >
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
