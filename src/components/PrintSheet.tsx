import { useEffect, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { Printer, X } from "lucide-react";
import { Icon } from "./Icon";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtDateTime, fmtDistance, fmtDuration, fmtTime, routeArrow } from "../lib/format";
import type { TransitStopTime } from "../lib/api";
import { DEFAULT_PRINT_OPTIONS, onPrintMapImage, setPrintOptionsAsker, type PrintOptions, type PrintScale } from "../lib/print";
import { useDialog } from "../lib/useDialog";
import { pinGlyph } from "../map/overlays/waypoints";
import { RouteDetails } from "./RouteDetails";

/**
 * Printable version of the selected route: hidden on screen, the only thing on paper
 * (see `@media print` in styles.css). It is rendered only around printing, with a north-up map
 * overview when the map supplied one (lib/print.ts), at the scale chosen in the print dialog,
 * with a legend. The print dialog (scale 1:25,000 / 1:50,000 / fitted, ITM grid) is here too.
 */
export function PrintSheet() {
  const [active, setActive] = useState(false);
  const [mapImage, setMapImage] = useState<{ img: string | null; opts: PrintOptions }>({ img: null, opts: DEFAULT_PRINT_OPTIONS });
  useEffect(() => {
    // beforeprint fires right before the snapshot: render synchronously
    const before = () => flushSync(() => setActive(true));
    const after = () => setActive(false);
    window.addEventListener("beforeprint", before);
    window.addEventListener("afterprint", after);
    const off = onPrintMapImage((img, opts) => flushSync(() => setMapImage({ img, opts })));
    return () => {
      window.removeEventListener("beforeprint", before);
      window.removeEventListener("afterprint", after);
      off();
    };
  }, []);
  return (
    <>
      <PrintOptionsDialog />
      {active && <Sheet mapImage={mapImage.img} opts={mapImage.opts} />}
    </>
  );
}

/** Scale and grid, asked before printing a route (lib/print.ts awaits the answer). */
function PrintOptionsDialog() {
  const t = useT();
  const [ask, setAsk] = useState<{ current: PrintOptions; resolve: (o: PrintOptions | null) => void } | null>(null);
  const [opts, setOpts] = useState<PrintOptions>(DEFAULT_PRINT_OPTIONS);
  const ref = useRef<HTMLDivElement>(null);
  const okRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    setPrintOptionsAsker(
      (current) =>
        new Promise((resolve) => {
          setOpts(current);
          setAsk({ current, resolve });
        }),
    );
    return () => setPrintOptionsAsker(null);
  }, []);
  const done = (o: PrintOptions | null) => {
    ask?.resolve(o);
    setAsk(null);
  };
  useDialog(ref, () => done(null), { active: !!ask, initialFocus: okRef });
  if (!ask) return null;
  const scales: { v: PrintScale; label: string }[] = [
    { v: "fit", label: t("printScaleFit") },
    { v: 25000, label: "1:25,000" },
    { v: 50000, label: "1:50,000" },
  ];
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/25 no-print" onMouseDown={(e) => e.target === e.currentTarget && done(null)}>
      <div ref={ref} role="dialog" aria-modal="true" aria-label={t("printOptionsTitle")} className="popover rounded-[var(--radius)] w-[360px] max-w-[calc(100%-32px)] p-4 flex flex-col gap-3 fade-in">
        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold m-0">{t("printOptionsTitle")}</h2>
          <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={() => done(null)} aria-label={t("close")}>
            <Icon as={X} size={16} />
          </button>
        </div>
        <div className="flex flex-col gap-1.5">
          <span className="section-label m-0">{t("printScale")}</span>
          <div className="seg" role="group" aria-label={t("printScale")}>
            {scales.map((s) => (
              <button key={String(s.v)} aria-pressed={opts.scale === s.v} onClick={() => setOpts({ ...opts, scale: s.v })}>
                <span className="num">{s.label}</span>
              </button>
            ))}
          </div>
        </div>
        <label className="row justify-between cursor-pointer">
          <span className="flex flex-col">
            <span className="text-base">{t("printGrid")}</span>
            <span className="text-xs text-muted">{t("printGridHint")}</span>
          </span>
          <button type="button" role="switch" aria-checked={opts.grid} className="switch" onClick={() => setOpts({ ...opts, grid: !opts.grid })} />
        </label>
        <div className="flex justify-end gap-2">
          <button className="btn btn-sm" onClick={() => done(null)}>
            {t("cancel")}
          </button>
          <button ref={okRef} className="btn btn-primary btn-sm" onClick={() => done(opts)}>
            <Icon as={Printer} size={14} /> {t("print")}
          </button>
        </div>
      </div>
    </div>
  );
}

function Pin({ kind, index }: { kind: "start" | "end" | "via"; index: number }) {
  return <span className={`print-pin print-pin-${kind}`} aria-hidden dangerouslySetInnerHTML={{ __html: pinGlyph(kind, index) }} />;
}

function Sheet({ mapImage, opts }: { mapImage: string | null; opts: PrintOptions }) {
  const t = useT();
  const routes = useStore((s) => s.routes);
  const selectedId = useStore((s) => s.selectedRouteId);
  const waypoints = useStore((s) => s.waypoints);
  const units = useStore((s) => s.settings.units);
  const lang = useStore((s) => s.settings.language);
  // the selected route, else the first one (never an empty page)
  const route = routes.find((r) => r.id === selectedId) ?? routes[0];
  if (!route) return null;

  const from = waypoints[0]?.label ?? t("printFrom");
  const to = waypoints[waypoints.length - 1]?.label ?? t("printTo");
  const modeKey = route.mode === "car" ? "modeCar" : route.mode === "foot" ? "modeFoot" : route.mode === "bike" ? "modeBike" : "modeTransit";
  const vias = waypoints.slice(1, -1).map((w) => w?.label).filter(Boolean) as string[];

  return (
    <div className="print-sheet" aria-hidden="true">
      <div className="text-2xl font-semibold mb-1 flex items-center gap-2 flex-wrap">
        <Pin kind="start" index={0} /> {from} {routeArrow(lang)} <Pin kind="end" index={waypoints.length - 1} /> {to}
      </div>
      {vias.length > 0 && (
        <div className="text-sm mb-1">
          {t("via")}: {vias.map((v, i) => `${i + 1}. ${v}`).join(" · ")}
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
      {mapImage && (
        <>
          <img src={mapImage} alt={t("printOverview")} className="block w-full max-h-[110mm] object-contain mb-1 border border-[#c9ced6] rounded" />
          <div className="print-legend text-xs mb-4 flex items-center gap-4 flex-wrap">
            {route.mode === "transit" ? (
              <>
                <span className="flex items-center gap-1">
                  <span className="inline-block w-6 h-1.5 rounded" style={{ background: "var(--route-car)" }} /> {t("printLegendRide")}
                </span>
                <span className="flex items-center gap-1">
                  <span className="inline-block w-6 border-t-[3px] border-dotted" style={{ borderColor: "#6b7280" }} /> {t("printLegendWalk")}
                </span>
              </>
            ) : (
              <span className="flex items-center gap-1">
                <span className="inline-block w-6 h-1.5 rounded" style={{ background: "var(--route-car)" }} /> {t("printLegendRoute")}
              </span>
            )}
            <span className="flex items-center gap-1">
              <Pin kind="start" index={0} /> {t("printLegendStart")}
            </span>
            <span className="flex items-center gap-1">
              <Pin kind="end" index={1} /> {t("printLegendEnd")}
            </span>
            {opts.scale !== "fit" && <span className="num">{t("printScale")}: 1:{opts.scale.toLocaleString("en-US")}</span>}
            {opts.grid && <span>{t("printGridLegend")}</span>}
          </div>
        </>
      )}
      {route.mode === "transit" ? <TransitSummary /> : <RouteDetails route={route} />}
      <div className="text-xs mt-4">© OpenStreetMap contributors · Protomaps · MapsOffline</div>
    </div>
  );
}

/** Public transport on paper: numbered directions – walk to which stop (name and number, how
 *  far, how long), board which line towards where and when, get off where and when. */
function TransitSummary() {
  const t = useT();
  const routes = useStore((s) => s.routes);
  const selectedId = useStore((s) => s.selectedRouteId);
  const units = useStore((s) => s.settings.units);
  const route = routes.find((r) => r.id === selectedId) ?? routes[0];
  if (!route) return null;
  const stop = (st: TransitStopTime | undefined, name: string) =>
    st?.code ? t("printStopCode").replace("{name}", st.name || name).replace("{code}", st.code) : st?.name || name;
  const fill = (s: string, v: Record<string, string>) => Object.entries(v).reduce((a, [k, x]) => a.split(`{${k}}`).join(x), s);
  const lines: { text: string; strong?: boolean }[] = [];
  route.legs.forEach((l, i) => {
    const tr = l.transit;
    if (tr) {
      const line =
        tr.vehicle === "rail"
          ? t("printLineTrain")
          : fill(t(tr.vehicle === "light_rail" || tr.vehicle === "tram" ? "printLineLightRail" : "printLineBus"), { n: tr.routeShortName });
      lines.push({
        strong: true,
        text: fill(t("printBoard"), { time: fmtTime(tr.departIso), line, agency: tr.agency, headsign: tr.headsign || tr.toStop, stop: stop(tr.stops[0], tr.fromStop) }),
      });
      lines.push({ text: fill(t("printAlight"), { time: fmtTime(tr.arriveIso), stop: stop(tr.stops[tr.stops.length - 1], tr.toStop), n: String(tr.numStops) }) });
      return;
    }
    // a walk: to the next ride's stop, or to the destination
    const next = route.legs.slice(i + 1).find((x) => x.transit)?.transit;
    const transfer = route.legs.slice(0, i).some((x) => x.transit);
    if (next && l.distanceM < 20) return; // same stop
    const v = { d: fmtDistance(l.distanceM, t, units), t: fmtDuration(l.durationSec, t) };
    lines.push({
      text: next
        ? fill(t(transfer ? "printWalkTransfer" : "printWalkToStop"), { ...v, stop: stop(next.stops[0], next.fromStop) })
        : fill(t("printWalkToDest"), v),
    });
  });
  return (
    <div className="flex flex-col gap-2">
      <div className="text-sm font-semibold">{t("printTransitSteps")}</div>
      <ol className="m-0 ps-5 flex flex-col gap-1.5 num">
        {lines.map((x, i) => (
          <li key={i} className={x.strong ? "font-semibold" : undefined}>
            {x.text}
          </li>
        ))}
      </ol>
    </div>
  );
}
