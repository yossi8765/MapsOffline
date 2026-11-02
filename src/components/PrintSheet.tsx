import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtDateTime, fmtDistance, fmtDuration, fmtTime } from "../lib/format";
import { RouteDetails } from "./RouteDetails";

/**
 * Printable version of the selected route: hidden on screen, the only thing on paper
 * (see `@media print` in styles.css). The map canvas is not printed.
 */
export function PrintSheet() {
  const t = useT();
  const routes = useStore((s) => s.routes);
  const selectedId = useStore((s) => s.selectedRouteId);
  const waypoints = useStore((s) => s.waypoints);
  const units = useStore((s) => s.settings.units);
  const lang = useStore((s) => s.settings.language);
  const route = routes.find((r) => r.id === selectedId);
  if (!route) return null;

  const from = waypoints[0]?.label ?? "A";
  const to = waypoints[waypoints.length - 1]?.label ?? "B";
  const modeKey = route.mode === "car" ? "modeCar" : route.mode === "foot" ? "modeFoot" : route.mode === "bike" ? "modeBike" : "modeTransit";
  const vias = waypoints.slice(1, -1).map((w) => w?.label).filter(Boolean) as string[];

  return (
    <div className="print-sheet" aria-hidden="true">
      <div className="text-2xl font-semibold mb-1">
        {from} ← {to}
      </div>
      {vias.length > 0 && (
        <div className="text-sm mb-1">
          {t("via")}: {vias.join(" · ")}
        </div>
      )}
      <div className="text-sm num mb-4">
        {t(modeKey)} · {fmtDuration(route.durationSec, t)}
        {route.mode === "transit" && route.departIso
          ? ` · ${t("depart")} ${fmtTime(route.departIso)} · ${t("arrive")} ${fmtTime(route.arriveIso)}`
          : ` · ${fmtDistance(route.distanceM, t, units)}`}
        {" · "}
        {t("printedAt")} {fmtDateTime(new Date().toISOString(), lang)}
      </div>
      <RouteDetails route={route} />
      <div className="text-xs mt-4">© OpenStreetMap contributors · MapsOffline</div>
    </div>
  );
}
