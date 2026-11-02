import { AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../lib/store";

export function Toasts() {
  const toasts = useStore((s) => s.toasts);
  const dismiss = useStore((s) => s.dismissToast);
  if (!toasts.length) return null;
  return (
    <div className="absolute bottom-10 start-1/2 -translate-x-1/2 rtl:translate-x-1/2 z-50 flex flex-col items-center gap-2 no-print pointer-events-none" aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={clsx(
            "popover pointer-events-auto rounded-[var(--radius-sm)] ps-3 pe-1 py-1 min-h-10 flex items-center gap-2.5 max-w-[440px] text-sm fade-in",
          )}
          role="status"
        >
          {t.kind === "error" ? (
            <AlertCircle size={16} className="text-danger shrink-0" />
          ) : t.kind === "success" ? (
            <CheckCircle2 size={16} className="text-success shrink-0" />
          ) : (
            <Info size={16} className="text-muted shrink-0" />
          )}
          <span className="flex-1 py-1 justify">{t.text}</span>
          <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={() => dismiss(t.id)} aria-label="close">
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
