import { X } from "lucide-react";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtDistance } from "../lib/format";
import { categoryOf, POI_CATEGORIES } from "../lib/poiCategories";
import { kindIcon } from "./SearchBox";

/** Search radii offered in the list (metres). */
const RADII = [300, 600, 1000, 2000, 5000];

/** "What's nearby" (context menu): named places around a point, nearest first, within a chosen
 *  radius and optionally of one kind (fuel only, synagogues only …; both remembered). Picking one
 *  opens its card in the search panel. Sits on the free side of the map, under the layers
 *  popover's spot. */
export function NearbyList() {
  const t = useT();
  const nearby = useStore((s) => s.nearby);
  const units = useStore((s) => s.settings.units);
  if (!nearby) return null;
  const { lon, lat, radius, cat } = nearby;
  const close = () => useStore.getState().closeNearby();
  const reload = (o: { radius?: number; cat?: string }) => void useStore.getState().showNearby(lon, lat, o);
  const distM = (x: number, y: number) => Math.hypot((x - lon) * Math.cos((lat * Math.PI) / 180), y - lat) * 111_320;
  const kinds = [...POI_CATEGORIES].sort((a, b) => t(a.label).localeCompare(t(b.label), "he"));
  return (
    <div role="dialog" aria-label={t("nearbyTitle")} className="absolute top-3 end-[64px] z-30 w-[340px] max-h-[calc(100%-24px)] popover rounded-[var(--radius)] flex flex-col overflow-hidden no-print fade-in">
      <div className="flex items-center justify-between ps-4 pe-2 h-12 shrink-0">
        <h2 className="text-base font-semibold m-0">{t("nearbyTitle")}</h2>
        <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={close} aria-label={t("close")} title={t("close")}>
          <X size={16} />
        </button>
      </div>
      <div className="px-4 pb-2 flex flex-col gap-2 shrink-0">
        <div className="seg seg-sm" role="group" aria-label={t("nearbyRadius")}>
          {RADII.map((r) => (
            <button key={r} aria-pressed={radius === r} onClick={() => reload({ radius: r })}>
              <span className="num">{fmtDistance(r, t, units)}</span>
            </button>
          ))}
        </div>
        <select className="input h-9" aria-label={t("nearbyKind")} value={cat} onChange={(e) => reload({ cat: e.target.value })}>
          <option value="">{t("nearbyAll")}</option>
          {kinds.map((c) => (
            <option key={c.id} value={c.id}>
              {t(c.label)}
            </option>
          ))}
        </select>
      </div>
      <div className="flex-1 min-h-0 scroll-y pb-2">
        {nearby.loading && <div className="progress-line mx-4 my-3" role="progressbar" />}
        {!nearby.loading && nearby.items.length === 0 && <p className="m-0 px-4 py-2 text-sm text-muted">{t("nearbyEmpty")}</p>}
        <ul className="m-0 p-0 list-none">
          {nearby.items.map((r) => {
            const kind = categoryOf(r.category ?? "", r.name, POI_CATEGORIES);
            // the Hebrew category of the place itself ("בית כנסת"), else the picker's label
            const label = r.categoryLabel ?? (kind ? t(kind.label) : undefined);
            return (
              <li key={r.id}>
                <button
                  className="w-full flex items-center gap-3 px-4 min-h-11 py-1.5 text-start hover:bg-hover transition-colors duration-100"
                  onClick={() => {
                    useStore.getState().pickPlace({ lon: r.lon, lat: r.lat, label: r.name, result: { ...r, category: label ?? r.category } }, 17);
                  }}
                >
                  <span className="tile" style={kind ? { color: kind.color } : undefined}>
                    {kindIcon(r.kind)}
                  </span>
                  <span className="flex-1 min-w-0">
                    <span className="block truncate text-sm">{r.name}</span>
                    {label && <span className="block truncate text-xs text-muted">{label}</span>}
                  </span>
                  <span className="text-xs text-subtle num shrink-0">{fmtDistance(distM(r.lon, r.lat), t, units)}</span>
                </button>
              </li>
            );
          })}
        </ul>
      </div>
    </div>
  );
}
