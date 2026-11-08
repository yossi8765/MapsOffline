import { useEffect, useState } from "react";
import { BookOpen, Clock, Copy, Flag, Globe, MapPin, Navigation, Phone, Share2, Star, Tag } from "lucide-react";
import clsx from "clsx";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { InfoBar } from "./InfoBar";
import { findFavorite, useStore } from "../lib/store";
import { navigateTo } from "../lib/nav";
import { useT, type I18nKey } from "../lib/i18n";
import { copyText } from "../lib/format";
import { formatCoord } from "../lib/coords";
import { api, apiX, type GeoResult, type StopBoard, type WikiSummary } from "../lib/api";
import { POI_CATEGORIES } from "../lib/poiCategories";
import { SearchBox, kindIcon, stopCode, type Pick } from "./SearchBox";
import { countdown } from "./FavoritesPanel";
import { isSynagogue } from "./MapHoverCard";
import { MinyanimCard } from "./MinyanimCard";
import { QuickDestinations } from "./QuickDestinations";
import { fmtTime } from "../lib/format";

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
  const focusTick = useStore((s) => s.searchFocusTick);
  const external = useStore((s) => s.searchPick);
  const [picked, setPicked] = useState<Pick | null>(external);

  // a place opened from outside the panel (share link, "nearby" list, map) – and the card picked
  // here earlier, so switching panels and back keeps it
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
    <div className="px-4 pt-1 pb-5 flex flex-col gap-4">
      <SearchBox
        value={picked?.label ?? ""}
        onPick={onPick}
        focusTick={focusTick}
        // with a place open the box stays quiet: focusing it would cover the card with results
        autoFocus={!picked}
        onClear={() => {
          setPicked(null);
          useStore.setState({ searchPick: null });
          useStore.getState().setMarker(null);
        }}
      />
      {picked ? <PlaceCard key={`${picked.lon},${picked.lat},${picked.label}`} picked={picked} /> : <QuickDestinations />}
    </div>
  );
}

function PlaceCard({ picked }: { picked: Pick }) {
  const t = useT();
  const favorites = useStore((s) => s.favorites);
  const online = useStore((s) => s.online);
  const coordFormat = useStore((s) => s.settings.coordFormat ?? "dd");
  const saved = findFavorite(favorites, picked.lon, picked.lat);
  const r = picked.result;
  const kind = r?.kind ?? "coord";
  const coords = formatCoord(picked.lon, picked.lat, coordFormat);

  // sub line: city · kind (or POI category / stop code)
  const sub = [
    r?.city && !picked.label.includes(r.city) ? r.city : "",
    kind === "poi" ? (r?.categoryLabel ?? r?.category ?? t("kindPoi")) : kind === "stop" ? `${t("stopCode")} ${stopCode(r!)}` : t(KIND_LABEL[kind]),
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
    // the other end is usually empty: put the cursor there (it offers "my location" first)
    s.focusRouteField();
  };

  return (
    <div className="card p-4 flex flex-col gap-4 fade-in">
      <div className="flex items-start gap-3">
        <span className="tile bg-accent-soft text-accent">{kindIcon(kind, 18)}</span>
        <div className="flex-1 min-w-0 selectable">
          <h2 className="text-lg font-semibold leading-snug m-0">{picked.label}</h2>
          {sub && <div className="text-muted">{sub}</div>}
          {approx && <div className="text-xs text-warning mt-0.5">{approx}</div>}
          <bdi dir="ltr" className="block text-xs text-subtle num mt-1">
            {coords}
          </bdi>
        </div>
      </div>

      {/* round actions, Google-style: the primary one filled */}
      <div className="flex items-start justify-between gap-1">
        <RoundAction primary icon={Flag} label={t("routeHere")} onClick={() => go("to")} />
        <RoundAction icon={Navigation} label={t("navStart")} onClick={() => void navigateTo({ lon: picked.lon, lat: picked.lat, label: picked.label, result: picked.result })} />
        <RoundAction
          icon={Star}
          filled={!!saved}
          label={saved ? t("savedPlace") : t("savePlace")}
          pressed={!!saved}
          onClick={() => void useStore.getState().toggleFavorite(picked)}
        />
        <RoundAction
          icon={Copy}
          label={t("copyCoordsShort")}
          onClick={async () => {
            if (await copyText(coords)) useStore.getState().toast("success", t("copied"));
          }}
        />
        <RoundAction
          icon={Share2}
          label={t("shareLink")}
          tip={t("shareLinkHint")}
          onClick={() => useStore.getState().setShare({ kind: "place", lon: picked.lon, lat: picked.lat, label: picked.label })}
        />
      </div>

      {r && <PlaceDetails r={r} online={online} />}
      {kind === "poi" && r && <OpeningHours r={r} />}
      {kind === "stop" && r && <NextDepartures r={r} />}
      {kind === "poi" && r && isSynagogue(r.category) && <MinyanimCard lon={picked.lon} lat={picked.lat} name={r.name} />}
      {kind === "place" && <CityCategories name={r?.name ?? picked.label} />}
      {r && (r.wikipedia || r.wikidata) && <WikiCard tag={(r.wikipedia ?? r.wikidata)!} online={online} />}

    </div>
  );
}

/** Address, phone, website and brand of a place (OSM tags the address index keeps). */
function PlaceDetails({ r, online }: { r: GeoResult; online: boolean }) {
  const t = useT();
  const rows: { icon: typeof Phone; label: string; value: string; href?: string; ltr?: boolean }[] = [];
  if (r.address && r.kind !== "address") rows.push({ icon: MapPin, label: t("placeAddress"), value: r.address });
  if (r.phone) rows.push({ icon: Phone, label: t("placePhone"), value: r.phone, ltr: true });
  if (r.website) rows.push({ icon: Globe, label: t("placeWebsite"), value: r.website.replace(/^https?:\/\//, "").replace(/\/$/, ""), href: /^https?:/.test(r.website) ? r.website : `https://${r.website}`, ltr: true });
  const brand = r.brand ?? r.operator;
  if (brand && brand !== r.name) rows.push({ icon: Tag, label: r.brand ? t("placeBrand") : t("placeOperator"), value: brand });
  if (!rows.length) return null;
  return (
    <ul className="m-0 p-0 list-none flex flex-col gap-1.5">
      {rows.map((row) => (
        <li key={row.label} className="flex items-center gap-2.5 min-h-8">
          <Icon as={row.icon} size={16} className="text-muted shrink-0" />
          <span className="sr-only">{row.label}: </span>
          {row.href && online ? (
            <button className="flex-1 min-w-0 text-start text-accent hover:underline underline-offset-2 truncate-dir" onClick={() => void openUrl(row.href!).catch(() => {})}>
              <bdi dir="ltr">{row.value}</bdi>
            </button>
          ) : (
            <span className="flex-1 min-w-0 truncate-dir selectable">{row.ltr ? <bdi dir="ltr">{row.value}</bdi> : row.value}</span>
          )}
          <button
            className="btn btn-ghost btn-icon btn-sm text-muted shrink-0"
            aria-label={`${t("copy")}: ${row.label}`}
            onClick={async () => {
              if (await copyText(row.href ?? row.value)) useStore.getState().toast("success", t("copied"));
            }}
          >
            <Icon as={Copy} size={14} />
          </button>
        </li>
      ))}
      {rows.some((x) => x.href) && !online && <li className="text-xs text-subtle">{t("websiteNeedsInternet")}</li>}
    </ul>
  );
}

/** The Wikipedia summary kept in the address index (offline); the full article online. */
function WikiCard({ tag, online }: { tag: string; online: boolean }) {
  const t = useT();
  const [w, setW] = useState<WikiSummary | null | undefined>(undefined);
  const [more, setMore] = useState(false);
  useEffect(() => {
    let dead = false;
    apiX
      .wikiSummary(tag)
      .then((s) => !dead && setW(s))
      .catch(() => !dead && setW(null));
    return () => {
      dead = true;
    };
  }, [tag]);
  if (w === undefined) return <div className="skeleton h-16" aria-busy="true" />;
  if (!w || !w.extract) return null;
  const long = w.extract.length > 280;
  return (
    <section className="flex flex-col gap-1.5" aria-labelledby="wiki-title">
      <h3 id="wiki-title" className="section-label m-0 p-0 flex items-center gap-1.5">
        <Icon as={BookOpen} size={14} /> {t("wikiTitle")}
      </h3>
      <p className="m-0 text-sm selectable" lang={w.lang}>
        {long && !more ? `${w.extract.slice(0, 280).replace(/\s+\S*$/, "")}…` : w.extract}
      </p>
      <div className="flex gap-3 text-sm">
        {long && (
          <button className="text-accent hover:underline underline-offset-2" onClick={() => setMore((m) => !m)}>
            {more ? t("showLess") : t("showMore")}
          </button>
        )}
        {online && w.url && (
          <button className="text-accent hover:underline underline-offset-2" onClick={() => void openUrl(w.url).catch(() => {})}>
            {t("wikiOpen")}
          </button>
        )}
      </div>
      <p className="m-0 text-xs text-subtle">{t("wikiCredit")}</p>
    </section>
  );
}

function RoundAction({
  icon,
  label,
  onClick,
  primary,
  pressed,
  tip,
  filled,
  flip,
}: {
  icon: typeof Flag;
  label: string;
  onClick: () => void;
  primary?: boolean;
  pressed?: boolean;
  tip?: string;
  filled?: boolean;
  flip?: boolean;
}) {
  const btn = (
    <button className="group flex-1 min-w-0 flex flex-col items-center gap-1.5" onClick={onClick} aria-pressed={pressed}>
      <span
        className={clsx(
          "w-10 h-10 rounded-full flex items-center justify-center transition-colors duration-150",
          primary ? "bg-[var(--accent-solid)] text-[var(--accent-fg)] group-hover:bg-[var(--accent-solid-hover)]" : "border border-line-strong text-accent group-hover:bg-hover",
          pressed && "text-warning",
        )}
      >
        <Icon as={icon} size={18} fill={filled ? "currentColor" : "none"} className={flip ? "rtl:-scale-x-100" : undefined} />
      </span>
      <span className="text-xs text-center leading-tight text-fg w-full break-words [text-align-last:center]">{label}</span>
    </button>
  );
  return tip ? <Tip text={tip}>{btn}</Tip> : btn;
}

function OpeningHours({ r }: { r: GeoResult }) {
  const t = useT();
  if (!r.openingHours && r.openNow === undefined && !r.kosher) return null;
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2 flex-wrap">
        <Icon as={Clock} size={14} className="text-muted" />
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
  if (!board) return <div className="progress-line" role="progressbar" aria-label={t("loading")} />;
  const deps = board.lines
    .flatMap((l) => l.next.map(([rel, iso]) => ({ l, rel, iso })))
    .sort((a, b) => a.rel - b.rel)
    .slice(0, 6);
  return (
    <div className="flex flex-col gap-1.5">
      <h3 className="section-label m-0 p-0">{t("nextDepartures")}</h3>
      {deps.length === 0 ? (
        board.nextServiceIso ? (
          <InfoBar severity="info">{t("nextServiceAt").replace("{t}", nextServiceText(board.nextServiceIso, t))}</InfoBar>
        ) : (
          <p className="m-0 text-muted">{t("noDeparturesSoon")}</p>
        )
      ) : (
        <ul className="m-0 p-0 list-none flex flex-col">
          {deps.map(({ l, rel, iso }, i) => {
            const c = countdown(rel, iso, board.at, now, t);
            return (
              <li key={i} className="flex items-center gap-2 min-h-8">
                <span className="inline-flex items-center justify-center h-6 min-w-7 px-1.5 rounded text-white text-xs font-semibold num shrink-0" style={{ background: l.color ? `#${l.color.replace(/^#/, "")}` : "var(--route-alt)" }}>
                  {l.shortName}
                </span>
                <span className="flex-1 min-w-0 truncate-dir text-muted">{l.headsign}</span>
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

/** "מוצ״ש 19:40" style text for the first departure after a service break. */
export function nextServiceText(iso: string, t: (k: I18nKey) => string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  const today = new Date();
  const days = Math.round((new Date(d.toDateString()).getTime() - new Date(today.toDateString()).getTime()) / 86_400_000);
  const day = days <= 0 ? t("todayLabel") : days === 1 ? t("tomorrowLabel") : t(`weekday${d.getDay()}` as I18nKey);
  return `${day} ${fmtTime(iso)}`;
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
