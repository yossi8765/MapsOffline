import { useEffect, useState } from "react";
import { BookOpen, Clock, Copy, Flag, Link2, Navigation, Star } from "lucide-react";
import clsx from "clsx";
import { openUrl } from "@tauri-apps/plugin-opener";
import { findFavorite, makeMapLink, useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { copyText, fmtCoords } from "../lib/format";
import { api, type GeoResult, type StopBoard } from "../lib/api";
import { POI_CATEGORIES } from "../lib/poiCategories";
import { SearchBox, kindIcon, stopCode, type Pick } from "./SearchBox";
import { countdown } from "./FavoritesPanel";
import { MinyanimCard } from "./MinyanimCard";

/** Optional fields the geocoder may attach to POIs (opening hours). */
type GeoExtra = GeoResult;

/** Categories offered by "What's in <city>?" (ids of lib/poiCategories). */
const CITY_CATEGORIES = ["synagogue", "restaurant", "cafe", "supermarket", "pharmacy", "fuel", "parking", "park", "sights", "hotel"];

const KIND_LABEL: Record<GeoResult["kind"], I18nKey> = {
  address: "kindAddress",
  street: "kindStreet",
  place: "kindPlace",
  poi: "kindPoi",
  stop: "kindStop",
  coord: "kindCoord",
};

export function SearchPanel() {
  const t = useT();
  const focusTick = useStore((s) => s.searchFocusTick);
  const external = useStore((s) => s.searchPick);
  const [picked, setPicked] = useState<Pick | null>(external);

  // a place opened from outside the panel (share link, "nearby" list, map)
  useEffect(() => {
    if (external) setPicked({ lon: external.lon, lat: external.lat, label: external.label, result: external.result });
  }, [external]);

  const onPick = (p: Pick) => {
    setPicked(p);
    const s = useStore.getState();
    // remembered, so the card is still there when the user comes back to this tab
    useStore.setState((st) => ({ searchPick: { ...p, tick: (st.searchPick?.tick ?? 0) + 1 } }));
    s.setMarker({ lonlat: [p.lon, p.lat], label: p.label });
    s.flyToPoint([p.lon, p.lat], p.result?.kind === "place" ? 13 : p.result?.kind === "street" ? 15 : 17);
  };

  return (
    // pt-1: room for the input's 3 px focus ring at the top of the scroll area (it was clipped)
    <div className="px-4 pt-1 pb-5 flex flex-col gap-4">
      <SearchBox
        value={picked?.label ?? ""}
        onPick={onPick}
        focusTick={focusTick}
        autoFocus
        onClear={() => {
          setPicked(null);
          useStore.setState({ searchPick: null });
          useStore.getState().setMarker(null);
        }}
      />
      {picked ? <PlaceCard key={`${picked.lon},${picked.lat},${picked.label}`} picked={picked} /> : <p className="m-0 px-1 text-sm text-muted">{t("searchHint")}</p>}
    </div>
  );
}

function PlaceCard({ picked }: { picked: Pick }) {
  const t = useT();
  const favorites = useStore((s) => s.favorites);
  const online = useStore((s) => s.online);
  const saved = findFavorite(favorites, picked.lon, picked.lat);
  const r = picked.result as GeoExtra | undefined;
  const kind = r?.kind ?? "coord";

  // sub line: city · kind (or POI category / stop code)
  const sub = [
    r?.city && !picked.label.includes(r.city) ? r.city : "",
    kind === "poi" ? (r?.category ?? t("kindPoi")) : kind === "stop" ? `${t("stopCode")} ${stopCode(r!)}` : t(KIND_LABEL[kind]),
  ]
    .filter(Boolean)
    .join(" · ");

  // address the index only knows approximately (missing house number)
  const approx =
    kind === "address" && r && r.precision !== "exact"
      ? r.precision === "interpolated" && (r.nearHouseNumber ?? r.houseNumber)
        ? t("approxNearNumber").replace("{n}", (r.nearHouseNumber ?? r.houseNumber)!)
        : t("approxStreet")
      : "";

  const go = (end: "from" | "to") => {
    const s = useStore.getState();
    const p = { lon: picked.lon, lat: picked.lat, label: picked.label, result: picked.result };
    if (end === "to") s.setTo(p);
    else s.setFrom(p);
    s.setPanel("route");
  };

  return (
    <div className="card p-4 flex flex-col gap-4 fade-in">
      <div className="flex items-start gap-3">
        <span className="tile bg-accent-soft text-accent">{kindIcon(kind, 18)}</span>
        <div className="flex-1 min-w-0 selectable">
          <div className="text-lg font-semibold leading-snug">{picked.label}</div>
          {sub && <div className="text-sm text-muted">{sub}</div>}
          {approx && <div className="text-xs text-warning mt-0.5">{approx}</div>}
          <bdi dir="ltr" className="block text-xs text-subtle num mt-1">
            {fmtCoords(picked.lon, picked.lat)}
          </bdi>
        </div>
      </div>

      {/* round actions, Google-style: the primary one filled */}
      <div className="flex items-start justify-between gap-1">
        <RoundAction primary icon={<Flag size={18} strokeWidth={1.75} />} label={t("routeHere")} onClick={() => go("to")} />
        <RoundAction icon={<Navigation size={18} strokeWidth={1.75} className="rtl:-scale-x-100" />} label={t("routeFromHere")} onClick={() => go("from")} />
        <RoundAction
          icon={<Star size={18} strokeWidth={1.75} fill={saved ? "currentColor" : "none"} />}
          label={saved ? t("savedPlace") : t("savePlace")}
          pressed={!!saved}
          onClick={() => void useStore.getState().toggleFavorite(picked)}
        />
        <RoundAction
          icon={<Copy size={18} strokeWidth={1.75} />}
          label={t("copyCoordsShort")}
          onClick={async () => {
            if (await copyText(fmtCoords(picked.lon, picked.lat))) useStore.getState().toast("success", t("copied"));
          }}
        />
        <RoundAction
          icon={<Link2 size={18} strokeWidth={1.75} />}
          label={t("shareLink")}
          title={t("shareLinkHint")}
          onClick={async () => {
            const zoom = Math.max(useStore.getState().mapZoom, kind === "place" ? 13 : 16);
            if (await copyText(makeMapLink(picked.lon, picked.lat, zoom, picked.label))) useStore.getState().toast("success", t("linkCopied"));
          }}
        />
      </div>

      {kind === "poi" && r && <OpeningHours r={r} />}
      {kind === "stop" && r && <NextDepartures r={r} />}
      {kind === "poi" && r && /בית כנסת|synagogue/i.test(r.category ?? "") && <MinyanimCard lon={picked.lon} lat={picked.lat} name={r.name} />}
      {kind === "place" && <CityCategories name={r?.name ?? picked.label} />}

      {(kind === "place" || kind === "poi") && online && (
        <button
          className="self-start text-sm text-accent hover:underline underline-offset-2 inline-flex items-center gap-1.5"
          onClick={() => void openUrl(`https://www.hamichlol.org.il/${encodeURIComponent((r?.name ?? picked.label).replace(/ /g, "_"))}`).catch(() => {})}
        >
          <BookOpen size={14} strokeWidth={1.75} /> {t("michlolInfo")}
        </button>
      )}
    </div>
  );
}

function RoundAction({ icon, label, onClick, primary, pressed, title }: { icon: React.ReactNode; label: string; onClick: () => void; primary?: boolean; pressed?: boolean; title?: string }) {
  return (
    <button className="group flex-1 min-w-0 flex flex-col items-center gap-1.5" onClick={onClick} title={title ?? label} aria-pressed={pressed}>
      <span
        className={clsx(
          "w-10 h-10 rounded-full flex items-center justify-center transition-colors duration-150",
          primary ? "bg-[var(--accent-solid)] text-[var(--accent-fg)] group-hover:bg-[var(--accent-solid-hover)]" : "border border-line-strong text-accent group-hover:bg-hover",
          pressed && "text-warning",
        )}
      >
        {icon}
      </span>
      <span className="text-xs text-center leading-tight text-fg w-full break-words [text-align-last:center]">{label}</span>
    </button>
  );
}

function OpeningHours({ r }: { r: GeoExtra }) {
  const t = useT();
  if (!r.openingHours && r.openNow === undefined && !r.kosher) return null;
  return (
    <div className="flex flex-col gap-1 text-sm">
      <div className="flex items-center gap-2">
        <Clock size={14} strokeWidth={1.75} className="text-muted" />
        {r.openNow !== undefined && <span className={r.openNow ? "text-success font-medium" : "text-danger font-medium"}>{r.openNow ? t("openNow") : t("closedNow")}</span>}
        {r.openNow && r.closesAt && <span className="text-muted">· {t("closesAt").replace("{t}", r.closesAt)}</span>}
        {r.openNow === false && r.opensAt && <span className="text-muted">· {t("opensAt").replace("{t}", r.opensAt)}</span>}
        {r.kosher && <span className="chip chip-success">{t("kosher")}</span>}
      </div>
      {r.openingHours && (
        <div className="text-xs text-muted selectable" dir="ltr" style={{ textAlign: "end" }}>
          <span className="sr-only">{t("openingHours")}: </span>
          {r.openingHours}
        </div>
      )}
    </div>
  );
}

/** The next departures at a stop (schedule-based), soonest first, at most 6. */
function NextDepartures({ r }: { r: GeoResult }) {
  const t = useT();
  const [board, setBoard] = useState<(StopBoard & { at: number }) | null>(null);
  const [failed, setFailed] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    let dead = false;
    api
      .transitStopBoard(stopCode(r) || undefined, r.lon, r.lat)
      .then((b) => !dead && setBoard({ ...b, at: Date.now() }))
      .catch(() => !dead && setFailed(true));
    const id = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => {
      dead = true;
      window.clearInterval(id);
    };
  }, [r]);
  if (failed) return null;
  if (!board) return <div className="progress-line" role="progressbar" />;
  const deps = board.lines
    .flatMap((l) => l.next.map(([rel, iso]) => ({ l, rel, iso })))
    .sort((a, b) => a.rel - b.rel)
    .slice(0, 6);
  return (
    <div className="flex flex-col gap-1.5">
      <h3 className="section-label m-0 p-0">{t("nextDepartures")}</h3>
      {deps.length === 0 ? (
        <p className="m-0 text-sm text-muted">{t("noDeparturesSoon")}</p>
      ) : (
        <ul className="m-0 p-0 list-none flex flex-col">
          {deps.map(({ l, rel, iso }, i) => {
            const c = countdown(rel, iso, board.at, now, t);
            return (
              <li key={i} className="flex items-center gap-2 min-h-8 text-sm">
                <span className="inline-flex items-center justify-center h-6 min-w-7 px-1.5 rounded text-white text-xs font-semibold num shrink-0" style={{ background: l.color ? `#${l.color.replace(/^#/, "")}` : "var(--route-alt)" }}>
                  {l.shortName}
                </span>
                <span className="flex-1 min-w-0 truncate text-muted">{l.headsign}</span>
                <span className={clsx("num shrink-0", c.soon ? "text-success font-medium" : "text-fg")}>{c.text}</span>
              </li>
            );
          })}
        </ul>
      )}
      <p className="m-0 text-xs text-subtle">{t("scheduleTimes")}</p>
    </div>
  );
}

/** "What's in <city>?": category chips that show those places on the map. */
function CityCategories({ name }: { name: string }) {
  const t = useT();
  const cats = useStore((s) => s.layers.poiCats);
  const setLayers = useStore((s) => s.setLayers);
  const list = CITY_CATEGORIES.map((id) => POI_CATEGORIES.find((c) => c.id === id)).filter((c): c is NonNullable<typeof c> => !!c);
  return (
    <div className="flex flex-col gap-1.5">
      <h3 className="section-label m-0 p-0">{t("whatIsIn").replace("{city}", name)}</h3>
      <div className="flex flex-wrap gap-1.5">
        {list.map((c) => {
          const on = cats.includes(c.id);
          return (
            <button key={c.id} type="button" className="choice" aria-pressed={on} onClick={() => setLayers({ poiCats: on ? cats.filter((x) => x !== c.id) : [...cats, c.id] })}>
              {t(c.label)}
            </button>
          );
        })}
      </div>
    </div>
  );
}
