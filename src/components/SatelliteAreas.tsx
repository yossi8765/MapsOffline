import { useCallback, useEffect, useRef, useState } from "react";
import { Crop, MapPin, Pause, Play, Plus, RefreshCw, Search, Trash2, X } from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { InfoTip } from "./InfoTip";
import { InfoBar } from "./InfoBar";
import { api, apiX, type GeoResult, type LonLat, type SatelliteArea, type SatelliteAreaEstimate, type SatelliteAreaProgress, type SatelliteAreaStatus } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { askConfirm, fmtBytes } from "../lib/format";
import { useTauriEvent } from "../lib/useTauriEvent";
import { AREA_ZOOMS, RADII_KM, boxAround, fmtEstimate, newAreaId, withoutArea, zoomLabel, type Box } from "../lib/satAreas";

type Method = "place" | "map";

/**
 * Sharp aerial imagery for chosen areas (Data panel): the country archive stays at a moderate
 * zoom; cities / places the user cares about get their own sharp archive. Explanations sit behind
 * the "!" (clean-UI rule); visible are only the areas, their states and the form.
 */
export function SatelliteAreas() {
  const t = useT();
  const areas = useStore((s) => s.settings.satelliteAreas ?? []);
  const draft = useStore((s) => s.satAreaDraft);
  const [status, setStatus] = useState<Record<string, SatelliteAreaStatus>>({});
  const [progress, setProgress] = useState<Record<string, SatelliteAreaProgress>>({});
  const [paused, setPaused] = useState<Record<string, boolean>>({});
  const [adding, setAdding] = useState<{ center?: LonLat } | null>(null);
  const sectionRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(() => {
    void apiX
      .satelliteAreasStatus()
      .then((l) => setStatus(Object.fromEntries(l.map((s) => [s.id, s]))))
      .catch(() => undefined);
  }, []);
  useEffect(refresh, [refresh, areas.length]);
  useTauriEvent<SatelliteAreaProgress>("satellite-area-progress", (p) => {
    setProgress((m) => ({ ...m, [p.id]: p }));
    if (p.phase === "done" || p.phase === "failed" || p.phase === "cancelled") {
      refresh();
      void useStore.getState().refreshTiles();
    }
  });

  // started from the map's context menu: open the form around that point
  useEffect(() => {
    if (!draft) return;
    setAdding({ center: draft.center });
    requestAnimationFrame(() => sectionRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" }));
  }, [draft]);

  const install = (id: string) => {
    setProgress((m) => ({ ...m, [id]: { id, phase: "queued", done: 0, total: 0, message: "" } }));
    void apiX.satelliteAreaInstall(id).catch((e) => useStore.getState().toastError(e));
  };

  const remove = async (a: SatelliteArea) => {
    if (!(await askConfirm(t("satAreaDeleteConfirm").replace("{name}", a.name), t("delete"), t("cancel"), t("appName")))) return;
    try {
      await apiX.satelliteAreaDelete(a.id);
      await useStore.getState().updateSettings({ satelliteAreas: withoutArea(useStore.getState().settings.satelliteAreas ?? [], a.id) });
      setProgress((m) => {
        const n = { ...m };
        delete n[a.id];
        return n;
      });
      refresh();
      void useStore.getState().refreshTiles();
    } catch (e) {
      useStore.getState().toastError(e);
    }
  };

  const add = async (a: SatelliteArea) => {
    await useStore.getState().updateSettings({ satelliteAreas: [...(useStore.getState().settings.satelliteAreas ?? []), a] });
    setAdding(null);
    install(a.id);
  };

  return (
    <div ref={sectionRef} className="flex flex-col gap-2">
      <div className="flex items-center gap-2 px-0.5">
        <h2 className="section-label m-0 p-0">{t("satAreasTitle")}</h2>
        <InfoTip text={t("satAreasHint")} />
        <span className="flex-1" />
        {!adding && (
          <button className="btn btn-ghost btn-sm" onClick={() => setAdding({})}>
            <Icon as={Plus} size={14} /> {t("satAreaAdd")}
          </button>
        )}
      </div>
      {areas.length > 0 && (
        <div className="group-list">
          {areas.map((a) => {
            const st = status[a.id];
            const p = progress[a.id];
            const active = !!p && !["done", "failed", "cancelled"].includes(p.phase);
            const pct = p && p.total > 0 ? Math.min(100, Math.round((p.done / p.total) * 100)) : null;
            return (
              <div key={a.id} className="flex flex-col gap-1.5 px-3 py-2.5">
                <div className="flex items-center gap-2">
                  <div className="flex-1 min-w-0">
                    <div className="font-medium truncate-dir">{a.name}</div>
                    <div className="text-xs text-subtle num">
                      {zoomLabel(a.zoom, t)}
                      {st?.present && (
                        <>
                          {" · "}
                          <bdi dir="ltr">{fmtBytes(st.sizeBytes)}</bdi>
                        </>
                      )}
                    </div>
                  </div>
                  {active ? (
                    <>
                      <span className="pill pill-accent">{p.phase === "queued" ? t("phaseQueued") : paused[a.id] ? t("phasePaused") : t("phaseDownloading")}</span>
                      <Tip text={paused[a.id] ? t("resumeUpdate") : t("pauseUpdate")}>
                        <button
                          className="btn btn-ghost btn-icon btn-sm text-muted"
                          aria-label={paused[a.id] ? t("resumeUpdate") : t("pauseUpdate")}
                          onClick={() => {
                            const v = !paused[a.id];
                            setPaused((m) => ({ ...m, [a.id]: v }));
                            void apiX.satelliteAreaPause(a.id, v);
                          }}
                        >
                          <Icon as={paused[a.id] ? Play : Pause} size={14} />
                        </button>
                      </Tip>
                      <Tip text={t("cancelComponent")}>
                        <button className="btn btn-ghost btn-icon btn-sm text-muted" aria-label={t("cancelComponent")} onClick={() => void apiX.satelliteAreaCancel(a.id)}>
                          <Icon as={X} size={15} />
                        </button>
                      </Tip>
                    </>
                  ) : (
                    <>
                      {st?.present && !st.partial ? (
                        <span className="pill pill-success">{t("present")}</span>
                      ) : st?.partial ? (
                        <button className="btn btn-sm btn-primary" onClick={() => install(a.id)}>
                          {t("satAreaResume")}
                        </button>
                      ) : (
                        <button className="btn btn-sm btn-primary" onClick={() => install(a.id)}>
                          {t("downloadComponent")}
                        </button>
                      )}
                      {st?.present && !st.partial && (
                        <Tip text={t("update")}>
                          <button className="btn btn-ghost btn-icon btn-sm text-muted" aria-label={`${t("update")}: ${a.name}`} onClick={() => install(a.id)}>
                            <Icon as={RefreshCw} size={14} />
                          </button>
                        </Tip>
                      )}
                      <Tip text={t("delete")}>
                        <button className="btn btn-ghost btn-icon btn-sm text-muted" aria-label={`${t("delete")}: ${a.name}`} onClick={() => void remove(a)}>
                          <Icon as={Trash2} size={14} />
                        </button>
                      </Tip>
                    </>
                  )}
                </div>
                {active && (
                  <div className="flex flex-col gap-1 fade-in">
                    <div className="h-1 rounded-full bg-surface-2 overflow-hidden" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined} aria-label={a.name}>
                      <div className={clsx("h-full bg-accent rounded-full transition-[width] duration-300", pct === null && "animate-pulse w-1/3")} style={pct === null ? undefined : { width: `${pct}%` }} />
                    </div>
                    {p.message && <div className="text-xs text-muted truncate-dir">{p.message}</div>}
                  </div>
                )}
                {p?.phase === "failed" && (
                  <InfoBar severity="error" title={t("phaseFailed")}>
                    <span className="selectable break-words">{p.message}</span>
                  </InfoBar>
                )}
              </div>
            );
          })}
        </div>
      )}
      {adding && <AreaForm key={JSON.stringify(adding.center ?? null)} center={adding.center} onCancel={() => setAdding(null)} onAdd={(a) => void add(a)} />}
    </div>
  );
}

function AreaForm({ center, onCancel, onAdd }: { center?: LonLat; onCancel: () => void; onAdd: (a: SatelliteArea) => void }) {
  const t = useT();
  const [method, setMethod] = useState<Method>("place");
  const [name, setName] = useState(center ? t("satAreaHere") : "");
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<GeoResult[]>([]);
  const [point, setPoint] = useState<LonLat | null>(center ?? null);
  const [radius, setRadius] = useState(center ? 2 : 5);
  const [drawn, setDrawn] = useState<Box | null>(null);
  const [zoom, setZoom] = useState(17);
  const [est, setEst] = useState<SatelliteAreaEstimate | null>(null);

  const bbox: Box | null = method === "map" ? drawn : point ? boxAround(point, radius) : null;

  // place search (the offline index, or online search when it is missing)
  useEffect(() => {
    const q = query.trim();
    if (method !== "place" || q.length < 2) {
      setResults([]);
      return;
    }
    const h = setTimeout(() => {
      void api
        .geocodeSearch(q, useStore.getState().mapCenter, 6)
        .then(setResults)
        .catch(() => setResults([]));
    }, 250);
    return () => clearTimeout(h);
  }, [query, method]);

  // estimate of the chosen box
  const key = bbox ? `${bbox.join(",")}:${zoom}` : "";
  useEffect(() => {
    if (!bbox) {
      setEst(null);
      return;
    }
    const h = setTimeout(() => {
      void apiX
        .satelliteAreaEstimate(bbox, zoom)
        .then(setEst)
        .catch(() => setEst(null));
    }, 200);
    return () => clearTimeout(h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const pickOnMap = () => {
    setMethod("map");
    useStore.getState().pickArea((b) => {
      setDrawn(b);
      useStore.getState().fitToBox(b);
    });
  };

  const ready = !!bbox && !!est?.ok && name.trim().length > 0;

  return (
    <div className="card flex flex-col gap-3 p-3 fade-in">
      <label className="flex flex-col gap-1">
        <span className="text-sm">{t("satAreaName")}</span>
        <input className="input" value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
      </label>

      <div className="seg seg-sm" role="tablist" aria-label={t("satAreaHow")}>
        <button role="tab" aria-selected={method === "place"} aria-pressed={method === "place"} onClick={() => setMethod("place")}>
          <Icon as={Search} size={14} /> {t("satAreaByPlace")}
        </button>
        <button role="tab" aria-selected={method === "map"} aria-pressed={method === "map"} onClick={pickOnMap}>
          <Icon as={Crop} size={14} /> {t("satAreaOnMap")}
        </button>
      </div>

      {method === "place" ? (
        <div className="flex flex-col gap-2">
          {!center && (
            <input className="input" value={query} placeholder={t("satAreaSearchPh")} onChange={(e) => setQuery(e.target.value)} aria-label={t("satAreaByPlace")} />
          )}
          {results.length > 0 && !point && (
            <div className="group-list" role="listbox">
              {results.map((r) => (
                <button
                  key={r.id}
                  role="option"
                  aria-selected={false}
                  className="row text-start"
                  onClick={() => {
                    setPoint([r.lon, r.lat]);
                    if (!name.trim()) setName(r.name);
                    setRadius(r.kind === "place" ? 5 : 2);
                    useStore.getState().flyToPoint([r.lon, r.lat], 12);
                  }}
                >
                  <Icon as={MapPin} size={14} className="text-muted" />
                  <span className="truncate-dir">{r.name}</span>
                </button>
              ))}
            </div>
          )}
          {point && (
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-sm">{t("satAreaRadius")}</span>
              <div className="seg seg-sm seg-fit">
                {RADII_KM.map((km) => (
                  <button key={km} aria-pressed={radius === km} onClick={() => setRadius(km)}>
                    <bdi>{t("satKm").replace("{n}", String(km))}</bdi>
                  </button>
                ))}
              </div>
              {!center && (
                <button className="btn btn-ghost btn-sm" onClick={() => setPoint(null)}>
                  {t("satAreaOtherPlace")}
                </button>
              )}
            </div>
          )}
        </div>
      ) : (
        <div className="flex items-center gap-2 text-sm">
          <span className="flex-1 text-muted">{drawn ? t("satAreaDrawn") : t("areaPickHint")}</span>
          <button className="btn btn-sm" onClick={pickOnMap}>
            <Icon as={Crop} size={14} /> {drawn ? t("satAreaRedraw") : t("satAreaOnMap")}
          </button>
        </div>
      )}

      <div className="flex flex-col gap-1">
        <span className="text-sm">{t("satAreaSharpness")}</span>
        <div className="seg seg-sm">
          {AREA_ZOOMS.map((z) => (
            <button key={z.zoom} aria-pressed={zoom === z.zoom} onClick={() => setZoom(z.zoom)}>
              {t(z.label)}
            </button>
          ))}
        </div>
      </div>

      {bbox && est && (
        <div className={clsx("text-sm num", est.ok ? "text-muted" : "text-danger")}>
          {est.ok ? fmtEstimate(est, t) : t("satAreaTooLarge")}
        </div>
      )}

      <div className="flex gap-2">
        <button
          className="btn btn-primary flex-1"
          disabled={!ready}
          onClick={() => bbox && onAdd({ id: newAreaId(), name: name.trim(), bbox, zoom })}
        >
          {t("downloadComponent")}
        </button>
        <button
          className="btn"
          onClick={() => {
            useStore.getState().cancelAreaPick();
            onCancel();
          }}
        >
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}
