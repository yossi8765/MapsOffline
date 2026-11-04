import { useState } from "react";
import { AlertCircle, CheckCircle2, ChevronDown, Copy, Info, X } from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { copyText } from "../lib/format";
import type { Toast } from "../lib/store";

function ToastItem({ t }: { t: Toast }) {
  const tr = useT();
  const dismiss = useStore((s) => s.dismissToast);
  const hold = useStore((s) => s.holdToast);
  const [open, setOpen] = useState(false);
  const isError = t.kind === "error";
  return (
    <div
      className="popover pointer-events-auto rounded-[var(--radius)] ps-3 pe-1 py-1 min-h-10 flex items-start gap-2.5 w-max max-w-[460px] text-sm fade-in"
      role={isError ? "alert" : "status"}
      onMouseEnter={() => hold(t.id, true)}
      onMouseLeave={() => hold(t.id, false)}
      onFocus={() => hold(t.id, true)}
      onBlur={() => hold(t.id, false)}
    >
      <span className="mt-2.5 shrink-0">
        {isError ? (
          <Icon as={AlertCircle} className="text-danger" />
        ) : t.kind === "success" ? (
          <Icon as={CheckCircle2} className="text-success" />
        ) : (
          <Icon as={Info} className="text-muted" />
        )}
      </span>
      <div className="flex-1 min-w-0 py-2">
        <div className="justify">
          {t.text}
          {(t.count ?? 1) > 1 && (
            <span className="chip chip-muted ms-2 align-middle num" dir="ltr">
              ×{t.count}
            </span>
          )}
        </div>
        {t.details && (
          <div className="mt-1">
            <button className="inline-flex items-center gap-1 text-xs text-muted hover:text-fg" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
              <Icon as={ChevronDown} size={12} className={clsx("transition-transform", open && "rotate-180")} />
              {tr("errDetails")}
            </button>
            {open && (
              <div className="mt-1 flex items-start gap-1">
                <pre className="selectable flex-1 m-0 p-2 rounded-[var(--radius-sm)] bg-surface-2 text-xs whitespace-pre-wrap break-all max-h-40 overflow-auto" dir="ltr">
                  {t.details}
                </pre>
                <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={() => void copyText(t.details!)} aria-label={tr("errCopyDetails")} title={tr("errCopyDetails")}>
                  <Icon as={Copy} size={14} />
                </button>
              </div>
            )}
          </div>
        )}
      </div>
      {t.action && (
        <button
          className="btn btn-ghost btn-sm text-accent font-semibold mt-1 shrink-0"
          onClick={() => {
            t.action!.run();
            dismiss(t.id);
          }}
        >
          {t.action.label}
        </button>
      )}
      <button className="btn btn-ghost btn-icon btn-sm text-muted mt-1 shrink-0" onClick={() => dismiss(t.id)} aria-label={tr("close")}>
        <Icon as={X} size={14} />
      </button>
    </div>
  );
}

/** Toast stack above the map's bottom edge. Errors have role=alert and stay until closed;
 *  hovering pauses the timer; identical toasts merge with a ×N count. */
export function Toasts() {
  const toasts = useStore((s) => s.toasts);
  // the "online map" banner sits at bottom-10: stack the toasts above it
  const banner = useStore((s) => s.tilesChecked && !s.tilesUrl && !!s.onlineTilesUrl);
  return (
    <div
      className={clsx("absolute start-1/2 -translate-x-1/2 rtl:translate-x-1/2 z-50 flex flex-col items-center gap-2 no-print pointer-events-none", banner ? "bottom-[88px]" : "bottom-10")}
      aria-live="polite"
    >
      {toasts.map((t) => (
        <ToastItem key={t.id} t={t} />
      ))}
    </div>
  );
}
