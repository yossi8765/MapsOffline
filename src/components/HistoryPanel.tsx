import { useEffect, useState } from "react";
import { Bike, Bus, Car, Clock, Footprints, History, Search, Trash2 } from "lucide-react";
import { api, type GeoResult, type HistoryItem, type RouteRequest } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { askConfirm, fmtAgo, fmtDistance, fmtDuration } from "../lib/format";
import { kindIcon, resultLabel } from "./SearchBox";

type Tab = "search" | "route";

interface RoutePayload {
  request: RouteRequest;
  labels?: string[];
  summary?: { durationSec: number; distanceM: number; mode: string; transfers: number };
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

  const items = history.filter((h) => (tab === "search" ? h.kind === "search" : h.kind === "route")).filter((h) => !filter || h.label.includes(filter));

  const restoreSearch = (h: HistoryItem) => {
    const r = h.payload as GeoResult | null;
    if (!r || typeof r !== "object" || !("lon" in r)) return;
    const s = useStore.getState();
    s.setMarker({ lonlat: [r.lon, r.lat], label: resultLabel(r) });
    s.flyToPoint([r.lon, r.lat], r.kind === "place" ? 13 : 16);
  };
  const restoreRoute = (h: HistoryItem) => {
    const p = h.payload as RoutePayload | null;
    if (!p?.request) return;
    useStore.getState().loadRequest(p.request, p.labels);
  };
  const del = async (id: number) => {
    try {
      await api.historyDelete(id);
      await loadHistory();
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

  const modeIcon = (m?: string) =>
    m === "foot" ? <Footprints size={16} strokeWidth={1.75} /> : m === "bike" ? <Bike size={16} strokeWidth={1.75} /> : m === "transit" ? <Bus size={16} strokeWidth={1.75} /> : <Car size={16} strokeWidth={1.75} />;

  return (
    <div className="px-4 pb-5 flex flex-col gap-3">
      <div className="seg" role="tablist">
        <button role="tab" aria-selected={tab === "route"} aria-pressed={tab === "route"} onClick={() => setTab("route")}>
          <span>{t("tabRoutes")}</span>
        </button>
        <button role="tab" aria-selected={tab === "search"} aria-pressed={tab === "search"} onClick={() => setTab("search")}>
          <span>{t("tabSearches")}</span>
        </button>
      </div>

      {(
        <>
          <div className="relative">
            <Search size={16} strokeWidth={1.75} className="absolute start-3 top-1/2 -translate-y-1/2 text-muted pointer-events-none" />
            <input className="input h-9 ps-9" placeholder={t("filterPlaceholder")} value={filter} onChange={(e) => setFilter(e.target.value)} />
          </div>
          {items.length === 0 ? (
            <EmptyState icon={<History size={18} strokeWidth={1.75} />} text={t("historyEmpty")} />
          ) : (
            <div className="flex flex-col -mx-2">
              {items.map((h) => {
                const p = tab === "route" ? (h.payload as RoutePayload | null) : null;
                const r = tab === "search" ? (h.payload as GeoResult | null) : null;
                return (
                  <div key={h.id} className="group flex items-center gap-1 rounded hover:bg-hover focus-within:bg-hover px-2 transition-colors duration-150">
                    <button className="flex-1 min-w-0 flex items-center gap-3 py-2 text-start rounded" onClick={() => (tab === "search" ? restoreSearch(h) : restoreRoute(h))}>
                      <span className="tile">
                        {tab === "search" ? (r && typeof r === "object" && "kind" in r ? kindIcon(r.kind) : <Clock size={16} strokeWidth={1.75} />) : modeIcon(p?.request?.mode)}
                      </span>
                      <span className="flex-1 min-w-0">
                        <span className="block truncate text-base font-medium">{h.label}</span>
                        {p?.summary && (
                          <span className="block truncate text-xs text-muted num">
                            {fmtDuration(p.summary.durationSec, t)} · {fmtDistance(p.summary.distanceM, t, units)}
                          </span>
                        )}
                      </span>
                      <span className="shrink-0 text-xs text-subtle group-hover:hidden group-focus-within:hidden">{fmtAgo(h.createdIso, lang)}</span>
                    </button>
                    <button
                      className="btn btn-ghost btn-icon btn-sm text-muted hidden group-hover:inline-flex group-focus-within:inline-flex"
                      onClick={() => void del(h.id)}
                      title={t("delete")}
                      aria-label={t("delete")}
                    >
                      <Trash2 size={14} strokeWidth={1.75} />
                    </button>
                  </div>
                );
              })}
              <button className="btn btn-ghost btn-sm text-muted hover:!text-danger self-start mt-2 mx-1" onClick={() => void clear()}>
                <Trash2 size={14} strokeWidth={1.75} /> {t("clearAll")}
              </button>
            </div>
          )}
        </>
      )}

    </div>
  );
}

/** One plain sentence: no icon circle, no centred hero. */
function EmptyState({ text }: { icon?: React.ReactNode; text: string }) {
  return <p className="m-0 px-1 py-2 text-sm text-muted">{text}</p>;
}
