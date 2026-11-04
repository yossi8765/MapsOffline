import { useEffect, useRef } from "react";
import { X } from "lucide-react";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { LayersPanel } from "./LayersPanel";

/** Map layers as a popover next to the map buttons (free side of the map: the sidebar is on the
 *  start side, the map controls on the end side). Opened by `store.toggleLayers()` from a map
 *  button; closes on Escape or a click outside. Rendered by App. */
export function LayersPopover() {
  const t = useT();
  const open = useStore((s) => s.layersOpen);
  const setOpen = useStore((s) => s.setLayersOpen);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    const onDown = (e: PointerEvent) => {
      const el = e.target as HTMLElement | null;
      // the map button that toggles the popover handles its own click
      if (ref.current && el && !ref.current.contains(el) && !el.closest("[data-layers-toggle]")) setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown, true);
    };
  }, [open, setOpen]);

  if (!open) return null;
  return (
    <div
      ref={ref}
      role="dialog"
      aria-label={t("layersButton")}
      className="absolute top-3 end-[64px] z-30 w-[340px] max-h-[calc(100%-24px)] popover rounded-[var(--radius)] flex flex-col overflow-hidden no-print fade-in"
    >
      <div className="flex items-center justify-between ps-4 pe-2 h-12 shrink-0">
        <h2 className="text-base font-semibold m-0">{t("layersButton")}</h2>
        <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={() => setOpen(false)} aria-label={t("close")} title={t("close")}>
          <X size={16} />
        </button>
      </div>
      <div className="flex-1 min-h-0 scroll-y">
        <LayersPanel />
      </div>
    </div>
  );
}
