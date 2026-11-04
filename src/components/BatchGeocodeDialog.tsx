import { useRef, useState } from "react";
import { FileSpreadsheet, Map as MapIcon, Save, X } from "lucide-react";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { Icon } from "./Icon";
import { InfoBar } from "./InfoBar";
import { api, apiX, type GeoResult } from "../lib/api";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { guessAddressColumn, guessDelimiter, parseCsv, toCsv, type Delimiter } from "../lib/csv";
import { useDialog } from "../lib/useDialog";
import { resultLabel } from "./SearchBox";

const CHUNK = 100;

const PRECISION_KEY: Record<GeoResult["precision"], I18nKey> = {
  exact: "precisionExact",
  interpolated: "precisionInterpolated",
  street: "precisionStreet",
  city: "precisionCity",
};

/** GeoJSON of the found rows: every CSV column becomes a property. */
export function batchToGeoJson(header: string[], rows: string[][], results: (GeoResult | null)[]): string {
  const features = rows.flatMap((r, i) => {
    const g = results[i];
    if (!g) return [];
    const props: Record<string, string> = {};
    header.forEach((h, j) => (props[h || `col${j + 1}`] = r[j] ?? ""));
    props.matched = resultLabel(g);
    props.precision = g.precision;
    return [{ type: "Feature", properties: props, geometry: { type: "Point", coordinates: [g.lon, g.lat] } }];
  });
  return JSON.stringify({ type: "FeatureCollection", features });
}

function Dialog() {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const close = () => useStore.getState().setBatchOpen(false);
  useDialog(ref, close);
  const [fileName, setFileName] = useState("");
  const [delim, setDelim] = useState<Delimiter>(",");
  const [header, setHeader] = useState<string[]>([]);
  const [rows, setRows] = useState<string[][]>([]);
  const [col, setCol] = useState(0);
  const [cityCol, setCityCol] = useState(-1);
  const [results, setResults] = useState<(GeoResult | null)[] | null>(null);
  const [done, setDone] = useState(0);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = async (f: File) => {
    setError(null);
    setResults(null);
    const text = await f.text();
    const d = guessDelimiter(text.replace(/^﻿/, "").split(/\r?\n/, 1)[0] ?? "");
    const all = parseCsv(text, d);
    if (all.length < 2) {
      setError(t("batchEmpty"));
      return;
    }
    setFileName(f.name);
    setDelim(d);
    setHeader(all[0]);
    setRows(all.slice(1));
    setCol(guessAddressColumn(all[0]));
    setCityCol(all[0].findIndex((h) => /עיר|ישוב|יישוב|city|town/i.test(h)));
  };
  const pick = () => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".csv,.txt,text/csv";
    input.onchange = () => {
      const f = input.files?.[0];
      if (f) void load(f);
    };
    input.click();
  };

  const run = async () => {
    setRunning(true);
    setDone(0);
    setError(null);
    const out: (GeoResult | null)[] = [];
    const queries = rows.map((r) => [r[col] ?? "", cityCol >= 0 ? (r[cityCol] ?? "") : ""].filter(Boolean).join(", "));
    try {
      for (let i = 0; i < queries.length; i += CHUNK) {
        const part = queries.slice(i, i + CHUNK);
        let res: (GeoResult | null)[];
        try {
          res = await apiX.geocodeBatch(part, useStore.getState().mapCenter);
        } catch {
          // a backend without geocode_batch: one search per row (slower, same result)
          res = await Promise.all(part.map((q) => (q.trim() ? api.geocodeSearch(q, useStore.getState().mapCenter, 1).then((r) => r[0] ?? null) : Promise.resolve(null))));
        }
        out.push(...res);
        setDone(Math.min(queries.length, i + CHUNK));
      }
      setResults(out);
    } catch (e) {
      setError(useStore.getState().t("errGeneric") + `: ${String(e)}`);
    } finally {
      setRunning(false);
    }
  };

  const found = results?.filter(Boolean).length ?? 0;
  const base = fileName.replace(/\.[^.]+$/, "") || "addresses";

  const saveCsv = async () => {
    if (!results) return;
    const head = [...header, "lon", "lat", "precision", "matched"];
    const body = rows.map((r, i) => {
      const g = results[i];
      return [...r, g ? g.lon.toFixed(6) : "", g ? g.lat.toFixed(6) : "", g ? g.precision : "", g ? resultLabel(g) : ""];
    });
    await saveText(`${base}-geocoded.csv`, "csv", toCsv([head, ...body], delim === "\t" ? "," : delim));
  };
  const saveGeo = async () => {
    if (!results) return;
    await saveText(`${base}.geojson`, "json", batchToGeoJson(header, rows, results));
  };
  const saveText = async (name: string, ext: "csv" | "json", text: string) => {
    const st = useStore.getState();
    try {
      // .csv is not an allowed export type of save_text_file: written as .txt next to it would
      // confuse; ask for .json / .txt names the backend accepts when it refuses .csv
      const path = await saveDialog({ defaultPath: name, filters: [{ name: ext.toUpperCase(), extensions: ext === "csv" ? ["csv", "txt"] : ["geojson", "json"] }] });
      if (!path) return;
      await api.saveTextFile(path, text);
      st.toast("success", `${t("exportSaved")} ${path}`);
    } catch (e) {
      st.toastError(e);
    }
  };
  const showOnMap = async () => {
    if (!results) return;
    const st = useStore.getState();
    try {
      await apiX.userLayerSave(base, "import", batchToGeoJson(header, rows, results));
      // the map side (user layers) listens for this and reloads its list
      window.dispatchEvent(new CustomEvent("user-layers-changed"));
      st.toast("success", t("batchOnMap"));
      close();
    } catch (e) {
      st.toastError(e);
    }
  };

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center dialog-backdrop no-print" onClick={close}>
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby="batch-title"
        className="dialog w-[640px] max-w-[calc(100%-32px)] max-h-[calc(100%-48px)] p-5 flex flex-col gap-4 fade-in"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h2 id="batch-title" className="text-xl font-semibold m-0">
            {t("batchTitle")}
          </h2>
          <button className="btn btn-ghost btn-icon btn-sm text-muted" onClick={close} aria-label={t("close")}>
            <Icon as={X} />
          </button>
        </div>
        <p className="m-0 text-muted">{t("batchHint")}</p>
        <div className="flex items-center gap-2">
          <button className="btn" onClick={pick}>
            <Icon as={FileSpreadsheet} /> {t("batchPick")}
          </button>
          {fileName && <bdi className="truncate-dir text-muted">{fileName}</bdi>}
        </div>
        {error && <InfoBar severity="error">{error}</InfoBar>}
        {header.length > 0 && (
          <div className="grid grid-cols-2 gap-3">
            <label className="flex flex-col gap-1 text-xs text-muted">
              {t("batchAddressColumn")}
              <select className="input h-9" value={col} onChange={(e) => setCol(Number(e.target.value))}>
                {header.map((h, i) => (
                  <option key={i} value={i}>
                    {h || `#${i + 1}`}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex flex-col gap-1 text-xs text-muted">
              {t("batchCityColumn")}
              <select className="input h-9" value={cityCol} onChange={(e) => setCityCol(Number(e.target.value))}>
                <option value={-1}>{t("none")}</option>
                {header.map((h, i) => (
                  <option key={i} value={i}>
                    {h || `#${i + 1}`}
                  </option>
                ))}
              </select>
            </label>
          </div>
        )}
        {rows.length > 0 && (
          <div className="flex flex-col gap-2 min-h-0">
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted flex-1">{t("batchRows").replace("{n}", String(rows.length))}</span>
              <button className="btn btn-primary" disabled={running} onClick={() => void run()}>
                {t("batchRun")}
              </button>
            </div>
            {running && (
              <div className="flex flex-col gap-1" aria-live="polite">
                <div className="h-1 rounded-full bg-surface-2 overflow-hidden" role="progressbar" aria-valuemin={0} aria-valuemax={rows.length} aria-valuenow={done} aria-label={t("batchRun")}>
                  <div className="h-full bg-accent rounded-full transition-[width] duration-300" style={{ width: `${(done / rows.length) * 100}%` }} />
                </div>
                <span className="text-xs text-muted num">
                  {done} / {rows.length}
                </span>
              </div>
            )}
            {results && (
              <>
                <p className="m-0 text-sm" aria-live="polite">
                  {t("batchFound").replace("{n}", String(found)).replace("{total}", String(rows.length))}
                </p>
                <div className="scroll-y max-h-60 border border-line rounded-[var(--radius-sm)]">
                  <table className="w-full text-sm border-collapse">
                    <thead className="sticky top-0 bg-surface-2">
                      <tr>
                        <th className="text-start px-2 py-1 font-semibold">{header[col]}</th>
                        <th className="text-start px-2 py-1 font-semibold">{t("batchMatched")}</th>
                        <th className="text-start px-2 py-1 font-semibold">{t("batchPrecision")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {rows.slice(0, 200).map((r, i) => {
                        const g = results[i];
                        return (
                          <tr key={i} className="border-t border-line">
                            <td className="px-2 py-1 truncate-dir max-w-48">{r[col]}</td>
                            <td className={clsx("px-2 py-1 truncate-dir max-w-56", !g && "text-danger")}>{g ? resultLabel(g) : t("batchNotFound")}</td>
                            <td className="px-2 py-1">{g ? <span className={clsx("chip", g.precision === "exact" ? "chip-success" : "chip-warning")}>{t(PRECISION_KEY[g.precision])}</span> : null}</td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
                <div className="flex flex-wrap gap-2 justify-end">
                  <button className="btn" onClick={() => void saveCsv()}>
                    <Icon as={Save} /> {t("batchSaveCsv")}
                  </button>
                  <button className="btn" onClick={() => void saveGeo()}>
                    <Icon as={Save} /> GeoJSON
                  </button>
                  <button className="btn btn-primary" disabled={!found} onClick={() => void showOnMap()}>
                    <Icon as={MapIcon} /> {t("batchShowOnMap")}
                  </button>
                </div>
              </>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

/** Address list (CSV) → coordinates, for offices and organisations: pick the column, run the
 *  offline geocoder on every row, save the result as CSV / GeoJSON or show it as a map layer. */
export function BatchGeocodeDialog() {
  const open = useStore((s) => s.batchOpen);
  return open ? <Dialog /> : null;
}
