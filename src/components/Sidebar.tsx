import { Database, History, Layers, PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen, Route, Search, Settings, Star } from "lucide-react";
import clsx from "clsx";
import { useStore, type Panel } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { RoutePanel } from "./RoutePanel";
import { SearchPanel } from "./SearchPanel";
import { HistoryPanel } from "./HistoryPanel";
import { FavoritesPanel } from "./FavoritesPanel";
import { LayersPanel } from "./LayersPanel";
import { DataPanel } from "./DataPanel";
import { SettingsPanel } from "./SettingsPanel";

const ITEMS: { id: Panel; icon: React.ReactNode; label: I18nKey }[] = [
  { id: "route", icon: <Route size={20} strokeWidth={1.75} />, label: "panelRoute" },
  { id: "search", icon: <Search size={20} strokeWidth={1.75} />, label: "panelSearch" },
  { id: "favorites", icon: <Star size={20} strokeWidth={1.75} />, label: "panelFavorites" },
  { id: "history", icon: <History size={20} strokeWidth={1.75} />, label: "panelHistory" },
  { id: "layers", icon: <Layers size={20} strokeWidth={1.75} />, label: "panelLayers" },
  { id: "data", icon: <Database size={20} strokeWidth={1.75} />, label: "panelData" },
  { id: "settings", icon: <Settings size={20} strokeWidth={1.75} />, label: "panelSettings" },
];

export function Sidebar() {
  const t = useT();
  const panel = useStore((s) => s.panel);
  const collapsed = useStore((s) => s.sidebarCollapsed);
  const setPanel = useStore((s) => s.setPanel);
  const toggle = useStore((s) => s.toggleSidebar);
  const lang = useStore((s) => s.settings.language);
  const rtl = lang === "he";
  const updating = useStore((s) => s.dataStatus?.updating);

  // panel sits on the inline-end side of the rail: in RTL it opens to the left
  const CloseIcon = rtl ? PanelLeftClose : PanelRightClose;
  const OpenIcon = rtl ? PanelLeftOpen : PanelRightOpen;

  return (
    <div className="absolute top-3 bottom-3 start-3 z-20 flex items-stretch no-print pointer-events-none">
      <div className="panel-surface rounded-[var(--radius)] flex items-stretch overflow-hidden pointer-events-auto">
        {/* rail */}
        <nav className="w-[56px] shrink-0 flex flex-col items-center py-2 gap-1" aria-label={t("appName")}>
          {ITEMS.map((it) => {
            const active = panel === it.id && !collapsed;
            return (
              <button
                key={it.id}
                title={t(it.label)}
                aria-label={t(it.label)}
                aria-pressed={active}
                onClick={() => (panel === it.id ? toggle() : setPanel(it.id))}
                className={clsx(
                  "relative w-10 h-10 rounded-[10px] flex items-center justify-center transition-colors duration-150",
                  active ? "bg-accent-soft text-accent" : "text-muted hover:text-fg hover:bg-hover",
                )}
              >
                {active && <span className="absolute -start-2 top-2.5 bottom-2.5 w-[3px] rounded-e-full bg-accent" />}
                {it.icon}
                {it.id === "data" && updating && (
                  <span className="absolute top-2 end-2 w-2 h-2 rounded-full bg-accent ring-2 ring-[var(--surface-solid)] animate-pulse" />
                )}
              </button>
            );
          })}
          <div className="flex-1" />
          <button
            className="w-10 h-10 rounded-[10px] flex items-center justify-center text-muted hover:text-fg hover:bg-hover transition-colors duration-150"
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
          <div className="flex items-center px-4 h-14 shrink-0">
            <h1 className="text-lg font-semibold m-0">{t(ITEMS.find((i) => i.id === panel)!.label)}</h1>
          </div>
          <div key={panel} className="flex-1 min-h-0 scroll-y fade-in">
            {panel === "route" && <RoutePanel />}
            {panel === "search" && <SearchPanel />}
            {panel === "favorites" && <FavoritesPanel />}
            {panel === "history" && <HistoryPanel />}
            {panel === "layers" && <LayersPanel />}
            {panel === "data" && <DataPanel />}
            {panel === "settings" && <SettingsPanel />}
          </div>
        </div>
      </div>
    </div>
  );
}
