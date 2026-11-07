import { useEffect, useRef, useState } from "react";
import {
  ArrowUp,
  Camera,
  Car,
  Minus,
  Plus,
  ChevronLeft,
  ChevronRight,
  LocateFixed,
  CornerLeftDown,
  CornerRightDown,
  CornerUpLeft,
  CornerUpRight,
  Flag,
  GitFork,
  Merge,
  MoveUpLeft,
  MoveUpRight,
  Pause,
  Play,
  Route as RouteIcon,
  Search as SearchIcon,
  X,
  RotateCcw,
  Undo2,
  Volume2,
  VolumeX,
} from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import type { Lane, Maneuver, Route, RouteRequest } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtDistance, fmtDuration, fmtTime } from "../lib/format";
import { errorText } from "../lib/api";
import { flatSteps, isWalkMode, previewStep, navigateVia, resumeGps, routesFromHere, setNavVoice, setSimPaused, setSimSpeed, startNavigation, stopNavigation, switchRoute } from "../lib/nav";
import { VehiclePicker } from "./VehiclePicker";
import { SearchBox } from "./SearchBox";

function maneuverGlyph(m: Maneuver | null | undefined, size: number) {
  switch (m) {
    case "turn-left":
      return <Icon as={CornerUpLeft} size={size} />;
    case "turn-right":
      return <Icon as={CornerUpRight} size={size} />;
    case "slight-left":
      return <Icon as={MoveUpLeft} size={size} />;
    case "slight-right":
      return <Icon as={MoveUpRight} size={size} />;
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
    case "arrive":
      return <Icon as={Flag} size={size} />;
    default:
      return <Icon as={ArrowUp} size={size} />;
  }
}

/** Arrow of one OSM turn:lanes value ("left", "slight_right", "through"…). */
function laneGlyph(dir: string, size: number) {
  switch (dir) {
    case "left":
      return <Icon as={CornerUpLeft} size={size} />;
    case "right":
      return <Icon as={CornerUpRight} size={size} />;
    case "slight_left":
    case "merge_to_left":
      return <Icon as={MoveUpLeft} size={size} />;
    case "slight_right":
    case "merge_to_right":
      return <Icon as={MoveUpRight} size={size} />;
    case "sharp_left":
      return <Icon as={CornerLeftDown} size={size} />;
    case "sharp_right":
      return <Icon as={CornerRightDown} size={size} />;
    case "reverse":
      return <Icon as={Undo2} size={size} />;
    default:
      return <Icon as={ArrowUp} size={size} />;
  }
}

/** Lanes ahead, drawn left to right as on the road (also in Hebrew): the lanes that lead along
 *  the route are bright, the others dimmed. */
export function LaneStrip({ lanes }: { lanes: Lane[] }) {
  const t = useT();
  if (!lanes.length) return null;
  return (
    <div className="flex justify-center gap-1 px-4 py-2 bg-[#1b1d21] border-t border-line" dir="ltr" role="img" aria-label={t("lanesLabel").replace("{n}", String(lanes.filter((l) => l.active).length))}>
      {lanes.map((l, i) => (
        <span key={i} className={clsx("flex items-center justify-center w-9 h-10 rounded border", l.active ? "text-white border-white/60" : "text-white/30 border-white/15")}>
          {(l.directions.length ? l.directions : ["through"]).slice(0, 2).map((d, j) => (
            <span key={j}>{laneGlyph(d, 18)}</span>
          ))}
        </span>
      ))}
    </div>
  );
}

/** Distance to the next maneuver, rounded the way drivers read it. */
function maneuverDistance(m: number, t: ReturnType<typeof useT>, units: string) {
  const r = m < 100 ? Math.round(m / 10) * 10 : m < 1000 ? Math.round(m / 50) * 50 : m;
  return fmtDistance(r, t, units);
}

/** Speeding alert tone: two short beeps (Web Audio, no file to ship). */
function speedBeep() {
  try {
    const ctx = new AudioContext();
    [0, 0.22].forEach((at) => {
      const o = ctx.createOscillator();
      const g = ctx.createGain();
      o.frequency.value = 1046;
      g.gain.setValueAtTime(0.0001, ctx.currentTime + at);
      g.gain.exponentialRampToValueAtTime(0.35, ctx.currentTime + at + 0.02);
      g.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + at + 0.16);
      o.connect(g).connect(ctx.destination);
      o.start(ctx.currentTime + at);
      o.stop(ctx.currentTime + at + 0.18);
    });
    window.setTimeout(() => void ctx.close(), 800);
  } catch {
    /* no audio */
  }
}

/** Beeps when the driver goes above the limit, again every 30 s while still above it. */
function useSpeedAlert(over: boolean, on: boolean) {
  useEffect(() => {
    if (!over || !on) return;
    speedBeep();
    const id = window.setInterval(speedBeep, 30000);
    return () => window.clearInterval(id);
  }, [over, on]);
}

/** "Change route" while navigating: routes from the current position, pick one to drive on. */
/** Search while navigating: the place found becomes a stop on the way or the new destination. */
function NavSearch({ onClose }: { onClose: () => void }) {
  const t = useT();
  const [picked, setPicked] = useState<{ lon: number; lat: number; label: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const go = (kind: "stop" | "destination") => {
    if (!picked) return;
    setBusy(true);
    setErr(null);
    navigateVia(picked, kind).then(onClose, (e) => {
      setBusy(false);
      setErr(errorText(e));
    });
  };
  return (
    <div className="popover rounded-[var(--radius)] p-3 flex flex-col gap-2 fade-in" role="dialog" aria-label={t("navSearchTitle")}>
      <div className="flex items-center gap-2">
        <span className="font-medium flex-1">{t("navSearchTitle")}</span>
        <button className="btn btn-ghost btn-icon btn-sm" aria-label={t("close")} onClick={onClose}>
          <Icon as={X} size={16} />
        </button>
      </div>
      <SearchBox
        value={picked?.label ?? ""}
        autoFocus
        saveHistory
        onPick={(p) => {
          setPicked({ lon: p.lon, lat: p.lat, label: p.label });
          setErr(null);
        }}
        onClear={() => setPicked(null)}
      />
      {picked && (
        <div className="flex gap-2">
          <button className="btn flex-1" disabled={busy} onClick={() => go("stop")}>
            {t("navAddStop")}
          </button>
          <button className="btn btn-primary flex-1" disabled={busy} onClick={() => go("destination")}>
            {t("navNewDestination")}
          </button>
        </div>
      )}
      {busy && <div className="progress-line" role="progressbar" aria-label={t("navChangeRouteLoading")} />}
      {err && <p className="m-0 text-muted">{err}</p>}
    </div>
  );
}

function ChangeRoute({ onClose }: { onClose: () => void }) {
  const t = useT();
  const units = useStore((s) => s.settings.units);
  const remaining = useStore((s) => s.navigation?.update?.remainingSec ?? null);
  const [res, setRes] = useState<{ routes: Route[]; request: RouteRequest } | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    routesFromHere().then(
      (r) => {
        if (!live) return;
        if (r) setRes(r);
        else setErr(t("navChangeRouteNone"));
      },
      (e) => {
        if (live) setErr(errorText(e));
      },
    );
    return () => {
      live = false;
    };
  }, [t]);
  return (
    <div className="popover rounded-[var(--radius)] p-3 flex flex-col gap-2 fade-in" role="dialog" aria-label={t("navChangeRouteTitle")}>
      <div className="flex items-center gap-2">
        <span className="font-medium flex-1">{t("navChangeRouteTitle")}</span>
        <button className="btn btn-ghost btn-icon btn-sm" aria-label={t("close")} onClick={onClose}>
          <Icon as={X} size={16} />
        </button>
      </div>
      {!res && !err && (
        <div className="flex items-center gap-3 text-muted">
          <div className="progress-line flex-1" role="progressbar" aria-label={t("navChangeRouteLoading")} />
          <span className="shrink-0">{t("navChangeRouteLoading")}</span>
        </div>
      )}
      {err && <p className="m-0 text-muted">{err}</p>}
      {res && res.routes.length === 0 && <p className="m-0 text-muted">{t("navChangeRouteNone")}</p>}
      {res &&
        res.routes.map((r) => {
          const diff = remaining != null ? r.durationSec - remaining : null;
          const diffText =
            diff == null || Math.abs(diff) < 60 ? t("navCurrentRoute") : (diff < 0 ? t("navDiffFaster") : t("navDiffSlower")).replace("{n}", fmtDuration(Math.abs(diff), t));
          return (
            <button
              key={r.id}
              className="btn justify-between h-auto py-2 text-start"
              onClick={() => {
                void switchRoute(r, res.request).catch((e) => useStore.getState().toastError(e));
                onClose();
              }}
            >
              <span className="flex flex-col items-start gap-0.5 num">
                <span className="font-semibold">{fmtDuration(r.durationSec, t)}</span>
                <span className="text-xs text-muted">{fmtDistance(r.distanceM, t, units)}</span>
              </span>
              <span className={clsx("text-sm", diff != null && diff <= -60 ? "text-success" : "text-muted")}>{diffText}</span>
            </button>
          );
        })}
    </div>
  );
}

/** The round red-rimmed speed-limit sign. */
function SpeedSign({ kmh, label }: { kmh: number; label: string }) {
  return (
    <span className="w-11 h-11 shrink-0 rounded-full border-[4px] border-[#d92626] bg-white text-black font-bold num flex items-center justify-center text-base" role="img" aria-label={label}>
      {kmh}
    </span>
  );
}

/**
 * Live navigation overlay, drawn over the map while `store.navigation` is set: the next maneuver
 * (with lanes and a speed camera warning) on top, the trip summary, current speed / limit / street
 * and the controls at the bottom. Escape ends navigation after a confirmation (App.tsx).
 */
export function NavigationView() {
  const t = useT();
  const nav = useStore((s) => s.navigation);
  const units = useStore((s) => s.settings.units);
  // The whole map is the navigation screen: the side panel folds away while navigating and
  // comes back (as it was) when navigation ends.
  const active = !!nav;
  const [vehicleOpen, setVehicleOpen] = useState(false);
  const [changeOpen, setChangeOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const showSpeedometer = useStore((s) => s.settings.navSpeedometer ?? true);
  const speedAlert = useStore((s) => s.settings.navSpeedAlert ?? true);
  const nu = nav?.update;
  const speeding = !!nu && !nav?.arrived && nu.speedKmh != null && nu.speedLimitKmh != null && nu.speedKmh > nu.speedLimitKmh + 3;
  useSpeedAlert(speeding, speedAlert && showSpeedometer);
  const wasCollapsed = useRef<boolean | null>(null);
  useEffect(() => {
    const st = useStore.getState();
    if (active) {
      wasCollapsed.current = st.sidebarCollapsed;
      if (!st.sidebarCollapsed) useStore.setState({ sidebarCollapsed: true });
    } else if (wasCollapsed.current !== null) {
      useStore.setState({ sidebarCollapsed: wasCollapsed.current });
      wasCollapsed.current = null;
    }
  }, [active]);
  if (!nav) return null;
  const u = nav.update;
  const walk = isWalkMode(nav.route.mode);
  const noGps = !!nav.error && !nav.simulate && /LOCATION_(DENIED|UNAVAILABLE)/.test(nav.error);
  const over = u?.speedKmh != null && u.speedLimitKmh != null && u.speedKmh > u.speedLimitKmh + 3;
  const steps = flatSteps(nav.route);
  const manual = nav.manualStep != null ? steps[nav.manualStep] : null;
  const shownIndex = nav.manualStep ?? Math.min(u?.stepIndex ?? 0, Math.max(steps.length - 1, 0));
  // RTL: "next" points to the reading direction's end (left in Hebrew)
  const rtl = typeof document !== "undefined" && document.documentElement.dir === "rtl";

  return (
    // pb-10: the bottom bar sits above the map's attribution line
    <div className="absolute inset-0 z-30 pointer-events-none flex flex-col justify-between p-3 pb-10 no-print" role="region" aria-label={t("navStart")}>
      {/* next maneuver */}
      <div className="self-center w-full max-w-[520px] pointer-events-auto popover rounded-[var(--radius)] overflow-hidden fade-in" aria-live="polite">
        {noGps ? (
          <div className="p-4 flex flex-col gap-3">
            <p className="m-0">{t("navNoGps")}</p>
            <div className="flex gap-2">
              <button className="btn btn-primary flex-1" onClick={() => void startNavigation(nav.routeId, true)}>
                {t("navSimulate")}
              </button>
              <button className="btn" onClick={() => void stopNavigation()}>
                {t("navEnd")}
              </button>
            </div>
          </div>
        ) : manual ? (
          <>
            <div className="flex items-center gap-4 p-4 bg-accent-soft">
              <span className="w-14 h-14 shrink-0 rounded-[var(--radius)] bg-[var(--accent-solid)] text-[var(--accent-fg)] flex items-center justify-center">{maneuverGlyph(manual.step.maneuver, 32)}</span>
              <div className="flex-1 min-w-0">
                <div className="text-sm text-muted num">{t("navStepOf").replace("{n}", String((nav.manualStep ?? 0) + 1)).replace("{m}", String(steps.length))}</div>
                <div className="text-lg leading-snug truncate-dir">{manual.step.instruction}</div>
                {manual.step.distanceM > 0 && manual.step.maneuver !== "arrive" && (
                  <div className="text-sm text-muted num">
                    {t("navThenFor")} <bdi>{fmtDistance(manual.step.distanceM, t, units)}</bdi>
                  </div>
                )}
              </div>
            </div>
            <div className="flex items-center gap-2 px-4 py-2 bg-warning-soft text-warning border-t border-line" role="status">
              <span className="flex-1">{t("navManualHint")}</span>
              <button className="btn btn-sm btn-primary shrink-0" onClick={resumeGps}>
                <Icon as={LocateFixed} size={14} /> {t("navBackToGps")}
              </button>
            </div>
          </>
        ) : nav.arrived ? (
          <div className="flex items-center gap-4 p-4">
            <span className="w-14 h-14 shrink-0 rounded-[var(--radius)] bg-success-soft text-success flex items-center justify-center">
              <Icon as={Flag} size={30} />
            </span>
            <div className="flex-1 text-xl font-semibold">{t("navArrived")}</div>
          </div>
        ) : !u ? (
          <div className="flex items-center gap-3 p-4 text-muted">
            <div className="progress-line flex-1" role="progressbar" aria-label={t("navWaiting")} />
            <span className="shrink-0">{nav.error ?? t("navWaiting")}</span>
          </div>
        ) : (
          <>
            <div className="flex items-center gap-4 p-4 bg-accent-soft">
              <span className="w-14 h-14 shrink-0 rounded-[var(--radius)] bg-[var(--accent-solid)] text-[var(--accent-fg)] flex items-center justify-center">{maneuverGlyph(u.maneuver, 32)}</span>
              <div className="flex-1 min-w-0">
                <div className="text-2xl font-semibold num leading-tight">
                  <bdi>{maneuverDistance(u.distToManeuverM, t, units)}</bdi>
                </div>
                <div className="text-lg leading-snug truncate-dir">{u.instruction}</div>
              </div>
            </div>
            {u.lanes && u.lanes.length > 1 && <LaneStrip lanes={u.lanes} />}
            {u.cameraAheadM != null && u.cameraAheadM < 1000 && (
              <div className="flex items-center gap-2 px-4 py-2 bg-warning-soft text-warning border-t border-line" role="alert">
                <Icon as={Camera} />
                <span>{t("navCamera").replace("{d}", maneuverDistance(u.cameraAheadM, t, units))}</span>
              </div>
            )}
            {u.nextInstruction && u.maneuver !== "arrive" && (
              <div className="flex items-center gap-2 px-4 py-2 text-muted border-t border-line">
                <span className="shrink-0">{t("navThen")}</span>
                <span className="shrink-0 text-fg">{maneuverGlyph(u.nextManeuver, 16)}</span>
                <span className="truncate-dir">{u.nextInstruction}</span>
              </div>
            )}
            {(u.offRoute || nav.rerouting) && (
              <div className="flex items-center gap-2 px-4 py-2 bg-warning-soft text-warning border-t border-line" role="status">
                <div className="progress-line w-10 shrink-0" />
                <span>{t("navRerouting")}</span>
              </div>
            )}
          </>
        )}
      </div>

      {/* trip summary + speed + controls */}
      <div className="self-center w-full max-w-[520px] pointer-events-auto flex flex-col gap-2 fade-in" data-nav-bottom>
        {/* speed: a lone circle at the bottom left (the posted limit above it when known) */}
        {u && !nav.arrived && showSpeedometer && (
          <div className="self-start rtl:self-end flex flex-col items-center gap-2 pointer-events-none" data-speedometer>
            {u.speedLimitKmh != null && <SpeedSign kmh={u.speedLimitKmh} label={t("speedLimit").replace("{n}", String(u.speedLimitKmh))} />}
            <span
              className={clsx(
                "w-16 h-16 shrink-0 rounded-full border-[3px] shadow-lg flex flex-col items-center justify-center leading-none num transition-colors duration-200",
                "bg-surface-solid", over ? "border-[#d92626] text-[#d92626]" : "border-line-strong",
              )}
              role="img"
              aria-label={u.speedKmh != null ? `${Math.round(u.speedKmh)} ${t("kmh")}${over ? ` · ${t("navOverSpeed")}` : ""}` : t("navNoSpeed")}
            >
              <span className="text-2xl font-bold">{u.speedKmh != null ? Math.round(u.speedKmh) : "–"}</span>
              <span className={clsx("text-[10px]", over ? "text-[#d92626]" : "text-muted")}>{t("kmh")}</span>
            </span>
          </div>
        )}
        {changeOpen && u && !nav.arrived && <ChangeRoute onClose={() => setChangeOpen(false)} />}
        {searchOpen && !nav.arrived && <NavSearch onClose={() => setSearchOpen(false)} />}
        {vehicleOpen && (nav.route.mode === "car" || nav.route.mode === "truck") && (
          <div className="popover rounded-[var(--radius)] p-3 flex flex-col gap-2 fade-in" role="dialog" aria-label={t("navVehicle")}>
            <span className="font-medium">{t("navVehicle")}</span>
            <VehiclePicker onPicked={() => setVehicleOpen(false)} />
          </div>
        )}
        {nav.simulate && !nav.arrived && (
          <div className="flex items-center gap-1 self-start popover rounded-[var(--radius)] ps-3 pe-1 py-1" role="group" aria-label={t("simSpeedTitle")}>
            <Tip text={nav.simPaused ? t("simResume") : t("simPause")}>
              <button
                className={clsx("btn btn-icon btn-sm", nav.simPaused ? "btn-primary" : "btn-ghost")}
                aria-label={nav.simPaused ? t("simResume") : t("simPause")}
                aria-pressed={!!nav.simPaused}
                onClick={() => setSimPaused(!nav.simPaused)}
              >
                <Icon as={nav.simPaused ? Play : Pause} size={16} className="fill-current" />
              </button>
            </Tip>
            <span className="text-sm text-muted me-1">{t("simSpeedTitle")}</span>
            <Tip text={t("simSlower")}>
              <button className="btn btn-ghost btn-icon btn-sm" aria-label={t("simSlower")} disabled={walk ? nav.simSpeedKmh <= 1 : nav.simSpeedKmh !== 0 && nav.simSpeedKmh <= 10} onClick={() => setSimSpeed(walk ? nav.simSpeedKmh - 1 : nav.simSpeedKmh === 0 ? 50 : nav.simSpeedKmh - 10)}>
                <Icon as={Minus} size={16} />
              </button>
            </Tip>
            <span className="min-w-20 text-center font-semibold num">
              {nav.simSpeedKmh === 0 ? t("simSpeedRoad") : <bdi>{`${nav.simSpeedKmh} ${t("kmh")}`}</bdi>}
            </span>
            <Tip text={t("simFaster")}>
              <button className="btn btn-ghost btn-icon btn-sm" aria-label={t("simFaster")} disabled={walk ? nav.simSpeedKmh >= 10 : nav.simSpeedKmh >= 300} onClick={() => setSimSpeed(walk ? nav.simSpeedKmh + 1 : nav.simSpeedKmh === 0 ? 60 : nav.simSpeedKmh + 10)}>
                <Icon as={Plus} size={16} />
              </button>
            </Tip>
            {!walk && (
              <button className={clsx("btn btn-sm", nav.simSpeedKmh === 0 ? "btn-primary" : "btn-ghost")} aria-pressed={nav.simSpeedKmh === 0} onClick={() => setSimSpeed(nav.simSpeedKmh === 0 ? 50 : 0)}>
                {t("simSpeedRoad")}
              </button>
            )}
          </div>
        )}
        {/* narrow screens: the summary takes its own line above the buttons */}
        <div className="nav-bar popover rounded-[var(--radius)] flex flex-wrap items-center gap-x-0.5 sm:gap-x-3 gap-y-1 ps-3 sm:ps-4 pe-2 py-2">
          <div className="flex-1 basis-full sm:basis-0 min-w-0 flex items-baseline gap-3 flex-wrap num">
            {u && !nav.arrived ? (
              <>
                <span className="text-lg font-semibold text-success">{fmtDuration(u.remainingSec, t)}</span>
                <span className="text-muted">{fmtDistance(u.remainingM, t, units)}</span>
                <span className="text-muted">
                  {t("navEtaLabel")} <bdi className="text-fg">{fmtTime(u.etaIso)}</bdi>
                </span>
              </>
            ) : (
              <span className="text-muted">{nav.simulate ? t("navSimulated") : nav.arrived ? t("navArrived") : t("navWaiting")}</span>
            )}
            {nav.simulate && u && !nav.arrived && <span className="chip chip-muted">{t("navSimulated")}</span>}
          </div>
          {steps.length > 1 && !nav.arrived && (
            <div className="flex items-center shrink-0" role="group" aria-label={t("navStepsGroup")}>
              {nav.manualStep != null && (
                <Tip text={t("navBackToGps")}>
                  <button className="btn btn-ghost btn-icon text-accent" aria-label={t("navBackToGps")} onClick={resumeGps}>
                    <Icon as={LocateFixed} size={18} />
                  </button>
                </Tip>
              )}
              <Tip text={t("navPrevStep")}>
                <button className="btn btn-ghost btn-icon" aria-label={t("navPrevStep")} disabled={shownIndex <= 0} onClick={() => previewStep(-1)}>
                  <Icon as={rtl ? ChevronRight : ChevronLeft} size={20} />
                </button>
              </Tip>
              <Tip text={t("navNextStep")}>
                <button className="btn btn-ghost btn-icon" aria-label={t("navNextStep")} disabled={shownIndex >= steps.length - 1} onClick={() => previewStep(1)}>
                  <Icon as={rtl ? ChevronLeft : ChevronRight} size={20} />
                </button>
              </Tip>
            </div>
          )}
          {!nav.arrived && nav.request && (
            <Tip text={t("navSearchTitle")}>
              <button
                className="btn btn-ghost btn-icon shrink-0"
                aria-label={t("navSearchTitle")}
                aria-expanded={searchOpen}
                onClick={() => {
                  setSearchOpen((v) => !v);
                  setChangeOpen(false);
                }}
              >
                <Icon as={SearchIcon} size={18} />
              </button>
            </Tip>
          )}
          {u && !nav.arrived && nav.request && (
            <Tip text={t("navChangeRoute")}>
              <button className="btn btn-ghost btn-icon shrink-0" aria-label={t("navChangeRoute")} aria-expanded={changeOpen} onClick={() => setChangeOpen((v) => !v)}>
                <Icon as={RouteIcon} size={18} />
              </button>
            </Tip>
          )}
          {(nav.route.mode === "car" || nav.route.mode === "truck") && (
            <Tip text={t("navVehicle")}>
              <button className="btn btn-ghost btn-icon shrink-0" aria-label={t("navVehicle")} aria-expanded={vehicleOpen} onClick={() => setVehicleOpen((v) => !v)}>
                <Icon as={Car} size={18} />
              </button>
            </Tip>
          )}
          <Tip text={nav.voice ? t("navMute") : t("navUnmute")}>
            <button className="btn btn-ghost btn-icon shrink-0" aria-pressed={!nav.voice} aria-label={nav.voice ? t("navMute") : t("navUnmute")} onClick={() => void setNavVoice(!nav.voice)}>
              <Icon as={nav.voice ? Volume2 : VolumeX} size={18} />
            </button>
          </Tip>
          <Tip text={t("navEnd")} shortcut="Esc">
            <button className="btn btn-danger shrink-0" onClick={() => void stopNavigation()}>
              {t("navEnd")}
            </button>
          </Tip>
        </div>
      </div>
    </div>
  );
}
