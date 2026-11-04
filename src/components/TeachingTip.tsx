import { useEffect, useRef, useState } from "react";
import { Lightbulb, X } from "lucide-react";
import { Icon } from "./Icon";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";

interface TipDef {
  id: string;
  /** CSS selector of the element the tail points at. */
  target: string;
  title: I18nKey;
  body: I18nKey;
}

/** Hidden features worth one hint each, in this order; each is shown once (settings.teachingTipsSeen). */
export const TEACHING_TIPS: TipDef[] = [
  { id: "mapMenu", target: ".maplibregl-canvas", title: "ttMapMenuTitle", body: "ttMapMenuBody" },
  { id: "palette", target: "[data-tt='search']", title: "ttPaletteTitle", body: "ttPaletteBody" },
  { id: "import", target: "[data-layers-toggle]", title: "ttImportTitle", body: "ttImportBody" },
];

/** The first tip not seen yet whose target is on screen. */
export function nextTip(seen: string[]): TipDef | undefined {
  return TEACHING_TIPS.find((t) => !seen.includes(t.id));
}

/**
 * WinUI TeachingTip: a callout with a tail that points at a control, shown once per feature a
 * few seconds after start (never over a dialog or during navigation). "הבנתי" or Escape marks
 * it seen.
 */
export function TeachingTip() {
  const t = useT();
  const seen = useStore((s) => s.settings.teachingTipsSeen ?? []);
  const loaded = useStore((s) => s.settingsLoaded);
  const busy = useStore((s) => !!s.navigation || s.paletteOpen || s.shortcutsOpen || !!s.share || s.batchOpen || !s.tilesUrl);
  const [ready, setReady] = useState(false);
  const [rect, setRect] = useState<DOMRect | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const tip = loaded ? nextTip(seen) : undefined;

  useEffect(() => {
    const h = window.setTimeout(() => setReady(true), 4000);
    return () => window.clearTimeout(h);
  }, []);

  useEffect(() => {
    if (!ready || !tip || busy) {
      setRect(null);
      return;
    }
    const el = document.querySelector(tip.target) as HTMLElement | null;
    setRect(el ? el.getBoundingClientRect() : null);
  }, [ready, tip, busy]);

  const dismiss = () => {
    if (!tip) return;
    setRect(null);
    void useStore.getState().updateSettings({ teachingTipsSeen: [...seen, tip.id] });
  };

  useEffect(() => {
    if (!rect) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") dismiss();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rect]);

  if (!tip || !rect) return null;
  // the map: point at its centre; small controls: below them
  const big = rect.width > 400;
  const top = big ? rect.top + rect.height / 2 - 60 : rect.bottom + 10;
  const left = big ? rect.left + rect.width / 2 - 150 : Math.max(8, Math.min(window.innerWidth - 308, rect.left + rect.width / 2 - 150));

  return (
    <div ref={ref} className="teaching-tip fade-in no-print" style={{ top, left }} role="dialog" aria-labelledby="tt-title">
      {!big && <span aria-hidden className="absolute -top-[7px] w-3 h-3 rotate-45 bg-surface-raised border-s border-t border-line" style={{ left: Math.min(288, Math.max(12, rect.left + rect.width / 2 - left - 6)) }} />}
      <div className="flex items-start gap-3">
        <Icon as={Lightbulb} size={18} className="text-accent mt-0.5 shrink-0" />
        <div className="flex-1 min-w-0">
          <div id="tt-title" className="font-semibold text-sm">
            {t(tip.title)}
          </div>
          <p className="text-sm text-muted mt-1">{t(tip.body)}</p>
          <div className="mt-3 flex justify-end">
            <button className="btn btn-primary btn-sm" onClick={dismiss} autoFocus>
              {t("ttGotIt")}
            </button>
          </div>
        </div>
        <button className="btn btn-ghost btn-icon btn-sm -m-1" onClick={dismiss} aria-label={t("close")}>
          <Icon as={X} size={14} />
        </button>
      </div>
    </div>
  );
}
