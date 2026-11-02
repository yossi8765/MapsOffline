import { useState } from "react";
import { Flag, Navigation, Search, Star } from "lucide-react";
import clsx from "clsx";
import { findFavorite, useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtCoords } from "../lib/format";
import { SearchBox, kindIcon, stopCode, type Pick } from "./SearchBox";

export function SearchPanel() {
  const t = useT();
  const focusTick = useStore((s) => s.searchFocusTick);
  const [picked, setPicked] = useState<Pick | null>(null);
  const favorites = useStore((s) => s.favorites);
  const saved = picked ? findFavorite(favorites, picked.lon, picked.lat) : undefined;

  const onPick = (p: Pick) => {
    setPicked(p);
    const s = useStore.getState();
    s.setMarker({ lonlat: [p.lon, p.lat], label: p.label });
    s.flyToPoint([p.lon, p.lat], p.result?.kind === "place" ? 13 : p.result?.kind === "street" ? 15 : 17);
  };

  return (
    <div className="px-4 pb-5 flex flex-col gap-4">
      <SearchBox value={picked?.label ?? ""} onPick={onPick} focusTick={focusTick} autoFocus onClear={() => { setPicked(null); useStore.getState().setMarker(null); }} />
      {picked ? (
        <div className="card p-4 fade-in">
          <div className="flex items-start gap-3">
            <span className="tile bg-accent-soft text-accent">{picked.result ? kindIcon(picked.result.kind, 18) : kindIcon("coord", 18)}</span>
            <div className="flex-1 min-w-0">
              <div className="text-lg font-semibold leading-snug">{picked.label}</div>
              {picked.result && (picked.result.kind === "poi" || picked.result.kind === "stop") && (
                <div className="text-sm text-muted">
                  {picked.result.kind === "stop" ? `${t("stopCode")} ${stopCode(picked.result)}` : picked.result.category}
                </div>
              )}
              <div className="text-xs text-subtle num mt-1" dir="ltr" style={{ textAlign: "start" }}>
                {fmtCoords(picked.lon, picked.lat)}
              </div>
            </div>
          </div>
          <div className="flex gap-2 mt-4">
            <button
              className="btn flex-1"
              onClick={() => {
                useStore.getState().setFrom({ lon: picked.lon, lat: picked.lat, label: picked.label, result: picked.result });
                useStore.getState().setPanel("route");
              }}
            >
              <Navigation size={14} /> {t("useAsFrom")}
            </button>
            <button
              className="btn btn-primary !h-9 !text-[13px] flex-1"
              onClick={() => {
                useStore.getState().setTo({ lon: picked.lon, lat: picked.lat, label: picked.label, result: picked.result });
                useStore.getState().setPanel("route");
              }}
            >
              <Flag size={14} /> {t("useAsTo")}
            </button>
            <button
              className={clsx("btn btn-icon shrink-0", saved && "text-warning")}
              title={saved ? t("favoriteRemove") : t("ctxAddFavorite")}
              aria-label={saved ? t("favoriteRemove") : t("ctxAddFavorite")}
              aria-pressed={!!saved}
              onClick={() => void useStore.getState().toggleFavorite(picked)}
            >
              <Star size={16} strokeWidth={1.75} fill={saved ? "currentColor" : "none"} />
            </button>
          </div>
        </div>
      ) : (
        <div className="flex flex-col items-center text-center gap-2 pt-10 pb-6 px-6">
          <span className="tile w-10 h-10 rounded-full">
            <Search size={18} strokeWidth={1.75} />
          </span>
          <p className="m-0 text-sm text-muted max-w-[260px]">{t("searchHint")}</p>
        </div>
      )}
    </div>
  );
}
