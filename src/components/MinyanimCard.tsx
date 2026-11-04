import { useEffect, useState } from "react";
import clsx from "clsx";
import { api, type MinyanTable, type NextMinyan } from "../lib/api";
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

/**
 * Prayer times of a synagogue (Nedarim Plus weekly snapshot) and the next minyanim nearby.
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
    // online the backend asks Nedarim Plus live, offline (or when that fails) it reads the snapshot
    const a = api.minyanimFor(lon, lat, name).then((v) => alive && setTable(v), () => undefined);
    const b = api.minyanimNext(lon, lat, 600, 120).then((v) => alive && setNext(v.slice(0, 5)), () => undefined);
    void Promise.all([a, b]).then(() => alive && setLoaded(true));
    return () => {
      alive = false;
    };
  }, [lon, lat, name]);

  if (!table && next.length === 0 && loaded && installed === false)
    return (
      <div className="card p-3 flex flex-col gap-2">
        <div className="text-sm font-semibold">{t("minyanimTitle")}</div>
        <p className="m-0 text-sm text-muted">{t("minyanimNotInstalled")}</p>
        <button className="btn btn-sm self-start" onClick={() => useStore.getState().setPanel("data")}>
          {t("openDataPanel")}
        </button>
      </div>
    );
  if (!table && next.length === 0) return null;

  const days = TABS[tab].days;
  const rows = (table?.minyanim ?? []).filter((m) => [...m.days].some((d) => days.includes(d))).sort((a, b) => minutesOf(a.time) - minutesOf(b.time));
  // the next prayer today in this synagogue is highlighted
  const now = new Date().getHours() * 60 + new Date().getMinutes();
  const nextHere = days.includes(today) ? rows.find((m) => m.days.includes(today) && minutesOf(m.time) >= now) : undefined;

  return (
    <div className="flex flex-col gap-2">
      {table && (
        <div className="card p-3 flex flex-col gap-2">
          <div className="text-sm font-semibold">{t("minyanimTitle")}</div>
          <div className="seg seg-sm">
            {TABS.map((x, i) => (
              <button key={x.key} aria-pressed={tab === i} onClick={() => setTab(i)}>
                <span>{t(x.key)}</span>
              </button>
            ))}
          </div>
          {rows.length === 0 ? (
            <p className="m-0 text-sm text-muted">{t("minyanimNone")}</p>
          ) : (
            <ul className="m-0 p-0 list-none flex flex-col">
              {rows.map((m, i) => (
                <li key={i} className={clsx("flex items-baseline gap-2 py-1 px-1.5 rounded text-sm", m === nextHere && "bg-accent-soft text-accent font-semibold")}>
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
      {next.length > 0 && (
        <div className="card p-3 flex flex-col gap-1.5">
          <div className="text-sm font-semibold">{t("minyanimNearby")}</div>
          <ul className="m-0 p-0 list-none flex flex-col">
            {next.map((m, i) => (
              <li key={i} className="flex items-baseline gap-2 py-1 text-sm">
                <span className="num w-12 shrink-0" dir="ltr">
                  {m.time}
                </span>
                <span className="flex-1 min-w-0 truncate">
                  {m.prayer} · {m.name ?? m.address ?? ""}
                </span>
                <span className="text-xs text-muted num shrink-0">
                  {t("minyanInMinutes").replace("{n}", String(m.inMinutes))} · {fmtDistance(m.distanceM, t, units)}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
