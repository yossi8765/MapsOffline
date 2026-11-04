import { X } from "lucide-react";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";

/** Keys shown as the user sees them on a Hebrew/English keyboard: Latin letter / Hebrew letter. */
export const SHORTCUTS: [string, I18nKey][] = [
  ["/  ·  Ctrl+K", "shortcutSearch"],
  ["D / ג", "shortcutRoute"],
  ["L / ך", "shortcutLocate"],
  ["N / מ", "shortcutNorth"],
  ["T / א", "shortcut3d"],
  ["Ctrl+H", "shortcutHistory"],
  ["+ / −", "shortcutZoom"],
  ["Esc", "shortcutEsc"],
  ["?", "shortcutHelp"],
];

/** The shortcut table ("?" key). */
export function ShortcutsDialog() {
  const t = useT();
  const open = useStore((s) => s.shortcutsOpen);
  if (!open) return null;
  const close = () => useStore.setState({ shortcutsOpen: false });
  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/20 no-print" onClick={close}>
      <div role="dialog" aria-label={t("shortcutsTitle")} className="popover rounded-[var(--radius)] w-[360px] max-w-[calc(100%-32px)] p-4 fade-in" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between mb-2">
          <h2 className="text-base font-semibold m-0">{t("shortcutsTitle")}</h2>
          <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={close} aria-label={t("close")} title={t("close")}>
            <X size={16} />
          </button>
        </div>
        <div className="group-list">
          {SHORTCUTS.map(([k, v]) => (
            <div key={k} className="row min-h-10 justify-between">
              <span className="text-base">{t(v)}</span>
              <kbd className="kbd" dir="ltr">
                {k}
              </kbd>
            </div>
          ))}
        </div>
        <p className="m-0 mt-2 text-xs text-muted">{t("shortcutsNote")}</p>
      </div>
    </div>
  );
}
