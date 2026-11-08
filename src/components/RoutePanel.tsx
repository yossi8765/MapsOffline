import { useEffect, useState } from "react";
import { InfoTip } from "./InfoTip";
import { ArrowDown, ArrowUp, ArrowUpDown, Bike, Bus, CalendarClock, Car, ChevronDown, Circle, Footprints, GripVertical, MapPin, Plus, Route, SlidersHorizontal, Trash2, X } from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { InfoBar } from "./InfoBar";
import { sortTransit, useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import type { Mode, TimeKind } from "../lib/api";
import { fmtTime, isoToLocalInput, localInputToIso, nextFiveMinutes, toLocalInput } from "../lib/format";
import { SearchBox } from "./SearchBox";
import { RouteResults, ServiceNoticeBar } from "./RouteResults";

/** The four travel kinds of the segmented control; truck and wheelchair are options of car / walk. */
const MODES: { id: "car" | "foot" | "bike" | "transit"; icon: typeof Car; label: I18nKey }[] = [
  { id: "car", icon: Car, label: "modeCar" },
  { id: "foot", icon: Footprints, label: "modeFoot" },
  { id: "bike", icon: Bike, label: "modeBike" },
  { id: "transit", icon: Bus, label: "modeTransit" },
];

/** The segmented-control button a mode belongs to. */
export function baseMode(m: Mode): "car" | "foot" | "bike" | "transit" {
  return m === "truck" ? "car" : m === "wheelchair" ? "foot" : m;
}

/** "יום ג׳, 5 בנוב׳ 08:15" (Hebrew) / "Tue 5 Nov 08:15" for a datetime-local value. */
export function longDateTime(iso: string | undefined, lang: string): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return "";
  try {
    return new Intl.DateTimeFormat(lang === "he" ? "he-IL" : "en-IL", { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", hour12: false }).format(d);
  } catch {
    return d.toLocaleString();
  }
}

export function RoutePanel() {
  const t = useT();
  const waypoints = useStore((s) => s.waypoints);
  const mode = useStore((s) => s.mode);
  const time = useStore((s) => s.time);
  const avoid = useStore((s) => s.avoid);
  const vehicle = useStore((s) => s.vehicle);
  const accessible = useStore((s) => s.accessible);
  const transitPref = useStore((s) => s.settings.transitPreference);
  const avoidAreasPref = useStore((s) => s.settings.avoidAreas);
  const lang = useStore((s) => s.settings.language);
  const alternativesCount = useStore((s) => s.alternativesCount);
  const routes = useStore((s) => s.routes);
  const loading = useStore((s) => s.routeLoading);
  const error = useStore((s) => s.routeError);
  const notice = useStore((s) => s.routeNotice);
  const maxWaypoints = useStore((s) => (s.alternativesCount === 1 ? 25 : 7));
  const routeFocusTick = useStore((s) => s.routeFocusTick);
  const firstEmpty = waypoints.findIndex((w) => !w);
  const { setWaypoint, addVia, removeWaypoint, moveWaypoint, swapEnds, setMode, setTime, setAvoid, setAlternativesCount, computeRoute, clearRoute, optimizeOrder, setVehicle, setAccessible } =
    useStore.getState();
  const [showOptions, setShowOptions] = useState(false);
  const [showTrip, setShowTrip] = useState(false);
  const [dragFrom, setDragFrom] = useState<number | null>(null);
  // a long computation (first transit query loads the timetable): say what is going on
  const [slow, setSlow] = useState(false);
  useEffect(() => {
    if (!loading) {
      setSlow(false);
      return;
    }
    const h = window.setTimeout(() => setSlow(true), 1500);
    return () => window.clearTimeout(h);
  }, [loading]);

  const n = waypoints.length;
  const canCompute = waypoints.every(Boolean);
  const base = baseMode(mode);

  const onTimeKind = (kind: TimeKind) => {
    if (kind === "now") setTime({ kind: "now" });
    else setTime({ kind, iso: time.iso ?? localInputToIso(isoToLocalInput()) });
  };
  /** Quick time chips: +15 min, +1 h from the shown time; tomorrow 08:00. */
  const shiftTime = (minutes: number | "tomorrow") => {
    const cur = time.iso ? new Date(time.iso) : nextFiveMinutes();
    let d: Date;
    if (minutes === "tomorrow") {
      d = new Date();
      d.setDate(d.getDate() + 1);
      d.setHours(8, 0, 0, 0);
    } else d = new Date(cur.getTime() + minutes * 60_000);
    setTime({ kind: time.kind === "now" ? "depart" : time.kind, iso: localInputToIso(toLocalInput(d)) });
  };

  const dropOn = (to: number) => {
    if (dragFrom === null || dragFrom === to) return;
    // move step by step so the trip stops follow (moveWaypoint swaps neighbours)
    const dir = to > dragFrom ? 1 : -1;
    for (let i = dragFrom; i !== to; i += dir) useStore.getState().moveWaypoint(i, dir as 1 | -1);
    setDragFrom(null);
  };

  return (
    <div className="px-4 pb-5 flex flex-col gap-4">
      {/* mode */}
      <div className="seg" role="group" aria-label={t("travelMode")}>
        {MODES.map((m) => (
          <button key={m.id} aria-pressed={base === m.id} onClick={() => setMode(m.id)}>
            <Icon as={m.icon} />
            <span>{t(m.label)}</span>
          </button>
        ))}
      </div>
      {/* mode options: truck dimensions, wheelchair */}
      {base === "car" && (
        <div className="flex flex-col gap-2 -mt-2">
          <div className="seg seg-sm" role="group" aria-label={t("vehicleKind")}>
            <button aria-pressed={mode === "car"} onClick={() => setMode("car")}>
              <span>{t("vehicleCar")}</span>
            </button>
            <button aria-pressed={mode === "truck"} onClick={() => setMode("truck")}>
              <span>{t("vehicleTruck")}</span>
            </button>
          </div>
          {mode === "truck" && (
            <div className="grid grid-cols-3 gap-2 fade-in">
              {(
                [
                  ["heightM", "truckHeight", 0.5, 6],
                  ["weightT", "truckWeight", 1, 60],
                  ["widthM", "truckWidth", 1, 4],
                ] as const
              ).map(([k, label, min, max]) => (
                <label key={k} className="flex flex-col gap-1 text-xs text-muted">
                  {t(label)}
                  <input
                    type="number"
                    inputMode="decimal"
                    step={k === "weightT" ? 0.5 : 0.1}
                    min={min}
                    max={max}
                    className="input num h-9"
                    value={vehicle[k] ?? ""}
                    onChange={(e) => setVehicle({ [k]: e.target.value === "" ? undefined : Number(e.target.value) })}
                  />
                </label>
              ))}
            </div>
          )}
        </div>
      )}
      {base === "foot" && (
        <div className="-mt-2">
          <Toggle label={t("wheelchairRoute")} info={t("wheelchairRouteHint")} checked={mode === "wheelchair"} onChange={(v) => setMode(v ? "wheelchair" : "foot")} bare />
        </div>
      )}

      {/* waypoints */}
      <div className="flex gap-2">
        <ol className="flex-1 flex flex-col gap-2 m-0 p-0 list-none" aria-label={t("routePoints")}>
          {waypoints.map((wp, i) => {
            const isFirst = i === 0;
            const isLast = i === n - 1;
            const isVia = !isFirst && !isLast;
            const icon = isFirst ? (
              <Icon as={Circle} size={14} strokeWidth={2.5} className="text-accent" />
            ) : isLast ? (
              <Icon as={MapPin} className="text-danger" />
            ) : (
              <span className="block w-2.5 h-2.5 mx-[3px] rounded-full border-2 border-current" />
            );
            const label = isFirst ? t("from") : isLast ? t("to") : `${t("via")} ${i}`;
            return (
              <li
                key={i}
                className={clsx("flex items-center gap-1 group rounded", dragFrom !== null && dragFrom !== i && "outline-dashed outline-1 outline-line-strong")}
                onDragOver={(e) => dragFrom !== null && e.preventDefault()}
                onDrop={() => dropOn(i)}
              >
                {isVia && (
                  <span
                    draggable
                    onDragStart={() => setDragFrom(i)}
                    onDragEnd={() => setDragFrom(null)}
                    className="text-subtle cursor-grab -ms-1"
                    aria-hidden
                  >
                    <Icon as={GripVertical} size={14} />
                  </span>
                )}
                <div className="flex-1 min-w-0">
                  <SearchBox
                    compact
                    leading={icon}
                    label={label}
                    focusTick={i === firstEmpty ? routeFocusTick : undefined}
                    value={wp?.label ?? ""}
                    placeholder={label}
                    onPick={(p) => setWaypoint(i, { lon: p.lon, lat: p.lat, label: p.label, result: p.result })}
                    onClear={() => setWaypoint(i, null)}
                    saveHistory={false}
                  />
                </div>
                {isVia && (
                  // visible on hover AND when anything in the row has the keyboard focus
                  <div className="flex flex-col opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 transition-opacity">
                    <button className="text-muted hover:text-fg h-4" onClick={() => moveWaypoint(i, -1)} aria-label={`${t("moveUp")}: ${label}`} disabled={i === 1}>
                      <Icon as={ArrowUp} size={12} />
                    </button>
                    <button className="text-muted hover:text-fg h-4" onClick={() => moveWaypoint(i, 1)} aria-label={`${t("moveDown")}: ${label}`} disabled={i === n - 2}>
                      <Icon as={ArrowDown} size={12} />
                    </button>
                  </div>
                )}
                {isVia && (
                  <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={() => removeWaypoint(i)} aria-label={`${t("remove")}: ${label}`}>
                    <Icon as={X} size={14} />
                  </button>
                )}
              </li>
            );
          })}
        </ol>
        <div className="flex flex-col justify-center">
          <Tip text={t("swap")}>
            <button className="btn btn-ghost btn-icon text-muted hover:text-fg" onClick={swapEnds} aria-label={t("swap")}>
              <Icon as={ArrowUpDown} size={18} />
            </button>
          </Tip>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-x-1 gap-y-0 -mt-2 -mx-1">
        <button className="btn btn-ghost btn-sm text-muted shrink-0" onClick={() => addVia()} disabled={n >= maxWaypoints}>
          <Icon as={Plus} size={14} /> {t("addVia")}
        </button>
        {n >= 3 && canCompute && mode !== "transit" && (
          <Tip text={t("shortestOrderHint")}>
            <button className="btn btn-ghost btn-sm text-muted shrink-0" onClick={() => void optimizeOrder(false)} disabled={loading}>
              <Icon as={Route} size={14} /> {t("shortestOrder")}
            </button>
          </Tip>
        )}
        {n >= 3 && mode !== "transit" && (
          <button className="btn btn-ghost btn-sm text-muted shrink-0" onClick={() => setShowTrip((v) => !v)} aria-expanded={showTrip}>
            <Icon as={CalendarClock} size={14} /> {t("tripPlanner")}
          </button>
        )}
        <div className="flex-1" />
        {(routes.length > 0 || waypoints.some(Boolean)) && (
          <button className="btn btn-ghost btn-sm text-muted shrink-0" onClick={clearRoute}>
            <Icon as={Trash2} size={14} /> {t("clearRoute")}
          </button>
        )}
      </div>
      {n >= maxWaypoints && <p className="m-0 -mt-2 text-xs text-muted">{t(maxWaypoints === 7 ? "maxWaypointsHint" : "maxWaypointsReached")}</p>}
      {showTrip && n >= 3 && mode !== "transit" && <TripPlanner />}

      {/* time */}
      <div className="flex flex-col gap-2">
        <div className="seg" role="group" aria-label={t("timeLabel")}>
          {(["now", "depart", "arrive"] as TimeKind[]).map((k) => (
            <button key={k} aria-pressed={time.kind === k} onClick={() => onTimeKind(k)}>
              <span>{t(k === "now" ? "timeNow" : k === "depart" ? "timeDepart" : "timeArrive")}</span>
            </button>
          ))}
        </div>
        {time.kind !== "now" && (
          <div className="flex flex-col gap-1.5 fade-in">
            <input
              type="datetime-local"
              className="input num"
              aria-label={t(time.kind === "depart" ? "timeDepart" : "timeArrive")}
              value={isoToLocalInput(time.iso)}
              onChange={(e) => e.target.value && setTime({ kind: time.kind, iso: localInputToIso(e.target.value) })}
            />
            <div className="flex items-center gap-1.5 flex-wrap">
              <span className="text-xs text-muted flex-1 min-w-0">{longDateTime(time.iso, lang)}</span>
              <button className="choice" onClick={() => shiftTime(15)}>
                {t("timePlus15")}
              </button>
              <button className="choice" onClick={() => shiftTime(60)}>
                {t("timePlusHour")}
              </button>
              <button className="choice" onClick={() => shiftTime("tomorrow")}>
                {t("timeTomorrowMorning")}
              </button>
            </div>
            {time.kind === "arrive" && time.iso && Date.parse(time.iso) < Date.now() && <p className="m-0 text-xs text-warning">{t("arriveInPast")}</p>}
          </div>
        )}
      </div>

      {/* options */}
      <div>
        <button
          className="w-full flex items-center gap-2 h-9 px-1 rounded text-muted hover:text-fg transition-colors"
          onClick={() => setShowOptions((v) => !v)}
          aria-expanded={showOptions}
          aria-controls="route-options"
        >
          <Icon as={SlidersHorizontal} />
          <span className="flex-1 text-start">
            {t("avoid")} / {t("alternatives")}
          </span>
          <Icon as={ChevronDown} className={clsx("transition-transform duration-150", showOptions && "rotate-180")} />
        </button>
        {showOptions && (
          <div id="route-options" className="group-list mt-1 fade-in">
            {base === "car" && (
              <>
                <Toggle label={t("avoidToll")} checked={avoid.toll} onChange={(v) => setAvoid({ toll: v })} />
                <Toggle label={t("avoidMotorway")} checked={avoid.motorway} onChange={(v) => setAvoid({ motorway: v })} />
              </>
            )}
            {mode !== "transit" && (
              <Toggle
                label={t("avoidUnpaved")}
                hint={avoid.unpaved && avoid.unpavedLongOnly ? t("avoidUnpavedLongHint") : undefined}
                checked={avoid.unpaved}
                onChange={(v) => setAvoid({ unpaved: v, unpavedLongOnly: v && useStore.getState().settings.unpaved !== "avoid" })}
              />
            )}
            {(base === "foot" || base === "bike") && <Toggle label={t("avoidHills")} info={t("avoidHillsHint")} checked={!!avoid.hills} onChange={(v) => setAvoid({ hills: v })} />}
            {base === "car" && <Toggle label={t("avoidHardTurns")} checked={avoid.hardTurns} onChange={(v) => setAvoid({ hardTurns: v })} />}
            {mode === "transit" && (
              <>
                <div className="row justify-between gap-2 flex-wrap">
                  <span>{t("transitPref")}</span>
                  <div className="seg seg-sm grid grid-cols-2 w-full">
                    {(["fastest", "shortest", "fewestTransfers", "leastWalking"] as const).map((p) => (
                      <button
                        key={p}
                        aria-pressed={(transitPref ?? "fastest") === p}
                        onClick={() => {
                          const st = useStore.getState();
                          void st.updateSettings({ transitPreference: p });
                          // re-order what is already on screen; the best one becomes the selected route
                          if (st.routes.length && st.routes[0].mode === "transit") {
                            const all = sortTransit(st.transitAll.length ? st.transitAll : st.routes, p, st.time.kind);
                            const routes = all.slice(0, Math.max(1, st.alternativesCount));
                            useStore.setState({ transitAll: all, routes, selectedRouteId: routes[0].id, routeTick: st.routeTick + 1 });
                          }
                        }}
                      >
                        <span>{t(p === "fastest" ? "transitPrefFastest" : p === "shortest" ? "transitPrefShortest" : p === "fewestTransfers" ? "transitPrefTransfers" : "transitPrefWalking")}</span>
                      </button>
                    ))}
                  </div>
                </div>
                <Toggle label={t("transitAccessible")} info={t("transitAccessibleHint")} checked={accessible} onChange={setAccessible} />
              </>
            )}
            {mode !== "transit" && (
              <Toggle
                label={t(avoidAreasPref === "a" ? "avoidAreaA" : "avoidAreasAB")}
                checked={avoid.areaA || avoid.areaB}
                onChange={(v) => setAvoid({ areaA: v, areaB: v && avoidAreasPref !== "a" })}
              />
            )}
            {mode !== "transit" && (
              <>
                <Toggle
                  label={t("kohanimRoute")}
                  info={t("kohanimHint")}
                  checked={(avoid.kohanim ?? 0) > 0}
                  onChange={(v) => {
                    // remembered for the next routes; the association's standard to start with
                    const level = v ? useStore.getState().settings.kohanimLevel || 2 : 0;
                    setAvoid({ kohanim: level });
                    void useStore.getState().updateSettings({ kohanimLevel: level });
                  }}
                />
                {(avoid.kohanim ?? 0) > 0 && (
                  <div className="px-3 py-2.5 flex flex-col gap-2 fade-in">
                    <span id="kohanim-level" className="text-sm">
                      {t("kohanimLevel")}
                    </span>
                    {/* chips that wrap: "התאחדות טהרת כהנים" did not fit a segment of a third */}
                    <div className="flex flex-wrap gap-1.5" role="radiogroup" aria-labelledby="kohanim-level">
                      {([1, 2, 3] as const).map((l) => (
                        <button
                          key={l}
                          type="button"
                          role="radio"
                          className="choice"
                          aria-checked={avoid.kohanim === l}
                          aria-pressed={avoid.kohanim === l}
                          onClick={() => {
                            setAvoid({ kohanim: l });
                            void useStore.getState().updateSettings({ kohanimLevel: l });
                          }}
                        >
                          {t(l === 1 ? "kohanimBasic" : l === 2 ? "kohanimAssociation" : "kohanimStrict")}
                        </button>
                      ))}
                    </div>
                  </div>
                )}
              </>
            )}
            {(avoid.points?.length ?? 0) > 0 && (
              <div className="row justify-between">
                <span>{t("avoidedRoads").replace("{n}", String(avoid.points!.length))}</span>
                <button className="btn btn-ghost btn-sm text-accent" onClick={() => setAvoid({ points: undefined })}>
                  {t("clearAvoidedRoads")}
                </button>
              </div>
            )}
            <div className="row justify-between">
              <span id="alt-count">{t("alternatives")}</span>
              <div className="seg seg-sm w-36" role="group" aria-labelledby="alt-count">
                {[1, 2, 3, 5].map((k) => (
                  <button key={k} aria-pressed={alternativesCount === k} onClick={() => setAlternativesCount(k)}>
                    <span className="num">{k}</span>
                  </button>
                ))}
              </div>
            </div>
          </div>
        )}
      </div>

      {/* Every change recomputes the route by itself, so there is no "compute" button: only a
          thin progress line while it runs, and "try again" when it failed. */}

      {loading && (
        <div className="flex flex-col gap-1.5" aria-live="polite">
          <div className="progress-line" role="progressbar" aria-label={t("computing")} />
          {slow && <p className="m-0 text-xs text-muted">{t(mode === "transit" ? "computingTransitSlow" : "computingSlow")}</p>}
        </div>
      )}
      {!loading && notice && <ServiceNoticeBar notice={notice} />}
      {error && !loading && (
        <InfoBar
          severity="error"
          action={
            canCompute ? (
              <button className="btn btn-sm" onClick={() => void computeRoute()}>
                {t("retry")}
              </button>
            ) : undefined
          }
        >
          {error}
        </InfoBar>
      )}
      {!loading && routes.length > 0 && <RouteResults />}
    </div>
  );
}

/** Trip planner: how long to stay at each stop and when it is open; "plan by times" asks the
 *  engine for the best order (up to 25 stops) and shows the planned arrival at each. */
function TripPlanner() {
  const t = useT();
  const waypoints = useStore((s) => s.waypoints);
  const stops = useStore((s) => s.tripStops);
  const arrivals = useStore((s) => s.tripArrivals);
  const loading = useStore((s) => s.routeLoading);
  const setTripStop = useStore((s) => s.setTripStop);
  const timeOf = (iso?: string) => (iso ? fmtTime(iso) : "");
  const fromHHMM = (v: string) => {
    if (!v) return undefined;
    const [h, m] = v.split(":").map(Number);
    const d = new Date();
    d.setHours(h, m, 0, 0);
    return localInputToIso(toLocalInput(d));
  };
  return (
    <div className="card p-3 flex flex-col gap-2 fade-in">
      <div className="text-sm font-semibold inline-flex items-center gap-1.5">
        {t("tripPlanner")}
        <InfoTip text={t("tripPlannerHint")} />
      </div>
      <div className="flex flex-col gap-2">
        {waypoints.map((wp, i) =>
          i === 0 ? null : (
            <div key={i} className="flex flex-col gap-1">
              <div className="flex items-center gap-2 text-sm min-w-0">
                <span className="truncate-dir flex-1 min-w-0">{wp?.label || `${t("via")} ${i}`}</span>
                {arrivals?.[i] && <span className="chip num shrink-0">{timeOf(arrivals[i])}</span>}
              </div>
              <div className="grid grid-cols-3 gap-1.5">
                <label className="flex flex-col text-xs text-muted gap-0.5">
                  {t("tripStay")}
                  <input
                    type="number"
                    min={0}
                    step={5}
                    className="input num h-8"
                    value={stops[i]?.stayMin ?? ""}
                    onChange={(e) => setTripStop(i, { stayMin: e.target.value === "" ? undefined : Number(e.target.value) })}
                  />
                </label>
                <label className="flex flex-col text-xs text-muted gap-0.5">
                  {t("tripOpenFrom")}
                  <input type="time" className="input num h-8" value={stops[i]?.windowStartIso ? timeOf(stops[i]!.windowStartIso) : ""} onChange={(e) => setTripStop(i, { windowStartIso: fromHHMM(e.target.value) })} />
                </label>
                <label className="flex flex-col text-xs text-muted gap-0.5">
                  {t("tripOpenTo")}
                  <input type="time" className="input num h-8" value={stops[i]?.windowEndIso ? timeOf(stops[i]!.windowEndIso) : ""} onChange={(e) => setTripStop(i, { windowEndIso: fromHHMM(e.target.value) })} />
                </label>
              </div>
            </div>
          ),
        )}
      </div>
      <div className="flex gap-2 justify-end">
        <button className="btn btn-sm" disabled={loading || !waypoints.every(Boolean)} onClick={() => void useStore.getState().optimizeTrip(true)}>
          {t("tripPlanKeepEnd")}
        </button>
        <button className="btn btn-primary btn-sm" disabled={loading || !waypoints.every(Boolean)} onClick={() => void useStore.getState().optimizeTrip(false)}>
          {t("tripPlan")}
        </button>
      </div>
    </div>
  );
}

export function Toggle({ label, hint, info, checked, onChange, bare }: { label: string; hint?: string; info?: string; checked: boolean; onChange: (v: boolean) => void; bare?: boolean }) {
  return (
    <label className={clsx(bare ? "flex items-center gap-3 justify-between py-1 cursor-pointer" : "row justify-between cursor-pointer")}>
      <span className="flex flex-col min-w-0">
        <span className="inline-flex items-center gap-1.5">
          {label}
          {info && <InfoTip text={info} />}
        </span>
        {hint && <span className="text-xs text-muted">{hint}</span>}
      </span>
      <button type="button" role="switch" aria-checked={checked} aria-label={label} className="switch" onClick={() => onChange(!checked)} />
    </label>
  );
}
