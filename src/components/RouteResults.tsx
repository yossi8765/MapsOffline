import { Bus, ChevronDown, ChevronLeft, ChevronRight, Footprints, TrainFront, TramFront, Wifi, WifiOff } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import type { Leg, Route } from "../lib/api";
import { fmtDistance, fmtDuration, fmtTime, tagLabel } from "../lib/format";
import { RouteDetails } from "./RouteDetails";

/** Display names of the online routing services (the backend sends lowercase ids). */
const PROVIDER_NAMES: Record<string, string> = { osrm: "OSRM", transitous: "Transitous" };

export function vehicleIcon(leg: Leg, size = 14) {
  if (!leg.transit) return <Footprints size={size} />;
  switch (leg.transit.vehicle) {
    case "rail":
      return <TrainFront size={size} />;
    case "light_rail":
    case "tram":
    case "subway":
      return <TramFront size={size} />;
    default:
      return <Bus size={size} />;
  }
}

export function legColorHex(leg: Leg): string {
  if (leg.transit?.color) return leg.transit.color.startsWith("#") ? leg.transit.color : `#${leg.transit.color}`;
  if (leg.transit) return leg.transit.vehicle === "rail" ? "#7c3aed" : leg.transit.vehicle === "light_rail" ? "#dc2626" : "#0891b2";
  return "#64748b";
}

function TransitStrip({ route, rtl }: { route: Route; rtl: boolean }) {
  const parts = route.legs.filter((l) => l.transit || l.distanceM > 80);
  const Sep = rtl ? ChevronLeft : ChevronRight;
  return (
    <div className="flex items-center gap-1 flex-wrap">
      {parts.map((l, i) => (
        <span key={i} className="inline-flex items-center gap-1">
          {i > 0 && <Sep size={12} className="text-subtle" />}
          {l.transit ? (
            <span
              className="inline-flex items-center gap-1 h-[22px] px-1.5 rounded text-white text-xs font-semibold num"
              style={{ background: legColorHex(l) }}
            >
              {vehicleIcon(l, 12)}
              <span className="max-w-[120px] truncate">{l.transit.routeShortName || l.transit.routeLongName}</span>
            </span>
          ) : (
            <span className="inline-flex items-center gap-0.5 text-muted text-xs num">
              <Footprints size={12} />
              {Math.round(l.durationSec / 60)}
            </span>
          )}
        </span>
      ))}
    </div>
  );
}

const VIOLATION_KEY: Record<string, I18nKey> = {
  toll: "violationToll",
  unpaved: "violationUnpaved",
  motorway: "violationMotorway",
  areas: "violationAreas",
  areaA: "violationAreas",
  areaB: "violationAreas",
  ferry: "violationFerry",
};

/** Notes above the results: what the selected route could not avoid (when the engine reports
 *  it in `avoidViolations`), and for public transport that times come from the timetable,
 *  with a warning when the timetable is more than 14 days old. */
function RouteNotes() {
  const t = useT();
  const routes = useStore((s) => s.routes);
  const selectedId = useStore((s) => s.selectedRouteId);
  const transitUpdated = useStore((s) => s.dataStatus?.transit?.updatedIso);
  const sel = (routes.find((r) => r.id === selectedId) ?? routes[0]) as (Route & { avoidViolations?: string[] }) | undefined;
  if (!sel) return null;
  const violations = [...new Set((sel.avoidViolations ?? []).map((v) => VIOLATION_KEY[v]).filter(Boolean))];
  const ageDays = transitUpdated ? Math.floor((Date.now() - Date.parse(transitUpdated)) / 86_400_000) : 0;
  const transit = sel.mode === "transit";
  if (!violations.length && !transit) return null;
  return (
    <div className="flex flex-col gap-1 px-0.5 text-xs">
      {violations.length > 0 && (
        <p className="m-0 text-warning">
          {t("couldNotAvoid")} {violations.map((k) => t(k)).join(", ")}
        </p>
      )}
      {transit && <p className="m-0 text-muted">{t("scheduleTimes")}</p>}
      {transit && ageDays > 14 && <p className="m-0 text-warning">{t("gtfsOld").replace("{n}", String(ageDays))}</p>}
    </div>
  );
}

export function RouteResults() {
  const t = useT();
  const routes = useStore((s) => s.routes);
  const selectedId = useStore((s) => s.selectedRouteId);
  const toggleRoute = useStore((s) => s.toggleRoute);
  const detailsOpen = useStore((s) => s.detailsOpen);
  const units = useStore((s) => s.settings.units);
  const routeSource = useStore((s) => s.routeSource);
  const routeProvider = useStore((s) => s.routeProvider);
  const rtl = useStore((s) => s.settings.language) === "he";

  return (
    <div className="flex flex-col gap-3 pt-1">
      <div className="flex items-center justify-between gap-2 px-0.5">
        <h2 className="section-label m-0 p-0">{t("routeResults")}</h2>
        {routeSource && (
          <div className="flex items-center gap-1.5 text-xs text-muted min-w-0">
            {routeSource === "online" ? <Wifi size={13} /> : <WifiOff size={13} />}
            <span className="truncate">
              {routeSource === "online" ? t("sourceOnline") : t("sourceOffline")}
              {routeProvider && <span className="text-subtle"> · {PROVIDER_NAMES[routeProvider] ?? routeProvider}</span>}
              {routes[0]?.mode === "car" && <span className="text-subtle"> · {t("noTraffic")}</span>}
            </span>
          </div>
        )}
      </div>
      <RouteNotes />
      <div className="flex flex-col gap-2" role="listbox">
        {routes.map((r) => {
          const sel = r.id === selectedId;
          const secondary =
            (r.mode === "transit" && r.departIso ? `${fmtTime(r.departIso)} – ${fmtTime(r.arriveIso)}` : fmtDistance(r.distanceM, t, units)) +
            // estimated Rav-Kav fare ("ערך צבור") of a transit route
            (r.fareEstimateIls != null ? ` · ${t("fareApprox").replace("{n}", r.fareEstimateIls.toFixed(r.fareEstimateIls % 1 ? 1 : 0))}` : "");
          const open = sel && detailsOpen;
          return (
            <div
              key={r.id}
              role="option"
              aria-selected={sel}
              className={clsx(
                "relative w-full rounded-[var(--radius-card)] border overflow-hidden transition-colors duration-150",
                sel ? "border-accent bg-accent-soft/40" : "border-line bg-surface-solid hover:border-line-strong",
              )}
            >
              {sel && <span className="absolute start-0 top-3 bottom-3 w-[3px] rounded-e-full bg-accent" />}
              <button
                type="button"
                className={clsx("w-full px-4 py-3 text-start", !sel && "hover:bg-hover")}
                aria-expanded={open}
                onClick={() => toggleRoute(r.id)}
              >
              <div className="flex items-baseline justify-between gap-3">
                <div className={clsx("text-xl font-semibold num", sel && "text-accent")}>{fmtDuration(r.durationSec, t)}</div>
                <div className="flex items-center gap-2 shrink-0">
                  <span className="text-sm text-muted num">{secondary}</span>
                  <ChevronDown size={16} className={clsx("text-subtle transition-transform duration-200", open && "rotate-180")} />
                </div>
              </div>
              {r.mode === "transit" ? (
                <div className="mt-2 flex flex-col gap-1.5">
                  <TransitStrip route={r} rtl={rtl} />
                  <div className="text-xs text-muted num">
                    {r.transfers === 0 ? t("noTransfers") : `${r.transfers} ${r.transfers === 1 ? t("transfer") : t("transfers")}`} · {t("walking")}{" "}
                    {fmtDistance(r.walkM, t, units)}
                  </div>
                </div>
              ) : null}
              {r.tags.length > 0 && (
                <div className="flex gap-1 mt-2 flex-wrap">
                  {r.tags.map((tag) => (
                    <span key={tag} className={clsx("chip", tag === "fastest" ? "" : "chip-muted")}>
                      {tagLabel(tag, t)}
                    </span>
                  ))}
                </div>
              )}
              </button>
              {open && (
                <div className="border-t border-line px-3 pb-3 pt-1 fade-in">
                  <RouteDetails route={r} embedded />
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
