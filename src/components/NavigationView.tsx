import { useEffect, useRef } from "react";
import {
  ArrowUp,
  Camera,
  CornerLeftDown,
  CornerRightDown,
  CornerUpLeft,
  CornerUpRight,
  Flag,
  GitFork,
  Merge,
  MoveUpLeft,
  MoveUpRight,
  RotateCcw,
  Undo2,
  Volume2,
  VolumeX,
} from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import type { Lane, Maneuver } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtDistance, fmtDuration, fmtTime } from "../lib/format";
import { setNavVoice, startNavigation, stopNavigation } from "../lib/nav";

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
  const noGps = !!nav.error && !nav.simulate && /LOCATION_(DENIED|UNAVAILABLE)/.test(nav.error);
  const over = u?.speedKmh != null && u.speedLimitKmh != null && u.speedKmh > u.speedLimitKmh + 3;

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
      <div className="self-center w-full max-w-[520px] pointer-events-auto flex flex-col gap-2 fade-in">
        {u && !nav.arrived && (u.speedKmh != null || u.speedLimitKmh != null || u.street || u.approximate) && (
          <div className="flex items-center gap-3 self-start popover rounded-[var(--radius)] px-3 py-2">
            {u.speedLimitKmh != null && <SpeedSign kmh={u.speedLimitKmh} label={t("speedLimit").replace("{n}", String(u.speedLimitKmh))} />}
            {u.speedKmh != null && (
              <span className={clsx("flex flex-col items-center leading-none num", over && "text-danger")}>
                <span className="text-xl font-semibold">{Math.round(u.speedKmh)}</span>
                <span className="text-xs text-muted">{t("kmh")}</span>
              </span>
            )}
            {u.street && <span className="truncate-dir max-w-56 font-medium">{u.street}</span>}
            {u.approximate && (
              <Tip text={t("navApproximateHint")}>
                <span className="chip chip-warning" tabIndex={0}>
                  {t("navApproximate")}
                </span>
              </Tip>
            )}
          </div>
        )}
        <div className="popover rounded-[var(--radius)] flex items-center gap-3 ps-4 pe-2 py-2">
          <div className="flex-1 min-w-0 flex items-baseline gap-3 flex-wrap num">
            {u && !nav.arrived ? (
              <>
                <span className="text-lg font-semibold text-success">{fmtDuration(u.remainingSec, t)}</span>
                <span className="text-muted">{fmtDistance(u.remainingM, t, units)}</span>
                <span className="text-muted">
                  {t("navEtaLabel")} <bdi className="text-fg">{fmtTime(u.etaIso)}</bdi>
                </span>
              </>
            ) : (
              <span className="text-muted">{nav.simulate ? t("navSimulated") : t("navStart")}</span>
            )}
            {nav.simulate && u && !nav.arrived && <span className="chip chip-muted">{t("navSimulated")}</span>}
          </div>
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
