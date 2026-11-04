import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { Briefcase, Bus, Clock, Heart, Home, LocateFixed, MapPin, Milestone, Search, Star, Store, X } from "lucide-react";
import clsx from "clsx";
import { api, type GeoResult, type Favorite, type HistoryItem } from "../lib/api";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";

export interface Pick {
  lon: number;
  lat: number;
  label: string;
  result?: GeoResult;
}

interface Props {
  value: string;
  placeholder?: string;
  autoFocus?: boolean;
  focusTick?: number;
  onPick: (p: Pick) => void;
  onClear?: () => void;
  saveHistory?: boolean;
  /** Hide the "my location" row (e.g. when a bus stop must be picked). */
  noLocation?: boolean;
  compact?: boolean;
  leading?: React.ReactNode;
}

export function kindIcon(kind: GeoResult["kind"], size = 16) {
  switch (kind) {
    case "address":
      return <Home size={size} />;
    case "street":
      return <Milestone size={size} />;
    case "place":
      return <MapPin size={size} />;
    case "poi":
      return <Store size={size} />;
    case "stop":
      return <Bus size={size} />;
    case "coord":
      return <LocateFixed size={size} />;
  }
}

export const FAV_ICONS = ["home", "briefcase", "star", "heart", "map-pin"] as const;

export function favIcon(icon: string, size = 16) {
  switch (icon) {
    case "home":
      return <Home size={size} />;
    case "briefcase":
      return <Briefcase size={size} />;
    case "heart":
      return <Heart size={size} />;
    case "map-pin":
      return <MapPin size={size} />;
    default:
      return <Star size={size} />;
  }
}

/** Stop code of a transit-stop result (its category is "stop:<code>"). */
export function stopCode(r: GeoResult): string {
  return r.kind === "stop" && r.category?.startsWith("stop:") ? r.category.slice(5) : "";
}

export function resultLabel(r: GeoResult): string {
  if (r.kind === "address" && r.street && r.houseNumber) {
    const city = r.city ? `, ${r.city}` : "";
    return `${r.street} ${r.houseNumber}${city}`;
  }
  if (r.city && r.kind !== "place" && !r.name.includes(r.city)) return `${r.name}, ${r.city}`;
  return r.name;
}

/** English-layout keys → the Hebrew letters on the same keys (standard Israeli layout). */
const EN_TO_HE: Record<string, string> = {
  q: "/", w: "'", e: "ק", r: "ר", t: "א", y: "ט", u: "ו", i: "ן", o: "ם", p: "פ",
  a: "ש", s: "ד", d: "ג", f: "כ", g: "ע", h: "י", j: "ח", k: "ל", l: "ך", ";": "ף",
  z: "ז", x: "ס", c: "ב", v: "ה", b: "נ", n: "מ", m: "צ", ",": "ת", ".": "ץ", "/": ".",
};

/** The query as it was meant when typed with the English layout on, or null when it was not. */
export function keyboardFix(q: string): string | null {
  if (!/[a-z]/i.test(q) || /[֐-׿]/.test(q)) return null;
  return [...q.toLowerCase()].map((ch) => EN_TO_HE[ch] ?? ch).join("");
}

/** Final letters fold to their regular form so "שלום" matches "שלומ..." while typing. */
const FOLD: Record<string, string> = { ך: "כ", ם: "מ", ן: "נ", ף: "פ", ץ: "צ" };
const fold = (s: string) => [...s.toLowerCase()].map((c) => FOLD[c] ?? c).join("");

/** `name` with the parts that match a query word in bold (case and final-letter insensitive). */
export function Highlight({ text, query }: { text: string; query: string }) {
  const words = query
    .split(/[\s,]+/)
    .map((w) => fold(w.replace(/["'׳״]/g, "")))
    .filter((w) => w.length >= 2);
  if (!words.length) return <>{text}</>;
  const f = fold(text);
  const marks = new Array<boolean>(text.length).fill(false);
  for (const w of words) {
    let i = f.indexOf(w);
    while (i >= 0) {
      for (let k = i; k < i + w.length && k < marks.length; k++) marks[k] = true;
      i = f.indexOf(w, i + w.length);
    }
  }
  const out: React.ReactNode[] = [];
  let i = 0;
  while (i < text.length) {
    let j = i;
    while (j < text.length && marks[j] === marks[i]) j++;
    out.push(marks[i] ? <b key={i} className="font-semibold">{text.slice(i, j)}</b> : text.slice(i, j));
    i = j;
  }
  return <>{out}</>;
}

/** The results answer something else than what was typed (fuzzy street / city match): no query
 *  word of 3+ letters occurs in the best result. */
function reinterpreted(q: string, r: GeoResult | undefined): string | null {
  if (!r) return null;
  const words = q.split(/[\s,]+/).filter((w) => w.length >= 3 && !/\d/.test(w)).map(fold);
  if (!words.length) return null;
  const label = fold(resultLabel(r));
  return words.some((w) => label.includes(w)) ? null : resultLabel(r);
}

const PRECISION_KEY: Record<GeoResult["precision"], I18nKey> = {
  exact: "precisionExact",
  interpolated: "precisionInterpolated",
  street: "precisionStreet",
  city: "precisionCity",
};

type Row =
  | { type: "me" }
  | { type: "result"; r: GeoResult }
  | { type: "fav"; f: Favorite }
  | { type: "recent"; h: HistoryItem; r: GeoResult };

export function SearchBox({ value, placeholder, autoFocus, focusTick, onPick, onClear, saveHistory = true, compact, leading, noLocation }: Props) {
  const t = useT();
  const [text, setText] = useState(value);
  const [open, setOpen] = useState(false);
  const [results, setResults] = useState<GeoResult[]>([]);
  const [loading, setLoading] = useState(false);
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const seq = useRef(0);
  const favorites = useStore((s) => s.favorites);
  const history = useStore((s) => s.history);
  const mapCenter = useStore((s) => s.mapCenter);

  useEffect(() => setText(value), [value]);
  useEffect(() => {
    if (focusTick) inputRef.current?.focus();
  }, [focusTick]);

  // debounced search
  useEffect(() => {
    const q = text.trim();
    if (!open) return;
    if (!q) {
      // invalidate a request still in flight, or its results would land in the empty box
      seq.current++;
      setResults([]);
      setLoading(false);
      return;
    }
    const id = ++seq.current;
    setLoading(true);
    const h = setTimeout(async () => {
      try {
        const res = await api.geocodeSearch(q, mapCenter, 12);
        if (id === seq.current) {
          setResults(res);
          setActive(0);
        }
      } catch (e) {
        if (id === seq.current) {
          setResults([]);
          const code = String(e);
          if (!code.includes("NO_GEOCODE_DATA") && !code.includes("not implemented")) console.warn(e);
        }
      } finally {
        if (id === seq.current) setLoading(false);
      }
    }, 200);
    return () => clearTimeout(h);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, open]);

  const rows: Row[] = useMemo(() => {
    if (text.trim()) return results.map((r) => ({ type: "result", r }));
    const out: Row[] = [...(noLocation ? [] : [{ type: "me" } as Row]), ...favorites.slice(0, 5).map((f): Row => ({ type: "fav", f }))];
    const recents = history.filter((h) => h.kind === "search").slice(0, 6);
    for (const h of recents) {
      const r = h.payload as GeoResult | null;
      if (r && typeof r === "object" && "lon" in r) out.push({ type: "recent", h, r });
    }
    return out;
  }, [text, results, favorites, history, noLocation]);

  // the best result clearly beats the next one (score ratio above 1.6): mark it as "best match"
  const best = results.length > 0 && (results.length === 1 || (results[0].score ?? 0) > 1.6 * Math.max(results[1].score ?? 0, 1e-9));
  // the backend silently retries a Latin query as Hebrew typed on the wrong layout: say so
  const fixedQuery = useMemo(() => {
    const fixed = keyboardFix(text.trim());
    return fixed && results.length > 0 && /[֐-׿]/.test(resultLabel(results[0])) ? fixed : null;
  }, [text, results]);

  const removeRecent = async (id: number) => {
    try {
      await api.historyDelete(id);
      await useStore.getState().loadHistory();
    } catch (e) {
      useStore.getState().toastError(e);
    }
  };
  const clearRecent = async () => {
    try {
      await api.historyClear("search");
      await useStore.getState().loadHistory();
    } catch (e) {
      useStore.getState().toastError(e);
    }
  };

  const choose = async (row: Row) => {
    let pick: Pick;
    if (row.type === "me") {
      setOpen(false);
      const loc = await useStore.getState().locate(false);
      if (!loc) return;
      pick = { lon: loc.lon, lat: loc.lat, label: t("myLocation") };
      setText(pick.label);
      onPick(pick);
      return;
    }
    if (row.type === "fav") {
      pick = { lon: row.f.lon, lat: row.f.lat, label: row.f.name };
    } else {
      const r = row.r;
      pick = { lon: r.lon, lat: r.lat, label: resultLabel(r), result: r };
      if (saveHistory && row.type === "result") void useStore.getState().addHistory("search", pick.label, r);
    }
    setText(pick.label);
    setOpen(false);
    onPick(pick);
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (!open) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, rows.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (rows[active]) choose(rows[active]);
    } else if (e.key === "Escape") {
      setOpen(false);
      inputRef.current?.blur();
    }
  };

  return (
    <div className="relative">
      <div className="relative flex items-center">
        {leading ? (
          <span className="absolute start-3 w-4 flex items-center justify-center text-muted pointer-events-none">{leading}</span>
        ) : (
          <Search size={16} strokeWidth={1.75} className="absolute start-3 text-muted pointer-events-none" />
        )}
        <input
          ref={inputRef}
          className={clsx("input ps-10 pe-9 truncate", compact && "h-10")}
          placeholder={placeholder ?? t("searchPlaceholder")}
          value={text}
          autoFocus={autoFocus}
          onChange={(e) => {
            setText(e.target.value);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
          onBlur={() => setTimeout(() => setOpen(false), 150)}
          onKeyDown={onKey}
          role="combobox"
          aria-expanded={open}
        />
        {(text || value) && (
          <button
            className="absolute end-1.5 w-7 h-7 rounded flex items-center justify-center text-subtle hover:text-fg hover:bg-hover transition-colors"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              setText("");
              setResults([]);
              onClear?.();
              inputRef.current?.focus();
            }}
            aria-label={t("searchClear")}
            title={t("searchClear")}
          >
            <X size={14} />
          </button>
        )}
      </div>

      {open && (rows.length > 0 || loading || text.trim()) && (
        <div className="absolute z-40 mt-1.5 w-full popover rounded-[var(--radius-card)] overflow-hidden max-h-80 scroll-y py-1 fade-in" role="listbox" onMouseDown={(e) => e.preventDefault()}>
          {loading && rows.length === 0 && <div className="progress-line mx-3 my-3" role="progressbar" aria-busy="true" />}
          {fixedQuery ? (
            <div className="px-3 pt-1.5 pb-1 text-xs text-muted">
              {t("keyboardFixed")} <bdi className="font-medium text-fg">{fixedQuery}</bdi>
            </div>
          ) : (
            text.trim() &&
            !loading &&
            // the backend says how it read the query (abbreviation, typo, category + place);
            // otherwise fall back to the local guess
            (results[0]?.interpretation ?? reinterpreted(text.trim(), results[0])) && (
              <div className="px-3 pt-1.5 pb-1 text-xs text-muted">
                {t("showingFor")} <bdi className="font-medium text-fg">{results[0]?.interpretation ?? reinterpreted(text.trim(), results[0])}</bdi>
              </div>
            )
          )}
          {!loading && text.trim() && rows.length === 0 && (
            <div className="px-3 py-3 text-sm">
              <div className="font-medium">{t("searchNoResults")}</div>
              <div className="text-muted text-xs mt-1">{t("searchHint")}</div>
            </div>
          )}
          {rows.map((row, i) => {
            const isFav = row.type === "fav";
            const isMe = row.type === "me";
            const r = row.type === "fav" || row.type === "me" ? null : row.r;
            // section headers between favorites and recents (only while the box is empty)
            const header =
              !text.trim() && !isMe && (i === 0 || rows[i - 1].type !== row.type) ? (
                <div className="px-3 pt-1.5 pb-1 section-label flex items-center justify-between gap-2">
                  <span>{isFav ? t("favorites") : t("recent")}</span>
                  {row.type === "recent" && (
                    <button type="button" className="btn btn-ghost btn-sm text-muted h-6 -me-2 normal-case tracking-normal" onClick={() => void clearRecent()}>
                      {t("clearList")}
                    </button>
                  )}
                </div>
              ) : null;
            const name = isFav ? row.f.name : isMe ? t("myLocation") : resultLabel(r!);
            const secondary = isMe
              ? t("myLocationHint")
              : isFav
              ? (row.f.address ?? "")
              : r!.kind === "poi"
                ? (r!.category ?? "")
                : r!.kind === "stop"
                  ? [stopCode(r!) ? `${t("stopCode")} ${stopCode(r!)}` : "", r!.city && !name.includes(r!.city) ? r!.city : ""]
                      .filter(Boolean)
                      .join(" · ")
                  : r!.kind === "address" || r!.kind === "street"
                    ? ""
                    : r!.city && !name.includes(r!.city)
                      ? r!.city
                      : "";
            return (
              <Fragment key={i}>
              {header}
              <button
                role="option"
                aria-selected={i === active}
                className={clsx("w-full flex items-center gap-3 px-3 min-h-12 py-1.5 text-start transition-colors duration-100", i === active ? "bg-hover" : "")}
                onMouseEnter={() => setActive(i)}
                onClick={() => void choose(row)}
              >
                <span className={clsx("tile", (isFav || isMe) && "text-accent")}>
                  {isMe ? <LocateFixed size={16} strokeWidth={1.75} /> : isFav ? favIcon(row.f.icon) : row.type === "recent" ? <Clock size={16} strokeWidth={1.75} /> : kindIcon(r!.kind)}
                </span>
                <span className="flex-1 min-w-0">
                  <span className="block truncate text-base">{row.type === "result" ? <Highlight text={name} query={fixedQuery ?? text} /> : name}</span>
                  {(secondary || (row.type === "result" && i === 0 && best && rows.length > 1)) && (
                    <span className="block truncate text-xs text-muted">
                      {row.type === "result" && i === 0 && best && rows.length > 1 && <span className="text-accent font-medium">{t("bestMatch")}{secondary ? " · " : ""}</span>}
                      {secondary}
                    </span>
                  )}
                </span>
                {/* precision is news only when it is not exact */}
                {r && r.kind === "address" && r.precision !== "exact" && (
                  <span className={clsx("chip", r.precision === "interpolated" ? "chip-warning" : "chip-muted")}>
                    {t(PRECISION_KEY[r.precision])}
                  </span>
                )}
                {r && r.kind !== "address" && r.kind !== "coord" && row.type === "result" && (
                  <span className="text-xs text-subtle shrink-0">{t(`kind${r.kind[0].toUpperCase()}${r.kind.slice(1)}` as I18nKey)}</span>
                )}
                {row.type === "recent" && (
                  <span
                    role="button"
                    tabIndex={-1}
                    className="btn btn-ghost btn-icon btn-sm text-subtle hover:text-fg -me-1.5 shrink-0"
                    title={t("delete")}
                    aria-label={t("delete")}
                    onClick={(e) => {
                      e.stopPropagation();
                      void removeRecent(row.h.id);
                    }}
                  >
                    <X size={14} strokeWidth={2} />
                  </span>
                )}
              </button>
              </Fragment>
            );
          })}
        </div>
      )}
    </div>
  );
}
