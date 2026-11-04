/**
 * Floating bars of the map tools (stream F2): measuring (ג-1), drawing / avoid area (ג-9),
 * isochrone legend (ג-11) and the track recorder (ג-8). Each shows only while its tool is active,
 * at the bottom centre of the map.
 */
import { useEffect, useRef, useState } from "react";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { Check, Download, Loader2, MapPin, Pentagon, Spline, Square, Type, Undo2, X } from "lucide-react";
import clsx from "clsx";
import { api } from "../lib/api";
import { useStore } from "../lib/store";
import { DRAW_COLORS, type DrawMode, type IsochroneMode } from "../lib/store/map";
import { useT } from "../lib/i18n";
import { fmtDistance, fmtDuration } from "../lib/format";
import { climb, densify, fmtArea } from "../map/measure";
import { lineToGpx } from "../map/geoFiles";
import { measureSummary, isoColor } from "../map/overlays/tools";

function Bar({ children, label }: { children: React.ReactNode; label: string }) {
  return (
    <div role="toolbar" aria-label={label} className="tool-bar absolute bottom-10 start-1/2 -translate-x-1/2 rtl:translate-x-1/2 z-30 popover rounded-[var(--radius)] px-3 py-2 flex flex-col gap-1.5 min-w-[300px] max-w-[calc(100%-24px)] no-print fade-in">
      {children}
    </div>
  );
}

// ---------------------------------------------------------------- measure
export function MeasureBar() {
  const t = useT();
  const measure = useStore((s) => s.measure);
  const units = useStore((s) => s.settings.units);
  const he = useStore((s) => s.settings.language === "he");
  const [ele, setEle] = useState<{ up: number; down: number } | null>(null);
  const pts = measure?.points;
  useEffect(() => {
    setEle(null);
    if (!pts || pts.length < 2) return;
    let dead = false;
    const timer = setTimeout(() => {
      api
        .elevationProfile(densify(pts, 50))
        .then((e) => !dead && (e.some((v) => v !== null) ? setEle(climb(e)) : setEle(null)))
        .catch(() => !dead && setEle(null));
    }, 300);
    return () => {
      dead = true;
      clearTimeout(timer);
    };
  }, [pts]);
  if (!measure) return null;
  const sum = measureSummary(measure);
  const st = useStore.getState();
  const exportGpx = async () => {
    try {
      const path = await saveDialog({ defaultPath: `${t("measureTitle")}.gpx`, filters: [{ name: "GPX", extensions: ["gpx"] }] });
      if (!path) return;
      const line = measure.area && measure.points.length >= 3 ? [...measure.points, measure.points[0]] : measure.points;
      await api.saveTextFile(path, lineToGpx(t("measureTitle"), line));
      st.toast("success", `${t("exportSaved")} ${path}`);
    } catch (e) {
      st.toastError(e);
    }
  };
  return (
    <Bar label={t("measureTitle")}>
      <div className="flex items-center gap-3">
        <span className="text-sm font-semibold">{t("measureTitle")}</span>
        <span className="num text-base font-semibold" aria-live="polite">
          {fmtDistance(sum.lengthM, t, units)}
        </span>
        {sum.areaM2 !== null && <span className="num text-sm">{fmtArea(sum.areaM2, he)}</span>}
        {ele && (
          <span className="num text-xs text-muted" dir="ltr">
            ↑{Math.round(ele.up)} ↓{Math.round(ele.down)} {he ? "מ׳" : "m"}
          </span>
        )}
        <span className="flex-1" />
        <button className={clsx("btn btn-ghost btn-sm", measure.area && "text-accent")} aria-pressed={measure.area} onClick={() => st.setMeasureArea(!measure.area)} title={t("measureArea")}>
          <Pentagon size={15} strokeWidth={1.75} /> {t("measureArea")}
        </button>
        <button className="btn btn-ghost btn-icon btn-sm" onClick={() => st.undoMeasurePoint()} disabled={!measure.points.length} aria-label={t("measureUndo")} title={`${t("measureUndo")} (Backspace)`}>
          <Undo2 size={15} strokeWidth={1.75} />
        </button>
        <button className="btn btn-ghost btn-icon btn-sm" onClick={() => void exportGpx()} disabled={measure.points.length < 2} aria-label={t("measureExport")} title={t("measureExport")}>
          <Download size={15} strokeWidth={1.75} />
        </button>
        <button className="btn btn-ghost btn-icon btn-sm" onClick={() => st.stopMeasure()} aria-label={t("close")} title={`${t("close")} (Esc)`}>
          <X size={15} strokeWidth={1.75} />
        </button>
      </div>
      <div className="text-xs text-muted">{t("measureHint")}</div>
    </Bar>
  );
}

// ---------------------------------------------------------------- drawing
const MODES: { id: DrawMode; icon: React.ReactNode; key: "drawPoint" | "drawLine" | "drawPolygon" | "drawText" }[] = [
  { id: "point", icon: <MapPin size={15} strokeWidth={1.75} />, key: "drawPoint" },
  { id: "line", icon: <Spline size={15} strokeWidth={1.75} />, key: "drawLine" },
  { id: "polygon", icon: <Square size={15} strokeWidth={1.75} />, key: "drawPolygon" },
  { id: "text", icon: <Type size={15} strokeWidth={1.75} />, key: "drawText" },
];

export function DrawToolbar() {
  const t = useT();
  const d = useStore((s) => s.drawing);
  const [name, setName] = useState("");
  const textRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (d?.mode === "text") textRef.current?.focus();
  }, [d?.mode]);
  if (!d) return null;
  const st = useStore.getState();
  const avoid = d.purpose === "avoid";
  const canFinish = (d.mode === "line" && d.current.length >= 2) || (d.mode === "polygon" && d.current.length >= 3);
  const hasAny = d.features.length > 0 || canFinish;
  return (
    <Bar label={avoid ? t("drawAvoidTitle") : t("drawTitle")}>
      <div className="flex items-center gap-1.5 flex-wrap">
        <span className="text-sm font-semibold me-1">{avoid ? t("drawAvoidTitle") : t("drawTitle")}</span>
        {!avoid && (
          <div className="seg seg-sm" role="group" aria-label={t("drawMode")}>
            {MODES.map((m) => (
              <button key={m.id} aria-pressed={d.mode === m.id} onClick={() => st.setDrawMode(m.id)} title={t(m.key)}>
                <span className="flex items-center gap-1">
                  {m.icon}
                  {t(m.key)}
                </span>
              </button>
            ))}
          </div>
        )}
        {!avoid && (
          <div className="flex items-center gap-1 ms-1" role="radiogroup" aria-label={t("drawColor")}>
            {DRAW_COLORS.map((c) => (
              <button key={c} role="radio" aria-checked={d.color === c} aria-label={c} className={clsx("w-5 h-5 rounded-full border-2", d.color === c ? "border-fg" : "border-transparent")} style={{ background: c }} onClick={() => st.setDrawColor(c)} />
            ))}
          </div>
        )}
        <span className="flex-1" />
        {canFinish && (
          <button className="btn btn-sm" onClick={() => st.finishDrawFeature()} title={`${t("drawFinishShape")} (Enter)`}>
            <Check size={14} /> {t("drawFinishShape")}
          </button>
        )}
        <button className="btn btn-ghost btn-icon btn-sm" onClick={() => st.undoDrawPoint()} aria-label={t("measureUndo")} title={`${t("measureUndo")} (Backspace)`}>
          <Undo2 size={15} strokeWidth={1.75} />
        </button>
        <button className="btn btn-ghost btn-icon btn-sm" onClick={() => st.cancelDrawing()} aria-label={t("cancel")} title={`${t("cancel")} (Esc)`}>
          <X size={15} strokeWidth={1.75} />
        </button>
      </div>
      {d.mode === "text" && (
        <input ref={textRef} className="input h-8" placeholder={t("drawTextPlaceholder")} value={d.text} onChange={(e) => st.setDrawText(e.target.value)} />
      )}
      <div className="flex items-center gap-2">
        {!avoid && <input className="input h-8 flex-1" placeholder={t("drawNamePlaceholder")} value={name} onChange={(e) => setName(e.target.value)} aria-label={t("drawNamePlaceholder")} />}
        <span className={clsx("text-xs text-muted", avoid && "flex-1")}>{avoid ? t("drawAvoidHint") : d.mode === "text" ? t("drawTextHint") : d.mode === "point" ? t("drawPointHint") : t("drawShapeHint")}</span>
        <button className="btn btn-primary btn-sm" disabled={!hasAny} onClick={() => void st.saveDrawing(name.trim())}>
          {avoid ? t("drawAvoidApply") : t("save")}
        </button>
      </div>
    </Bar>
  );
}

// ---------------------------------------------------------------- isochrone
const ISO_MODES: IsochroneMode[] = ["car", "bike", "foot", "transit"];

export function IsochroneLegend() {
  const t = useT();
  const iso = useStore((s) => s.isochrone);
  if (!iso) return null;
  const st = useStore.getState();
  const modeKey = (m: IsochroneMode) => (m === "car" ? "modeCar" : m === "foot" ? "modeFoot" : m === "bike" ? "modeBike" : m === "transit" ? "modeTransit" : "isoWheelchair");
  return (
    <Bar label={t("isoTitle")}>
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-sm font-semibold">{t("isoTitle")}</span>
        <div className="seg seg-sm" role="group" aria-label={t("isoMode")}>
          {ISO_MODES.map((m) => (
            <button key={m} aria-pressed={iso.mode === m} onClick={() => st.showIsochrone(iso.at, m)}>
              <span>{t(modeKey(m))}</span>
            </button>
          ))}
        </div>
        <span className="flex-1" />
        {iso.loading && <Loader2 size={15} className="animate-spin text-muted" aria-label={t("loading")} />}
        <button className="btn btn-ghost btn-icon btn-sm" onClick={() => st.clearIsochrone()} aria-label={t("close")} title={`${t("close")} (Esc)`}>
          <X size={15} strokeWidth={1.75} />
        </button>
      </div>
      <div className="flex items-center gap-3 text-xs">
        {[...iso.minutes].sort((a, b) => a - b).map((m, i) => (
          <span key={m} className="flex items-center gap-1">
            <span className="w-3 h-3 rounded-sm" style={{ background: isoColor(i), opacity: 0.6 }} />
            <span className="num">
              {m} {t("minutes")}
            </span>
          </span>
        ))}
        <span className="flex-1" />
        <div className="seg seg-sm" role="group" aria-label={t("isoRange")}>
          {[
            [5, 10, 15],
            [10, 20, 30],
            [15, 30, 45, 60],
          ].map((set) => (
            <button key={set.join()} aria-pressed={iso.minutes.join() === set.join()} onClick={() => st.setIsochroneMinutes(set)}>
              <span className="num">{set[set.length - 1]}</span>
            </button>
          ))}
        </div>
      </div>
      {iso.error && <div className="text-xs text-danger">{iso.error}</div>}
      {!iso.loading && !iso.error && iso.bands && iso.bands.length === 0 && <div className="text-xs text-muted">{t("isoEmpty")}</div>}
    </Bar>
  );
}

// ---------------------------------------------------------------- track recorder
export function TrackRecorder() {
  const t = useT();
  const track = useStore((s) => s.track);
  const units = useStore((s) => s.settings.units);
  if (!track.recording && !track.starting) return null;
  const u = track.update;
  return (
    <div role="status" className="absolute top-3 start-1/2 -translate-x-1/2 rtl:translate-x-1/2 z-30 popover rounded-full ps-3 pe-1.5 h-10 flex items-center gap-3 no-print fade-in">
      <span className="rec-dot" aria-hidden />
      <span className="text-sm font-semibold">{t("trackRecording")}</span>
      {u && (
        <span className="num text-sm text-muted">
          {fmtDistance(u.distanceM, t, units)} · {fmtDuration(u.durationSec, t)} · ↑{Math.round(u.ascentM)}
        </span>
      )}
      <button className="btn btn-sm btn-danger" disabled={track.starting} onClick={() => useStore.getState().toggleTrackRecording()}>
        {track.starting ? <Loader2 size={14} className="animate-spin" /> : <Square size={12} fill="currentColor" />} {t("trackStop")}
      </button>
    </div>
  );
}
