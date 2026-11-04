import { useEffect, useState } from "react";
import { Accessibility, Bike, Bus, Car, Clock, Footprints, Search, Trash2, Truck } from "lucide-react";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { api, type GeoResult, type HistoryItem, type RouteRequest } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { askConfirm, fmtAgo, fmtDistance, fmtDuration } from "../lib/format";
import { matchesAll } from "../lib/text";
import { kindIcon, resultLabel } from "./SearchBox";

type Tab = "search" | "route";

interface RoutePayload {
  request: RouteRequest;
  labels?: string[];
  summary?: { durationSec: number; distanceM: number; mode: string; transfers: number };
}

/** History rows matching the filter: every typed word, case and final-letter insensitive. */
export function filterHistory(items: HistoryItem[], tab: Tab, filter: string): HistoryItem[] {
  return items.filter((h) => h.kind === tab).filter((h) => matchesAll(h.label, filter));
}

export function HistoryPanel() {
  const t = useT();
  const [tab, setTab] = useState<Tab>("route");
  const [filter, setFilter] = useState("");
  const history = useStore((s) => s.history);
  const lang = useStore((s) => s.settings.language);
  const units = useStore((s) => s.settings.units);
  const { loadHistory, toastError } = useStore.getState();

  useEffect(() => {
    void loadHistory();
  }, [loadHistory]);

  const items = filterHistory(history, tab, filter);

  const restoreSearch = (h: HistoryItem) => {
    const r = h.payload as GeoResult | null;
    if (!r || typeof r !== "object" || !("lon" in r)) return;
    // the place card, as if picked again in the search box
    useStore.getState().pickPlace({ lon: r.lon, lat: r.lat, label: resultLabel(r), result: r }, r.kind === "place" ? 13 : 16);
  };
  const restoreRoute = (h: HistoryItem) => {
    const p = h.payload as RoutePayload | null;
    if (!p?.request) return;
    useStore.getState().loadRequest(p.request, p.labels);
  };
  const del = async (id: number) => {
    try {
      await api.historyDelete(id);
      useStore.setState((s) => ({ history: s.history.filter((h) => h.id !== id) }));
    } catch (e) {
      toastError(e);
    }
  };
  const clear = async () => {
    if (!(await askConfirm(t("clearConfirm"), t("clearAll"), t("cancel"), t("appName")))) return;
    try {
      await api.historyClear(tab === "search" ? "search" : "route");
      await loadHistory();
    } catch (e) {
      toastError(e);
    }
  };

  const modeIcon = (m?: string) => (
    <Icon as={m === "foot" ? Footprints : m === "bike" ? Bike : m === "transit" ? Bus : m === "truck" ? Truck : m === "wheelchair" ? Accessibility : Car} />
  );

  return (
    <div className="px-4 pb-5 flex flex-col gap-3">
      <div className="seg" role="group" aria-label={t("panelHistory")}>
        <button aria-pressed={tab === "route"} onClick={() => setTab("route")}>
          <span>{t("tabRoutes")}</span>
        </button>
        <button aria-pressed={tab === "search"} onClick={() => setTab("search")}>
          <span>{t("tabSearches")}</span>
        </button>
      </div>

      <div className="relative">
        <Icon as={Search} className="absolute start-3 top-1/2 -translate-y-1/2 text-muted pointer-events-none" />
        <input className="input h-9 ps-9" placeholder={t("filterPlaceholder")} aria-label={t("filterPlaceholder")} value={filter} onChange={(e) => setFilter(e.target.value)} />
      </div>
      {items.length === 0 ? (
        <p className="m-0 px-1 py-2 text-muted">{filter ? t("searchNoResults") : t("historyEmpty")}</p>
      ) : (
        <ul className="flex flex-col -mx-2 m-0 p-0 list-none">
          {items.map((h) => {
            const p = tab === "route" ? (h.payload as RoutePayload | null) : null;
            const r = tab === "search" ? (h.payload as GeoResult | null) : null;
            return (
              <li key={h.id} className="group flex items-center gap-1 rounded hover:bg-hover focus-within:bg-hover px-2 transition-colors duration-150">
                <button className="flex-1 min-w-0 flex items-center gap-3 py-2 text-start rounded" onClick={() => (tab === "search" ? restoreSearch(h) : restoreRoute(h))}>
                  <span className="tile">{tab === "search" ? r && typeof r === "object" && "kind" in r ? kindIcon(r.kind) : <Icon as={Clock} /> : modeIcon(p?.request?.mode)}</span>
                  <span className="flex-1 min-w-0">
                    <span className="block truncate-dir font-medium">{h.label}</span>
                    {p?.summary && (
                      <span className="block truncate text-xs text-muted num">
                        {fmtDuration(p.summary.durationSec, t)} · {fmtDistance(p.summary.distanceM, t, units)}
                      </span>
                    )}
                  </span>
                  <span className="shrink-0 text-xs text-subtle group-hover:hidden group-focus-within:hidden">{fmtAgo(h.createdIso, lang)}</span>
                </button>
                <Tip text={t("delete")}>
                  <button
                    className="btn btn-ghost btn-icon btn-sm text-muted hidden group-hover:inline-flex group-focus-within:inline-flex"
                    onClick={() => void del(h.id)}
                    aria-label={`${t("delete")}: ${h.label}`}
                  >
                    <Icon as={Trash2} size={14} />
                  </button>
                </Tip>
              </li>
            );
          })}
          <li className="list-none">
            <button className="btn btn-ghost btn-sm text-muted hover:!text-danger self-start mt-2 mx-1" onClick={() => void clear()}>
              <Icon as={Trash2} size={14} /> {t("clearAll")}
            </button>
          </li>
        </ul>
      )}
    </div>
  );
}
