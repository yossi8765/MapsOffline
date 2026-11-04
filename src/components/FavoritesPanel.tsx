import { useEffect, useState } from "react";
import { Bus, ChevronDown, Download, Eye, EyeOff, Flag, Loader2, Navigation, Pencil, Plus, Route as RouteIcon, StickyNote, Trash2, Upload } from "lucide-react";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { InfoBar } from "./InfoBar";
import { api, type Favorite, type LineSummary, type LineTimes, type RouteRequest, type StopBoard } from "../lib/api";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { fmtTime } from "../lib/format";
import { matchesAll } from "../lib/text";
import { SearchBox, favIcon, FAV_ICONS, kindIcon, type Pick } from "./SearchBox";
import { nextServiceText } from "./SearchPanel";

export type FavKind = "line" | "address" | "stop" | "place" | "route";
const SECTIONS: { kind: FavKind; label: I18nKey }[] = [
  { kind: "line", label: "favLines" },
  { kind: "stop", label: "favStops" },
  { kind: "address", label: "favAddresses" },
  { kind: "place", label: "favPlaces" },
  { kind: "route", label: "favRoutes" },
];
type AddKind = "address" | "line" | "stop";

/** Colours offered for a favorite (map pin / tile tint). */
export const FAV_COLORS = ["#1a66d9", "#15803d", "#c2410c", "#be123c", "#6d28d9", "#0f766e", "#b45309", "#475569"];

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

export function countdown(relSec: number, iso: string, generatedMs: number, nowMs: number, t: ReturnType<typeof useT>): { text: string; soon: boolean } {
  const secs = relSec - Math.round((nowMs - generatedMs) / 1000);
  if (secs < -60) return { text: t("departed"), soon: false };
  if (secs < 60) return { text: t("nowLabel"), soon: true };
  const min = Math.round(secs / 60);
  if (min <= 90) return { text: `${t("inMinutes")} ${min} ${t("minShort")}`, soon: min <= 5 };
  return { text: fmtTime(iso), soon: false };
}

/** File format of the favorites export (`.json`): the fields favorites_add takes, plus the
 *  group / colour / note (version 2; version 1 files still import). */
interface FavFile {
  app: "MapsOffline";
  kind: "favorites";
  version: 1 | 2;
  favorites: Omit<Favorite, "id" | "createdIso">[];
}

async function exportFavorites(list: Favorite[], t: ReturnType<typeof useT>) {
  const st = useStore.getState();
  try {
    const d = new Date();
    const stamp = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
    const path = await saveDialog({ defaultPath: `MapsOffline-favorites-${stamp}.json`, filters: [{ name: t("favFileFilter"), extensions: ["json"] }] });
    if (!path) return;
    const file: FavFile = {
      app: "MapsOffline",
      kind: "favorites",
      version: 2,
      favorites: list.map(({ name, icon, lon, lat, address, kind, data, group, color, note }) => ({ name, icon, lon, lat, address, kind, data, group, color, note })),
    };
    await api.saveTextFile(path, JSON.stringify(file, null, 2));
    st.toast("success", `${t("favExported")} ${path}`);
  } catch (e) {
    st.toastError(e);
  }
}

/** Import from a file picked in the webview (no file-system permission needed). favorites_add
 *  returns the existing row for the same spot / line, so importing twice adds nothing. */
function importFavorites(t: ReturnType<typeof useT>) {
  const input = document.createElement("input");
  input.type = "file";
  input.accept = ".json,application/json";
  input.onchange = async () => {
    const st = useStore.getState();
    const f = input.files?.[0];
    if (!f) return;
    try {
      const json = JSON.parse(await f.text()) as Partial<FavFile>;
      if (json.kind !== "favorites" || !Array.isArray(json.favorites)) throw new Error("not a favorites file");
      const before = st.favorites.length;
      for (const x of json.favorites) {
        if (typeof x?.name !== "string" || !Number.isFinite(x.lon) || !Number.isFinite(x.lat)) continue;
        const added = await api.favoritesAdd(x.name, x.icon || "star", x.lon, x.lat, x.address, x.kind ?? "address", x.data);
        if (added && (x.group || x.color || x.note)) await useStore.getState().patchFavorite(added.id, { group: x.group ?? null, color: x.color ?? null, note: x.note ?? null });
      }
      await st.loadFavorites();
      st.toast("success", t("favImported").replace("{n}", String(Math.max(0, useStore.getState().favorites.length - before))));
    } catch (e) {
      st.toastError(e);
    }
  };
  input.click();
}

export function FavoritesPanel() {
  const t = useT();
  const favorites = useStore((s) => s.favorites);
  const groups = useStore((s) => s.favoriteGroups);
  const linePreview = useStore((s) => s.linePreview);
  const { loadFavorites, toastError, showLine, clearLine } = useStore.getState();
  const [editing, setEditing] = useState<Favorite | null>(null);
  const [adding, setAdding] = useState<AddKind | null>(null);
  const [name, setName] = useState("");
  const [icon, setIcon] = useState<string>("star");
  const [group, setGroup] = useState("");
  const [color, setColor] = useState<string>("");
  const [note, setNote] = useState("");
  const [pick, setPick] = useState<Pick | null>(null);
  const [groupFilter, setGroupFilter] = useState<string | null>(null);
  const visibleGroups = useStore((s) => s.visibleFavoriteGroups);
  const toggleGroupOnMap = useStore((s) => s.toggleFavoriteGroupOnMap);
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

  const resetFields = (f?: Favorite) => {
    setName(f?.name ?? "");
    setIcon(f?.icon ?? "star");
    setGroup(f?.group ?? "");
    setColor(f?.color ?? "");
    setNote(f?.note ?? "");
  };
  const startEdit = (f: Favorite) => {
    setEditing(f);
    setAdding(null);
    resetFields(f);
  };
  const startAdd = async (k: AddKind) => {
    setAdding(k);
    setEditing(null);
    resetFields();
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
  const extras = () => ({ group: group.trim() || null, color: color || null, note: note.trim() || null });
  const submit = async () => {
    try {
      let id: number | null = null;
      if (editing) {
        await useStore.getState().patchFavorite(editing.id, { name: name || editing.name, icon, ...extras() });
        cancel();
        return;
      } else if (adding === "line" && line && agency) {
        const data: Record<string, unknown> = { routeShortName: line.shortName, routeLongName: line.longName, agency };
        if (line.color) data.color = line.color;
        if (headsign) data.headsign = headsign;
        const label = headsign ? `${t("lineLabel")} ${line.shortName} ${t("towards")} ${headsign}` : `${t("lineLabel")} ${line.shortName} · ${agency}`;
        id = (await api.favoritesAdd(name || label, "bus", 0, 0, line.longName, "line", data)).id;
      } else if (adding && pick) {
        const kind = adding === "stop" ? "stop" : favKindOf(pick);
        if (adding === "stop" && pick.result?.kind !== "stop") {
          useStore.getState().toast("error", t("pickAStop"));
          return;
        }
        const code = pick.result?.category?.startsWith("stop:") ? pick.result.category.slice(5) : undefined;
        id = (await api.favoritesAdd(name || pick.label, icon, pick.lon, pick.lat, pick.label, kind, code ? { code } : undefined)).id;
      } else return;
      const ex = extras();
      if (id != null && (ex.group || ex.color || ex.note)) await useStore.getState().patchFavorite(id, ex);
      else await loadFavorites();
      cancel();
    } catch (e) {
      toastError(e);
    }
  };
  const remove = (f: Favorite) => {
    if (openStop?.favId === f.id) setOpenStop(null);
    if (lineTimes?.favId === f.id) setLineTimes(null);
    useStore.getState().removeFavorite(f);
  };

  const openLine = async (f: Favorite) => {
    if (lineTimes?.favId === f.id) {
      setLineTimes(null);
      clearLine();
      return;
    }
    const d = (f.data ?? {}) as { routeShortName?: string; routeLongName?: string; agency?: string; headsign?: string };
    if (!d.routeShortName) return;
    setBusy(f.id);
    try {
      const [times] = await Promise.all([api.transitLineTimes(d.routeShortName, d.agency ?? "", d.headsign, d.routeLongName), showLine(f)]);
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

  const filteredLines = lines.filter((l) => !lineFilter || l.shortName.startsWith(lineFilter) || matchesAll(l.longName, lineFilter));

  const form = (
    <form
      className="card p-3 flex flex-col gap-3 fade-in"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          cancel();
        }
      }}
    >
      {adding === "line" && (
        <>
          <select className="input h-9" value={agency} onChange={(e) => void chooseAgency(e.target.value)} aria-label={t("pickAgency")}>
            <option value="">{t("pickAgency")}</option>
            {(agencies ?? []).map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
          {agency && (
            <>
              <input className="input h-9" placeholder={t("pickLine")} aria-label={t("pickLine")} value={lineFilter} onChange={(e) => setLineFilter(e.target.value)} />
              <div className="flex flex-wrap gap-1.5 max-h-40 scroll-y">
                {filteredLines.slice(0, 120).map((l) => {
                  // one agency can run several lines with the same number in different towns:
                  // those get their route name next to the number
                  const shared = filteredLines.some((o) => o !== l && o.shortName === l.shortName);
                  return (
                    <button key={`${l.shortName}|${l.longName}`} type="button" className="choice max-w-full" aria-pressed={line?.shortName === l.shortName && line?.longName === l.longName} onClick={() => setLine(l)} aria-label={`${l.shortName} ${l.longName}`}>
                      <span className="num">{l.shortName}</span>
                      {shared && <span className="truncate-dir max-w-[220px] font-normal">{l.longName}</span>}
                    </button>
                  );
                })}
                {filteredLines.length === 0 && <span className="text-xs text-muted">{t("noLines")}</span>}
              </div>
              {line && <div className="text-xs text-muted truncate-dir">{line.longName}</div>}
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
      {adding !== "line" && <input className="input h-9" placeholder={t("favoriteName")} aria-label={t("favoriteName")} value={name} onChange={(e) => setName(e.target.value)} />}
      {(editing ? editing.kind === "address" : adding === "address") && (
        <div className="seg w-fit" role="radiogroup" aria-label={t("favoriteIcon")}>
          {FAV_ICONS.map((i) => (
            <button key={i} type="button" role="radio" className="!w-9 !flex-none" aria-checked={icon === i} aria-pressed={icon === i} onClick={() => setIcon(i)} aria-label={iconLabel(i)}>
              {favIcon(i)}
            </button>
          ))}
        </div>
      )}
      {/* group, colour, note */}
      <div className="flex flex-col gap-1">
        <input className="input h-9" list="fav-groups" placeholder={t("favGroup")} aria-label={t("favGroup")} value={group} onChange={(e) => setGroup(e.target.value)} />
        <datalist id="fav-groups">
          {groups.map((g) => (
            <option key={g} value={g} />
          ))}
        </datalist>
      </div>
      <div className="flex items-center gap-1.5 flex-wrap" role="radiogroup" aria-label={t("favColor")}>
        <span className="text-xs text-muted me-1">{t("favColor")}</span>
        <button type="button" role="radio" aria-checked={!color} aria-label={t("favColorNone")} onClick={() => setColor("")} className={clsx("w-6 h-6 rounded-full border border-line-strong", !color && "ring-2 ring-offset-2 ring-[var(--fg)]")} />
        {FAV_COLORS.map((c) => (
          <button key={c} type="button" role="radio" aria-checked={color === c} aria-label={c} onClick={() => setColor(c)} className={clsx("w-6 h-6 rounded-full", color === c && "ring-2 ring-offset-2 ring-[var(--fg)]")} style={{ background: c }} />
        ))}
      </div>
      <textarea className="input py-2 h-auto min-h-16 resize-y" rows={2} placeholder={t("favNote")} aria-label={t("favNote")} value={note} onChange={(e) => setNote(e.target.value)} />
      <div className="flex gap-2 justify-end">
        <button type="button" className="btn btn-sm" onClick={cancel}>
          {t("cancel")}
        </button>
        <button type="submit" className="btn btn-sm btn-primary" disabled={adding === "line" ? !line : adding ? !pick : false}>
          {t("save")}
        </button>
      </div>
    </form>
  );

  const goTo = (f: Favorite) => {
    const s = useStore.getState();
    s.setMarker({ lonlat: [f.lon, f.lat], label: f.name });
    s.flyToPoint([f.lon, f.lat], f.kind === "place" ? 15 : 17);
  };
  const openRoute = (f: Favorite) => {
    const d = (f.data ?? {}) as { request?: RouteRequest; labels?: string[] };
    if (d.request) useStore.getState().loadRequest(d.request, d.labels);
  };

  const row = (f: Favorite) => {
    const isLine = f.kind === "line";
    const isStop = f.kind === "stop";
    const isRoute = f.kind === "route";
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
      <div key={f.id} className={clsx("rounded -mx-2 transition-colors duration-150", open ? "bg-hover" : "hover:bg-hover focus-within:bg-hover")}>
        <div className="group flex items-center gap-1 px-2">
          <button
            className="flex-1 min-w-0 flex items-center gap-3 py-2 text-start rounded"
            onClick={() => (isLine ? void openLine(f) : isStop ? (open ? setOpenStop(null) : void openStopBoard(f)) : isRoute ? openRoute(f) : goTo(f))}
            aria-expanded={isLine || isStop ? open : undefined}
            aria-busy={busy === f.id}
          >
            {isLine ? (
              <span className="inline-flex items-center justify-center gap-1 h-8 min-w-8 px-2 rounded text-white text-sm font-semibold num shrink-0" style={{ background: color }}>
                <Icon as={Bus} size={14} />
                {d.routeShortName}
              </span>
            ) : (
              <span
                className={clsx("tile", f.kind === "address" && !f.color ? "bg-accent-soft text-accent" : "")}
                style={f.color ? { color: f.color, background: `color-mix(in srgb, ${f.color} 13%, transparent)` } : undefined}
              >
                {f.kind === "address" ? favIcon(f.icon, 16) : isRoute ? <Icon as={RouteIcon} /> : kindIcon(isStop ? "stop" : "poi", 16)}
              </span>
            )}
            <span className="flex-1 min-w-0">
              <span className="block truncate-dir font-medium">{f.name}</span>
              {secondary && <span className="block truncate-dir text-xs text-muted">{secondary}</span>}
              {f.note && (
                <span className="flex items-start gap-1 text-xs text-subtle mt-0.5">
                  <Icon as={StickyNote} size={12} className="mt-0.5 shrink-0" />
                  <span className="line-clamp-2">{f.note}</span>
                </span>
              )}
            </span>
            {(isLine || isStop) &&
              (busy === f.id ? (
                <Icon as={Loader2} size={15} className="text-subtle shrink-0 animate-spin" label={t("loading")} />
              ) : (
                <Icon as={ChevronDown} size={15} className={clsx("text-subtle shrink-0 transition-transform duration-150", open && "rotate-180")} />
              ))}
          </button>
          <div className={clsx("flex transition-opacity duration-150", open ? "" : "opacity-0 group-hover:opacity-100 group-focus-within:opacity-100")}>
            {!isLine && !isRoute && (
              <>
                <Tip text={t("useAsFrom")}>
                  <button
                    className="btn btn-ghost btn-icon btn-sm text-muted"
                    aria-label={`${t("useAsFrom")}: ${f.name}`}
                    onClick={() => {
                      useStore.getState().setFrom({ lon: f.lon, lat: f.lat, label: f.name });
                      useStore.getState().setPanel("route");
                    }}
                  >
                    <Icon as={Navigation} size={14} />
                  </button>
                </Tip>
                <Tip text={t("useAsTo")}>
                  <button
                    className="btn btn-ghost btn-icon btn-sm text-muted"
                    aria-label={`${t("useAsTo")}: ${f.name}`}
                    onClick={() => {
                      useStore.getState().setTo({ lon: f.lon, lat: f.lat, label: f.name });
                      useStore.getState().setPanel("route");
                    }}
                  >
                    <Icon as={Flag} size={14} />
                  </button>
                </Tip>
              </>
            )}
            <Tip text={t("edit")}>
              <button className="btn btn-ghost btn-icon btn-sm text-muted" aria-label={`${t("edit")}: ${f.name}`} onClick={() => startEdit(f)}>
                <Icon as={Pencil} size={14} />
              </button>
            </Tip>
            <Tip text={t("delete")}>
              <button className="btn btn-ghost btn-icon btn-sm text-muted" aria-label={`${t("delete")}: ${f.name}`} onClick={() => remove(f)}>
                <Icon as={Trash2} size={14} />
              </button>
            </Tip>
          </div>
        </div>

        {open && isLine && lineTimes && (
          <div className="px-2 pb-3 flex flex-col gap-3 fade-in">
            {lineTimes.times.directions.length === 0 &&
              (lineTimes.times.nextServiceIso ? (
                <InfoBar severity="info">{t("nextServiceAt").replace("{t}", nextServiceText(lineTimes.times.nextServiceIso, t))}</InfoBar>
              ) : (
                <div className="text-muted">{t("noMoreToday")}</div>
              ))}
            {lineTimes.times.directions.map((dir) => (
              <div key={dir.headsign} className="flex flex-col gap-1.5">
                <div className="font-medium truncate-dir">
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
                      <span key={i} className={clsx("chip", i === 0 ? "" : "chip-muted")} aria-label={i === 0 ? c.text : undefined}>
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
              <details>
                <summary className="cursor-pointer text-muted">
                  {t("lineStops")} ({linePreview.shape.stops.length})
                </summary>
                <ol className="m-0 mt-1 p-0 list-none flex flex-col">
                  {linePreview.shape.stops.map((s, i) => (
                    <li key={i} className="flex items-center gap-2 h-7 cursor-pointer text-muted hover:text-fg" onClick={() => useStore.getState().flyToPoint([s.lon, s.lat], 16)}>
                      <span className="w-2 h-2 rounded-full shrink-0" style={{ background: color }} />
                      <span className="truncate-dir">{s.name}</span>
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
            {openStop.board.lines.length === 0 &&
              (openStop.board.nextServiceIso ? (
                <InfoBar severity="info">{t("nextServiceAt").replace("{t}", nextServiceText(openStop.board.nextServiceIso, t))}</InfoBar>
              ) : (
                <div className="text-muted">{t("noServiceToday")}</div>
              ))}
            {openStop.board.lines.map((l, i) => {
              const first = l.next[0];
              const c = first ? countdown(first[0], first[1], openStop.at, now, t) : null;
              return (
                <div key={i} className="flex items-center gap-2 min-h-9">
                  <span className="inline-flex items-center justify-center h-7 min-w-7 px-1.5 rounded text-white text-xs font-semibold num shrink-0" style={{ background: lineColor(l.color) }}>
                    {l.shortName}
                  </span>
                  <span className="flex-1 min-w-0">
                    <span className="block truncate-dir">
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
                  <span className={clsx("num shrink-0 text-end", c?.soon ? "text-success font-semibold" : c ? "text-fg" : "text-muted")}>{c ? c.text : t("noMoreToday")}</span>
                </div>
              );
            })}
          </div>
        )}
      </div>
    );
  };

  const inGroup = (f: Favorite) => groupFilter === null || (groupFilter === "" ? !f.group : f.group === groupFilter);
  const byKind = (k: FavKind) => {
    const list = favorites.filter((f) => (f.kind || "address") === k && inGroup(f));
    return k === "address" ? orderAddresses(list) : list;
  };

  return (
    <div className="px-4 pb-5 flex flex-col gap-4">
      {!editing && (
        <div className="flex flex-col gap-1.5">
          <div className="text-xs text-muted px-0.5 flex items-center gap-1" id="fav-add-label">
            <Icon as={Plus} size={12} strokeWidth={2} /> {t("addFavorite")}
          </div>
          <div className="seg" role="group" aria-labelledby="fav-add-label">
            {(["address", "line", "stop"] as AddKind[]).map((k) => (
              <button key={k} aria-pressed={adding === k} onClick={() => (adding === k ? cancel() : void startAdd(k))}>
                <span>{t(k === "address" ? "addAddressFav" : k === "line" ? "addLineFav" : "addStopFav")}</span>
              </button>
            ))}
          </div>
        </div>
      )}
      {adding && form}
      {groups.length > 0 && !adding && !editing && (
        <div className="flex flex-wrap gap-1.5" role="group" aria-label={t("favGroups")}>
          <button className="choice" aria-pressed={groupFilter === null} onClick={() => setGroupFilter(null)}>
            {t("nearbyAll")}
          </button>
          {[...groups, ""].map((g) => {
            const onMap = visibleGroups.includes(g);
            const label = g || t("favNoGroup");
            return (
              <span key={g || "__none"} className="inline-flex items-center gap-0.5">
                <button className="choice" aria-pressed={groupFilter === g} onClick={() => setGroupFilter(groupFilter === g ? null : g)}>
                  {label}
                </button>
                <button
                  className="btn btn-ghost btn-icon btn-sm"
                  aria-pressed={onMap}
                  aria-label={`${onMap ? t("favGroupHideOnMap") : t("favGroupShowOnMap")}: ${label}`}
                  title={onMap ? t("favGroupHideOnMap") : t("favGroupShowOnMap")}
                  onClick={() => toggleGroupOnMap(g)}
                >
                  <Icon as={onMap ? Eye : EyeOff} size={14} className={onMap ? "text-accent" : "text-muted"} />
                </button>
              </span>
            );
          })}
        </div>
      )}
      {favorites.length === 0 && !adding && (
        <div className="flex flex-col gap-1 px-1 py-2">
          <p className="m-0 text-muted">{t("favoritesEmpty")}</p>
          <p className="m-0 text-xs text-subtle">{t("favoritesHint")}</p>
        </div>
      )}
      {SECTIONS.map(({ kind, label }) => {
        const list = byKind(kind);
        if (list.length === 0) return null;
        return (
          <section key={kind} className="flex flex-col gap-1" aria-labelledby={`fav-sec-${kind}`}>
            <h2 id={`fav-sec-${kind}`} className="section-label m-0 px-0.5">
              {t(label)}
            </h2>
            {list.map((f) => (editing?.id === f.id ? <div key={f.id}>{form}</div> : row(f)))}
          </section>
        );
      })}
      {!editing && !adding && (
        <div className="flex items-center gap-2 pt-1">
          <button className="btn btn-sm btn-ghost text-muted" disabled={favorites.length === 0} onClick={() => void exportFavorites(favorites, t)}>
            <Icon as={Download} size={14} /> {t("favExport")}
          </button>
          <button className="btn btn-sm btn-ghost text-muted" onClick={() => importFavorites(t)}>
            <Icon as={Upload} size={14} /> {t("favImport")}
          </button>
        </div>
      )}
    </div>
  );
}
