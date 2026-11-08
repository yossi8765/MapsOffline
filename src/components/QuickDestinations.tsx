import { Briefcase, Clock, House } from "lucide-react";
import { Icon } from "./Icon";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { navigateTo } from "../lib/nav";
import type { Favorite, HistoryItem } from "../lib/api";

interface Dest {
  key: string;
  label: string;
  lon: number;
  lat: number;
  kind: "home" | "work" | "recent";
}

const near = (a: { lon: number; lat: number }, b: { lon: number; lat: number }) => Math.abs(a.lon - b.lon) < 3e-4 && Math.abs(a.lat - b.lat) < 3e-4;

/** The last few places routed or searched to, newest first, without "my location" and without
 *  repeats (the same spot under two labels counts once). */
export function recentDestinations(history: HistoryItem[], skip: { lon: number; lat: number }[], myLocationLabel: string, max = 3): Dest[] {
  const out: Dest[] = [];
  const add = (label: string | undefined, lon: unknown, lat: unknown) => {
    if (!label || typeof lon !== "number" || typeof lat !== "number" || label === myLocationLabel) return;
    const p = { lon, lat };
    if (skip.some((s) => near(s, p)) || out.some((d) => near(d, p) || d.label === label)) return;
    out.push({ key: `r${out.length}`, label, lon, lat, kind: "recent" });
  };
  for (const h of history) {
    if (out.length >= max) break;
    const p = h.payload as Record<string, unknown> | null;
    if (!p) continue;
    if (h.kind === "route") {
      const wps = (p.request as { waypoints?: { lon: number; lat: number }[] } | undefined)?.waypoints;
      const labels = p.labels as string[] | undefined;
      const last = wps?.[wps.length - 1];
      if (last) add(labels?.[labels.length - 1], last.lon, last.lat);
    } else if (h.kind === "search") {
      add((p.name as string | undefined) ?? h.label, p.lon, p.lat);
    }
  }
  return out;
}

/** One tap from the search screen into navigation: home, work, and the last destinations. */
export function QuickDestinations() {
  const t = useT();
  const favorites = useStore((s) => s.favorites);
  const history = useStore((s) => s.history);
  const home = favorites.find((f: Favorite) => f.icon === "home");
  const work = favorites.find((f: Favorite) => f.icon === "briefcase");
  const fixed: Dest[] = [
    ...(home ? [{ key: "home", label: t("goHome"), lon: home.lon, lat: home.lat, kind: "home" as const }] : []),
    ...(work ? [{ key: "work", label: t("goWork"), lon: work.lon, lat: work.lat, kind: "work" as const }] : []),
  ];
  const recent = recentDestinations(history, fixed, t("myLocation"));
  const all = [...fixed, ...recent];
  if (!all.length) return null;
  return (
    <section className="flex flex-col gap-1.5" aria-label={t("quickDestinations")}>
      <h2 className="section-label m-0 px-0.5">{t("quickDestinations")}</h2>
      <div className="group-list">
        {all.map((d) => (
          <button
            key={d.key}
            className="row w-full text-start hover:bg-hover"
            onClick={() => void navigateTo({ lon: d.lon, lat: d.lat, label: d.kind === "home" ? (home?.name ?? d.label) : d.kind === "work" ? (work?.name ?? d.label) : d.label })}
          >
            <Icon as={d.kind === "home" ? House : d.kind === "work" ? Briefcase : Clock} size={16} className={d.kind === "recent" ? "text-muted" : "text-accent"} />
            <span className={d.kind === "recent" ? "flex-1 min-w-0 truncate" : "flex-1 min-w-0 truncate font-medium"}>{d.label}</span>
          </button>
        ))}
      </div>
    </section>
  );
}
