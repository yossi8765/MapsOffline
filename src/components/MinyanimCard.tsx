import { useEffect, useState } from "react";
import clsx from "clsx";
import { Radio } from "lucide-react";
import { Icon } from "./Icon";
import { InfoBar } from "./InfoBar";
import { api, apiX, type MinyanTable, type NextMinyan } from "../lib/api";
import { useT, type I18nKey } from "../lib/i18n";
import { fmtDistance } from "../lib/format";
import { useStore } from "../lib/store";

/** Day groups of the timetable tabs: weekdays (Sunday–Thursday), Friday, Shabbat. */
const TABS: { key: I18nKey; days: string }[] = [
  { key: "minyanWeekdays", days: "אבגדה" },
  { key: "minyanFriday", days: "ו" },
  { key: "minyanShabbat", days: "ש" },
];
const DAY_LETTERS = "אבגדהוש";

function minutesOf(hhmm: string): number {
  const [h, m] = hhmm.split(":").map(Number);
  return h * 60 + m;
}

/** Nearby prayers grouped by synagogue (one heading with its distance, its times under it),
 *  synagogues ordered by their soonest prayer; the same prayer listed twice is shown once. */
export function groupNearby(list: NextMinyan[]): { key: string; name: string; distanceM: number; items: NextMinyan[] }[] {
  const groups = new Map<string, { key: string; name: string; distanceM: number; items: NextMinyan[] }>();
  for (const m of list) {
    const key = m.mosadId || `${m.lon.toFixed(5)},${m.lat.toFixed(5)}`;
    const g = groups.get(key) ?? { key, name: m.name ?? m.address ?? "", distanceM: m.distanceM, items: [] };
    if (!g.items.some((x) => x.time === m.time && x.prayer === m.prayer && (x.hall ?? "") === (m.hall ?? ""))) g.items.push(m);
    groups.set(key, g);
  }
  return [...groups.values()].map((g) => ({ ...g, items: g.items.sort((a, b) => a.inMinutes - b.inMinutes) })).sort((a, b) => a.items[0].inMinutes - b.items[0].inMinutes);
}

/**
 * Prayer times of a synagogue and the next minyanim nearby. The weekly snapshot shows at once;
 * when online, the backend fetches the live table from Nedarim Plus and sends it as the
 * "minyanim-live" event, which replaces the snapshot (marked "חי").
 * Renders nothing when the minyanim data is not installed or nothing matches the place.
 */
export function MinyanimCard({ lon, lat, name }: { lon: number; lat: number; name?: string }) {
  const t = useT();
  const units = useStore((s) => s.settings.units);
  const installed = useStore((s) => s.dataStatus?.minyanim?.present);
  const [table, setTable] = useState<MinyanTable | null>(null);
  const [next, setNext] = useState<NextMinyan[]>([]);
  const [loaded, setLoaded] = useState(false);
  const today = DAY_LETTERS[new Date().getDay()];
  const [tab, setTab] = useState(() => Math.max(0, TABS.findIndex((x) => x.days.includes(today))));

  useEffect(() => {
    let alive = true;
    setTable(null);
    setNext([]);
    setLoaded(false);
    const a = api.minyanimFor(lon, lat, name).then(
      (v) => alive && setTable((cur) => (cur?.live ? cur : v)),
      () => undefined,
    );
    const b = api.minyanimNext(lon, lat, 600, 120).then(
      (v) => alive && setNext(v.slice(0, 8)),
      () => undefined,
    );
    void Promise.all([a, b]).then(() => alive && setLoaded(true));
    // the live table for this same spot replaces the snapshot when it arrives
    let un: (() => void) | undefined;
    apiX
      .onMinyanimLive((p) => {
        if (!alive || Math.abs(p.lon - lon) > 1e-6 || Math.abs(p.lat - lat) > 1e-6) return;
        setTable({ ...p.table, live: true });
      })
      .then((f) => (alive ? (un = f) : f()))
      .catch(() => {});
    return () => {
      alive = false;
      un?.();
    };
  }, [lon, lat, name]);

  if (!table && next.length === 0 && loaded && installed === false)
    return (
      <InfoBar
        severity="info"
        title={t("minyanimTitle")}
        action={
          <button className="btn btn-sm" onClick={() => useStore.getState().setPanel("data")}>
            {t("openDataPanel")}
          </button>
        }
      >
        {t("minyanimNotInstalled")}
      </InfoBar>
    );
  if (!table && next.length === 0) return loaded ? null : <div className="skeleton h-20" aria-busy="true" />;

  const days = TABS[tab].days;
  const rows = (table?.minyanim ?? []).filter((m) => [...m.days].some((d) => days.includes(d))).sort((a, b) => minutesOf(a.time) - minutesOf(b.time));
  // the next prayer today in this synagogue is highlighted
  const now = new Date().getHours() * 60 + new Date().getMinutes();
  const nextHere = days.includes(today) ? rows.find((m) => m.days.includes(today) && minutesOf(m.time) >= now) : undefined;
  const groups = groupNearby(next).slice(0, 4);

  return (
    <div className="flex flex-col gap-2">
      {table && (
        <div className="card p-3 flex flex-col gap-2">
          <div className="font-semibold flex items-center gap-2">
            {t("minyanimTitle")}
            {table.live && (
              <span className="chip chip-success">
                <Icon as={Radio} size={12} />
                {t("liveLabel")}
              </span>
            )}
          </div>
          <div className="seg seg-sm" role="group" aria-label={t("minyanimTitle")}>
            {TABS.map((x, i) => (
              <button key={x.key} aria-pressed={tab === i} onClick={() => setTab(i)}>
                <span>{t(x.key)}</span>
              </button>
            ))}
          </div>
          {rows.length === 0 ? (
            <p className="m-0 text-muted">{t("minyanimNone")}</p>
          ) : (
            <ul className="m-0 p-0 list-none flex flex-col">
              {rows.map((m, i) => (
                <li key={i} className={clsx("flex items-baseline gap-2 py-1 px-1.5 rounded", m === nextHere && "bg-accent-soft text-accent font-semibold")}>
                  <span className="num w-12 shrink-0" dir="ltr">
                    {m.time}
                  </span>
                  <span className="flex-1 min-w-0">
                    {m.prayer}
                    {m.hall && <span className="text-muted font-normal"> · {m.hall}</span>}
                  </span>
                </li>
              ))}
            </ul>
          )}
          {table.message && <p className="m-0 text-xs text-muted">{table.message}</p>}
          <p className="m-0 text-xs text-subtle">
            {table.source} · {table.live ? `${t("minyanimLive")} ${table.fetched.slice(11)}` : table.fetched}
          </p>
        </div>
      )}
      {groups.length > 0 && (
        <div className="card p-3 flex flex-col gap-2">
          <div className="font-semibold">{t("minyanimNearby")}</div>
          {groups.map((g) => (
            <section key={g.key} className="flex flex-col gap-0.5">
              <div className="flex items-baseline gap-2">
                <bdi className="flex-1 min-w-0 truncate-dir font-medium">{g.name}</bdi>
                <span className="text-xs text-muted num shrink-0">{fmtDistance(g.distanceM, t, units)}</span>
              </div>
              <ul className="m-0 p-0 list-none flex flex-col">
                {g.items.map((m, i) => (
                  <li key={i} className="flex items-baseline gap-2 py-0.5">
                    <span className="num w-12 shrink-0" dir="ltr">
                      {m.time}
                    </span>
                    <span className="flex-1 min-w-0 truncate-dir">
                      {m.prayer}
                      {m.hall && <span className="text-muted"> · {m.hall}</span>}
                    </span>
                    <span className="text-xs text-muted num shrink-0">{t("minyanInMinutes").replace("{n}", String(m.inMinutes))}</span>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}
