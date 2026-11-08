import { BellRing, X } from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtDistance } from "../lib/format";
import { stopTransitAlert } from "../lib/transitAlert";

/** On the map while "wake me before my stop" runs: where to get off, how far, cancel. */
export function TransitAlertBar() {
  const t = useT();
  const a = useStore((s) => s.transitAlert);
  const units = useStore((s) => s.settings.units);
  if (!a) return null;
  return (
    <div
      className={clsx(
        "absolute top-3 start-1/2 -translate-x-1/2 rtl:translate-x-1/2 z-20 popover rounded-[var(--radius)] ps-3 pe-1.5 min-h-10 flex items-center gap-2.5 text-sm fade-in no-print max-w-[calc(100%-32px)]",
        a.warned && "ring-2 ring-[var(--accent)]",
      )}
      role="status"
      data-transit-alert
    >
      <Icon as={BellRing} size={18} className={clsx("shrink-0", a.warned ? "text-accent animate-pulse" : "text-muted")} />
      <div className="flex flex-col min-w-0 py-1">
        <span className="font-semibold truncate">{t("transitAlertBar").replace("{stop}", a.stop)}</span>
        <span className="text-xs text-muted num truncate">
          {a.line && `${t("transitAlertLine").replace("{n}", a.line)} · `}
          {a.rides > 1 && `${t("transitAlertRide").replace("{n}", String(a.ride)).replace("{m}", String(a.rides))} · `}
          {a.distM != null ? fmtDistance(a.distM, t, units) : "…"}
        </span>
      </div>
      <button className="btn btn-ghost btn-icon btn-sm shrink-0" aria-label={t("transitAlertStop")} onClick={stopTransitAlert}>
        <Icon as={X} size={16} />
      </button>
    </div>
  );
}
