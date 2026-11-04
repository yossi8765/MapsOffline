import { useEffect, useLayoutEffect, useRef, useState, type MutableRefObject } from "react";
import { Flag, Navigation } from "lucide-react";
import { api, type GeoResult, type LonLat, type Minyan } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";

/** What the card shows: a POI of the category layer or a transit stop (from the map feature). */
export interface HoverTarget {
  kind: "poi" | "stop";
  name: string;
  /** category label (POI) */
  sub?: string;
  /** stop code */
  code?: string;
  color?: string;
  /** a POI's full search result (category, opening hours) for the place card */
  result?: GeoResult;
  lonlat: LonLat;
  /** screen position of the feature inside the map container */
  x: number;
  y: number;
  /** opened by a click: stays until closed, not by the mouse leaving */
  pinned: boolean;
}

interface Props {
  target: HoverTarget;
  onEnter: () => void;
  onLeave: () => void;
  onClose: () => void;
  /** receives the card element (the hover hook moves a pinned card with the map through it) */
  elRef?: MutableRefObject<HTMLDivElement | null>;
}

/** Desktop hover card next to a map marker (hebrew-maps / Google style): the name and kind right
 *  away from the feature, a stop's next departures a moment later, and route buttons. */
export function MapHoverCard({ target, onEnter, onLeave, onClose, elRef }: Props) {
  const t = useT();
  const [next, setNext] = useState<{ line: string; color?: string; min: number; headsign: string }[] | null>(null);
  const synagogue = target.kind === "poi" && isSynagogue(target.result?.category, target.sub);
  // a synagogue: its next prayers today (Nedarim Plus), when the minyanim data is installed
  const [prayers, setPrayers] = useState<Minyan[] | null>(null);
  useEffect(() => {
    setPrayers(null);
    if (!synagogue) return;
    let dead = false;
    api
      .minyanimFor(target.lonlat[0], target.lonlat[1], target.name || undefined)
      .then((tb) => {
        if (dead) return;
        const d = new Date();
        const today = "אבגדהוש"[d.getDay()];
        const now = d.getHours() * 60 + d.getMinutes();
        const mins = (hhmm: string) => Number(hhmm.slice(0, 2)) * 60 + Number(hhmm.slice(3, 5));
        setPrayers((tb?.minyanim ?? []).filter((m) => m.days.includes(today) && mins(m.time) >= now).sort((a, b) => mins(a.time) - mins(b.time)).slice(0, 3));
      })
      .catch(() => !dead && setPrayers([]));
    return () => {
      dead = true;
    };
  }, [synagogue, target.lonlat, target.name]);

  useEffect(() => {
    setNext(null);
    if (target.kind !== "stop") return;
    let dead = false;
    api
      .transitStopBoard(target.code || undefined, target.lonlat[0], target.lonlat[1])
      .then((b) => {
        if (dead) return;
        const deps = b.lines
          .flatMap((l) => l.next.slice(0, 2).map(([sec]) => ({ line: l.shortName, color: l.color, min: Math.max(0, Math.round(sec / 60)), headsign: l.headsign })))
          .sort((a, c) => a.min - c.min)
          .slice(0, 3);
        setNext(deps);
      })
      .catch(() => !dead && setNext([]));
    return () => {
      dead = true;
    };
  }, [target.kind, target.code, target.lonlat]);

  const point = { lon: target.lonlat[0], lat: target.lonlat[1], label: target.name, result: target.result };
  const go = (to: boolean) => {
    const s = useStore.getState();
    if (to) s.setTo(point);
    else s.setFrom(point);
    s.setPanel("route");
    onClose();
  };
  const details = () => {
    const s = useStore.getState();
    if (typeof s.pickPlace === "function") s.pickPlace(point);
    onClose();
  };

  // place the card beside the marker (measured height: a stop with departures or a synagogue
  // with prayers is taller than the first guess), again whenever its content grows
  const ownRef = useRef<HTMLDivElement | null>(null);
  const setEl = (el: HTMLDivElement | null) => {
    ownRef.current = el;
    if (elRef) elRef.current = el;
  };
  useLayoutEffect(() => {
    if (ownRef.current) placeHoverCard(ownRef.current, target.x, target.y);
  }, [target.x, target.y, next, prayers]);

  return (
    <div
      ref={setEl}
      className="absolute z-30 popover rounded-[var(--radius)] p-3 flex flex-col gap-2 fade-in no-print"
      style={{ left: target.x + 16, top: Math.max(8, target.y - 24), width: CARD_W }}
      onMouseEnter={onEnter}
      onMouseLeave={onLeave}
      role="dialog"
      aria-label={target.name}
    >
      <div className="flex items-start gap-2">
        {target.color && <span className="mt-1.5 w-2.5 h-2.5 rounded-full shrink-0" style={{ background: target.color }} />}
        <div className="flex-1 min-w-0">
          <div className="font-semibold leading-snug selectable">{target.name || t("unknownPlace")}</div>
          <div className="text-xs text-muted">
            {target.kind === "stop" ? (target.code ? `${t("stopCode")} ${target.code}` : "") : target.sub}
          </div>
        </div>
      </div>
      {target.kind === "stop" && (
        <div className="flex flex-col gap-1 text-sm min-h-6">
          {next === null && <div className="progress-line" role="progressbar" />}
          {next && next.length === 0 && <div className="text-xs text-muted">{t("noMoreToday")}</div>}
          {next?.map((d, i) => (
            <div key={i} className="flex items-center gap-2">
              <span className="inline-flex items-center justify-center h-5 min-w-7 px-1.5 rounded text-white text-xs font-semibold num" style={{ background: d.color ? (d.color.startsWith("#") ? d.color : `#${d.color}`) : "var(--map-bus)" }}>
                {d.line}
              </span>
              <span className="flex-1 min-w-0 truncate text-muted text-xs">{d.headsign}</span>
              <span className="num text-xs shrink-0">
                {d.min} {t("minutes")}
              </span>
            </div>
          ))}
        </div>
      )}
      {synagogue && prayers && prayers.length > 0 && (
        <div className="flex flex-col gap-0.5 text-sm">
          <div className="text-xs text-muted">{t("minyanimToday")}</div>
          {prayers.map((m, i) => (
            <div key={i} className="flex items-baseline gap-2">
              <span className="num w-11 shrink-0" dir="ltr">
                {m.time}
              </span>
              <span className="flex-1 min-w-0 truncate">{m.prayer}</span>
            </div>
          ))}
        </div>
      )}
      <div className="flex gap-1.5">
        <button className="btn btn-sm btn-primary flex-1" onClick={() => go(true)}>
          <Flag size={13} /> {t("ctxToHere")}
        </button>
        <button className="btn btn-sm flex-1" onClick={() => go(false)}>
          <Navigation size={13} className="rtl:-scale-x-100" /> {t("ctxFromHere")}
        </button>
      </div>
      <button className="btn btn-ghost btn-sm text-muted self-start -ms-1.5" onClick={details}>
        {synagogue ? t("minyanimAllTimes") : t("hoverDetails")}
      </button>
    </div>
  );
}

const CARD_W = 260;

/** Put the card beside the marker at (x, y) (map container pixels), flipped to the other side or
 *  pushed up when it would leave the map. Called on open, when its content changes height, and by
 *  the hover hook on every map move of a pinned card (no React render per frame, report ד-4). */
export function placeHoverCard(el: HTMLElement, x: number, y: number): void {
  const host = el.parentElement ?? document.body;
  const hw = host.clientWidth || window.innerWidth;
  const hh = host.clientHeight || window.innerHeight;
  const h = el.offsetHeight || 180;
  const left = Math.min(Math.max(8, x + 16 + CARD_W > hw ? x - 16 - CARD_W : x + 16), hw - CARD_W - 8);
  const top = Math.max(8, Math.min(y - 24, hh - h - 8));
  el.style.left = `${left}px`;
  el.style.top = `${top}px`;
}

/** A synagogue by its category (search label, picker label or raw OSM id). */
export function isSynagogue(...cats: (string | undefined)[]): boolean {
  return cats.some((c) => !!c && /בית כנסת|בתי כנסת|synagogue|place_of_worship:jewish/i.test(c));
}
