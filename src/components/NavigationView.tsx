import { useEffect, useRef } from "react";
import {
  ArrowUp,
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
import type { Maneuver } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtDistance, fmtDuration, fmtTime } from "../lib/format";
import { setNavVoice, startNavigation, stopNavigation } from "../lib/nav";

function maneuverGlyph(m: Maneuver | null | undefined, size: number) {
  switch (m) {
    case "turn-left":
      return <CornerUpLeft size={size} />;
    case "turn-right":
      return <CornerUpRight size={size} />;
    case "slight-left":
      return <MoveUpLeft size={size} />;
    case "slight-right":
      return <MoveUpRight size={size} />;
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
    case "arrive":
      return <Flag size={size} />;
    default:
      return <ArrowUp size={size} />;
  }
}

/** Distance to the next maneuver, rounded the way drivers read it. */
function maneuverDistance(m: number, t: ReturnType<typeof useT>, units: string) {
  const r = m < 100 ? Math.round(m / 10) * 10 : m < 1000 ? Math.round(m / 50) * 50 : m;
  return fmtDistance(r, t, units);
}

/**
 * Live navigation overlay, drawn over the map while `store.navigation` is set: the next maneuver
 * on top, the trip summary and the controls at the bottom. Mount it once inside the map area.
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

  return (
    <div className="absolute inset-0 z-30 pointer-events-none flex flex-col justify-between p-3 no-print" role="region" aria-label={t("navStart")}>
      {/* next maneuver */}
      <div className="self-center w-full max-w-[520px] pointer-events-auto popover rounded-[var(--radius)] overflow-hidden fade-in">
        {noGps ? (
          <div className="p-4 flex flex-col gap-3">
            <p className="m-0 text-sm">{t("navNoGps")}</p>
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
              <Flag size={30} />
            </span>
            <div className="flex-1 text-xl font-semibold">{t("navArrived")}</div>
          </div>
        ) : !u ? (
          <div className="flex items-center gap-3 p-4 text-muted">
            <div className="progress-line flex-1" role="progressbar" />
            <span className="text-sm shrink-0">{nav.error ?? t("navWaiting")}</span>
          </div>
        ) : (
          <>
            <div className="flex items-center gap-4 p-4 bg-accent-soft">
              <span className="w-14 h-14 shrink-0 rounded-[var(--radius)] bg-[var(--accent-solid)] text-[var(--accent-fg)] flex items-center justify-center">
                {maneuverGlyph(u.maneuver, 32)}
              </span>
              <div className="flex-1 min-w-0">
                <div className="text-2xl font-semibold num leading-tight">
                  <bdi>{maneuverDistance(u.distToManeuverM, t, units)}</bdi>
                </div>
                <div className="text-lg leading-snug truncate-dir">{u.instruction}</div>
              </div>
            </div>
            {u.nextInstruction && u.maneuver !== "arrive" && (
              <div className="flex items-center gap-2 px-4 py-2 text-sm text-muted border-t border-line">
                <span className="shrink-0">{t("navThen")}</span>
                <span className="shrink-0 text-fg">{maneuverGlyph(u.nextManeuver, 16)}</span>
                <span className="truncate-dir">{u.nextInstruction}</span>
              </div>
            )}
            {(u.offRoute || nav.rerouting) && (
              <div className="flex items-center gap-2 px-4 py-2 text-sm bg-warning-soft text-warning border-t border-line" role="status">
                <div className="progress-line w-10 shrink-0" />
                <span>{t("navRerouting")}</span>
              </div>
            )}
          </>
        )}
      </div>

      {/* trip summary + controls */}
      <div className="self-center w-full max-w-[520px] pointer-events-auto popover rounded-[var(--radius)] flex items-center gap-3 ps-4 pe-2 py-2 fade-in">
        <div className="flex-1 min-w-0 flex items-baseline gap-3 flex-wrap num">
          {u && !nav.arrived ? (
            <>
              <span className="text-lg font-semibold text-success">{fmtDuration(u.remainingSec, t)}</span>
              <span className="text-sm text-muted">{fmtDistance(u.remainingM, t, units)}</span>
              <span className="text-sm text-muted">
                {t("navEtaLabel")} <bdi className="text-fg">{fmtTime(u.etaIso)}</bdi>
              </span>
            </>
          ) : (
            <span className="text-sm text-muted">{nav.simulate ? t("navSimulated") : t("navStart")}</span>
          )}
          {nav.simulate && u && !nav.arrived && <span className="chip chip-muted">{t("navSimulated")}</span>}
        </div>
        <button
          className="btn btn-ghost btn-icon shrink-0"
          aria-pressed={!nav.voice}
          title={nav.voice ? t("navMute") : t("navUnmute")}
          aria-label={nav.voice ? t("navMute") : t("navUnmute")}
          onClick={() => void setNavVoice(!nav.voice)}
        >
          {nav.voice ? <Volume2 size={18} strokeWidth={1.75} /> : <VolumeX size={18} strokeWidth={1.75} />}
        </button>
        <button className="btn btn-danger shrink-0" onClick={() => void stopNavigation()}>
          {t("navEnd")}
        </button>
      </div>
    </div>
  );
}
