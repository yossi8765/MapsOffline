/**
 * User tile layers (report ג-7): an online XYZ template or WMS service, or a local PMTiles /
 * MBTiles raster file, drawn over the map at a chosen opacity. Kept in Settings.customLayers; the
 * backend serves each at http://mo-user.localhost/<id>/{z}/{x}/{y} (and caches online tiles).
 */
import { useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ArrowDown, ArrowUp, FolderOpen, Plus, Trash2, X } from "lucide-react";
import { Icon } from "./Icon";
import { apiX, type CustomLayer } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { useDialog } from "../lib/useDialog";
import { validateCustomLayer } from "../map/customLayers";
import { InfoTip } from "./InfoTip";

const KINDS: CustomLayer["kind"][] = ["xyz", "wms", "pmtiles", "mbtiles"];

function newId(): string {
  return `L${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

export function CustomLayersDialog() {
  const t = useT();
  const open = useStore((s) => s.mapPopover === "customLayers");
  const list = useStore((s) => s.settings.customLayers ?? []);
  const ref = useRef<HTMLDivElement>(null);
  const [draft, setDraft] = useState<CustomLayer>({ id: "", name: "", kind: "xyz", url: "", opacity: 0.8, visible: true });
  const [err, setErr] = useState<string | null>(null);
  const close = () => useStore.getState().setMapPopover(null);
  useDialog(ref, close, { active: open });
  if (!open) return null;
  const save = (next: CustomLayer[]) => void useStore.getState().updateSettings({ customLayers: next });
  const patch = (id: string, p: Partial<CustomLayer>) => save(list.map((c) => (c.id === id ? { ...c, ...p } : c)));
  const move = (i: number, d: -1 | 1) => {
    const j = i + d;
    if (j < 0 || j >= list.length) return;
    const next = [...list];
    [next[i], next[j]] = [next[j], next[i]];
    save(next);
  };
  const add = () => {
    const v = validateCustomLayer(draft);
    if (v) {
      setErr(t(v === "name" ? "customErrName" : v === "template" ? "customErrTemplate" : "customErrUrl"));
      return;
    }
    setErr(null);
    save([...list, { ...draft, id: newId(), name: draft.name.trim(), url: draft.url.trim() }]);
    setDraft({ id: "", name: "", kind: draft.kind, url: "", opacity: 0.8, visible: true });
  };
  const pickFile = async () => {
    const path = await openDialog({ multiple: false, filters: [{ name: draft.kind.toUpperCase(), extensions: [draft.kind] }] });
    if (typeof path === "string") setDraft((d) => ({ ...d, url: path, name: d.name || (path.split(/[\\/]/).pop() ?? "").replace(/\.\w+$/, "") }));
  };
  const local = draft.kind === "pmtiles" || draft.kind === "mbtiles";
  return (
    <div ref={ref} role="dialog" aria-label={t("customLayersTitle")} className="map-popover absolute top-3 end-[64px] z-30 w-[360px] max-h-[calc(100%-24px)] popover rounded-[var(--radius)] flex flex-col overflow-hidden no-print fade-in">
      <div className="flex items-center justify-between ps-4 pe-2 h-12 shrink-0">
        <h2 className="text-base font-semibold m-0">{t("customLayersTitle")}</h2>
        <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={close} aria-label={t("close")} title={t("close")}>
          <Icon as={X} size={16} />
        </button>
      </div>
      <div className="flex-1 min-h-0 scroll-y px-4 pb-4 flex flex-col gap-4">
        {list.length === 0 && <p className="m-0 text-sm text-muted">{t("customLayersEmpty")}</p>}
        {list.length > 0 && (
          <ul className="m-0 p-0 list-none group-list">
            {list.map((c, i) => (
              <li key={c.id} className="row flex-col !items-stretch gap-1.5">
                <div className="flex items-center gap-2">
                  <span className="flex-1 min-w-0">
                    <span className="block truncate text-sm">{c.name}</span>
                    <span className="block truncate text-xs text-muted" dir="ltr">
                      {c.kind.toUpperCase()} · {c.url}
                    </span>
                  </span>
                  <button type="button" role="switch" aria-checked={c.visible} aria-label={t("userShow")} className="switch" onClick={() => patch(c.id, { visible: !c.visible })} />
                </div>
                <div className="flex items-center gap-1.5">
                  <label className="text-xs text-muted shrink-0" htmlFor={`op-${c.id}`}>
                    {t("customOpacity")}
                  </label>
                  <input id={`op-${c.id}`} type="range" min={0.1} max={1} step={0.05} value={c.opacity} onChange={(e) => patch(c.id, { opacity: Number(e.target.value) })} className="flex-1" />
                  <button className="btn btn-ghost btn-icon btn-sm" onClick={() => move(i, -1)} disabled={i === 0} aria-label={t("moveUp")}>
                    <Icon as={ArrowUp} size={14} />
                  </button>
                  <button className="btn btn-ghost btn-icon btn-sm" onClick={() => move(i, 1)} disabled={i === list.length - 1} aria-label={t("moveDown")}>
                    <Icon as={ArrowDown} size={14} />
                  </button>
                  <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={() => {
                      save(list.filter((x) => x.id !== c.id));
                      // the backend keeps a disk cache of online layers: drop it with the layer
                      void apiX.customLayerClearCache(c.id).catch(() => undefined);
                    }} aria-label={t("delete")}>
                    <Icon as={Trash2} size={14} />
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
        <section className="flex flex-col gap-2">
          <h3 className="section-label m-0 inline-flex items-center gap-1.5">
            {t("customAdd")}
            <InfoTip text={t("customHint")} />
          </h3>
          <input className="input h-9" placeholder={t("customName")} aria-label={t("customName")} value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} />
          <div className="seg seg-sm" role="group" aria-label={t("customKind")}>
            {KINDS.map((k) => (
              <button key={k} aria-pressed={draft.kind === k} onClick={() => setDraft({ ...draft, kind: k, url: "" })}>
                <span>{k.toUpperCase()}</span>
              </button>
            ))}
          </div>
          <div className="flex gap-1.5">
            <input className="input h-9 flex-1 min-w-0" dir="ltr" placeholder={local ? t("customFile") : draft.kind === "wms" ? "https://…/wms" : "https://…/{z}/{x}/{y}.png"} aria-label={t("customUrl")} value={draft.url} onChange={(e) => setDraft({ ...draft, url: e.target.value })} />
            {local && (
              <button className="btn btn-sm h-9" onClick={() => void pickFile()} aria-label={t("customPickFile")}>
                <Icon as={FolderOpen} size={14} />
              </button>
            )}
          </div>
          {draft.kind === "wms" && <input className="input h-9" dir="ltr" placeholder={t("customWmsLayers")} aria-label={t("customWmsLayers")} value={draft.wmsLayers ?? ""} onChange={(e) => setDraft({ ...draft, wmsLayers: e.target.value })} />}
          <input className="input h-9" placeholder={t("customAttribution")} aria-label={t("customAttribution")} value={draft.attribution ?? ""} onChange={(e) => setDraft({ ...draft, attribution: e.target.value || undefined })} />
          {err && <div className="text-xs text-danger">{err}</div>}
          <button className="btn btn-primary btn-sm self-start" onClick={add}>
            <Icon as={Plus} size={14} /> {t("customAddButton")}
          </button>
        </section>
      </div>
    </div>
  );
}
