import { useEffect, useMemo, useRef, useState } from "react";
import { Check, Layers, Loader2, LocateFixed, Search, SlidersHorizontal, X } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { POI_CATEGORIES, POI_GROUPS, QUICK_CATEGORIES, type PoiCategory } from "../lib/poiCategories";

/** Map control under the categories button: centre the map on the device's position. */
export function LocateButton() {
  const t = useT();
  const locating = useStore((s) => s.locating);
  const has = useStore((s) => !!s.myLocation);
  return (
    <button
      type="button"
      className={clsx("absolute top-[60px] end-3 z-20 no-print w-10 h-10 rounded popover flex items-center justify-center transition-colors duration-150 hover:bg-hover", has ? "text-accent" : "text-fg")}
      title={t("myLocation")}
      aria-label={t("myLocation")}
      aria-busy={locating}
      disabled={locating}
      onClick={() => void useStore.getState().locate(true)}
    >
      {locating ? <Loader2 size={18} strokeWidth={1.75} className="animate-spin" /> : <LocateFixed size={18} strokeWidth={1.75} />}
    </button>
  );
}

/** Map control under "my location": opens the map layers popover (LayersPopover, rendered by App). */
export function LayersButton() {
  const t = useT();
  const open = useStore((s) => s.layersOpen);
  return (
    <button
      type="button"
      data-layers-toggle
      className={clsx("absolute top-[108px] end-3 z-20 no-print w-10 h-10 rounded popover flex items-center justify-center transition-colors duration-150 hover:bg-hover", open ? "text-accent" : "text-fg")}
      title={t("layersButton")}
      aria-label={t("layersButton")}
      aria-expanded={open}
      onClick={() => useStore.getState().toggleLayers()}
    >
      <Layers size={18} strokeWidth={1.75} />
    </button>
  );
}

/**
 * Map control: a rounded button above the zoom buttons that opens the categories window
 * (quick picks, search, grouped / alphabetical list). Selected categories draw as markers.
 */
export function CategoryPicker() {
  const t = useT();
  const selected = useStore((s) => s.layers.poiCats);
  const setLayers = useStore((s) => s.setLayers);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<"groups" | "alpha">("groups");
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    window.setTimeout(() => inputRef.current?.focus(), 30);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const toggle = (id: string) => setLayers({ poiCats: selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id] });
  const labelOf = (c: PoiCategory) => t(c.label);
  const q = query.trim();
  const visible = useMemo(() => POI_CATEGORIES.filter((c) => !q || labelOf(c).includes(q)), [q, t]); // eslint-disable-line react-hooks/exhaustive-deps
  const alpha = useMemo(() => [...visible].sort((a, b) => labelOf(a).localeCompare(labelOf(b), "he")), [visible]); // eslint-disable-line react-hooks/exhaustive-deps

  const chip = (c: PoiCategory) => {
    const on = selected.includes(c.id);
    return (
      <button key={c.id} type="button" className="choice" aria-pressed={on} onClick={() => toggle(c.id)}>
        <span className="cat-glyph shrink-0" aria-hidden dangerouslySetInnerHTML={{ __html: c.svg }} />
        {labelOf(c)}
        {on && <Check size={12} strokeWidth={2.5} />}
      </button>
    );
  };

  return (
    <div ref={rootRef} className="absolute top-3 end-3 z-20 no-print">
      <button
        type="button"
        className={clsx("w-10 h-10 rounded popover flex items-center justify-center text-fg transition-colors duration-150 hover:bg-hover relative", (open || selected.length > 0) && "text-accent")}
        title={t("categoriesTitle")}
        aria-label={t("categoriesTitle")}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <SlidersHorizontal size={18} strokeWidth={1.75} />
        {selected.length > 0 && <span className="absolute -top-1 -end-1 min-w-4 h-4 px-1 rounded-full bg-accent text-white text-[10px] font-semibold num flex items-center justify-center">{selected.length}</span>}
      </button>

      {open && (
        <div className="absolute top-0 end-12 w-[340px] max-w-[calc(100vw-80px)] popover rounded-[var(--radius-card)] p-3 flex flex-col gap-3 fade-in" role="dialog" aria-label={t("categoriesTitle")}>
          <div className="flex items-center justify-between">
            <h2 className="section-label m-0 p-0">{t("categoriesTitle")}</h2>
            <button className="btn btn-ghost btn-icon btn-sm -me-1 text-muted" onClick={() => setOpen(false)} aria-label={t("close")}>
              <X size={15} />
            </button>
          </div>
          <div className="flex flex-wrap gap-1.5">{QUICK_CATEGORIES.map((id) => POI_CATEGORIES.find((c) => c.id === id)).filter(Boolean).map((c) => chip(c!))}</div>
          <div className="relative">
            <Search size={15} strokeWidth={1.75} className="absolute start-3 top-1/2 -translate-y-1/2 text-muted pointer-events-none" />
            <input ref={inputRef} className="input h-9 ps-9" placeholder={t("categoriesSearch")} value={query} onChange={(e) => setQuery(e.target.value)} />
          </div>
          <div className="flex items-center justify-between gap-2">
            <div className="seg w-fit">
              <button aria-pressed={sort === "groups"} onClick={() => setSort("groups")}>
                <span>{t("sortByTopic")}</span>
              </button>
              <button aria-pressed={sort === "alpha"} onClick={() => setSort("alpha")}>
                <span>{t("sortAlpha")}</span>
              </button>
            </div>
            {selected.length > 0 && (
              <button className="btn btn-ghost btn-sm text-muted" onClick={() => setLayers({ poiCats: [] })}>
                {t("clearAll")}
              </button>
            )}
          </div>
          <div className="scroll-y max-h-[55vh] flex flex-col gap-3 -mx-1 px-1">
            {sort === "alpha" ? (
              <div className="flex flex-wrap gap-1.5">{alpha.map(chip)}</div>
            ) : (
              POI_GROUPS.map((g) => {
                const list = visible.filter((c) => c.group === g.id);
                if (list.length === 0) return null;
                return (
                  <div key={g.id} className="flex flex-col gap-1.5">
                    <div className="text-xs font-semibold text-muted">{t(g.label)}</div>
                    <div className="flex flex-wrap gap-1.5">{list.map(chip)}</div>
                  </div>
                );
              })
            )}
            {visible.length === 0 && <div className="text-sm text-muted py-2">{t("noCategoriesMatch")}</div>}
          </div>
          <p className="m-0 text-xs text-subtle">{t("poiLayerHint")}</p>
        </div>
      )}
    </div>
  );
}
