import { Fragment, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Briefcase, Bus, Clock, Heart, Home, LocateFixed, MapPin, Milestone, Search, Star, Store, X } from "lucide-react";
import clsx from "clsx";
import { Icon } from "./Icon";
import { api, errorCode, type GeoResult, type Favorite, type HistoryItem } from "../lib/api";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { fold, matchRank, matchesAll } from "../lib/text";

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
  /** Accessible name when there is no visible label (the placeholder is not one). */
  label?: string;
}

export function kindIcon(kind: GeoResult["kind"], size = 16) {
  switch (kind) {
    case "address":
      return <Icon as={Home} size={size} />;
    case "street":
      return <Icon as={Milestone} size={size} />;
    case "place":
      return <Icon as={MapPin} size={size} />;
    case "poi":
      return <Icon as={Store} size={size} />;
    case "stop":
      return <Icon as={Bus} size={size} />;
    case "coord":
      return <Icon as={LocateFixed} size={size} />;
  }
}

export const FAV_ICONS = ["home", "briefcase", "star", "heart", "map-pin"] as const;

export function favIcon(icon: string, size = 16) {
  switch (icon) {
    case "home":
      return <Icon as={Home} size={size} />;
    case "briefcase":
      return <Icon as={Briefcase} size={size} />;
    case "heart":
      return <Icon as={Heart} size={size} />;
    case "map-pin":
      return <Icon as={MapPin} size={size} />;
    case "bus":
      return <Icon as={Bus} size={size} />;
    default:
      return <Icon as={Star} size={size} />;
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

/** `name` with the parts that match a query word in bold (case and final-letter insensitive). */
export function Highlight({ text, query }: { text: string; query: string }) {
  const words = query
    .split(/[\s,]+/)
    .map((w) => fold(w))
    .filter((w) => w.length >= 2);
  if (!words.length) return <>{text}</>;
  // fold() drops quote marks: compare char by char on a same-length fold (finals + case only)
  const f = [...text.toLowerCase()].map((c) => fold(c) || " ").join("");
  const marks = new Array<boolean>(f.length).fill(false);
  for (const w of words) {
    let i = f.indexOf(w);
    while (i >= 0) {
      for (let k = i; k < i + w.length && k < marks.length; k++) marks[k] = true;
      i = f.indexOf(w, i + w.length);
    }
  }
  const chars = [...text];
  const out: React.ReactNode[] = [];
  let i = 0;
  while (i < chars.length) {
    let j = i;
    while (j < chars.length && marks[j] === marks[i]) j++;
    const part = chars.slice(i, j).join("");
    out.push(marks[i] ? <b key={i} className="font-semibold">{part}</b> : part);
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

/** Favorites whose name or address matches the typed text, best first (shown above the results). */
export function matchingFavorites(favs: Favorite[], q: string, max = 3): Favorite[] {
  if (!q.trim()) return [];
  return favs
    .filter((f) => f.kind !== "line" && f.kind !== "route" && matchesAll(`${f.name} ${f.address ?? ""}`, q))
    .sort((a, b) => matchRank(a.name, q) - matchRank(b.name, q))
    .slice(0, max);
}

export function SearchBox({ value, placeholder, autoFocus, focusTick, onPick, onClear, saveHistory = true, compact, leading, noLocation, label }: Props) {
  const t = useT();
  const [text, setText] = useState(value);
  const [open, setOpen] = useState(false);
  // typed since the last pick: a focus without typing shows favorites / recents only when the
  // box is empty, so coming back to a picked place does not throw its results over the card
  const [dirty, setDirty] = useState(false);
  const [results, setResults] = useState<GeoResult[]>([]);
  const [loading, setLoading] = useState(false);
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const [rect, setRect] = useState<DOMRect | null>(null);
  const seq = useRef(0);
  const listId = useId();
  const favorites = useStore((s) => s.favorites);
  const history = useStore((s) => s.history);

  useEffect(() => {
    setText(value);
    setDirty(false);
  }, [value]);
  useEffect(() => {
    if (focusTick) inputRef.current?.focus();
  }, [focusTick]);

  // debounced search
  useEffect(() => {
    const q = text.trim();
    if (!open || !dirty) return;
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
        // the map centre at the time of the query (not a subscription: panning must not re-render the box)
        const res = await api.geocodeSearch(q, useStore.getState().mapCenter, 12);
        if (id === seq.current) {
          setResults(res);
          setActive(0);
        }
      } catch (e) {
        if (id === seq.current) {
          setResults([]);
          const code = errorCode(e);
          if (code !== "NO_GEOCODE_DATA" && code !== "NOT_IMPLEMENTED") console.warn(e);
        }
      } finally {
        if (id === seq.current) setLoading(false);
      }
    }, 200);
    return () => clearTimeout(h);
  }, [text, open, dirty]);

  const rows: Row[] = useMemo(() => {
    const q = text.trim();
    if (q && dirty) {
      const favs = matchingFavorites(favorites, q).map((f): Row => ({ type: "fav", f }));
      return [...favs, ...results.map((r): Row => ({ type: "result", r }))];
    }
    if (q) return [];
    const out: Row[] = [...(noLocation ? [] : [{ type: "me" } as Row]), ...favorites.filter((f) => f.kind !== "line" && f.kind !== "route").slice(0, 5).map((f): Row => ({ type: "fav", f }))];
    const recents = history.filter((h) => h.kind === "search").slice(0, 6);
    for (const h of recents) {
      const r = h.payload as GeoResult | null;
      if (r && typeof r === "object" && "lon" in r) out.push({ type: "recent", h, r });
    }
    return out;
  }, [text, dirty, results, favorites, history, noLocation]);

  const firstResult = rows.findIndex((r) => r.type === "result");
  // the best result clearly beats the next one (score ratio above 1.6): mark it as "best match"
  const best = results.length > 0 && (results.length === 1 || (results[0].score ?? 0) > 1.6 * Math.max(results[1].score ?? 0, 1e-9));
  // the backend silently retries a Latin query as Hebrew typed on the wrong layout: say so
  const fixedQuery = useMemo(() => {
    const fixed = keyboardFix(text.trim());
    return fixed && results.length > 0 && /[֐-׿]/.test(resultLabel(results[0])) ? fixed : null;
  }, [text, results]);

  const shown = open && (rows.length > 0 || loading || (dirty && !!text.trim()));

  // the list floats above the panel (portal): it must not stretch the panel's scroll area
  useLayoutEffect(() => {
    if (!shown) return;
    const measure = () => boxRef.current && setRect(boxRef.current.getBoundingClientRect());
    measure();
    window.addEventListener("resize", measure);
    window.addEventListener("scroll", measure, true);
    return () => {
      window.removeEventListener("resize", measure);
      window.removeEventListener("scroll", measure, true);
    };
  }, [shown]);

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
      setDirty(false);
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
    setDirty(false);
    setOpen(false);
    onPick(pick);
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (!shown) {
      if (e.key === "ArrowDown") {
        setOpen(true);
        setDirty(true);
      }
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, rows.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (rows[active]) void choose(rows[active]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    } else if (e.key === "Delete" && rows[active]?.type === "recent") {
      // the keyboard way to drop a recent search (the × is mouse-only)
      e.preventDefault();
      const row = rows[active] as Extract<Row, { type: "recent" }>;
      void removeRecent(row.h.id);
    }
  };

  const optionId = (i: number) => `${listId}-opt-${i}`;

  const list =
    shown && rect
      ? createPortal(
          <div
            id={listId}
            className="fixed z-[60] popover rounded-[var(--radius)] overflow-hidden max-h-[min(420px,60vh)] scroll-y py-1 fade-in no-print"
            style={{ top: rect.bottom + 6, left: rect.left, width: rect.width }}
            role="listbox"
            aria-label={label ?? placeholder ?? t("searchPlaceholder")}
            dir={document.documentElement.dir}
            onMouseDown={(e) => e.preventDefault()}
          >
            {loading && rows.length === 0 && <div className="progress-line mx-3 my-3" role="progressbar" aria-busy="true" aria-label={t("loading")} />}
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
              <div className="px-3 py-3">
                <div className="font-medium">{t("searchNoResults")}</div>
                <div className="text-muted text-xs mt-1">{t("searchHint")}</div>
              </div>
            )}
            {rows.map((row, i) => {
              const isFav = row.type === "fav";
              const isMe = row.type === "me";
              const r = row.type === "fav" || row.type === "me" ? null : row.r;
              // section headers between favorites, recents and results
              const showHeader = !isMe && (i === 0 || rows[i - 1].type !== row.type) && (isFav || row.type === "recent");
              const header = showHeader ? (
                <div className="px-3 pt-1.5 pb-1 section-label flex items-center justify-between gap-2" role="presentation">
                  <span>{isFav ? t("favorites") : t("recent")}</span>
                  {row.type === "recent" && (
                    <button type="button" tabIndex={-1} className="btn btn-ghost btn-xs text-muted -me-2" onClick={() => void clearRecent()}>
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
                    ? (r!.categoryLabel ?? r!.category ?? "")
                    : r!.kind === "stop"
                      ? [stopCode(r!) ? `${t("stopCode")} ${stopCode(r!)}` : "", r!.city && !name.includes(r!.city) ? r!.city : ""].filter(Boolean).join(" · ")
                      : r!.kind === "address" || r!.kind === "street"
                        ? ""
                        : r!.city && !name.includes(r!.city)
                          ? r!.city
                          : "";
              const isBest = row.type === "result" && i === firstResult && best && results.length > 1;
              return (
                <Fragment key={i}>
                  {header}
                  <div
                    id={optionId(i)}
                    role="option"
                    aria-selected={i === active}
                    className={clsx("w-full flex items-center gap-3 px-3 min-h-12 py-1.5 text-start cursor-pointer transition-colors duration-100", i === active ? "bg-hover" : "")}
                    onMouseEnter={() => setActive(i)}
                    onClick={() => void choose(row)}
                  >
                    <span className={clsx("tile", (isFav || isMe) && "text-accent")} style={isFav && row.f.color ? { color: row.f.color } : undefined}>
                      {isMe ? <Icon as={LocateFixed} /> : isFav ? favIcon(row.f.icon) : row.type === "recent" ? <Icon as={Clock} /> : kindIcon(r!.kind)}
                    </span>
                    <span className="flex-1 min-w-0">
                      <span className="block truncate-dir">{row.type === "result" || isFav ? <Highlight text={name} query={fixedQuery ?? text} /> : name}</span>
                      {(secondary || isBest) && (
                        <span className="block truncate-dir text-xs text-muted">
                          {isBest && (
                            <span className="text-accent font-medium">
                              {t("bestMatch")}
                              {secondary ? " · " : ""}
                            </span>
                          )}
                          {secondary}
                        </span>
                      )}
                    </span>
                    {/* precision is news only when it is not exact */}
                    {r && r.kind === "address" && r.precision !== "exact" && (
                      <span className={clsx("chip", r.precision === "interpolated" ? "chip-warning" : "chip-muted")}>{t(PRECISION_KEY[r.precision])}</span>
                    )}
                    {r && r.kind !== "address" && r.kind !== "coord" && row.type === "result" && (
                      <span className="text-xs text-subtle shrink-0">{t(`kind${r.kind[0].toUpperCase()}${r.kind.slice(1)}` as I18nKey)}</span>
                    )}
                    {row.type === "recent" && (
                      <button
                        type="button"
                        tabIndex={-1}
                        className="btn btn-ghost btn-icon btn-sm text-subtle hover:text-fg -me-1.5 shrink-0"
                        aria-label={`${t("delete")} (Delete)`}
                        onClick={(e) => {
                          e.stopPropagation();
                          void removeRecent(row.h.id);
                        }}
                      >
                        <Icon as={X} size={14} />
                      </button>
                    )}
                  </div>
                </Fragment>
              );
            })}
          </div>,
          document.body,
        )
      : null;

  return (
    <div className="relative">
      <div ref={boxRef} className="relative flex items-center">
        {leading ? (
          <span className="absolute start-3 w-4 flex items-center justify-center text-muted pointer-events-none">{leading}</span>
        ) : (
          <Icon as={Search} className="absolute start-3 text-muted pointer-events-none" />
        )}
        <input
          ref={inputRef}
          className={clsx("input ps-10 pe-9 truncate-dir", compact && "h-10")}
          placeholder={placeholder ?? t("searchPlaceholder")}
          aria-label={label ?? placeholder ?? t("searchPlaceholder")}
          value={text}
          autoFocus={autoFocus}
          onChange={(e) => {
            setText(e.target.value);
            setDirty(true);
            setOpen(true);
          }}
          onFocus={() => {
            // an empty box offers my location, favorites and recent searches
            if (!text.trim() || dirty) setOpen(true);
          }}
          onClick={() => setOpen(true)}
          onBlur={() => setOpen(false)}
          onKeyDown={onKey}
          role="combobox"
          aria-expanded={shown}
          aria-controls={shown ? listId : undefined}
          aria-autocomplete="list"
          aria-activedescendant={shown && rows[active] ? optionId(active) : undefined}
        />
        {(text || value) && (
          <button
            className="absolute end-1.5 w-7 h-7 rounded flex items-center justify-center text-subtle hover:text-fg hover:bg-hover transition-colors"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => {
              setText("");
              setResults([]);
              setDirty(true);
              onClear?.();
              inputRef.current?.focus();
            }}
            aria-label={t("searchClear")}
          >
            <Icon as={X} size={14} />
          </button>
        )}
      </div>
      {list}
    </div>
  );
}
