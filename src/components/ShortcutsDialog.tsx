import { useRef } from "react";
import { X } from "lucide-react";
import { Icon } from "./Icon";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { shortcutRows } from "../lib/shortcuts";
import { useDialog } from "../lib/useDialog";

/** The rows of the shortcut table (also listed in Settings). Built from lib/shortcuts.ts. */
export function ShortcutList() {
  const t = useT();
  return (
    <div className="group-list">
      {shortcutRows().map((r) => (
        <div key={r.label} className="row min-h-10 justify-between">
          <span>{t(r.label)}</span>
          <span className="flex gap-1 flex-wrap justify-end" dir="ltr">
            {r.keys.map((k) => (
              <kbd key={k} className="kbd">
                {k}
              </kbd>
            ))}
          </span>
        </div>
      ))}
    </div>
  );
}

function Dialog() {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const close = () => useStore.getState().setShortcutsOpen(false);
  useDialog(ref, close);
  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center dialog-backdrop no-print" onClick={close}>
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby="shortcuts-title"
        className="dialog w-[400px] max-w-[calc(100%-32px)] max-h-[calc(100%-48px)] flex flex-col p-5 fade-in"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between mb-3">
          <h2 id="shortcuts-title" className="text-xl font-semibold m-0">
            {t("shortcutsTitle")}
          </h2>
          <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={close} aria-label={t("close")}>
            <Icon as={X} />
          </button>
        </div>
        <div className="scroll-y min-h-0">
          <ShortcutList />
        </div>
      </div>
    </div>
  );
}

/** The shortcut table ("?" key). */
export function ShortcutsDialog() {
  const open = useStore((s) => s.shortcutsOpen);
  return open ? <Dialog /> : null;
}
