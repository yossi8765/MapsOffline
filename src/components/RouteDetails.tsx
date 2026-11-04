import { useEffect, useState } from "react";
import {
  ArrowUp,
  ChevronDown,
  Circle,
  Copy,
  CornerUpLeft,
  CornerUpRight,
  Download,
  Flag,
  Footprints,
  GitFork,
  Star,
  Merge,
  MoveUpLeft,
  MoveUpRight,
  Printer,
  Navigation,
  RotateCcw,
  Undo2,
  CornerLeftDown,
  CornerRightDown,
} from "lucide-react";
import clsx from "clsx";
import { findLineFavorite, useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import type { Leg, Maneuver, Route } from "../lib/api";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { copyText, fmtDistance, fmtDuration, fmtTime, routeArrow, routeToGpx, routeToKml, routeToText } from "../lib/format";
import { api } from "../lib/api";
import { printPage } from "../lib/print";
import { legColorHex, vehicleIcon } from "./RouteResults";

function maneuverIcon(m: Maneuver, size = 16) {
  switch (m) {
    case "depart":
      return <Circle size={size} />;
    case "arrive":
      return <Flag size={size} />;
    case "turn-left":
      return <CornerUpLeft size={size} />;
    case "turn-right":
      return <CornerUpRight size={size} />;
    case "slight-left":
      return <MoveUpLeft size={size} />;
    case "slight-right":
      return <MoveUpRight size={size} />;
    // sharp turns bend back down; the U-shaped arrow is the U-turn (it read as "undo" before)
    case "sharp-left":
      return <CornerLeftDown size={size} />;
    case "sharp-right":
      return <CornerRightDown size={size} />;
    case "uturn":
      return <Undo2 size={size} />;
    case "roundabout":
    case "exit-roundabout":
      return <RotateCcw size={size} />;
    case "merge":
      return <Merge size={size} />;
    case "fork":
      return <GitFork size={size} />;
    default:
      return <ArrowUp size={size} />;
  }
}

function StepsList({ route, leg, legIndex }: { route: Route; leg: Leg; legIndex: number }) {
  const t = useT();
  const units = useStore((s) => s.settings.units);
  const setHighlight = useStore((s) => s.setHighlight);
  const flyTo = useStore((s) => s.flyToPoint);
  const steps = leg.steps ?? [];
  if (!steps.length) {
    return (
      <div className="text-sm text-muted px-3 py-2 num">
        {fmtDistance(leg.distanceM, t, units)} · {fmtDuration(leg.durationSec, t)}
      </div>
    );
  }
  return (
    <ol className="flex flex-col m-0 p-0 list-none">
      {steps.map((s, i) => (
        <li
          key={i}
          tabIndex={0}
          className="flex items-center gap-3 px-3 min-h-[48px] cursor-pointer transition-colors duration-150 hover:bg-hover focus-visible:bg-hover"
          onMouseEnter={() => setHighlight({ routeId: route.id, legIndex, stepIndex: i })}
          onMouseLeave={() => setHighlight(null)}
          onClick={() => {
            const p = leg.geometry[Math.min(s.geometryStart, leg.geometry.length - 1)];
            if (p) flyTo(p, 16);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              const p = leg.geometry[Math.min(s.geometryStart, leg.geometry.length - 1)];
              if (p) flyTo(p, 16);
            }
          }}
        >
          <span
            className={clsx(
              "w-8 h-8 shrink-0 rounded-full flex items-center justify-center",
              s.maneuver === "arrive" ? "bg-danger-soft text-danger" : s.maneuver === "depart" ? "bg-accent-soft text-accent" : "bg-surface-2 text-fg",
            )}
          >
            {maneuverIcon(s.maneuver)}
          </span>
          <span className="flex-1 min-w-0 py-2.5 text-base">{s.instruction}</span>
          <span className="shrink-0 text-sm text-muted num">{s.distanceM > 0 && s.maneuver !== "arrive" ? fmtDistance(s.distanceM, t, units) : ""}</span>
        </li>
      ))}
    </ol>
  );
}

/** Stop code (the number on the sign) in small grey type after the stop name. */
function StopCode({ code }: { code?: string }) {
  if (!code) return null;
  return (
    <span className="text-xs font-normal text-subtle num mx-1.5" dir="ltr">
      {code}
    </span>
  );
}

function TransitLeg({ route, leg, legIndex }: { route: Route; leg: Leg; legIndex: number }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const setHighlight = useStore((s) => s.setHighlight);
  const flyTo = useStore((s) => s.flyToPoint);
  const tr = leg.transit!;
  const color = legColorHex(leg);
  const mid = tr.stops.slice(1, -1);
  const favorites = useStore((s) => s.favorites);
  const lineRef = { routeShortName: tr.routeShortName || tr.routeLongName, routeLongName: tr.routeLongName, agency: tr.agency, headsign: tr.headsign, color: tr.color };
  const savedLine = findLineFavorite(favorites, lineRef);
  return (
    <div className="flex gap-3" onMouseEnter={() => setHighlight({ routeId: route.id, legIndex })} onMouseLeave={() => setHighlight(null)}>
      <div className="w-11 shrink-0 flex flex-col text-sm font-semibold num" aria-hidden>
        <span className="leading-5" title={t("depart")}>{fmtTime(tr.departIso)}</span>
        <span className="flex-1" />
        <span className="leading-5" title={t("arrive")}>{fmtTime(tr.arriveIso)}</span>
      </div>
      <div className="w-3 shrink-0 flex flex-col items-center pt-1">
        <span className="w-3 h-3 shrink-0 rounded-full border-[3px] bg-surface-solid" style={{ borderColor: color }} />
        <span className="flex-1 w-1 -my-px" style={{ background: color }} />
        <span className="w-3 h-3 shrink-0 rounded-full border-[3px] bg-surface-solid mb-1" style={{ borderColor: color }} />
      </div>
      <div className="flex-1 min-w-0 flex flex-col gap-1.5">
        <button className="text-start min-w-0" onClick={() => flyTo([tr.stops[0]?.lon ?? leg.geometry[0][0], tr.stops[0]?.lat ?? leg.geometry[0][1]], 16)}>
          <div className="text-base font-semibold truncate-dir">
            {tr.fromStop}
            <StopCode code={tr.stops[0]?.code} />
          </div>
        </button>
        <div className="flex items-center gap-2 min-w-0">
          <span className="inline-flex items-center gap-1 h-[22px] px-2 rounded text-white text-xs font-semibold num shrink-0 max-w-[60%]" style={{ background: color }}>
            {vehicleIcon(leg, 12)}
            <span className="truncate">{tr.routeShortName || tr.routeLongName}</span>
          </span>
          <span className="text-sm text-muted truncate">{tr.headsign || tr.agency}</span>
          <button
            type="button"
            className={clsx("btn btn-ghost btn-icon btn-sm shrink-0 ms-auto -me-1 no-print", savedLine ? "text-warning" : "text-subtle hover:text-fg")}
            title={savedLine ? t("favoriteRemove") : t("favoriteLine")}
            aria-label={savedLine ? t("favoriteRemove") : t("favoriteLine")}
            aria-pressed={!!savedLine}
            onClick={() => void useStore.getState().toggleLineFavorite(lineRef, [tr.stops[0]?.lon ?? leg.geometry[0][0], tr.stops[0]?.lat ?? leg.geometry[0][1]])}
          >
            <Star size={14} strokeWidth={1.75} fill={savedLine ? "currentColor" : "none"} />
          </button>
        </div>
        {tr.agency && (
          <div className="flex items-center gap-1.5 text-xs text-subtle -mt-1 min-w-0">
            <OperatorLogo agency={tr.agency} />
            {tr.headsign && <span className="truncate">{tr.agency}</span>}
          </div>
        )}
        <button
          className="text-sm text-muted hover:text-fg flex items-center gap-1 self-start rounded -mx-1 px-1 h-7 transition-colors num"
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
        >
          <ChevronDown size={14} className={clsx("transition-transform duration-150", open && "rotate-180")} />
          {tr.numStops} {t("stops")} · {fmtDuration(leg.durationSec, t)}
        </button>
        {open && mid.length > 0 && (
          <ul className="m-0 p-0 list-none text-sm text-muted flex flex-col fade-in">
            {mid.map((s, i) => (
              <li
                key={i}
                className="flex justify-between gap-2 h-7 items-center cursor-pointer hover:text-fg rounded -mx-1 px-1 hover:bg-hover"
                onClick={() => flyTo([s.lon, s.lat], 16)}
              >
                <span className="truncate">
                  {s.name}
                  <StopCode code={s.code} />
                </span>
                <span className="num text-xs">{fmtTime(s.timeIso)}</span>
              </li>
            ))}
          </ul>
        )}
        <button className="text-start min-w-0 pb-1" onClick={() => { const l = tr.stops[tr.stops.length - 1]; if (l) flyTo([l.lon, l.lat], 16); }}>
          <div className="text-base font-semibold truncate-dir">
            {tr.toStop}
            <StopCode code={tr.stops[tr.stops.length - 1]?.code} />
          </div>
        </button>
      </div>
    </div>
  );
}

function WalkLeg({ route, leg, legIndex }: { route: Route; leg: Leg; legIndex: number }) {
  const t = useT();
  const units = useStore((s) => s.settings.units);
  const setHighlight = useStore((s) => s.setHighlight);
  const [open, setOpen] = useState(false);
  const hasSteps = (leg.steps?.length ?? 0) > 1;
  return (
    <div className="flex gap-3" onMouseEnter={() => setHighlight({ routeId: route.id, legIndex })} onMouseLeave={() => setHighlight(null)}>
      <div className="w-11 shrink-0" />
      <div className="w-3 shrink-0 flex flex-col items-center">
        <span className="flex-1 w-0 border-s-[3px] border-dotted border-[var(--fg-subtle)]" />
      </div>
      <div className="flex-1 min-w-0 py-2">
        <button
          className={clsx("flex items-center gap-2 text-sm text-muted num rounded -mx-1 px-1 h-7 transition-colors", hasSteps && "hover:text-fg")}
          onClick={() => hasSteps && setOpen((v) => !v)}
          aria-expanded={hasSteps ? open : undefined}
        >
          <Footprints size={14} />
          <span>
            {t("walking")} {fmtDistance(leg.distanceM, t, units)} · {fmtDuration(leg.durationSec, t)}
          </span>
          {hasSteps && <ChevronDown size={14} className={clsx("transition-transform duration-150", open && "rotate-180")} />}
        </button>
        {open && hasSteps && (
          <div className="fade-in mt-1 rounded border border-line overflow-hidden [&_li+li]:border-t [&_li+li]:border-line">
            <StepsList route={route} leg={leg} legIndex={legIndex} />
          </div>
        )}
      </div>
    </div>
  );
}

/** Walking time over the profile by Tobler's hiking function: 6·e^(−3.5·|slope + 0.05|) km/h on
 *  each sample step (the samples are spread evenly along the route, so every step is
 *  distance / (n − 1) long). Water: 0.5 l per walking hour, 0.75 l from June to September,
 *  rounded up to half a litre, at least 0.5 l. */
export function toblerWalk(elev: number[], distanceM: number, month = new Date().getMonth()): { sec: number; litres: number } {
  const n = elev.length;
  if (n < 2 || distanceM <= 0) return { sec: 0, litres: 0 };
  const step = distanceM / (n - 1);
  let sec = 0;
  for (let i = 1; i < n; i++) {
    const slope = (elev[i] - elev[i - 1]) / step;
    const kmh = 6 * Math.exp(-3.5 * Math.abs(slope + 0.05));
    sec += (step / 1000 / Math.max(kmh, 0.5)) * 3600;
  }
  const perHour = month >= 5 && month <= 8 ? 0.75 : 0.5;
  const litres = Math.max(0.5, Math.ceil(((sec / 3600) * perHour) / 0.5) * 0.5);
  return { sec, litres };
}

/** Sample the route every ~50 m (max 400 points) for the elevation profile. */
function sampleRoute(geometry: [number, number][], max = 400): [number, number][] {
  if (geometry.length <= max) return geometry;
  const step = (geometry.length - 1) / (max - 1);
  return Array.from({ length: max }, (_, i) => geometry[Math.round(i * step)]);
}

function ElevationProfile({ route }: { route: Route }) {
  const t = useT();
  const terrainUrl = useStore((s) => s.terrainUrl);
  const [data, setData] = useState<{ id: string; elev: number[] } | null>(null);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!terrainUrl) return;
    let alive = true;
    const pts = sampleRoute(route.geometry);
    api
      .elevationProfile(pts)
      .then((e) => {
        if (!alive) return;
        const elev = e.map((v) => v ?? NaN).filter((v) => !Number.isNaN(v));
        if (elev.length >= 2) setData({ id: route.id, elev });
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [route.id, route.geometry, terrainUrl]);
  if (!terrainUrl || !data || data.id !== route.id) return null;
  const e = data.elev;
  let up = 0;
  let down = 0;
  for (let i = 1; i < e.length; i++) {
    const d = e[i] - e[i - 1];
    if (d > 0) up += d;
    else down -= d;
  }
  const min = Math.min(...e);
  const max = Math.max(...e);
  const span = Math.max(max - min, 10);
  const W = 300;
  const H = 64;
  const pts = e.map((v, i) => `${((i / (e.length - 1)) * W).toFixed(1)},${(H - 4 - ((v - min) / span) * (H - 10)).toFixed(1)}`);
  return (
    <div className="card overflow-hidden">
      <button type="button" className="w-full flex items-center justify-between gap-2 px-3 h-10 text-start hover:bg-hover transition-colors" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        <span className="text-sm font-medium flex items-center gap-1.5">
          <ChevronDown size={14} className={clsx("transition-transform duration-150", open && "rotate-180")} />
          {t("elevationProfile")}
        </span>
        <span className="text-xs text-muted num shrink-0">
          ↑ {Math.round(up)} {t("metersShort")} · ↓ {Math.round(down)} {t("metersShort")}
        </span>
      </button>
      {open && (
        <div className="px-3 pb-3 flex flex-col gap-1 fade-in">
          <div className="text-xs text-muted num">
            {t("elevationRange")}: {Math.round(min)}–{Math.round(max)} {t("metersShort")}
          </div>
          <svg viewBox={`0 0 ${W} ${H}`} className="w-full h-16 block" preserveAspectRatio="none" aria-hidden>
            <polygon points={`0,${H} ${pts.join(" ")} ${W},${H}`} fill="var(--accent-soft)" />
            <polyline points={pts.join(" ")} fill="none" stroke="var(--accent)" strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
          </svg>
          {route.mode === "foot" &&
            (() => {
              const w = toblerWalk(e, route.distanceM);
              return (
                <div className="flex flex-col gap-0.5 pt-1 text-sm">
                  <span className="num">{t("toblerTime").replace("{t}", fmtDuration(w.sec, t))}</span>
                  <span className="num">{t("waterHint").replace("{x}", String(w.litres))}</span>
                  <p className="m-0 text-xs text-subtle">{t("walkingHintNote")}</p>
                </div>
              );
            })()}
        </div>
      )}
    </div>
  );
}

/** Ask where to save, write the file, and say where it went. */
async function exportRoute(fileName: string, ext: "gpx" | "kml", content: string, t: ReturnType<typeof useT>) {
  const st = useStore.getState();
  try {
    const path = await saveDialog({ defaultPath: fileName, filters: [{ name: ext.toUpperCase(), extensions: [ext] }] });
    if (!path) return; // cancelled
    await api.saveTextFile(path, content);
    st.toast("success", `${t("exportSaved")} ${path}`);
  } catch (e) {
    st.toastError(e);
  }
}

export function RouteDetails({ route, embedded = false }: { route: Route; embedded?: boolean }) {
  const t = useT();
  const lang = useStore((s) => s.settings.language);
  const waypoints = useStore((s) => s.waypoints);
  const name = `${waypoints[0]?.label ?? "A"} ${routeArrow(lang)} ${waypoints[waypoints.length - 1]?.label ?? "B"}`;
  const safe = name.replace(/[\\/:*?"<>|]/g, "_").slice(0, 60);

  return (
    <div className={clsx("flex flex-col gap-3", embedded ? "pt-1" : "fade-in pt-2")}>
      {!embedded && <h2 className="section-label m-0">{t("steps")}</h2>}
      <div className={clsx(embedded ? "-mx-3" : "card overflow-hidden", route.mode !== "transit" && "[&_li+li]:border-t [&_li+li]:border-line")}>
        {route.mode === "transit" ? (
          <div className="flex flex-col p-3">
            {route.legs.map((leg, i) =>
              leg.transit ? <TransitLeg key={i} route={route} leg={leg} legIndex={i} /> : <WalkLeg key={i} route={route} leg={leg} legIndex={i} />,
            )}
          </div>
        ) : (
          route.legs.map((leg, i) => (
            <div key={i}>
              {route.legs.length > 1 && (
                <div className={clsx("px-3 py-1.5 text-xs font-semibold text-muted bg-surface-2 num", i > 0 && "border-t border-line")}>{`${i + 1}/${route.legs.length}`}</div>
              )}
              <StepsList route={route} leg={leg} legIndex={i} />
            </div>
          ))
        )}
      </div>
      <ElevationProfile route={route} />
      <TransitDelays route={route} />
      {route.mode !== "transit" && (
        <button className="btn btn-primary w-full no-print" onClick={() => void useStore.getState().startNavigation(route.id)}>
          <Navigation size={16} strokeWidth={2} className="rtl:-scale-x-100" /> {t("navStart")}
        </button>
      )}
      <div className="flex flex-wrap gap-1.5 no-print">
        <button className="btn btn-sm" onClick={() => void exportRoute(`${safe}.gpx`, "gpx", routeToGpx(route, name), t)}>
          <Download size={14} strokeWidth={1.75} /> {t("exportGpx")}
        </button>
        <button className="btn btn-sm" onClick={() => void exportRoute(`${safe}.kml`, "kml", routeToKml(route, name), t)}>
          <Download size={14} strokeWidth={1.75} /> {t("exportKml")}
        </button>
        <button
          className="btn btn-sm"
          onClick={async () => {
            if (await copyText(routeToText(route, name, lang))) useStore.getState().toast("success", t("copied"));
          }}
        >
          <Copy size={14} strokeWidth={1.75} /> {t("copyText")}
        </button>
        <button className="btn btn-sm" onClick={() => void printPage()}>
          <Printer size={14} strokeWidth={1.75} /> {t("print")}
        </button>
      </div>
    </div>
  );
}

/** Operator logos shipped in public/operators (only those with a free licence, see CREDITS.md). */
let logoIndex: Promise<Record<string, string>> | null = null;
function OperatorLogo({ agency }: { agency: string }) {
  const [file, setFile] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    logoIndex ??= fetch("/operators/index.json")
      .then((r) => (r.ok ? r.json() : {}))
      .catch(() => ({}));
    void logoIndex.then((idx) => alive && setFile(idx[agency] ?? null));
    return () => {
      alive = false;
    };
  }, [agency]);
  if (!file) return null;
  return <img src={`/operators/${file}`} alt={agency} title={agency} className="h-4 max-w-16 object-contain shrink-0" />;
}

/** How late each line of a transit route usually is at its boarding stop (Open Bus Stride,
 *  online, slow: fetched after the route is shown; nothing offline or when switched off). */
function TransitDelays({ route }: { route: Route }) {
  const t = useT();
  const enabled = useStore((s) => s.settings.transitReliability ?? true);
  const [delays, setDelays] = useState<{ line: string; min: number }[] | null>(null);
  useEffect(() => {
    setDelays(null);
    if (route.mode !== "transit" || !enabled || !navigator.onLine) return;
    const legs = route.legs.filter((l) => l.transit && l.transit.stops.length > 0 && l.transit.departIso);
    if (!legs.length) return;
    let alive = true;
    api
      .transitDelays(legs.map((l) => ({ shortName: l.transit!.routeShortName, agency: l.transit!.agency, lon: l.transit!.stops[0].lon, lat: l.transit!.stops[0].lat, departIso: l.transit!.departIso })))
      .then((v) => {
        if (!alive) return;
        const out = legs.flatMap((l, i) => (v[i] != null && Math.abs(v[i]!) >= 1 ? [{ line: l.transit!.routeShortName || l.transit!.routeLongName, min: Math.round(v[i]!) }] : []));
        setDelays(out);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [route, enabled]);
  if (!delays || delays.length === 0) return null;
  return (
    <div className="card p-3 flex flex-col gap-1 text-sm">
      {delays.map((d, i) => (
        <div key={i} className={d.min > 0 ? "text-warning" : "text-success"}>
          {t("lineDelay").replace("{line}", d.line).replace("{n}", String(Math.abs(d.min))).replace("{dir}", d.min > 0 ? t("late") : t("early"))}
        </div>
      ))}
      <p className="m-0 text-xs text-subtle">{t("strideCredit")}</p>
    </div>
  );
}
