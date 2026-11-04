import { useEffect, useState } from "react";
import { flushSync } from "react-dom";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtDateTime, fmtDistance, fmtDuration, fmtTime, routeArrow } from "../lib/format";
import { onPrintMapImage } from "../lib/print";
import { RouteDetails } from "./RouteDetails";

/**
 * Printable version of the selected route: hidden on screen, the only thing on paper
 * (see `@media print` in styles.css). It is rendered only around printing (it used to render
 * RouteDetails – and fetch an elevation profile – for every route result), with a north-up map
 * overview when the map supplied one (lib/print.ts).
 */
export function PrintSheet() {
  const [active, setActive] = useState(false);
  const [mapImage, setMapImage] = useState<string | null>(null);
  useEffect(() => {
    // beforeprint fires right before the snapshot: render synchronously
    const before = () => flushSync(() => setActive(true));
    const after = () => setActive(false);
    window.addEventListener("beforeprint", before);
    window.addEventListener("afterprint", after);
    const off = onPrintMapImage((img) => flushSync(() => setMapImage(img)));
    return () => {
      window.removeEventListener("beforeprint", before);
      window.removeEventListener("afterprint", after);
      off();
    };
  }, []);
  if (!active) return null;
  return <Sheet mapImage={mapImage} />;
}

function Sheet({ mapImage }: { mapImage: string | null }) {
  const t = useT();
  const routes = useStore((s) => s.routes);
  const selectedId = useStore((s) => s.selectedRouteId);
  const waypoints = useStore((s) => s.waypoints);
  const units = useStore((s) => s.settings.units);
  const lang = useStore((s) => s.settings.language);
  // the selected route, else the first one (never an empty page)
  const route = routes.find((r) => r.id === selectedId) ?? routes[0];
  if (!route) return null;

  const from = waypoints[0]?.label ?? "A";
  const to = waypoints[waypoints.length - 1]?.label ?? "B";
  const modeKey = route.mode === "car" ? "modeCar" : route.mode === "foot" ? "modeFoot" : route.mode === "bike" ? "modeBike" : "modeTransit";
  const vias = waypoints.slice(1, -1).map((w) => w?.label).filter(Boolean) as string[];

  return (
    <div className="print-sheet" aria-hidden="true">
      <div className="text-2xl font-semibold mb-1">
        {from} {routeArrow(lang)} {to}
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
      {mapImage && <img src={mapImage} alt={t("printOverview")} className="block w-full max-h-[110mm] object-contain mb-4 border border-[#c9ced6] rounded" />}
      {route.mode === "transit" ? <TransitSummary /> : <RouteDetails route={route} />}
      <div className="text-xs mt-4">© OpenStreetMap contributors · MapsOffline</div>
    </div>
  );
}

/** Public transport on paper: per ride only the line, where to board and where to get off. */
function TransitSummary() {
  const t = useT();
  const routes = useStore((s) => s.routes);
  const selectedId = useStore((s) => s.selectedRouteId);
  const route = routes.find((r) => r.id === selectedId) ?? routes[0];
  if (!route) return null;
  return (
    <div className="flex flex-col gap-2">
      <div className="text-sm font-semibold">{t("printBoardAlight")}</div>
      <ol className="m-0 ps-5 flex flex-col gap-2">
        {route.legs.map((l, i) =>
          l.transit ? (
            <li key={i}>
              <span className="font-semibold num">{l.transit.routeShortName}</span> ({l.transit.agency}) · {fmtTime(l.transit.departIso)} {l.transit.fromStop} {routeArrow(useStore.getState().settings.language)} {fmtTime(l.transit.arriveIso)} {l.transit.toStop}
            </li>
          ) : (
            <li key={i} className="text-sm">
              {t("walking")} · {fmtDistance(l.distanceM, t)} · {fmtDuration(l.durationSec, t)}
            </li>
          ),
        )}
      </ol>
    </div>
  );
}
