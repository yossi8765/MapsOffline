import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, Plus, RefreshCw, Search, Trash2, X } from "lucide-react";
import clsx from "clsx";
import { api, type InstalledRegion, type RegionEntry, type RegionLocate, type RegionPart, type RegionProgress } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtBytes, fmtDateTime } from "../lib/format";

const CONTINENTS_HE: Record<string, string> = {
  africa: "אפריקה",
  antarctica: "אנטארקטיקה",
  asia: "אסיה",
  "australia-oceania": "אוסטרליה ואוקיאניה",
  "central-america": "מרכז אמריקה",
  europe: "אירופה",
  "north-america": "צפון אמריקה",
  "south-america": "דרום אמריקה",
  russia: "רוסיה",
};

const PARTS: RegionPart[] = ["map", "search", "routing"];
const PART_LABEL: Record<RegionPart, "regPartMap" | "regPartSearch" | "regPartRouting"> = { map: "regPartMap", search: "regPartSearch", routing: "regPartRouting" };
const ACTIVE = (p?: RegionProgress) => !!p && !["done", "failed", "cancelled"].includes(p.phase);

type PartProgress = Partial<Record<RegionPart, RegionProgress>>;

function localName(lang: string, id: string, iso: string | null, name: string, isContinent: boolean): string {
  if (lang === "en") return name;
  if (iso) {
    try {
      const n = new Intl.DisplayNames(["he"], { type: "region" }).of(iso.toUpperCase());
      if (n) return n;
    } catch {
      /* fall through to the English name */
    }
  }
  if (isContinent && CONTINENTS_HE[id]) return CONTINENTS_HE[id];
  return name;
}

export function RegionsSection() {
  const t = useT();
  const lang = useStore((s) => s.settings.language);
  const [installed, setInstalled] = useState<InstalledRegion[]>([]);
  const [progress, setProgress] = useState<Record<string, PartProgress>>({});
  const [running, setRunning] = useState<Set<string>>(new Set());
  const [pickerOpen, setPickerOpen] = useState(false);
  const [catalog, setCatalog] = useState<RegionEntry[] | null>(null);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [loadingCatalog, setLoadingCatalog] = useState(false);
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const [wantParts, setWantParts] = useState<Record<RegionPart, boolean>>({ map: true, search: true, routing: true });
  const [sizes, setSizes] = useState<Record<string, number | null | "loading">>({});

  const refreshInstalled = useCallback(async () => {
    try {
      setInstalled(await api.regionsInstalled());
    } catch (e) {
      useStore.getState().toastError(e);
    }
  }, []);

  useEffect(() => {
    void refreshInstalled();
  }, [refreshInstalled]);

  // Progress events from the backend (one per region and part).
  useEffect(() => {
    let un: (() => void) | undefined;
    let dead = false;
    void api
      .onRegionProgress((p) => {
        setProgress((cur) => {
          const next = { ...cur };
          const parts = { ...(next[p.id] ?? {}) };
          if (p.phase === "done" || p.phase === "cancelled") delete parts[p.part];
          else parts[p.part] = p;
          if (Object.keys(parts).length) next[p.id] = parts;
          else delete next[p.id];
          return next;
        });
        if (p.phase === "done" || p.phase === "failed" || p.phase === "cancelled") {
          void refreshInstalled();
          void useStore.getState().refreshTiles();
        }
      })
      .then((u) => {
        if (dead) u();
        else un = u;
      })
      .catch(() => {});
    return () => {
      dead = true;
      un?.();
    };
  }, [refreshInstalled]);

  const loadCatalog = useCallback(async (refresh: boolean) => {
    setLoadingCatalog(true);
    setCatalogError(null);
    try {
      setCatalog(await api.regionsCatalog(refresh));
    } catch (e) {
      setCatalogError(e instanceof Error ? e.message : typeof e === "string" ? e : t("regCatalogFailed"));
    } finally {
      setLoadingCatalog(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (pickerOpen && !catalog && !loadingCatalog && !catalogError) void loadCatalog(false);
  }, [pickerOpen, catalog, loadingCatalog, catalogError, loadCatalog]);

  // Download size of the selected region, fetched lazily.
  const requested = useRef(new Set<string>());
  useEffect(() => {
    if (!selected || requested.current.has(selected)) return;
    requested.current.add(selected);
    const id = selected;
    setSizes((s) => ({ ...s, [id]: "loading" }));
    api
      .regionSize(id)
      .then((r) => setSizes((s) => ({ ...s, [id]: r.pbfBytes })))
      .catch(() => setSizes((s) => ({ ...s, [id]: null })));
  }, [selected]);

  const install = useCallback(
    async (id: string, parts: RegionPart[]) => {
      if (parts.length === 0) return;
      setProgress((cur) => {
        const next = { ...cur };
        const kept = { ...(next[id] ?? {}) };
        for (const p of PARTS) if (kept[p]?.phase === "failed") delete kept[p];
        if (Object.keys(kept).length) next[id] = kept;
        else delete next[id];
        return next;
      });
      setRunning((s) => new Set(s).add(id));
      try {
        await api.regionInstall(id, parts);
      } catch (e) {
        useStore.getState().toastError(e);
      } finally {
        setRunning((s) => {
          const n = new Set(s);
          n.delete(id);
          return n;
        });
        void refreshInstalled();
        void useStore.getState().refreshTiles();
      }
    },
    [refreshInstalled],
  );

  const cancel = (id: string) => {
    api.regionCancel(id).catch((e) => useStore.getState().toastError(e));
  };

  const remove = async (r: InstalledRegion, label: string) => {
    if (!window.confirm(`${t("regDeleteConfirm")} ${label}`)) return;
    try {
      await api.regionDelete(r.id);
    } catch (e) {
      useStore.getState().toastError(e);
    }
    void refreshInstalled();
    void useStore.getState().refreshTiles();
  };

  // ---- catalogue helpers
  const byId = useMemo(() => new Map((catalog ?? []).map((e) => [e.id, e])), [catalog]);
  const nameOf = useCallback(
    (e: RegionEntry) => localName(lang, e.id, e.iso, e.name, e.parent === null),
    [lang],
  );
  const cmp = useCallback((a: RegionEntry, b: RegionEntry) => nameOf(a).localeCompare(nameOf(b), lang), [nameOf, lang]);
  const children = useMemo(() => {
    const m = new Map<string | null, RegionEntry[]>();
    for (const e of catalog ?? []) {
      const key = e.parent && byId.has(e.parent) ? e.parent : e.parent === null ? null : "__orphan";
      const arr = m.get(key) ?? [];
      arr.push(e);
      m.set(key, arr);
    }
    for (const arr of m.values()) arr.sort(cmp);
    // entries whose parent is missing from the catalogue go at the top level
    const orphans = m.get("__orphan");
    if (orphans) m.set(null, [...(m.get(null) ?? []), ...orphans].sort(cmp));
    return m;
  }, [catalog, byId, cmp]);

  const installedIds = useMemo(() => new Set(installed.map((r) => r.id)), [installed]);
  const q = query.trim().toLowerCase();
  const matches = useMemo(() => {
    if (!q || !catalog) return [];
    return catalog
      .filter((e) => `${nameOf(e)} ${e.name} ${e.id}`.toLowerCase().includes(q))
      .sort(cmp)
      .slice(0, 80);
  }, [q, catalog, nameOf, cmp]);

  // places by name (a city, a country, in any language) → the regions that hold them
  const [places, setPlaces] = useState<RegionLocate[] | null>(null);
  const [locating, setLocating] = useState(false);
  useEffect(() => {
    const text = query.trim();
    if (text.length < 2 || !pickerOpen) {
      setPlaces(null);
      return;
    }
    let dead = false;
    const timer = window.setTimeout(() => {
      setLocating(true);
      api
        .regionsLocate(text)
        .then((r) => !dead && setPlaces(r))
        .catch(() => !dead && setPlaces([]))
        .finally(() => !dead && setLocating(false));
    }, 700);
    return () => {
      dead = true;
      window.clearTimeout(timer);
    };
  }, [query, pickerOpen]);

  const selectedEntry = selected ? byId.get(selected) : undefined;

  const pick = (e: RegionEntry) => {
    setSelected(e.id);
    setWantParts({ map: true, search: true, routing: true });
  };

  const toggle = (id: string) =>
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const renderEntry = (e: RegionEntry, depth: number, flat: boolean) => {
    const kids = flat ? [] : (children.get(e.id) ?? []);
    const open = expanded.has(e.id);
    const isSel = selected === e.id;
    const parent = flat && e.parent ? byId.get(e.parent) : undefined;
    return (
      <div key={e.id}>
        <div
          className={clsx("flex items-center gap-1 pe-2 min-h-9 rounded-md", isSel ? "bg-surface-2" : "hover:bg-surface-2/60")}
          style={{ paddingInlineStart: 8 + depth * 16 }}
        >
          {kids.length > 0 ? (
            <button
              type="button"
              className="btn btn-ghost btn-icon btn-sm text-muted shrink-0"
              aria-label={open ? t("regCollapse") : t("regExpand")}
              aria-expanded={open}
              onClick={() => toggle(e.id)}
            >
              {open ? <ChevronDown size={14} strokeWidth={2} /> : <ChevronRight size={14} strokeWidth={2} className="rtl:-scale-x-100" />}
            </button>
          ) : (
            <span className="w-7 shrink-0" />
          )}
          <button
            type="button"
            className="flex-1 min-w-0 text-start bg-transparent border-0 p-0 py-1.5 text-sm cursor-pointer text-fg"
            onClick={() => {
              if (e.builtin) return;
              if (e.parent === null && kids.length > 0) toggle(e.id);
              else pick(e);
            }}
          >
            <span className="truncate">{nameOf(e)}</span>
            {parent && <span className="text-xs text-muted"> · {nameOf(parent)}</span>}
            {e.builtin && <span className="block text-xs text-muted">{t("regBuiltin")}</span>}
          </button>
          {installedIds.has(e.id) && <span className="pill pill-success shrink-0">{t("regInstalled")}</span>}
        </div>
        {open && kids.map((k) => renderEntry(k, depth + 1, false))}
      </div>
    );
  };

  const rootEntries = children.get(null) ?? [];

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between px-0.5">
        <h2 className="section-label m-0 p-0">{t("regTitle")}</h2>
        {!pickerOpen && (
          <button className="btn btn-sm -me-1" onClick={() => setPickerOpen(true)}>
            <Plus size={14} strokeWidth={2} /> {t("regAdd")}
          </button>
        )}
      </div>
      <p className="m-0 text-xs text-muted px-0.5">{t("regHint")}</p>

      {installed.length > 0 && (
        <div className="group-list">
          {installed.map((r) => (
            <InstalledCard
              key={r.id}
              r={r}
              label={localName(lang, r.id, r.iso, r.name, false)}
              prog={progress[r.id]}
              busy={r.busy || running.has(r.id) || PARTS.some((p) => ACTIVE(progress[r.id]?.[p]))}
              lang={lang}
              onUpdate={() => void install(r.id, PARTS.filter((p) => r[p].present).length ? PARTS.filter((p) => r[p].present) : PARTS)}
              onCancel={() => cancel(r.id)}
              onDelete={() => void remove(r, localName(lang, r.id, r.iso, r.name, false))}
            />
          ))}
        </div>
      )}
      {installed.length === 0 && !pickerOpen && <p className="m-0 text-xs text-subtle px-0.5">{t("regNone")}</p>}

      {/* progress for regions that are downloading but not (yet) listed as installed */}
      {Object.keys(progress)
        .filter((id) => !installedIds.has(id))
        .map((id) => (
          <div key={id} className="group-list">
            <ProgressBlock
              title={(() => {
                const e = byId.get(id);
                return e ? nameOf(e) : id;
              })()}
              prog={progress[id]}
              onCancel={() => cancel(id)}
            />
          </div>
        ))}

      {pickerOpen && (
        <div className="group-list fade-in">
          <div className="px-3 py-2.5 flex items-center gap-2">
            <div className="relative flex-1 min-w-0">
              <Search size={14} strokeWidth={1.75} className="absolute top-1/2 -translate-y-1/2 start-2.5 text-muted pointer-events-none" />
              <input
                className="input w-full !ps-8"
                value={query}
                placeholder={t("regSearchPlaceholder")}
                onChange={(e) => setQuery(e.target.value)}
                autoFocus
              />
            </div>
            <button className="btn btn-ghost btn-icon btn-sm text-muted shrink-0" onClick={() => setPickerOpen(false)} title={t("regClose")} aria-label={t("regClose")}>
              <X size={15} strokeWidth={2} />
            </button>
          </div>

          {selectedEntry && (
            <div className="px-3 py-3 flex flex-col gap-2.5 fade-in">
              <div className="text-base font-semibold">{nameOf(selectedEntry)}</div>
              <div className="text-xs text-muted num">
                {(() => {
                  const sz = sizes[selectedEntry.id];
                  if (sz === "loading" || sz === undefined) return t("regSizeLoading");
                  if (sz === null) return t("regSizeUnknown");
                  return (
                    <>
                      {t("regSizeSearch")} <bdi dir="ltr">{fmtBytes(sz)}</bdi> · {t("regSizeMap")} <bdi dir="ltr">{fmtBytes(sz * 1.5)}</bdi>
                    </>
                  );
                })()}
              </div>
              <div className="flex flex-wrap gap-1.5">
                {PARTS.map((p) => (
                  <button
                    key={p}
                    type="button"
                    className="choice"
                    aria-pressed={wantParts[p]}
                    onClick={() => setWantParts((w) => ({ ...w, [p]: !w[p] }))}
                  >
                    {t(PART_LABEL[p])}
                  </button>
                ))}
              </div>
              {installedIds.has(selectedEntry.id) && <p className="m-0 text-xs text-muted">{t("regAlreadyInstalled")}</p>}
              <button
                className="btn btn-primary w-full"
                disabled={!PARTS.some((p) => wantParts[p]) || running.has(selectedEntry.id)}
                onClick={() => {
                  const id = selectedEntry.id;
                  void install(id, PARTS.filter((p) => wantParts[p]));
                  setSelected(null);
                  setPickerOpen(false);
                  setQuery("");
                }}
              >
                {t("regDownload")}
              </button>
            </div>
          )}

          <div className="max-h-80 overflow-y-auto p-1.5">
            {loadingCatalog && <p className="m-0 px-2 py-3 text-xs text-muted">{t("regLoading")}</p>}
            {catalogError && (
              <div className="px-2 py-3 flex flex-col gap-2 items-start">
                <p className="m-0 text-xs text-danger break-words">
                  {t("regCatalogFailed")}: {catalogError}
                </p>
                <button className="btn btn-sm" onClick={() => void loadCatalog(true)}>
                  <RefreshCw size={14} strokeWidth={1.75} /> {t("regRetry")}
                </button>
              </div>
            )}
            {catalog && q && (locating || (places && places.length > 0)) && (
              <div className="flex flex-col gap-1 pb-2 mb-1 border-b border-line">
                <div className="px-2 pt-1 text-xs text-muted">{locating && !places ? t("regLocating") : t("regPlaces")}</div>
                {(places ?? []).map((pl, i) => (
                  <div key={i} className="px-2 py-1.5 flex flex-col gap-1.5">
                    <div className="text-sm">
                      <span className="font-medium">{pl.name}</span>
                      {pl.detail && <span className="text-xs text-muted"> · {pl.detail}</span>}
                    </div>
                    <div className="flex flex-wrap items-center gap-1">
                      <span className="text-xs text-muted">{t("regDownloadOne")}</span>
                      {pl.regions.map((id) => {
                        const e = byId.get(id);
                        if (!e) return null;
                        return (
                          <button
                            key={id}
                            type="button"
                            className="choice"
                            aria-pressed={selected === id}
                            disabled={e.builtin}
                            title={e.builtin ? t("regBuiltin") : undefined}
                            onClick={() => pick(e)}
                          >
                            {nameOf(e)}
                            {installedIds.has(id) && " ✓"}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                ))}
              </div>
            )}
            {catalog && q && (matches.length ? matches.map((e) => renderEntry(e, 0, true)) : !places?.length && !locating && <p className="m-0 px-2 py-3 text-xs text-muted">{t("regNoResults")}</p>)}
            {catalog && !q && rootEntries.map((e) => renderEntry(e, 0, false))}
          </div>
        </div>
      )}
    </div>
  );
}

function InstalledCard({
  r,
  label,
  prog,
  busy,
  lang,
  onUpdate,
  onCancel,
  onDelete,
}: {
  r: InstalledRegion;
  label: string;
  prog?: PartProgress;
  busy: boolean;
  lang: string;
  onUpdate: () => void;
  onCancel: () => void;
  onDelete: () => void;
}) {
  const t = useT();
  const partLine = (p: RegionPart) => {
    const st = r[p];
    return (
      <span key={p}>
        {t(PART_LABEL[p])}:{" "}
        {st.present ? <bdi dir="ltr">{fmtBytes(st.sizeBytes)}</bdi> : <span className="text-subtle">{t("regPartMissing")}</span>}
      </span>
    );
  };
  return (
    <div className="flex flex-col gap-2 px-3 py-3">
      <div className="flex items-start gap-3">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="text-base font-semibold">{label}</span>
            {busy ? <span className="pill pill-accent">{t("regBusy")}</span> : <span className="pill pill-success">{t("regInstalled")}</span>}
          </div>
          <div className="text-xs text-muted mt-0.5 flex flex-wrap gap-x-3 num">{PARTS.map(partLine)}</div>
          {r.updatedIso && (
            <div className="text-xs text-subtle mt-0.5">
              {t("updated")} <bdi dir="ltr">{fmtDateTime(r.updatedIso, lang)}</bdi>
            </div>
          )}
        </div>
        {!busy && (
          <div className="flex items-center gap-1 shrink-0 -me-1.5">
            <button className="btn btn-sm" onClick={onUpdate}>
              {t("update")}
            </button>
            <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={onDelete} title={t("regDelete")} aria-label={t("regDelete")}>
              <Trash2 size={15} strokeWidth={1.75} />
            </button>
          </div>
        )}
      </div>
      {(busy || prog) && <ProgressBlock prog={prog} onCancel={busy ? onCancel : undefined} />}
    </div>
  );
}

function ProgressBlock({ title, prog, onCancel }: { title?: string; prog?: PartProgress; onCancel?: () => void }) {
  const t = useT();
  const entries = PARTS.map((p) => prog?.[p]).filter((p): p is RegionProgress => !!p);
  return (
    <div className={clsx("flex flex-col gap-2 fade-in", title && "px-3 py-3")}>
      {title && <div className="text-base font-semibold">{title}</div>}
      {entries.length === 0 && onCancel && <div className="text-xs text-muted">{t("phaseQueued")}</div>}
      {entries.map((p) => {
        const active = ACTIVE(p);
        const pct = p.total > 0 ? Math.min(100, Math.round((p.done / p.total) * 100)) : null;
        return (
          <div key={p.part}>
            <div className="flex justify-between gap-2 text-xs mb-1.5">
              <span className={clsx(p.phase === "failed" ? "text-danger whitespace-normal break-words" : "text-muted truncate")}>
                {t(PART_LABEL[p.part])}
                {": "}
                {p.phase === "failed" ? `${t("phaseFailed")}${p.message ? ` - ${p.message}` : ""}` : p.message || t(p.phase === "queued" ? "phaseQueued" : "phaseDownloading")}
              </span>
              {pct !== null && active && <span className="text-muted num shrink-0">{pct}%</span>}
            </div>
            {active && (
              <div className="h-1 rounded-full bg-surface-2 overflow-hidden" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined}>
                <div
                  className={clsx("h-full bg-accent rounded-full transition-[width] duration-300 ease-out", pct === null && "animate-pulse w-1/3")}
                  style={pct !== null ? { width: `${pct}%` } : undefined}
                />
              </div>
            )}
          </div>
        );
      })}
      {onCancel && (
        <button className="btn btn-sm btn-danger self-start" onClick={onCancel}>
          <X size={14} strokeWidth={2} /> {t("regCancel")}
        </button>
      )}
    </div>
  );
}
