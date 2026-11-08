import { SIM_SPEEDS, SIM_WALK_SPEEDS, isWalkMode } from "../lib/nav";
import { useEffect, useRef, useState } from "react";
import {
  ArrowUp,
  Ban,
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
  MoreHorizontal,
  MoveUpLeft,
  MoveUpRight,
  Printer,
  Navigation,
  RotateCcw,
  Share2,
  Undo2,
  CornerLeftDown,
  CornerRightDown,
  Car,
  BellRing,
} from "lucide-react";
import { startTransitAlert, stopTransitAlert } from "../lib/transitAlert";
import clsx from "clsx";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { findLineFavorite, useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import type { Leg, Maneuver, Route, Step } from "../lib/api";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { copyText, fmtDistance, fmtDuration, fmtTime, routeArrow, routeToGpx, routeToKml, routeToText } from "../lib/format";
import { api } from "../lib/api";
import { useMenuKeys } from "../lib/useDialog";
import { legColorHex, vehicleIcon } from "./RouteResults";
import { InfoTip } from "./InfoTip";

function maneuverIcon(m: Maneuver, size = 16) {
  switch (m) {
    case "depart":
      return <Icon as={Circle} size={size} />;
    case "arrive":
      return <Icon as={Flag} size={size} />;
    case "turn-left":
      return <Icon as={CornerUpLeft} size={size} />;
    case "turn-right":
      return <Icon as={CornerUpRight} size={size} />;
    case "slight-left":
      return <Icon as={MoveUpLeft} size={size} />;
    case "slight-right":
      return <Icon as={MoveUpRight} size={size} />;
    // sharp turns bend back down; the U-shaped arrow is the U-turn (it read as "undo" before)
    case "sharp-left":
      return <Icon as={CornerLeftDown} size={size} />;
    case "sharp-right":
      return <Icon as={CornerRightDown} size={size} />;
    case "uturn":
      return <Icon as={Undo2} size={size} />;
    case "roundabout":
    case "exit-roundabout":
      return <Icon as={RotateCcw} size={size} />;
    case "merge":
      return <Icon as={Merge} size={size} />;
    case "fork":
      return <Icon as={GitFork} size={size} />;
    default:
      return <Icon as={ArrowUp} size={size} />;
  }
}

/** A step as listed: a roundabout entry and its exit are one line ("היכנס לכיכר וצא ביציאה
 *  השנייה לכביש 45", distance to the next manoeuvre after the exit). */
export interface ShownStep {
  step: Step;
  /** index of the first original step (map highlight, fly-to) */
  index: number;
  /** original steps it stands for */
  count: number;
  distanceM: number;
}

export function mergeSteps(steps: Step[]): ShownStep[] {
  const out: ShownStep[] = [];
  for (let i = 0; i < steps.length; i++) {
    const s = steps[i];
    const next = steps[i + 1];
    if (s.maneuver === "roundabout" && next?.maneuver === "exit-roundabout") {
      out.push({ step: { ...s, street: s.street ?? next.street, ref: s.ref ?? next.ref }, index: i, count: 2, distanceM: s.distanceM + next.distanceM });
      i++;
    } else out.push({ step: s, index: i, count: 1, distanceM: s.distanceM });
  }
  return out;
}

/** "יציאה 5 · לכיוון ירושלים · כביש 443" under an instruction. */
function stepExtras(s: Step, t: ReturnType<typeof useT>): string {
  return [s.exit ? t("stepExit").replace("{n}", s.exit) : "", s.destination ? t("stepTowards").replace("{d}", s.destination) : "", s.ref && !s.instruction.includes(s.ref) ? t("stepRoad").replace("{n}", s.ref) : ""]
    .filter(Boolean)
    .join(" · ");
}

/** A point inside a step's stretch of road (between its start and the next step's start). */
function stepPoint(leg: Leg, steps: Step[], i: number, count: number): [number, number] | null {
  const a = steps[i].geometryStart;
  const b = steps[i + count]?.geometryStart ?? leg.geometry.length - 1;
  const mid = Math.min(leg.geometry.length - 1, Math.max(a, Math.round((a + b) / 2)));
  return leg.geometry[mid] ?? null;
}

function StepsList({ route, leg, legIndex }: { route: Route; leg: Leg; legIndex: number }) {
  const t = useT();
  const units = useStore((s) => s.settings.units);
  const setHighlight = useStore((s) => s.setHighlight);
  const flyTo = useStore((s) => s.flyToPoint);
  const steps = leg.steps ?? [];
  const canAvoid = route.mode !== "transit";
  if (!steps.length) {
    return (
      <div className="text-muted px-3 py-2 num">
        {fmtDistance(leg.distanceM, t, units)} · {fmtDuration(leg.durationSec, t)}
      </div>
    );
  }
  const shown = mergeSteps(steps);
  const fly = (s: Step) => {
    const p = leg.geometry[Math.min(s.geometryStart, leg.geometry.length - 1)];
    if (p) flyTo(p, 16);
  };
  return (
    <ol className="flex flex-col m-0 p-0 list-none">
      {shown.map(({ step: s, index, count, distanceM }) => {
        const extras = stepExtras(s, t);
        const avoidable = canAvoid && s.maneuver !== "arrive" && s.maneuver !== "depart" && distanceM > 30;
        return (
          <li
            key={index}
            tabIndex={0}
            className="group flex items-center gap-3 px-3 min-h-[48px] cursor-pointer transition-colors duration-150 hover:bg-hover focus-visible:bg-hover"
            onMouseEnter={() => setHighlight({ routeId: route.id, legIndex, stepIndex: index, stepCount: count })}
            onMouseLeave={() => setHighlight(null)}
            onFocus={() => setHighlight({ routeId: route.id, legIndex, stepIndex: index, stepCount: count })}
            onClick={() => fly(s)}
            onKeyDown={(e) => {
              if (e.key === "Enter") fly(s);
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
            <span className="flex-1 min-w-0 py-2.5">
              <span className="block">{s.instruction}</span>
              {extras && <span className="block text-xs text-muted">{extras}</span>}
            </span>
            {s.speedLimitKmh != null && (
              <span className="shrink-0 w-7 h-7 rounded-full border-[2.5px] border-[#d92626] bg-white text-black text-xs font-bold num flex items-center justify-center" aria-label={t("speedLimit").replace("{n}", String(s.speedLimitKmh))}>
                {s.speedLimitKmh}
              </span>
            )}
            <span className="shrink-0 text-muted num">{distanceM > 0 && s.maneuver !== "arrive" ? fmtDistance(distanceM, t, units) : ""}</span>
            {avoidable && (
              <Tip text={t("avoidThisRoad")}>
                <button
                  type="button"
                  className="btn btn-ghost btn-icon btn-sm text-subtle hover:text-danger shrink-0 -me-1.5 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 no-print"
                  aria-label={`${t("avoidThisRoad")}: ${s.street ?? s.instruction}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    const p = stepPoint(leg, steps, index, count);
                    if (p) useStore.getState().avoidRoadAt(p);
                  }}
                >
                  <Icon as={Ban} size={14} />
                </button>
              </Tip>
            )}
          </li>
        );
      })}
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
      <div className="w-11 shrink-0 flex flex-col font-semibold num">
        <span className="leading-5">
          <span className="sr-only">{t("depart")} </span>
          {fmtTime(tr.departIso)}
        </span>
        <span className="flex-1" />
        <span className="leading-5">
          <span className="sr-only">{t("arrive")} </span>
          {fmtTime(tr.arriveIso)}
        </span>
      </div>
      <div className="w-3 shrink-0 flex flex-col items-center pt-1" aria-hidden>
        <span className="w-3 h-3 shrink-0 rounded-full border-[3px] bg-surface-solid" style={{ borderColor: color }} />
        <span className="flex-1 w-1 -my-px" style={{ background: color }} />
        <span className="w-3 h-3 shrink-0 rounded-full border-[3px] bg-surface-solid mb-1" style={{ borderColor: color }} />
      </div>
      <div className="flex-1 min-w-0 flex flex-col gap-1.5">
        <button className="text-start min-w-0" onClick={() => flyTo([tr.stops[0]?.lon ?? leg.geometry[0][0], tr.stops[0]?.lat ?? leg.geometry[0][1]], 16)}>
          <div className="font-semibold truncate-dir">
            {tr.fromStop}
            <StopCode code={tr.stops[0]?.code} />
          </div>
        </button>
        <div className="flex items-center gap-2 min-w-0">
          <span className="inline-flex items-center gap-1 h-[22px] px-2 rounded text-white text-xs font-semibold num shrink-0 max-w-[60%]" style={{ background: color }}>
            {vehicleIcon(leg, 12)}
            <span className="truncate-dir">{tr.routeShortName || tr.routeLongName}</span>
          </span>
          <span className="text-muted truncate-dir">{tr.headsign || tr.agency}</span>
          <Tip text={savedLine ? t("favoriteRemove") : t("favoriteLine")}>
            <button
              type="button"
              className={clsx("btn btn-ghost btn-icon btn-sm shrink-0 ms-auto -me-1 no-print", savedLine ? "text-warning" : "text-subtle hover:text-fg")}
              aria-label={savedLine ? t("favoriteRemove") : t("favoriteLine")}
              aria-pressed={!!savedLine}
              onClick={() => void useStore.getState().toggleLineFavorite(lineRef, [tr.stops[0]?.lon ?? leg.geometry[0][0], tr.stops[0]?.lat ?? leg.geometry[0][1]])}
            >
              <Icon as={Star} size={14} fill={savedLine ? "currentColor" : "none"} />
            </button>
          </Tip>
        </div>
        {tr.agency && (
          <div className="flex items-center gap-1.5 text-xs text-subtle -mt-1 min-w-0">
            <OperatorLogo agency={tr.agency} />
            {tr.headsign && <span className="truncate-dir">{tr.agency}</span>}
          </div>
        )}
        <button className="text-muted hover:text-fg flex items-center gap-1 self-start rounded -mx-1 px-1 h-7 transition-colors num" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
          <Icon as={ChevronDown} size={14} className={clsx("transition-transform duration-150", open && "rotate-180")} />
          {tr.numStops} {t("stops")} · {fmtDuration(leg.durationSec, t)}
        </button>
        {open && mid.length > 0 && (
          <ul className="m-0 p-0 list-none text-muted flex flex-col fade-in">
            {mid.map((s, i) => (
              <li key={i} className="flex justify-between gap-2 h-7 items-center cursor-pointer hover:text-fg rounded -mx-1 px-1 hover:bg-hover" onClick={() => flyTo([s.lon, s.lat], 16)}>
                <span className="truncate-dir">
                  {s.name}
                  <StopCode code={s.code} />
                </span>
                <span className="num text-xs">{fmtTime(s.timeIso)}</span>
              </li>
            ))}
          </ul>
        )}
        <button
          className="text-start min-w-0 pb-1"
          onClick={() => {
            const l = tr.stops[tr.stops.length - 1];
            if (l) flyTo([l.lon, l.lat], 16);
          }}
        >
          <div className="font-semibold truncate-dir">
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
      <div className="w-3 shrink-0 flex flex-col items-center" aria-hidden>
        <span className="flex-1 w-0 border-s-[3px] border-dotted border-[var(--fg-subtle)]" />
      </div>
      <div className="flex-1 min-w-0 py-2">
        <button
          className={clsx("flex items-center gap-2 text-muted num rounded -mx-1 px-1 h-7 transition-colors", hasSteps && "hover:text-fg")}
          onClick={() => hasSteps && setOpen((v) => !v)}
          aria-expanded={hasSteps ? open : undefined}
        >
          <Icon as={Footprints} size={14} />
          <span>
            {t("walking")} {fmtDistance(leg.distanceM, t, units)} · {fmtDuration(leg.durationSec, t)}
          </span>
          {hasSteps && <Icon as={ChevronDown} size={14} className={clsx("transition-transform duration-150", open && "rotate-180")} />}
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

/** Round axis ticks ("nice numbers") between lo and hi, about `count` of them. */
export function niceTicks(lo: number, hi: number, count = 4): number[] {
  const span = hi - lo;
  if (!(span > 0)) return [lo];
  const raw = span / count;
  const mag = 10 ** Math.floor(Math.log10(raw));
  // the "nice" step (1, 2, 2.5, 5 × 10^n) closest to the raw one
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).reduce((best, s) => (Math.abs(s - raw) < Math.abs(best - raw) ? s : best));
  const out: number[] = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9; v += step) out.push(Math.round(v * 1000) / 1000);
  return out;
}

function ElevationProfile({ route }: { route: Route }) {
  const t = useT();
  const terrainUrl = useStore((s) => s.terrainUrl);
  const units = useStore((s) => s.settings.units);
  const [data, setData] = useState<{ id: string; elev: number[]; pts: [number, number][] } | null>(null);
  const [open, setOpen] = useState(false);
  const [hover, setHover] = useState<number | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  useEffect(() => {
    if (!terrainUrl) return;
    let alive = true;
    const pts = sampleRoute(route.geometry);
    api
      .elevationProfile(pts)
      .then((e) => {
        if (!alive) return;
        const keep = e.map((v, i) => [v, i] as const).filter(([v]) => v != null && !Number.isNaN(v));
        if (keep.length >= 2) setData({ id: route.id, elev: keep.map(([v]) => v as number), pts: keep.map(([, i]) => pts[i]) });
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
  // chart area inside the viewBox: room for the height labels at the start, distance below
  const W = 320;
  const H = 110;
  const L = 34;
  const B = 18;
  const T = 6;
  const x = (i: number) => L + (i / (e.length - 1)) * (W - L - 4);
  const y = (v: number) => H - B - ((v - min) / span) * (H - B - T);
  const pts = e.map((v, i) => `${x(i).toFixed(1)},${y(v).toFixed(1)}`);
  const km = route.distanceM / 1000;
  const yTicks = niceTicks(min, min + span, 3);
  const xTicks = niceTicks(0, km, 4).filter((k) => k <= km);
  const onMove = (ev: React.MouseEvent<SVGSVGElement>) => {
    const svg = svgRef.current;
    if (!svg) return;
    const r = svg.getBoundingClientRect();
    const fx = ((ev.clientX - r.left) / r.width) * W;
    const i = Math.round(((fx - L) / (W - L - 4)) * (e.length - 1));
    if (i < 0 || i >= e.length) return;
    setHover(i);
    // legIndex -1: a point only (no leg / step stretch); the map draws `point` as a dot
    useStore.getState().setHighlight({ routeId: route.id, legIndex: -1, point: data.pts[i] });
  };
  const leave = () => {
    setHover(null);
    useStore.getState().setHighlight(null);
  };
  return (
    <div className="card overflow-hidden">
      <button type="button" className="w-full flex items-center justify-between gap-2 px-3 h-10 text-start hover:bg-hover transition-colors" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        <span className="font-medium flex items-center gap-1.5">
          <Icon as={ChevronDown} size={14} className={clsx("transition-transform duration-150", open && "rotate-180")} />
          {t("elevationProfile")}
        </span>
        <span className="text-xs text-muted num shrink-0">
          ↑ {Math.round(up)} {t("metersShort")} · ↓ {Math.round(down)} {t("metersShort")}
        </span>
      </button>
      {open && (
        <div className="px-3 pb-3 flex flex-col gap-1 fade-in">
          <div className="text-xs text-muted num flex justify-between">
            <span>
              {t("elevationRange")}: {Math.round(min)}–{Math.round(max)} {t("metersShort")}
            </span>
            {hover != null && (
              <span className="text-fg" aria-live="polite">
                {fmtDistance((hover / (e.length - 1)) * route.distanceM, t, units)} · {Math.round(e[hover])} {t("metersShort")}
              </span>
            )}
          </div>
          {/* the chart reads left-to-right (distance) in both languages */}
          <svg ref={svgRef} viewBox={`0 0 ${W} ${H}`} className="w-full h-28 block" style={{ direction: "ltr" }} onMouseMove={onMove} onMouseLeave={leave} role="img" aria-label={`${t("elevationProfile")}: ${Math.round(min)}–${Math.round(max)} ${t("metersShort")}`}>
            {yTicks.map((v) => (
              <g key={`y${v}`}>
                <line x1={L} x2={W - 4} y1={y(v)} y2={y(v)} stroke="var(--border)" strokeWidth="1" />
                <text x={L - 4} y={y(v) + 3} textAnchor="end" fontSize="9" fill="var(--fg-subtle)">
                  {Math.round(v)}
                </text>
              </g>
            ))}
            {xTicks.map((k) => (
              <text key={`x${k}`} x={L + (k / Math.max(km, 1e-6)) * (W - L - 4)} y={H - 4} textAnchor="middle" fontSize="9" fill="var(--fg-subtle)">
                {k}
                {k === xTicks[xTicks.length - 1] ? ` ${t("km")}` : ""}
              </text>
            ))}
            <polygon points={`${L},${H - B} ${pts.join(" ")} ${W - 4},${H - B}`} fill="var(--accent-soft)" />
            <polyline points={pts.join(" ")} fill="none" stroke="var(--accent)" strokeWidth="1.5" />
            {hover != null && (
              <g>
                <line x1={x(hover)} x2={x(hover)} y1={T} y2={H - B} stroke="var(--fg-muted)" strokeWidth="1" strokeDasharray="2 2" />
                <circle cx={x(hover)} cy={y(e[hover])} r="3.5" fill="var(--accent)" stroke="var(--surface-solid)" strokeWidth="1.5" />
              </g>
            )}
          </svg>
          {route.mode === "foot" &&
            (() => {
              const w = toblerWalk(e, route.distanceM);
              return (
                <div className="flex flex-col gap-0.5 pt-1">
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

/** The "…" menu of the route command bar. */
function MoreMenu({ items, onClose }: { items: { label: string; icon: typeof Copy; run: () => void }[]; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useMenuKeys(ref);
  useEffect(() => {
    const down = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    document.addEventListener("pointerdown", down, true);
    document.addEventListener("keydown", key, true);
    return () => {
      document.removeEventListener("pointerdown", down, true);
      document.removeEventListener("keydown", key, true);
    };
  }, [onClose]);
  return (
    <div ref={ref} role="menu" className="absolute top-full mt-1 end-0 z-30 popover rounded-[var(--radius)] py-1 min-w-48 fade-in">
      {items.map((it) => (
        <button
          key={it.label}
          role="menuitem"
          className="w-full flex items-center gap-2.5 px-3 h-9 text-start hover:bg-hover focus-visible:bg-hover"
          onClick={() => {
            it.run();
            onClose();
          }}
        >
          <Icon as={it.icon} className="text-muted" />
          {it.label}
        </button>
      ))}
    </div>
  );
}

export function RouteDetails({ route, embedded = false }: { route: Route; embedded?: boolean }) {
  const t = useT();
  const lang = useStore((s) => s.settings.language);
  const waypoints = useStore((s) => s.waypoints);
  const [more, setMore] = useState(false);
  const [simPick, setSimPick] = useState(false);
  const walk = isWalkMode(route.mode);
  const simSaved = useStore((s) => (walk ? s.settings.navSimWalkKmh : s.settings.navSimSpeedKmh));
  const name = `${waypoints[0]?.label ?? t("from")} ${routeArrow(lang)} ${waypoints[waypoints.length - 1]?.label ?? t("to")}`;
  const safe = name.replace(/[\\/:*?"<>|]/g, "_").slice(0, 60);
  const st = useStore.getState;

  const moreItems = [
    { label: t("exportKml"), icon: Download, run: () => void exportRoute(`${safe}.kml`, "kml", routeToKml(route, name), t) },
    {
      label: t("copyText"),
      icon: Copy,
      run: async () => {
        if (await copyText(routeToText(route, name, lang))) st().toast("success", t("copied"));
      },
    },
    { label: t("saveRoute"), icon: Star, run: () => void st().saveRouteFavorite() },
  ];

  return (
    <div className={clsx("flex flex-col gap-3", embedded ? "pt-1" : "fade-in pt-2")}>
      {/* command bar (WinUI CommandBar) first: "start navigation" must be at hand, not after a long step list */}
      {route.mode !== "transit" && (
        <div className="flex items-center gap-2 no-print">
          <button className="btn btn-primary flex-1" onClick={() => void st().startNavigation(route.id)}>
            <Icon as={Navigation} className="rtl:-scale-x-100" /> {t("navStart")}
          </button>
          <button className={clsx("btn", simPick && "btn-primary")} aria-expanded={simPick} onClick={() => setSimPick((v) => !v)}>
            <Icon as={Car} /> {t("navSimulate")}
          </button>
        </div>
      )}
      {route.mode === "transit" && <TransitAlertButton route={route} />}
      <div className="flex items-center gap-1 no-print relative" role="toolbar" aria-label={t("routeActions")}>
        <button className="btn btn-ghost btn-sm flex-1" onClick={() => void exportRoute(`${safe}.gpx`, "gpx", routeToGpx(route, name), t)}>
          <Icon as={Download} size={14} /> GPX
        </button>
        <button className="btn btn-ghost btn-sm flex-1" onClick={() => st().requestPrint()}>
          <Icon as={Printer} size={14} /> {t("print")}
        </button>
        <button className="btn btn-ghost btn-sm flex-1" onClick={() => st().setShare({ kind: "route", routeId: route.id })}>
          <Icon as={Share2} size={14} /> {t("share")}
        </button>
        <Tip text={t("moreActions")}>
          <button className="btn btn-ghost btn-icon btn-sm" aria-label={t("moreActions")} aria-haspopup="menu" aria-expanded={more} onClick={() => setMore((v) => !v)}>
            <Icon as={MoreHorizontal} />
          </button>
        </Tip>
        {more && <MoreMenu items={moreItems} onClose={() => setMore(false)} />}
      </div>
      {simPick && route.mode !== "transit" && (
        <div className="card p-3 flex flex-col gap-2 no-print fade-in" role="group" aria-label={t("simSpeedTitle")}>
          <div className="flex items-center justify-between gap-2">
            <span className="font-medium inline-flex items-center gap-1.5">
              {t("simSpeedTitle")}
              {!walk && <InfoTip text={t("simSpeedHint")} />}
            </span>
            <button className="btn btn-ghost btn-sm" onClick={() => setSimPick(false)}>
              {t("cancel")}
            </button>
          </div>
          <div className="flex flex-wrap gap-1.5">
            {(walk ? SIM_WALK_SPEEDS : SIM_SPEEDS).map((v) => (
              <button
                key={v}
                className={clsx("btn btn-sm", (simSaved ?? -1) === v && "btn-primary")}
                onClick={() => {
                  setSimPick(false);
                  void st().updateSettings(walk ? { navSimWalkKmh: v } : { navSimSpeedKmh: v });
                  void st().startNavigation(route.id, true, v);
                }}
              >
                {v === 0 ? t("simSpeedRoad") : <bdi className="num">{`${v} ${t("kmh")}`}</bdi>}
              </button>
            ))}
          </div>
        </div>
      )}
      {!embedded && <h2 className="section-label m-0">{t("steps")}</h2>}
      <div className={clsx(embedded ? "-mx-3" : "card overflow-hidden", route.mode !== "transit" && "[&_li+li]:border-t [&_li+li]:border-line")}>
        {route.mode === "transit" ? (
          <div className="flex flex-col p-3">
            {route.legs.map((leg, i) => (leg.transit ? <TransitLeg key={i} route={route} leg={leg} legIndex={i} /> : <WalkLeg key={i} route={route} leg={leg} legIndex={i} />))}
          </div>
        ) : (
          route.legs.map((leg, i) => (
            <div key={i}>
              {route.legs.length > 1 && <div className={clsx("px-3 py-1.5 text-xs font-semibold text-muted bg-surface-2 num", i > 0 && "border-t border-line")}>{`${i + 1}/${route.legs.length}`}</div>}
              <StepsList route={route} leg={leg} legIndex={i} />
            </div>
          ))
        )}
      </div>
      <ElevationProfile route={route} />
      <TransitDelays route={route} />
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
  return <img src={`/operators/${file}`} alt={agency} className="h-4 max-w-16 object-contain shrink-0" />;
}

/** How late each line of a transit route usually is at its boarding stop (Open Bus Stride,
 *  online, slow: fetched after the route is shown; nothing offline or when switched off). */
function TransitDelays({ route }: { route: Route }) {
  const t = useT();
  const enabled = useStore((s) => s.settings.transitReliability ?? true);
  const online = useStore((s) => s.online);
  const [delays, setDelays] = useState<{ line: string; min: number }[] | null>(null);
  useEffect(() => {
    setDelays(null);
    if (route.mode !== "transit" || !enabled || !online) return;
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
  }, [route, enabled, online]);
  if (!delays || delays.length === 0) return null;
  return (
    <div className="card p-3 flex flex-col gap-1">
      {delays.map((d, i) => (
        <div key={i} className={d.min > 0 ? "text-warning" : "text-success"}>
          {t("lineDelay").replace("{line}", d.line).replace("{n}", String(Math.abs(d.min))).replace("{dir}", d.min > 0 ? t("late") : t("early"))}
        </div>
      ))}
      <p className="m-0 text-xs text-subtle">{t("strideCredit")}</p>
    </div>
  );
}

/** "Wake me before my stop" for a public-transport route (lib/transitAlert.ts). */
function TransitAlertButton({ route }: { route: Route }) {
  const t = useT();
  const active = useStore((s) => s.transitAlert?.routeId === route.id);
  return (
    <div className="flex items-center gap-2 no-print">
      <button className={clsx("btn flex-1", !active && "btn-primary")} aria-pressed={active} onClick={() => (active ? stopTransitAlert() : startTransitAlert(route))}>
        <Icon as={BellRing} /> {active ? t("transitAlertStop") : t("transitAlertStart")}
      </button>
      <InfoTip text={t("transitAlertHint")} />
    </div>
  );
}
