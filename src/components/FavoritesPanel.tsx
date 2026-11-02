import { useEffect, useState } from "react";
import { Bus, ChevronDown, Flag, Navigation, Pencil, Plus, Star, Trash2 } from "lucide-react";
import clsx from "clsx";
import { api, type Favorite, type LineSummary, type LineTimes, type StopBoard } from "../lib/api";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { fmtTime } from "../lib/format";
import { SearchBox, favIcon, FAV_ICONS, kindIcon, type Pick } from "./SearchBox";

export type FavKind = "line" | "address" | "stop" | "place";
const SECTIONS: { kind: FavKind; label: I18nKey }[] = [
  { kind: "line", label: "favLines" },
  { kind: "stop", label: "favStops" },
  { kind: "address", label: "favAddresses" },
  { kind: "place", label: "favPlaces" },
];
type AddKind = "address" | "line" | "stop";

/** Which favorites section a search result belongs to. */
export function favKindOf(pick: Pick): FavKind {
  const k = pick.result?.kind;
  return k === "stop" ? "stop" : k === "poi" || k === "place" ? "place" : "address";
}

/** Home, then work, then the rest in insertion order. */
function orderAddresses(list: Favorite[]): Favorite[] {
  const rank = (f: Favorite) => (f.icon === "home" ? 0 : f.icon === "briefcase" ? 1 : 2);
  return [...list].sort((a, b) => rank(a) - rank(b) || a.id - b.id);
}

function lineColor(c?: string): string {
  return c ? (c.startsWith("#") ? c : `#${c}`) : "#0891b2";
}

/** "now" / "in N min" / clock time, from a schedule offset in seconds and the board's age. */
function useClock(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 15_000);
    return () => window.clearInterval(id);
  }, []);
  return now;
}

function countdown(relSec: number, iso: string, generatedMs: number, nowMs: number, t: ReturnType<typeof useT>): { text: string; soon: boolean } {
  const secs = relSec - Math.round((nowMs - generatedMs) / 1000);
  if (secs < -60) return { text: t("departed"), soon: false };
  if (secs < 60) return { text: t("nowLabel"), soon: true };
  const min = Math.round(secs / 60);
  if (min <= 90) return { text: `${t("inMinutes")} ${min} ${t("minShort")}`, soon: min <= 5 };
  return { text: fmtTime(iso), soon: false };
}

export function FavoritesPanel() {
  const t = useT();
  const favorites = useStore((s) => s.favorites);
  const linePreview = useStore((s) => s.linePreview);
  const { loadFavorites, toastError, showLine, clearLine } = useStore.getState();
  const [editing, setEditing] = useState<Favorite | null>(null);
  const [adding, setAdding] = useState<AddKind | null>(null);
  const [name, setName] = useState("");
  const [icon, setIcon] = useState<string>("star");
  const [pick, setPick] = useState<Pick | null>(null);
  // line picker
  const [agencies, setAgencies] = useState<string[] | null>(null);
  const [agency, setAgency] = useState("");
  const [lines, setLines] = useState<LineSummary[]>([]);
  const [lineFilter, setLineFilter] = useState("");
  const [line, setLine] = useState<LineSummary | null>(null);
  const [headsign, setHeadsign] = useState("");
  // opened items
  const [openStop, setOpenStop] = useState<{ favId: number; board: StopBoard; at: number } | null>(null);
  const [lineTimes, setLineTimes] = useState<{ favId: number; times: LineTimes; at: number } | null>(null);
  const [busy, setBusy] = useState<number | null>(null);
  const now = useClock();

  useEffect(() => {
    void loadFavorites();
  }, [loadFavorites]);

  // the stop board ages: refetch every 5 minutes while open
  useEffect(() => {
    if (!openStop) return;
    const f = favorites.find((x) => x.id === openStop.favId);
    if (!f || now - openStop.at < 5 * 60_000) return;
    void openStopBoard(f);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [now]);

  const iconLabel = (i: string) => t(i === "home" ? "iconHome" : i === "briefcase" ? "iconWork" : i === "heart" ? "iconHeart" : i === "map-pin" ? "iconPin" : "iconStar");

  const startEdit = (f: Favorite) => {
    setEditing(f);
    setAdding(null);
    setName(f.name);
    setIcon(f.icon);
  };
  const startAdd = async (k: AddKind) => {
    setAdding(k);
    setEditing(null);
    setName("");
    setIcon(k === "line" ? "bus" : "star");
    setPick(null);
    setLine(null);
    setHeadsign("");
    setLineFilter("");
    if (k === "line" && agencies === null) {
      try {
        setAgencies(await api.transitAgencies());
      } catch (e) {
        toastError(e);
        setAgencies([]);
      }
    }
  };
  const chooseAgency = async (a: string) => {
    setAgency(a);
    setLine(null);
    setLines([]);
    if (!a) return;
    try {
      setLines(await api.transitLines(a));
    } catch (e) {
      toastError(e);
    }
  };
  const cancel = () => {
    setEditing(null);
    setAdding(null);
  };
  const submit = async () => {
    try {
      if (editing) await api.favoritesUpdate(editing.id, name || editing.name, icon);
      else if (adding === "line" && line && agency) {
        const data: Record<string, unknown> = { routeShortName: line.shortName, routeLongName: line.longName, agency };
        if (line.color) data.color = line.color;
        if (headsign) data.headsign = headsign;
        const label = headsign ? `${t("lineLabel")} ${line.shortName} ${t("towards")} ${headsign}` : `${t("lineLabel")} ${line.shortName} · ${agency}`;
        await api.favoritesAdd(name || label, "bus", 0, 0, line.longName, "line", data);
      } else if (adding && pick) {
        const kind = adding === "stop" ? "stop" : favKindOf(pick);
        if (adding === "stop" && pick.result?.kind !== "stop") {
          toastError(t("pickAStop"));
          return;
        }
        const code = pick.result?.category?.startsWith("stop:") ? pick.result.category.slice(5) : undefined;
        await api.favoritesAdd(name || pick.label, icon, pick.lon, pick.lat, pick.label, kind, code ? { code } : undefined);
      } else return;
      await loadFavorites();
      cancel();
    } catch (e) {
      toastError(e);
    }
  };
  const remove = async (f: Favorite) => {
    try {
      if (linePreview?.favId === f.id) clearLine();
      if (openStop?.favId === f.id) setOpenStop(null);
      if (lineTimes?.favId === f.id) setLineTimes(null);
      await api.favoritesDelete(f.id);
      await loadFavorites();
    } catch (e) {
      toastError(e);
    }
  };

  const openLine = async (f: Favorite) => {
    if (lineTimes?.favId === f.id) {
      setLineTimes(null);
      clearLine();
      return;
    }
    const d = (f.data ?? {}) as { routeShortName?: string; agency?: string; headsign?: string };
    if (!d.routeShortName) return;
    setBusy(f.id);
    try {
      const [times] = await Promise.all([api.transitLineTimes(d.routeShortName, d.agency ?? "", d.headsign), showLine(f)]);
      setLineTimes({ favId: f.id, times, at: Date.now() });
      setOpenStop(null);
    } catch (e) {
      toastError(e);
    } finally {
      setBusy(null);
    }
  };
  const openStopBoard = async (f: Favorite) => {
    const d = (f.data ?? {}) as { code?: string };
    setBusy(f.id);
    try {
      const board = await api.transitStopBoard(d.code, f.lon, f.lat);
      setOpenStop({ favId: f.id, board, at: Date.now() });
      setLineTimes(null);
      clearLine();
      const s = useStore.getState();
      s.setMarker({ lonlat: [board.stop.lon, board.stop.lat], label: board.stop.name });
      s.flyToPoint([board.stop.lon, board.stop.lat], 16);
    } catch (e) {
      toastError(e);
    } finally {
      setBusy(null);
    }
  };

  const filteredLines = lines.filter((l) => !lineFilter || l.shortName.startsWith(lineFilter) || l.longName.includes(lineFilter));

  const form = (
    <div className="card p-3 flex flex-col gap-3 fade-in">
      {adding === "line" && (
        <>
          <select className="input h-9" value={agency} onChange={(e) => void chooseAgency(e.target.value)}>
            <option value="">{t("pickAgency")}</option>
            {(agencies ?? []).map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
          {agency && (
            <>
              <input className="input h-9" placeholder={t("pickLine")} value={lineFilter} onChange={(e) => setLineFilter(e.target.value)} />
              <div className="flex flex-wrap gap-1.5 max-h-40 scroll-y">
                {filteredLines.slice(0, 120).map((l) => (
                  <button key={l.shortName} type="button" className="choice" aria-pressed={line?.shortName === l.shortName} onClick={() => setLine(l)} title={l.longName}>
                    {l.shortName}
                  </button>
                ))}
                {filteredLines.length === 0 && <span className="text-xs text-muted">{t("noLines")}</span>}
              </div>
              {line && <div className="text-xs text-muted truncate">{line.longName}</div>}
              {line && line.headsigns.length > 1 && (
                <div className="flex flex-col gap-1">
                  <div className="text-xs text-muted">{t("pickDirection")}</div>
                  <div className="flex flex-wrap gap-1.5">
                    <button type="button" className="choice" aria-pressed={headsign === ""} onClick={() => setHeadsign("")}>
                      {t("allDirections")}
                    </button>
                    {line.headsigns.map((h) => (
                      <button key={h} type="button" className="choice" aria-pressed={headsign === h} onClick={() => setHeadsign(h)}>
                        {t("towards")} {h}
                      </button>
                    ))}
                  </div>
                </div>
              )}
            </>
          )}
        </>
      )}
      {(adding === "address" || adding === "stop") && (
        <SearchBox compact value={pick?.label ?? ""} onPick={(p) => setPick(p)} saveHistory={false} noLocation={adding === "stop"} placeholder={adding === "stop" ? t("searchStopPlaceholder") : undefined} />
      )}
      {adding !== "line" && <input className="input h-9" placeholder={t("favoriteName")} value={name} onChange={(e) => setName(e.target.value)} />}
      {(editing ? editing.kind === "address" : adding === "address") && (
        <div className="seg w-fit" role="radiogroup">
          {FAV_ICONS.map((i) => (
            <button key={i} className="!w-9 !flex-none" aria-pressed={icon === i} onClick={() => setIcon(i)} title={iconLabel(i)} aria-label={iconLabel(i)}>
              {favIcon(i)}
            </button>
          ))}
        </div>
      )}
      <div className="flex gap-2 justify-end">
        <button className="btn btn-sm" onClick={cancel}>
          {t("cancel")}
        </button>
        <button className="btn btn-sm btn-primary" onClick={() => void submit()} disabled={adding === "line" ? !line : adding ? !pick : false}>
          {t("save")}
        </button>
      </div>
    </div>
  );

  const goTo = (f: Favorite) => {
    const s = useStore.getState();
    s.setMarker({ lonlat: [f.lon, f.lat], label: f.name });
    s.flyToPoint([f.lon, f.lat], f.kind === "place" ? 15 : 17);
  };

  const row = (f: Favorite) => {
    const isLine = f.kind === "line";
    const isStop = f.kind === "stop";
    const open = (isLine && lineTimes?.favId === f.id) || (isStop && openStop?.favId === f.id);
    const d = (f.data ?? {}) as { routeShortName?: string; routeLongName?: string; agency?: string; color?: string; code?: string; headsign?: string };
    const color = lineColor(d.color);
    const secondary = isLine
      ? [d.headsign ? `${t("towards")} ${d.headsign}` : "", d.agency, d.headsign ? "" : d.routeLongName].filter(Boolean).join(" · ")
      : isStop
        ? [d.code ? `${t("stopCode")} ${d.code}` : "", f.address && f.address !== f.name ? f.address : ""].filter(Boolean).join(" · ")
        : f.address && f.address !== f.name
          ? f.address
          : "";
    return (
      <div key={f.id} className={clsx("rounded-[10px] -mx-2 transition-colors duration-150", open ? "bg-hover" : "hover:bg-hover focus-within:bg-hover")}>
        <div className="group flex items-center gap-1 px-2">
          <button
            className="flex-1 min-w-0 flex items-center gap-3 py-2 text-start rounded-lg"
            onClick={() => (isLine ? void openLine(f) : isStop ? (open ? setOpenStop(null) : void openStopBoard(f)) : goTo(f))}
            aria-expanded={isLine || isStop ? open : undefined}
            aria-busy={busy === f.id}
          >
            {isLine ? (
              <span className="inline-flex items-center justify-center gap-1 h-8 min-w-8 px-2 rounded-lg text-white text-sm font-semibold num shrink-0" style={{ background: color }}>
                <Bus size={13} />
                {d.routeShortName}
              </span>
            ) : (
              <span className={clsx("tile", f.kind === "address" ? "bg-accent-soft text-accent" : "")}>{f.kind === "address" ? favIcon(f.icon, 16) : kindIcon(isStop ? "stop" : "poi", 16)}</span>
            )}
            <span className="flex-1 min-w-0">
              <span className="block truncate text-base font-medium">{f.name}</span>
              {secondary && <span className="block truncate text-xs text-muted">{secondary}</span>}
            </span>
            {(isLine || isStop) && <ChevronDown size={15} className={clsx("text-subtle shrink-0 transition-transform duration-150", open && "rotate-180")} />}
          </button>
          <div className={clsx("flex transition-opacity duration-150", open ? "" : "opacity-0 group-hover:opacity-100 group-focus-within:opacity-100")}>
            {!isLine && (
              <>
                <button className="btn btn-ghost btn-icon btn-sm text-muted" title={t("useAsFrom")} onClick={() => { useStore.getState().setFrom({ lon: f.lon, lat: f.lat, label: f.name }); useStore.getState().setPanel("route"); }}>
                  <Navigation size={14} strokeWidth={1.75} />
                </button>
                <button className="btn btn-ghost btn-icon btn-sm text-muted" title={t("useAsTo")} onClick={() => { useStore.getState().setTo({ lon: f.lon, lat: f.lat, label: f.name }); useStore.getState().setPanel("route"); }}>
                  <Flag size={14} strokeWidth={1.75} />
                </button>
              </>
            )}
            <button className="btn btn-ghost btn-icon btn-sm text-muted" title={t("edit")} onClick={() => startEdit(f)}>
              <Pencil size={14} strokeWidth={1.75} />
            </button>
            <button className="btn btn-ghost btn-icon btn-sm text-muted" title={t("delete")} onClick={() => void remove(f)}>
              <Trash2 size={14} strokeWidth={1.75} />
            </button>
          </div>
        </div>

        {open && isLine && lineTimes && (
          <div className="px-2 pb-3 flex flex-col gap-3 fade-in">
            {lineTimes.times.directions.length === 0 && <div className="text-sm text-muted">{t("noMoreToday")}</div>}
            {lineTimes.times.directions.map((dir) => (
              <div key={dir.headsign} className="flex flex-col gap-1.5">
                <div className="text-sm font-medium truncate">
                  {t("towards")} {dir.headsign || dir.toStop}
                  <span className="text-xs text-muted font-normal">
                    {" · "}
                    {t("fromStopLabel")} {dir.fromStop} · {Math.round(dir.durationSec / 60)} {t("minShort")}
                  </span>
                </div>
                <div className="flex flex-wrap gap-1 num">
                  {dir.departures.map(([rel, iso], i) => {
                    const c = countdown(rel, iso, lineTimes.at, now, t);
                    return (
                      <span key={i} className={clsx("chip", i === 0 ? "" : "chip-muted")} title={i === 0 ? c.text : undefined}>
                        {fmtTime(iso)}
                      </span>
                    );
                  })}
                  {dir.departures.length === 0 && <span className="text-xs text-muted">{t("noMoreToday")}</span>}
                </div>
                {dir.departures[0] && (
                  <div className="text-xs text-muted">
                    {t("nextDeparture")}: {countdown(dir.departures[0][0], dir.departures[0][1], lineTimes.at, now, t).text}
                  </div>
                )}
              </div>
            ))}
            {linePreview?.favId === f.id && (
              <details className="text-sm">
                <summary className="cursor-pointer text-muted">{t("lineStops")} ({linePreview.shape.stops.length})</summary>
                <ol className="m-0 mt-1 p-0 list-none flex flex-col">
                  {linePreview.shape.stops.map((s, i) => (
                    <li key={i} className="flex items-center gap-2 h-7 cursor-pointer text-muted hover:text-fg" onClick={() => useStore.getState().flyToPoint([s.lon, s.lat], 16)}>
                      <span className="w-2 h-2 rounded-full shrink-0" style={{ background: color }} />
                      <span className="truncate">{s.name}</span>
                      {s.code && (
                        <span className="text-xs text-subtle num" dir="ltr">
                          {s.code}
                        </span>
                      )}
                    </li>
                  ))}
                </ol>
              </details>
            )}
          </div>
        )}

        {open && isStop && openStop && (
          <div className="px-2 pb-3 flex flex-col gap-1 fade-in">
            <div className="text-xs text-muted mb-1">{t("scheduleNotice")}</div>
            {openStop.board.lines.length === 0 && <div className="text-sm text-muted">{t("noServiceToday")}</div>}
            {openStop.board.lines.map((l, i) => {
              const first = l.next[0];
              const c = first ? countdown(first[0], first[1], openStop.at, now, t) : null;
              return (
                <div key={i} className="flex items-center gap-2 min-h-9">
                  <span className="inline-flex items-center justify-center h-7 min-w-7 px-1.5 rounded-md text-white text-xs font-semibold num shrink-0" style={{ background: lineColor(l.color) }}>
                    {l.shortName}
                  </span>
                  <span className="flex-1 min-w-0">
                    <span className="block truncate text-sm">
                      {l.terminus ? `${t("arrivesFrom")} ` : `${t("towards")} `}
                      {l.headsign}
                    </span>
                    <span className="block truncate text-xs text-subtle num">
                      {l.next
                        .slice(1, 4)
                        .map(([, iso]) => fmtTime(iso))
                        .join(" · ") || `${l.totalToday} ${t("todayTrips")}`}
                    </span>
                  </span>
                  <span className={clsx("text-sm num shrink-0 text-end", c?.soon ? "text-success font-semibold" : c ? "text-fg" : "text-muted")}>{c ? c.text : t("noMoreToday")}</span>
                </div>
              );
            })}
          </div>
        )}
      </div>
    );
  };

  const byKind = (k: FavKind) => {
    const list = favorites.filter((f) => (f.kind || "address") === k);
    return k === "address" ? orderAddresses(list) : list;
  };

  return (
    <div className="px-4 pb-5 flex flex-col gap-4">
      {!editing && (
        <div className="flex flex-col gap-1.5">
          <div className="text-xs text-muted px-0.5 flex items-center gap-1">
            <Plus size={12} strokeWidth={2} /> {t("addFavorite")}
          </div>
          <div className="seg" role="tablist">
            {(["address", "line", "stop"] as AddKind[]).map((k) => (
              <button key={k} role="tab" aria-pressed={adding === k} aria-selected={adding === k} onClick={() => (adding === k ? cancel() : void startAdd(k))}>
                <span>{t(k === "address" ? "addAddressFav" : k === "line" ? "addLineFav" : "addStopFav")}</span>
              </button>
            ))}
          </div>
        </div>
      )}
      {adding && form}
      {favorites.length === 0 && !adding && (
        <div className="flex flex-col items-center text-center gap-3 py-12 px-6">
          <span className="tile w-10 h-10 rounded-full">
            <Star size={18} strokeWidth={1.75} />
          </span>
          <p className="m-0 text-sm text-muted">{t("favoritesEmpty")}</p>
          <p className="m-0 text-xs text-subtle">{t("favoritesHint")}</p>
        </div>
      )}
      {SECTIONS.map(({ kind, label }) => {
        const list = byKind(kind);
        if (list.length === 0) return null;
        return (
          <section key={kind} className="flex flex-col gap-1">
            <h2 className="section-label m-0 px-0.5">{t(label)}</h2>
            {list.map((f) => (editing?.id === f.id ? <div key={f.id}>{form}</div> : row(f)))}
          </section>
        );
      })}
    </div>
  );
}
