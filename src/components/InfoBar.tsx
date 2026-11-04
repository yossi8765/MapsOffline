import type { ReactNode } from "react";
import { AlertTriangle, CheckCircle2, Info, X, XCircle } from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { useT } from "../lib/i18n";

export type Severity = "info" | "success" | "warning" | "error";

const ICONS = { info: Info, success: CheckCircle2, warning: AlertTriangle, error: XCircle } as const;

interface Props {
  severity?: Severity;
  /** Semibold first line (optional). */
  title?: ReactNode;
  children?: ReactNode;
  /** A button / link at the end (the fix: "למסך הנתונים", "נסה שוב"). */
  action?: ReactNode;
  /** Shown only when the bar can be dismissed; persistent problems pass nothing. */
  onClose?: () => void;
  className?: string;
}

/**
 * WinUI InfoBar: an in-place message that stays while its condition holds (missing data, online
 * map, old timetable, blocked host), with a fixed colour and icon per severity and an optional
 * action. Short confirmations ("הועתק") stay toasts.
 */
export function InfoBar({ severity = "info", title, children, action, onClose, className }: Props) {
  const t = useT();
  return (
    <div className={clsx("infobar", `infobar-${severity}`, className)} role={severity === "error" || severity === "warning" ? "alert" : "status"}>
      <Icon as={ICONS[severity]} size={16} className="infobar-icon" />
      <div className="flex-1 min-w-0 text-sm">
        {title && <div className="font-semibold">{title}</div>}
        {children && <div className={clsx("justify", title && "mt-0.5", "text-fg")}>{children}</div>}
        {action && <div className="mt-2 flex flex-wrap gap-2">{action}</div>}
      </div>
      {onClose && (
        <button className="btn btn-ghost btn-icon btn-sm -m-1 shrink-0" onClick={onClose} aria-label={t("close")}>
          <Icon as={X} size={14} />
        </button>
      )}
    </div>
  );
}
