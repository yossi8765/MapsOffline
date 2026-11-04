/**
 * "My layers" (report ג-2, ג-8, ג-9): imported GPX / KML / GeoJSON files, drawings and recorded
 * tracks, stored by the backend as user layers. Shown / hidden, recoloured, zoomed to, profiled,
 * routed along (map matching) or deleted. Also the import dialog with drag & drop.
 */
import { useEffect, useRef, useState } from "react";
import { userLayerData } from "../map/userLayerData";
import { Crosshair, FileUp, Loader2, Mountain, Navigation, Pencil, Trash2, Upload, X } from "lucide-react";
import { Icon } from "./Icon";
import clsx from "clsx";
import { api, type UserLayer } from "../lib/api";
import { useStore } from "../lib/store";
import { DRAW_COLORS } from "../lib/store/map";
import { useT } from "../lib/i18n";
import { useDialog } from "../lib/useDialog";
import { firstLine, lineElevations } from "../map/geoFiles";
import { climb, densify, pathLengthM } from "../map/measure";
import { fmtDistance } from "../lib/format";
import { InfoTip } from "./InfoTip";

const ACCEPT = ".gpx,.kml,.geojson,.json";

/** Read picked / dropped File objects and import them. */
async function importFileList(files: FileList | File[]): Promise<void> {
  const list = await Promise.all(Array.from(files).map(async (f) => ({ name: f.name, text: await f.text() })));
  if (list.length) await useStore.getState().importFiles(list);
}

export function ImportDialog() {
  const t = useT();
  const open = useStore((s) => s.importOpen);
  const ref = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [over, setOver] = useState(false);
  const close = () => useStore.getState().closeImport();
  useDialog(ref, close, { active: open });
  if (!open) return null;
  const run = async (files: FileList | File[]) => {
    setBusy(true);
    try {
      await importFileList(files);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center bg-black/20 no-print" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div ref={ref} role="dialog" aria-modal="true" aria-label={t("importTitle")} className="popover rounded-[var(--radius)] w-[420px] max-w-[calc(100%-32px)] p-4 flex flex-col gap-3 fade-in">
        <div className="flex items-center justify-between">
          <h2 className="text-base font-semibold m-0 inline-flex items-center gap-1.5">
            {t("importTitle")}
            <InfoTip text={t("importHint")} />
          </h2>
          <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={close} aria-label={t("close")}>
            <Icon as={X} size={16} />
          </button>
        </div>
        <div
          className={clsx("drop-zone rounded-[var(--radius)] border-2 border-dashed p-6 flex flex-col items-center gap-2 text-center", over ? "border-accent bg-accent-soft" : "border-line")}
          onDragOver={(e) => {
            e.preventDefault();
            setOver(true);
          }}
          onDragLeave={() => setOver(false)}
          onDrop={(e) => {
            e.preventDefault();
            setOver(false);
            if (e.dataTransfer.files.length) void run(e.dataTransfer.files);
          }}
        >
          {busy ? <Icon as={Loader2} size={22} className="animate-spin text-muted" /> : <Icon as={FileUp} size={22} className="text-muted" />}
          <div className="text-sm">{t("importDrop")}</div>
          <button className="btn btn-primary btn-sm" disabled={busy} onClick={() => inputRef.current?.click()}>
            <Icon as={Upload} size={14} /> {t("importPick")}
          </button>
          <input
            ref={inputRef}
            type="file"
            accept={ACCEPT}
            multiple
            hidden
            onChange={(e) => {
              if (e.target.files?.length) void run(e.target.files);
              e.target.value = "";
            }}
          />
        </div>
      </div>
    </div>
  );
}

function Profile({ layer }: { layer: UserLayer }) {
  const t = useT();
  const tick = useStore((s) => s.userLayersTick);
  const units = useStore((s) => s.settings.units);
  const [prof, setProf] = useState<{ ele: (number | null)[]; lengthM: number } | null | "none">(null);
  useEffect(() => {
    let dead = false;
    void (async () => {
      try {
        const fc = await userLayerData(layer.id, tick);
        const line = firstLine(fc);
        if (!line || line.length < 2) return !dead && setProf("none");
        const lengthM = pathLengthM(line);
        let ele = lineElevations(fc);
        if (!ele) ele = await api.elevationProfile(densify(line, 50));
        if (!dead) setProf(ele.some((e) => e !== null) ? { ele, lengthM } : "none");
      } catch {
        if (!dead) setProf("none");
      }
    })();
    return () => {
      dead = true;
    };
  }, [layer.id, tick]);
  if (prof === null) return <div className="progress-line" role="progressbar" />;
  if (prof === "none") return <div className="text-xs text-muted">{t("profileNone")}</div>;
  const vals = prof.ele.filter((e): e is number => e !== null);
  const lo = Math.min(...vals),
    hi = Math.max(...vals);
  const W = 280,
    H = 56;
  const pts = prof.ele
    .map((e, i) => (e === null ? null : `${((i / Math.max(1, prof.ele.length - 1)) * W).toFixed(1)},${(H - ((e - lo) / Math.max(1, hi - lo)) * (H - 4) - 2).toFixed(1)}`))
    .filter(Boolean)
    .join(" ");
  const c = climb(prof.ele);
  return (
    <div className="flex flex-col gap-1">
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full h-14" role="img" aria-label={t("elevationProfile")}>
        <polyline points={pts} fill="none" stroke={layer.color} strokeWidth={2} />
      </svg>
      <div className="flex justify-between text-xs text-muted num" dir="ltr">
        <span>{fmtDistance(prof.lengthM, t, units)}</span>
        <span>
          ↑{Math.round(c.up)} ↓{Math.round(c.down)} · {Math.round(lo)}–{Math.round(hi)} {t("metersShort")}
        </span>
      </div>
    </div>
  );
}

function LayerRow({ l }: { l: UserLayer }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [editing, setEditing] = useState(false);
  const [mode, setMode] = useState<"car" | "foot" | "bike">("foot");
  const st = useStore.getState();
  const kindLabel = t(l.kind === "track" ? "userKindTrack" : l.kind === "drawing" ? "userKindDrawing" : "userKindImport");
  return (
    <li className="flex flex-col gap-2 px-4 py-2 border-b border-line last:border-b-0">
      <div className="flex items-center gap-2">
        <button
          className="w-4 h-4 rounded-full shrink-0 border border-line"
          style={{ background: l.color }}
          aria-label={t("drawColor")}
          title={t("drawColor")}
          onClick={() => void st.updateUserLayer(l.id, { color: DRAW_COLORS[(DRAW_COLORS.indexOf(l.color) + 1) % DRAW_COLORS.length] })}
        />
        {editing ? (
          <input
            className="input h-8 flex-1 min-w-0"
            autoFocus
            defaultValue={l.name}
            aria-label={t("userRename")}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const v = e.currentTarget.value.trim();
                if (v) void st.updateUserLayer(l.id, { name: v });
                setEditing(false);
              } else if (e.key === "Escape") {
                e.stopPropagation();
                setEditing(false);
              }
            }}
            onBlur={() => setEditing(false)}
          />
        ) : (
          <button className="flex-1 min-w-0 text-start" onClick={() => setOpen((v) => !v)} aria-expanded={open}>
            <span className="block truncate text-sm">{l.name}</span>
            <span className="block text-xs text-muted">
              {kindLabel} · <span className="num">{l.features}</span> {t("userFeatures")}
            </span>
          </button>
        )}
        <button type="button" role="switch" aria-checked={l.visible} aria-label={t("userShow")} className="switch" onClick={() => void st.updateUserLayer(l.id, { visible: !l.visible })} />
      </div>
      {open && (
        <div className="flex flex-col gap-2 fade-in">
          <Profile layer={l} />
          <div className="flex flex-wrap items-center gap-1.5">
            {l.bbox && (
              <button className="btn btn-sm" onClick={() => st.fitToBox(l.bbox!)}>
                <Icon as={Crosshair} size={13} /> {t("userZoomTo")}
              </button>
            )}
            <select className="input h-7 w-auto text-xs" value={mode} onChange={(e) => setMode(e.target.value as typeof mode)} aria-label={t("userRouteMode")}>
              <option value="foot">{t("modeFoot")}</option>
              <option value="bike">{t("modeBike")}</option>
              <option value="car">{t("modeCar")}</option>
            </select>
            <button className="btn btn-sm" onClick={() => void st.routeAlongLayer(l.id, mode)}>
              <Icon as={Navigation} size={13} className="rtl:-scale-x-100" /> {t("userRouteAlong")}
            </button>
            <button className="btn btn-sm" onClick={() => setEditing(true)}>
              <Icon as={Pencil} size={13} /> {t("userRename")}
            </button>
            <span className="flex-1" />
            <button
              className={clsx("btn btn-sm", confirm ? "btn-danger" : "btn-ghost text-muted")}
              onClick={() => {
                if (!confirm) {
                  setConfirm(true);
                  setTimeout(() => setConfirm(false), 3000);
                  return;
                }
                void st.deleteUserLayer(l.id);
              }}
            >
              <Icon as={Trash2} size={13} /> {confirm ? t("userDeleteConfirm") : t("delete")}
            </button>
          </div>
        </div>
      )}
    </li>
  );
}

export function UserLayersPanel() {
  const t = useT();
  const open = useStore((s) => s.mapPopover === "userLayers");
  const layers = useStore((s) => s.userLayers);
  const track = useStore((s) => s.track);
  const ref = useRef<HTMLDivElement>(null);
  const close = () => useStore.getState().setMapPopover(null);
  useDialog(ref, close, { active: open, restoreFocus: true });
  if (!open) return null;
  const st = useStore.getState();
  return (
    <div ref={ref} role="dialog" aria-label={t("userLayersTitle")} className="map-popover absolute top-3 end-[64px] z-30 w-[340px] max-h-[calc(100%-24px)] popover rounded-[var(--radius)] flex flex-col overflow-hidden no-print fade-in">
      <div className="flex items-center justify-between ps-4 pe-2 h-12 shrink-0">
        <h2 className="text-base font-semibold m-0">{t("userLayersTitle")}</h2>
        <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={close} aria-label={t("close")} title={t("close")}>
          <Icon as={X} size={16} />
        </button>
      </div>
      <div className="px-4 pb-2 flex flex-wrap gap-1.5 shrink-0">
        <button className="btn btn-sm" onClick={() => st.openImport()}>
          <Icon as={FileUp} size={14} /> {t("importTitle")}
        </button>
        <button className="btn btn-sm" onClick={() => st.startDrawing()}>
          <Icon as={Pencil} size={14} /> {t("drawTitle")}
        </button>
        <button className={clsx("btn btn-sm", track.recording && "text-danger")} onClick={() => st.toggleTrackRecording()} disabled={track.starting}>
          <Icon as={Mountain} size={14} /> {track.recording ? t("trackStop") : t("trackStart")}
        </button>
      </div>
      <div className="flex-1 min-h-0 scroll-y">
        {layers.length === 0 ? (
          <p className="m-0 px-4 py-3 text-sm text-muted">{t("userLayersEmpty")}</p>
        ) : (
          <ul className="m-0 p-0 list-none">
            {[...layers].reverse().map((l) => (
              <LayerRow key={l.id} l={l} />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
