import { X } from "lucide-react";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtDistance } from "../lib/format";
import { kindIcon } from "./SearchBox";

/** "What's nearby" (context menu): up to 30 named places within 600 m, nearest first. Picking
 *  one opens its card in the search panel. Sits on the free side of the map, under the layers
 *  popover's spot. */
export function NearbyList() {
  const t = useT();
  const nearby = useStore((s) => s.nearby);
  const units = useStore((s) => s.settings.units);
  if (!nearby) return null;
  const { lon, lat } = nearby;
  const close = () => useStore.getState().closeNearby();
  const distM = (x: number, y: number) => Math.hypot((x - lon) * Math.cos((lat * Math.PI) / 180), y - lat) * 111_320;
  return (
    <div role="dialog" aria-label={t("nearbyTitle")} className="absolute top-3 end-[64px] z-30 w-[320px] max-h-[calc(100%-24px)] popover rounded-[var(--radius)] flex flex-col overflow-hidden no-print fade-in">
      <div className="flex items-center justify-between ps-4 pe-2 h-12 shrink-0">
        <h2 className="text-base font-semibold m-0">{t("nearbyTitle")}</h2>
        <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={close} aria-label={t("close")} title={t("close")}>
          <X size={16} />
        </button>
      </div>
      <div className="flex-1 min-h-0 scroll-y pb-2">
        {nearby.loading && <div className="progress-line mx-4 my-3" role="progressbar" />}
        {!nearby.loading && nearby.items.length === 0 && <p className="m-0 px-4 py-2 text-sm text-muted">{t("nearbyEmpty")}</p>}
        <ul className="m-0 p-0 list-none">
          {nearby.items.map((r) => (
            <li key={r.id}>
              <button
                className="w-full flex items-center gap-3 px-4 min-h-11 py-1.5 text-start hover:bg-hover transition-colors duration-100"
                onClick={() => {
                  useStore.getState().pickPlace({ lon: r.lon, lat: r.lat, label: r.name, result: r }, 17);
                }}
              >
                <span className="tile">{kindIcon(r.kind)}</span>
                <span className="flex-1 min-w-0">
                  <span className="block truncate text-sm">{r.name}</span>
                  {r.category && <span className="block truncate text-xs text-muted">{r.category}</span>}
                </span>
                <span className="text-xs text-subtle num shrink-0">{fmtDistance(distM(r.lon, r.lat), t, units)}</span>
              </button>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
