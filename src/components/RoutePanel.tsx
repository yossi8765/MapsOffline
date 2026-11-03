import { useState } from "react";
import { AlertCircle, ArrowDown, ArrowUp, ArrowUpDown, Bike, Bus, Car, ChevronDown, Circle, Footprints, MapPin, Plus, SlidersHorizontal, Trash2, X, Route } from "lucide-react";
import clsx from "clsx";
import { sortTransit, useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import type { Mode, TimeKind } from "../lib/api";
import { isoToLocalInput, localInputToIso } from "../lib/format";
import { SearchBox } from "./SearchBox";
import { RouteResults } from "./RouteResults";

const MODES: { id: Mode; icon: React.ReactNode }[] = [
  { id: "car", icon: <Car size={16} /> },
  { id: "foot", icon: <Footprints size={16} /> },
  { id: "bike", icon: <Bike size={16} /> },
  { id: "transit", icon: <Bus size={16} /> },
];

export function RoutePanel() {
  const t = useT();
  const waypoints = useStore((s) => s.waypoints);
  const mode = useStore((s) => s.mode);
  const time = useStore((s) => s.time);
  const avoid = useStore((s) => s.avoid);
  const transitPref = useStore((s) => s.settings.transitPreference);
  const avoidAreasPref = useStore((s) => s.settings.avoidAreas);
  const alternativesCount = useStore((s) => s.alternativesCount);
  const routes = useStore((s) => s.routes);
  const loading = useStore((s) => s.routeLoading);
  const error = useStore((s) => s.routeError);
  const { setWaypoint, addVia, removeWaypoint, moveWaypoint, swapEnds, setMode, setTime, setAvoid, setAlternativesCount, computeRoute, clearRoute, optimizeOrder } =
    useStore.getState();
  const [showOptions, setShowOptions] = useState(false);

  const n = waypoints.length;
  const canCompute = waypoints.every(Boolean);

  const onTimeKind = (kind: TimeKind) => {
    if (kind === "now") setTime({ kind: "now" });
    else setTime({ kind, iso: time.iso ?? localInputToIso(isoToLocalInput()) });
  };

  return (
    <div className="px-4 pb-5 flex flex-col gap-4">
      {/* mode */}
      <div className="seg">
        {MODES.map((m) => (
          <button key={m.id} aria-pressed={mode === m.id} onClick={() => setMode(m.id)} title={t(`mode${m.id[0].toUpperCase()}${m.id.slice(1)}` as "modeCar")}>
            {m.icon}
            <span>{t(`mode${m.id[0].toUpperCase()}${m.id.slice(1)}` as "modeCar")}</span>
          </button>
        ))}
      </div>

      {/* waypoints */}
      <div className="flex gap-2">
        <div className="flex-1 flex flex-col gap-2">
          {waypoints.map((wp, i) => {
            const isFirst = i === 0;
            const isLast = i === n - 1;
            const icon = isFirst ? <Circle size={14} strokeWidth={2.5} className="text-accent" /> : isLast ? <MapPin size={16} className="text-danger" /> : <span className="block w-2.5 h-2.5 mx-[3px] rounded-full border-2 border-current" />;
            return (
              <div key={i} className="flex items-center gap-1 group">
                <div className="flex-1">
                  <SearchBox
                    compact
                    leading={icon}
                    value={wp?.label ?? ""}
                    placeholder={isFirst ? t("from") : isLast ? t("to") : `${t("via")} ${i}`}
                    onPick={(p) => setWaypoint(i, { lon: p.lon, lat: p.lat, label: p.label, result: p.result })}
                    onClear={() => setWaypoint(i, null)}
                    saveHistory={false}
                  />
                </div>
                {!isFirst && !isLast && (
                  <div className="flex flex-col opacity-0 group-hover:opacity-100 transition-opacity">
                    <button className="text-muted hover:text-fg h-4" onClick={() => moveWaypoint(i, -1)} title={t("moveUp")} disabled={i === 1}>
                      <ArrowUp size={12} />
                    </button>
                    <button className="text-muted hover:text-fg h-4" onClick={() => moveWaypoint(i, 1)} title={t("moveDown")} disabled={i === n - 2}>
                      <ArrowDown size={12} />
                    </button>
                  </div>
                )}
                {!isFirst && !isLast && (
                  <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={() => removeWaypoint(i)} title={t("remove")}>
                    <X size={14} />
                  </button>
                )}
              </div>
            );
          })}
        </div>
        <div className="flex flex-col justify-center">
          <button className="btn btn-ghost btn-icon text-muted hover:text-fg" onClick={swapEnds} title={t("swap")} aria-label={t("swap")}>
            <ArrowUpDown size={18} strokeWidth={1.75} />
          </button>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-x-1 gap-y-0 -mt-2 -mx-1">
        <button className="btn btn-ghost btn-sm text-muted shrink-0" onClick={() => addVia()} disabled={n >= 7}>
          <Plus size={14} /> {t("addVia")}
        </button>
        {n >= 3 && canCompute && mode !== "transit" && (
          <button className="btn btn-ghost btn-sm text-muted shrink-0" onClick={() => void optimizeOrder(false)} disabled={loading} title={t("shortestOrderHint")}>
            <Route size={14} /> {t("shortestOrder")}
          </button>
        )}
        <div className="flex-1" />
        {(routes.length > 0 || waypoints.some(Boolean)) && (
          <button className="btn btn-ghost btn-sm text-muted shrink-0" onClick={clearRoute}>
            <Trash2 size={14} /> {t("clearRoute")}
          </button>
        )}
      </div>

      {/* time */}
      <div className="flex flex-col gap-2">
        <div className="seg">
          {(["now", "depart", "arrive"] as TimeKind[]).map((k) => (
            <button key={k} aria-pressed={time.kind === k} onClick={() => onTimeKind(k)}>
              <span>{t(k === "now" ? "timeNow" : k === "depart" ? "timeDepart" : "timeArrive")}</span>
            </button>
          ))}
        </div>
        {time.kind !== "now" && (
          <input
            type="datetime-local"
            className="input num fade-in"
            value={isoToLocalInput(time.iso)}
            onChange={(e) => e.target.value && setTime({ kind: time.kind, iso: localInputToIso(e.target.value) })}
          />
        )}
      </div>

      {/* options */}
      <div>
        <button
          className="w-full flex items-center gap-2 h-9 px-1 rounded text-sm text-muted hover:text-fg transition-colors"
          onClick={() => setShowOptions((v) => !v)}
          aria-expanded={showOptions}
        >
          <SlidersHorizontal size={16} strokeWidth={1.75} />
          <span className="flex-1 text-start">
            {t("avoid")} / {t("alternatives")}
          </span>
          <ChevronDown size={16} className={clsx("transition-transform duration-150", showOptions && "rotate-180")} />
        </button>
        {showOptions && (
          <div className="group-list mt-1 fade-in">
            {mode === "car" && (
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
            {mode === "car" && <Toggle label={t("avoidHardTurns")} checked={avoid.hardTurns} onChange={(v) => setAvoid({ hardTurns: v })} />}
            {mode === "transit" && (
              <div className="row justify-between gap-2 flex-wrap">
                <span>{t("transitPref")}</span>
                <div className="seg seg-sm seg-fit">
                  {(["fastest", "fewestTransfers", "leastWalking"] as const).map((p) => (
                    <button
                      key={p}
                      aria-pressed={(transitPref ?? "fastest") === p}
                      onClick={() => {
                        const st = useStore.getState();
                        void st.updateSettings({ transitPreference: p });
                        // re-order what is already on screen; the best one becomes the selected route
                        if (st.routes.length && st.routes[0].mode === "transit") {
                          const routes = sortTransit(st.routes, p, st.time.kind);
                          useStore.setState({ routes, selectedRouteId: routes[0].id, routeTick: st.routeTick + 1 });
                        }
                      }}
                    >
                      <span>{t(p === "fastest" ? "transitPrefFastest" : p === "fewestTransfers" ? "transitPrefTransfers" : "transitPrefWalking")}</span>
                    </button>
                  ))}
                </div>
              </div>
            )}
            {mode !== "transit" && (
              <Toggle
                label={t(avoidAreasPref === "a" ? "avoidAreaA" : "avoidAreasAB")}
                checked={avoid.areaA || avoid.areaB}
                onChange={(v) => setAvoid({ areaA: v, areaB: v && avoidAreasPref !== "a" })}
              />
            )}
            <div className="row justify-between">
              <span className="text-base">{t("alternatives")}</span>
              <div className="seg seg-sm w-36">
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
      {!canCompute && routes.length === 0 && !error && <p className="m-0 text-sm text-muted">{t("pickPointsHint")}</p>}

      {loading && <div className="progress-line" role="progressbar" aria-label={t("computing")} />}
      {error && !loading && (
        <div className="flex items-start gap-2.5 rounded-[var(--radius-card)] px-3 py-2.5 text-sm bg-danger-soft text-danger" role="alert">
          <AlertCircle size={16} className="shrink-0 mt-px" />
          <span className="min-w-0 flex-1">{error}</span>
          {canCompute && (
            <button className="btn btn-ghost btn-sm -my-1 -me-1.5 text-danger shrink-0" onClick={() => void computeRoute()}>
              {t("retry")}
            </button>
          )}
        </div>
      )}
      {!loading && routes.length > 0 && <RouteResults />}
    </div>
  );
}

export function Toggle({ label, hint, checked, onChange }: { label: string; hint?: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="row justify-between cursor-pointer">
      <span className="flex flex-col min-w-0">
        <span className="text-base">{label}</span>
        {hint && <span className="text-xs text-muted">{hint}</span>}
      </span>
      <button type="button" role="switch" aria-checked={checked} className="switch" onClick={() => onChange(!checked)} />
    </label>
  );
}
