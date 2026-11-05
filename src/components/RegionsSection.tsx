import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, FileUp, Plus, RefreshCw, Search, Trash2, X } from "lucide-react";
import clsx from "clsx";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorText,
  type ConflictPolicy,
  type InstalledRegion,
  type ObfCatalogEntry,
  type RegionEntry,
  type RegionLocate,
  type RegionPart,
  type RegionProgress,
} from "../lib/api";
import { useStore } from "../lib/store";
import { useTauriEvent } from "../lib/useTauriEvent";
import { translate, useT, type I18nKey } from "../lib/i18n";
import { askConfirm, fmtBytes, fmtDateTime } from "../lib/format";
import { obfRegionId, useRegionQueue, type QueueItem } from "../lib/regionQueue";
import { InfoTip } from "./InfoTip";

/** Geofabrik continent ids → i18n keys (the names are translated in lib/i18n.ts). */
const CONTINENT_KEY: Record<string, I18nKey> = {
  africa: "continent_africa",
  antarctica: "continent_antarctica",
  asia: "continent_asia",
  "australia-oceania": "continent_australia_oceania",
  "central-america": "continent_central_america",
  europe: "continent_europe",
  "north-america": "continent_north_america",
  "south-america": "continent_south_america",
  russia: "continent_russia",
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
  if (isContinent && CONTINENT_KEY[id]) return translate(lang, CONTINENT_KEY[id]);
  return name;
}

/** OsmAnd continent keys → the i18n continent names. */
const OBF_CONTINENT_KEY: Record<string, I18nKey> = {
  africa: "continent_africa",
  asia: "continent_asia",
  "australia-oceania": "continent_australia_oceania",
  centralamerica: "continent_central_america",
  europe: "continent_europe",
  northamerica: "continent_north_america",
  southamerica: "continent_south_america",
  russia: "continent_russia",
};
/** Country words of OsmAnd file names that are not the country's English name. */
const OBF_ISO: Record<string, string> = { us: "US", gb: "GB", uk: "GB" };
/** Ask before downloads larger than this. */
const BIG_DOWNLOAD = 20 * 1024 ** 3;

type Tab = "geofabrik" | "osmand";

/** A checkbox that can show "some of the children" (indeterminate). */
function Check({ state, onChange, disabled, label }: { state: boolean | "some"; onChange: (v: boolean) => void; disabled?: boolean; label: string }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = state === "some";
  }, [state]);
  return (
    <input
      ref={ref}
      type="checkbox"
      className="w-4 h-4 shrink-0 accent-[var(--accent)] cursor-pointer disabled:cursor-default"
      checked={state === true}
      disabled={disabled}
      aria-label={label}
      onChange={(e) => onChange(e.target.checked)}
    />
  );
}

export function RegionsSection() {
  const t = useT();
  const lang = useStore((s) => s.settings.language);
  const [installed, setInstalled] = useState<InstalledRegion[]>([]);
  const [progress, setProgress] = useState<Record<string, PartProgress>>({});
  const [pickerOpen, setPickerOpen] = useState(false);
  // one source for every country (Data panel → map source)
  const tab: Tab = useStore((s) => (s.settings.mapSource === "osmand" ? "osmand" : "geofabrik"));
  const [catalog, setCatalog] = useState<RegionEntry[] | null>(null);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [loadingCatalog, setLoadingCatalog] = useState(false);
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [wantParts, setWantParts] = useState<Record<RegionPart, boolean>>({ map: true, search: true, routing: true });
  const [sizes, setSizes] = useState<Record<string, number | null | "loading">>({});
  const [policy, setPolicy] = useState<ConflictPolicy>("replace");
  // OsmAnd's list
  const [obfList, setObfList] = useState<ObfCatalogEntry[] | null>(null);
  const [obfError, setObfError] = useState<string | null>(null);
  const [obfLoading, setObfLoading] = useState(false);
  const [obfChecked, setObfChecked] = useState<Set<string>>(new Set());
  const queue = useRegionQueue((s) => s.items);
  const queueCurrent = useRegionQueue((s) => s.current);
  const running = useMemo(() => new Set(queueCurrent ? [queueCurrent.key] : []), [queueCurrent]);

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
  // a data package may bring countries with it
  useTauriEvent("data-updated", () => void refreshInstalled());

  // Progress events from the backend (one per region and part).
  useTauriEvent<RegionProgress>("region-progress", (p) => {
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
  });

  const loadCatalog = useCallback(async (refresh: boolean) => {
    setLoadingCatalog(true);
    setCatalogError(null);
    try {
      setCatalog(await api.regionsCatalog(refresh));
    } catch (e) {
      setCatalogError(errorText(e) || t("regCatalogFailed"));
    } finally {
      setLoadingCatalog(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    // the Geofabrik list also gives the OsmAnd tab its country names in Hebrew
    if (pickerOpen && !catalog && !loadingCatalog && !catalogError) void loadCatalog(false);
  }, [pickerOpen, catalog, loadingCatalog, catalogError, loadCatalog]);

  const loadObf = useCallback(async (refresh: boolean) => {
    setObfLoading(true);
    setObfError(null);
    try {
      setObfList(await api.obfCatalog(refresh));
    } catch (e) {
      setObfError(errorText(e) || t("regObfFailed"));
    } finally {
      setObfLoading(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    if (pickerOpen && tab === "osmand" && !obfList && !obfLoading && !obfError) void loadObf(false);
  }, [pickerOpen, tab, obfList, obfLoading, obfError, loadObf]);

  // a finished queue item clears its old failure lines and refreshes the list
  const prevCurrent = useRef<string | null>(null);
  useEffect(() => {
    const key = queueCurrent?.key ?? null;
    if (key && key !== prevCurrent.current) {
      setProgress((cur) => {
        const next = { ...cur };
        const kept = { ...(next[key] ?? {}) };
        for (const p of PARTS) if (kept[p]?.phase === "failed") delete kept[p];
        if (Object.keys(kept).length) next[key] = kept;
        else delete next[key];
        return next;
      });
    }
    if (!key && prevCurrent.current) void refreshInstalled();
    prevCurrent.current = key;
  }, [queueCurrent, refreshInstalled]);

  const enqueue = useRegionQueue((s) => s.enqueue);
  const install = (id: string, parts: RegionPart[], label: string) => {
    if (parts.length) enqueue([{ kind: "geofabrik", key: id, label, parts, policy }]);
  };

  const cancel = (id: string) => {
    // a waiting item just leaves the queue; the running one is stopped
    if (useRegionQueue.getState().items.some((i) => i.key === id)) useRegionQueue.getState().remove(id);
    else api.regionCancel(id).catch((e) => useStore.getState().toastError(e));
  };

  const remove = async (r: InstalledRegion, label: string) => {
    if (!(await askConfirm(`${t("regDeleteConfirm")} ${label}`, t("delete"), t("cancel"), t("appName")))) return;
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
    if (text.length < 2 || !pickerOpen || tab !== "geofabrik") {
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
  }, [query, pickerOpen, tab]);

  const toggle = (id: string) =>
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  // ---- standard download: selection (a checked region includes its sub-regions)
  const ancestorChecked = useCallback(
    (e: RegionEntry) => {
      let p = e.parent ? byId.get(e.parent) : undefined;
      while (p) {
        if (checked.has(p.id)) return true;
        p = p.parent ? byId.get(p.parent) : undefined;
      }
      return false;
    },
    [byId, checked],
  );
  const effective = useMemo(() => [...checked].map((id) => byId.get(id)).filter((e): e is RegionEntry => !!e && !e.builtin && !ancestorChecked(e)), [checked, byId, ancestorChecked]);
  const setCheck = (id: string, on: boolean) =>
    setChecked((s) => {
      const n = new Set(s);
      if (on) n.add(id);
      else n.delete(id);
      return n;
    });
  const rootEntries = children.get(null) ?? [];
  const worldIds = rootEntries.filter((e) => !e.builtin).map((e) => e.id);
  const worldState: boolean | "some" = worldIds.length > 0 && worldIds.every((id) => checked.has(id)) ? true : checked.size ? "some" : false;

  // download sizes of the checked regions, one HEAD request at a time
  useEffect(() => {
    const missing = effective.find((e) => sizes[e.id] === undefined);
    if (!missing) return;
    const id = missing.id;
    setSizes((s) => ({ ...s, [id]: "loading" }));
    api
      .regionSize(id)
      .then((r) => setSizes((s) => ({ ...s, [id]: r.pbfBytes })))
      .catch(() => setSizes((s) => ({ ...s, [id]: null })));
  }, [effective, sizes]);
  const gfSize = useMemo(() => {
    let bytes = 0;
    let pending = false;
    for (const e of effective) {
      const sz = sizes[e.id];
      if (sz === "loading" || sz === undefined) pending = true;
      else if (typeof sz === "number") bytes += (wantParts.search || wantParts.routing ? sz : 0) + (wantParts.map ? sz * 1.5 : 0);
    }
    return { bytes, pending };
  }, [effective, sizes, wantParts]);

  // ---- OsmAnd: continent → country → files, names in Hebrew where the country is known
  const isoByName = useMemo(() => {
    const m = new Map<string, string>();
    for (const e of catalog ?? []) if (e.iso) m.set(e.name.toLowerCase(), e.iso);
    return m;
  }, [catalog]);
  const obfCountryName = useCallback(
    (word: string) => {
      const key = word.toLowerCase().replace(/-/g, " ");
      const iso = OBF_ISO[key] ?? isoByName.get(key);
      return iso ? localName(lang, "", iso, word.replace(/-/g, " "), false) : word.replace(/-/g, " ");
    },
    [isoByName, lang],
  );
  const obfLabel = useCallback(
    (e: ObfCatalogEntry) => {
      const rest = e.name.slice(e.country.length).trim();
      return rest ? `${obfCountryName(e.country)} · ${rest}` : obfCountryName(e.country);
    },
    [obfCountryName],
  );
  const obfTree = useMemo(() => {
    const byCont = new Map<string, Map<string, ObfCatalogEntry[]>>();
    for (const e of obfList ?? []) {
      if (q && !`${obfLabel(e)} ${e.name}`.toLowerCase().includes(q)) continue;
      const c = byCont.get(e.continent) ?? new Map<string, ObfCatalogEntry[]>();
      const arr = c.get(e.country) ?? [];
      arr.push(e);
      c.set(e.country, arr);
      byCont.set(e.continent, c);
    }
    const contName = (k: string) => (OBF_CONTINENT_KEY[k] ? translate(lang, OBF_CONTINENT_KEY[k]) : k);
    return [...byCont.entries()]
      .map(([k, c]) => ({
        key: k,
        name: contName(k),
        groups: [...c.entries()].map(([country, files]) => ({ country, name: obfCountryName(country), files })).sort((a, b) => a.name.localeCompare(b.name, lang)),
      }))
      .sort((a, b) => a.name.localeCompare(b.name, lang));
  }, [obfList, q, lang, obfLabel, obfCountryName]);
  const groupState = (files: ObfCatalogEntry[]): boolean | "some" => {
    const n = files.filter((f) => obfChecked.has(f.file)).length;
    return n === 0 ? false : n === files.length ? true : "some";
  };
  const setObf = (files: ObfCatalogEntry[], on: boolean) =>
    setObfChecked((s) => {
      const n = new Set(s);
      for (const f of files) {
        if (on) n.add(f.file);
        else n.delete(f.file);
      }
      return n;
    });
  const allObf = obfList ?? [];
  const obfWorldState = groupState(allObf);
  const obfSelected = allObf.filter((e) => obfChecked.has(e.file));
  const obfBytes = obfSelected.reduce((n, e) => n + e.sizeBytes, 0);

  const selectedCount = tab === "geofabrik" ? effective.length : obfSelected.length;
  const totalBytes = tab === "geofabrik" ? gfSize.bytes : obfBytes;

  const startDownload = async () => {
    if (!selectedCount) return;
    if (totalBytes > BIG_DOWNLOAD && !(await askConfirm(`${t("regBigConfirm")} ${fmtBytes(totalBytes)} ${t("regBigConfirmTail")}`, t("regDownload"), t("cancel"), t("appName")))) return;
    let items: QueueItem[];
    if (tab === "geofabrik") {
      const parts = PARTS.filter((p) => wantParts[p]);
      if (!parts.length) return;
      items = effective.map((e) => ({ kind: "geofabrik", key: e.id, label: nameOf(e), parts, policy }));
      setChecked(new Set());
    } else {
      items = obfSelected.map((e) => ({ kind: "osmand", key: e.regionId, label: obfLabel(e), file: e.file, policy }));
      setObfChecked(new Set());
    }
    enqueue(items);
    setPickerOpen(false);
    setQuery("");
  };

  const importFiles = async () => {
    try {
      const picked = await openDialog({ title: t("regImportObf"), multiple: true, directory: false, filters: [{ name: t("regObfFilter"), extensions: ["obf", "zip"] }] });
      const paths = (Array.isArray(picked) ? picked : picked ? [picked] : []).filter((p): p is string => typeof p === "string");
      if (!paths.length) return;
      enqueue(paths.map((p) => ({ kind: "obfFile", key: obfRegionId(p), label: p.split(/[\\/]/).pop() ?? p, path: p, policy })));
    } catch (e) {
      useStore.getState().toastError(e);
    }
  };

  const renderEntry = (e: RegionEntry, depth: number, flat: boolean) => {
    const kids = flat ? [] : (children.get(e.id) ?? []);
    const open = expanded.has(e.id);
    const parent = flat && e.parent ? byId.get(e.parent) : undefined;
    const included = ancestorChecked(e);
    return (
      <div key={e.id}>
        <div className={clsx("flex items-center gap-1.5 pe-2 min-h-9 rounded", checked.has(e.id) ? "bg-surface-2" : "hover:bg-surface-2/60")} style={{ paddingInlineStart: 4 + depth * 16 }}>
          {kids.length > 0 ? (
            <button type="button" className="btn btn-ghost btn-icon btn-sm text-muted shrink-0" aria-label={open ? t("regCollapse") : t("regExpand")} aria-expanded={open} onClick={() => toggle(e.id)}>
              {open ? <ChevronDown size={14} strokeWidth={1.75} /> : <ChevronRight size={14} strokeWidth={1.75} className="rtl:-scale-x-100" />}
            </button>
          ) : (
            <span className="w-7 shrink-0" />
          )}
          <Check state={included || checked.has(e.id)} disabled={e.builtin || included} label={nameOf(e)} onChange={(v) => setCheck(e.id, v)} />
          <button
            type="button"
            className="flex-1 min-w-0 text-start bg-transparent border-0 p-0 py-1.5 text-sm cursor-pointer text-fg"
            onClick={() => {
              if (e.builtin || included) return;
              if (kids.length > 0 && !checked.has(e.id)) toggle(e.id);
              else setCheck(e.id, !checked.has(e.id));
            }}
          >
            <span className="truncate">{nameOf(e)}</span>
            {parent && <span className="text-xs text-muted"> · {nameOf(parent)}</span>}
            {e.builtin && <span className="block text-xs text-muted">{t("regBuiltin")}</span>}
          </button>
          {included && <span className="text-xs text-subtle shrink-0">{t("regIncluded")}</span>}
          {installedIds.has(e.id) && <span className="pill pill-success shrink-0">{t("regInstalled")}</span>}
        </div>
        {open && kids.map((k) => renderEntry(k, depth + 1, false))}
      </div>
    );
  };

  const obfRow = (e: ObfCatalogEntry, depth: number, label: string) => (
    <label key={e.file} className={clsx("flex items-center gap-1.5 pe-2 min-h-9 rounded cursor-pointer", obfChecked.has(e.file) ? "bg-surface-2" : "hover:bg-surface-2/60")} style={{ paddingInlineStart: 4 + depth * 16 }}>
      <span className="w-7 shrink-0" />
      <Check state={obfChecked.has(e.file)} label={label} onChange={(v) => setObf([e], v)} />
      <span className="flex-1 min-w-0 truncate text-sm py-1.5">{label}</span>
      <bdi dir="ltr" className="text-xs text-muted num shrink-0">
        {fmtBytes(e.sizeBytes)}
      </bdi>
      {installedIds.has(e.regionId) && <span className="pill pill-success shrink-0">{t("regInstalled")}</span>}
    </label>
  );

  const groupRow = (key: string, label: string, files: ObfCatalogEntry[], depth: number, bold = false) => {
    const open = expanded.has(key) || !!q;
    return (
      <div className={clsx("flex items-center gap-1.5 pe-2 min-h-9 rounded hover:bg-surface-2/60")} style={{ paddingInlineStart: 4 + depth * 16 }}>
        <button type="button" className="btn btn-ghost btn-icon btn-sm text-muted shrink-0" aria-label={open ? t("regCollapse") : t("regExpand")} aria-expanded={open} onClick={() => toggle(key)}>
          {open ? <ChevronDown size={14} strokeWidth={1.75} /> : <ChevronRight size={14} strokeWidth={1.75} className="rtl:-scale-x-100" />}
        </button>
        <Check state={groupState(files)} label={label} onChange={(v) => setObf(files, v)} />
        <button type="button" className={clsx("flex-1 min-w-0 text-start bg-transparent border-0 p-0 py-1.5 text-sm cursor-pointer text-fg truncate", bold && "font-medium")} onClick={() => toggle(key)}>
          {label}
        </button>
        <bdi dir="ltr" className="text-xs text-subtle num shrink-0">
          {fmtBytes(files.reduce((n, f) => n + f.sizeBytes, 0))}
        </bdi>
      </div>
    );
  };

  const policyChoice = installed.length > 0 && (
    <div className="flex flex-col gap-1.5">
      <span className="text-xs text-muted inline-flex items-center gap-1.5">
        {t("regPolicyTitle")}
        <InfoTip text={t("regPolicyHint")} />
      </span>
      <div className="flex flex-wrap gap-1.5" role="radiogroup" aria-label={t("regPolicyTitle")}>
        {(["replace", "keepBoth", "skip"] as const).map((p) => (
          <button key={p} type="button" role="radio" className="choice" aria-checked={policy === p} aria-pressed={policy === p} onClick={() => setPolicy(p)}>
            {t(p === "replace" ? "regPolicyReplace" : p === "keepBoth" ? "regPolicyKeepBoth" : "regPolicySkip")}
          </button>
        ))}
      </div>
    </div>
  );

  // OsmAnd countries in the list: Hebrew name and the list entry (for "update") need both
  // lists; both are cached on disk, so loading them here is cheap
  const hasOsmand = installed.some((r) => r.source === "osmand");
  useEffect(() => {
    if (!hasOsmand) return;
    if (!catalog && !loadingCatalog && !catalogError) void loadCatalog(false);
    if (!obfList && !obfLoading && !obfError) void loadObf(false);
  }, [hasOsmand, catalog, loadingCatalog, catalogError, loadCatalog, obfList, obfLoading, obfError, loadObf]);
  const osmandLabel = (r: InstalledRegion) => {
    const e = (obfList ?? []).find((x) => x.regionId === r.id);
    return e ? obfLabel(e) : obfCountryName(r.name);
  };

  // progress of countries that are downloading / waiting but not (yet) installed
  const labelOf = (id: string) => {
    const qi = [...queue, ...(queueCurrent ? [queueCurrent] : [])].find((i) => i.key === id);
    if (qi) return qi.label;
    const e = byId.get(id);
    return e ? nameOf(e) : id;
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center justify-between gap-2 px-0.5">
        <h2 className="section-label m-0 p-0 whitespace-nowrap">{t("regTitle")}</h2>
        <div className="flex items-center gap-1 -me-1 ms-auto">
          <button className="btn btn-sm btn-ghost" onClick={() => void importFiles()} title={t("regImportObf")}>
            <FileUp size={14} strokeWidth={1.75} /> {t("regImportObf")}
          </button>
          {!pickerOpen && (
            <button className="btn btn-sm" onClick={() => setPickerOpen(true)}>
              <Plus size={14} strokeWidth={1.75} /> {t("regAdd")}
            </button>
          )}
        </div>
      </div>
      <p className="m-0 text-xs text-muted px-0.5">{t("regHint")}</p>

      {installed.length > 0 && (
        <div className="group-list">
          {installed.map((r) => {
            const label = r.source === "osmand" ? osmandLabel(r) : localName(lang, r.id, r.iso, r.name, false);
            const obfEntry = r.source === "osmand" ? (obfList ?? []).find((e) => e.regionId === r.id) : undefined;
            return (
              <InstalledCard
                key={r.id}
                r={r}
                label={label}
                prog={progress[r.id]}
                busy={r.busy || running.has(r.id) || PARTS.some((p) => ACTIVE(progress[r.id]?.[p]))}
                lang={lang}
                onUpdate={
                  r.source === "osmand"
                    ? obfEntry
                      ? () => enqueue([{ kind: "osmand", key: r.id, label, file: obfEntry.file, policy: "replace" }])
                      : undefined
                    : () => install(r.id, PARTS.filter((p) => r[p].present).length ? PARTS.filter((p) => r[p].present) : PARTS, label)
                }
                onCancel={() => cancel(r.id)}
                onDelete={() => void remove(r, label)}
              />
            );
          })}
        </div>
      )}
      {installed.length === 0 && !pickerOpen && !queue.length && !queueCurrent && <p className="m-0 text-xs text-subtle px-0.5">{t("regNone")}</p>}

      {Object.keys(progress)
        .filter((id) => !installedIds.has(id))
        .map((id) => (
          <div key={id} className="group-list">
            <ProgressBlock title={labelOf(id)} prog={progress[id]} onCancel={() => cancel(id)} />
          </div>
        ))}

      {queue.length > 0 && (
        <div className="group-list">
          <div className="px-3 py-2.5 flex flex-col gap-1.5">
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs text-muted">
                {t("regQueue")} <bdi className="num">({queue.length})</bdi>
              </span>
              <button className="btn btn-sm btn-ghost text-danger" onClick={() => useRegionQueue.getState().clear()}>
                {t("regQueueClear")}
              </button>
            </div>
            {queue.map((i) => (
              <div key={i.key} className="flex items-center gap-2 text-sm">
                <span className="flex-1 min-w-0 truncate">{i.label}</span>
                <button className="btn btn-ghost btn-icon btn-sm text-muted" title={t("regRemoveFromQueue")} aria-label={t("regRemoveFromQueue")} onClick={() => useRegionQueue.getState().remove(i.key)}>
                  <X size={14} strokeWidth={1.75} />
                </button>
              </div>
            ))}
          </div>
        </div>
      )}

      {pickerOpen && (
        <div className="group-list fade-in">
          <div className="px-3 pt-2.5 text-xs text-muted">{t(tab === "geofabrik" ? "regTabDownload" : "regTabOsmand")}</div>
          <div className="px-3 py-2.5 flex items-center gap-2">
            <div className="relative flex-1 min-w-0">
              <Search size={14} strokeWidth={1.75} className="absolute top-1/2 -translate-y-1/2 start-2.5 text-muted pointer-events-none" />
              <input className="input w-full !ps-8" value={query} placeholder={t("regSearchPlaceholder")} onChange={(e) => setQuery(e.target.value)} autoFocus />
            </div>
            <button className="btn btn-ghost btn-icon btn-sm text-muted shrink-0" onClick={() => setPickerOpen(false)} title={t("regClose")} aria-label={t("regClose")}>
              <X size={15} strokeWidth={1.75} />
            </button>
          </div>

          <div className="px-3 pb-2 flex items-center gap-1.5">
            {tab === "geofabrik" ? (
              <Check state={worldState} disabled={!catalog} label={t("regSelectAll")} onChange={(v) => setChecked(v ? new Set(worldIds) : new Set())} />
            ) : (
              <Check state={obfWorldState} disabled={!obfList} label={t("regSelectAll")} onChange={(v) => setObf(allObf, v)} />
            )}
            <span className="text-sm">{t("regSelectAll")}</span>
          </div>

          <div className="max-h-80 overflow-y-auto p-1.5 border-t border-line">
            {tab === "geofabrik" && (
              <>
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
                              <button key={id} type="button" className="choice" aria-pressed={checked.has(id)} disabled={e.builtin} title={e.builtin ? t("regBuiltin") : undefined} onClick={() => setCheck(id, !checked.has(id))}>
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
              </>
            )}
            {tab === "osmand" && (
              <>
                {obfLoading && <p className="m-0 px-2 py-3 text-xs text-muted">{t("regObfLoading")}</p>}
                {obfError && (
                  <div className="px-2 py-3 flex flex-col gap-2 items-start">
                    <p className="m-0 text-xs text-danger break-words">
                      {t("regObfFailed")}: {obfError}
                    </p>
                    <button className="btn btn-sm" onClick={() => void loadObf(true)}>
                      <RefreshCw size={14} strokeWidth={1.75} /> {t("regRetry")}
                    </button>
                  </div>
                )}
                {obfList && q && obfTree.length === 0 && <p className="m-0 px-2 py-3 text-xs text-muted">{t("regNoResults")}</p>}
                {obfTree.map((c) => {
                  const contFiles = c.groups.flatMap((g) => g.files);
                  const cKey = `obf:${c.key}`;
                  return (
                    <div key={c.key}>
                      {groupRow(cKey, c.name, contFiles, 0, true)}
                      {(expanded.has(cKey) || !!q) &&
                        c.groups.map((g) =>
                          g.files.length === 1 ? (
                            obfRow(g.files[0], 1, obfLabel(g.files[0]))
                          ) : (
                            <div key={g.country}>
                              {groupRow(`obf:${c.key}:${g.country}`, g.name, g.files, 1)}
                              {(expanded.has(`obf:${c.key}:${g.country}`) || !!q) && g.files.map((f) => obfRow(f, 2, f.name.slice(f.country.length).trim() || g.name))}
                            </div>
                          ),
                        )}
                    </div>
                  );
                })}
              </>
            )}
          </div>

          {selectedCount > 0 && (
            <div className="px-3 py-3 flex flex-col gap-2.5 border-t border-line fade-in">
              <div className="text-sm">
                {t("regSelected")} <bdi className="num">{selectedCount}</bdi>
                <span className="text-xs text-muted num">
                  {" · "}
                  {t("regSizeTotal")} <bdi dir="ltr">~{fmtBytes(totalBytes)}</bdi>
                  {tab === "geofabrik" && gfSize.pending && ` (${t("regSizeComputing")})`}
                </span>
              </div>
              {tab === "geofabrik" && (
                <div className="flex flex-wrap gap-1.5">
                  {PARTS.map((p) => (
                    <button key={p} type="button" className="choice" aria-pressed={wantParts[p]} onClick={() => setWantParts((w) => ({ ...w, [p]: !w[p] }))}>
                      {t(PART_LABEL[p])}
                    </button>
                  ))}
                </div>
              )}
              {policyChoice}
              <button className="btn btn-primary w-full" disabled={tab === "geofabrik" && !PARTS.some((p) => wantParts[p])} onClick={() => void startDownload()}>
                {t("regDownloadSelected")}
              </button>
            </div>
          )}
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
  /** missing: an OsmAnd country imported from a local file (update = import a newer file) */
  onUpdate?: () => void;
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
            {r.source === "osmand" && <span className="pill">{t("regSourceOsmand")}</span>}
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
            <button className="btn btn-sm" onClick={onUpdate} disabled={!onUpdate} title={onUpdate ? undefined : t("regNoOsmandFile")}>
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
  const all = PARTS.map((p) => prog?.[p]).filter((p): p is RegionProgress => !!p);
  // parts fed by one download (the OSM extract for search + routing, an OsmAnd file for all
  // three) report the same bytes: one bar named after all of them instead of identical bars
  const shared = (p: RegionProgress) => p.phase === "downloading" && p.total > 0;
  const entries: (RegionProgress & { parts: RegionPart[] })[] = [];
  for (const p of all) {
    const same = shared(p) ? entries.find((e) => shared(e) && e.total === p.total) : undefined;
    if (same) same.parts.push(p.part);
    else entries.push({ ...p, parts: [p.part] });
  }
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
                {p.parts.length > 1 ? `${t("sharedDownloadOf")} ${p.parts.map((x) => t(PART_LABEL[x])).join(", ")}` : t(PART_LABEL[p.part])}
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
          <X size={14} strokeWidth={1.75} /> {t("regCancel")}
        </button>
      )}
    </div>
  );
}
