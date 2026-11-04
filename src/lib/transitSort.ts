import type { Route, RouteTag, TransitPreference } from "./api";

/** Public-transport results in the user's preferred order (the first one is selected). */
export function sortTransit(routes: Route[], pref: TransitPreference, timeKind: string): Route[] {
  const arrival = (r: Route) => (r.arriveIso ? Date.parse(r.arriveIso) : Date.now() + r.durationSec * 1000);
  const departure = (r: Route) => (r.departIso ? Date.parse(r.departIso) : 0);
  // "arrive by": the latest departure is the best; otherwise the earliest arrival
  const time = (a: Route, b: Route) => (timeKind === "arrive" ? departure(b) - departure(a) : arrival(a) - arrival(b)) || a.durationSec - b.durationSec;
  const transfers = (r: Route) => Math.max(0, r.legs.filter((l) => l.kind === "transit").length - 1);
  const walk = (r: Route) => r.legs.filter((l) => l.kind === "walk").reduce((m, l) => m + l.distanceM, 0);
  // "fastest" = arrives first; "shortest" = the shortest trip (may leave later and arrive later)
  const by =
    pref === "fewestTransfers"
      ? (a: Route, b: Route) => transfers(a) - transfers(b) || time(a, b)
      : pref === "leastWalking"
        ? (a: Route, b: Route) => walk(a) - walk(b) || time(a, b)
        : pref === "shortest"
          ? (a: Route, b: Route) => a.durationSec - b.durationSec || time(a, b)
          : time;
  return retagTransit([...routes].sort(by), time);
}

/** Transit tags that match the preferences: "fastest" on the route that arrives first (by
 *  "arrive by": leaves last), "shortest" on the shortest trip; the providers' other tags stay. */
export function retagTransit(routes: Route[], time: (a: Route, b: Route) => number): Route[] {
  if (!routes.length) return routes;
  const first = routes.reduce((b, r) => (time(r, b) < 0 ? r : b));
  const short = routes.reduce((b, r) => (r.durationSec < b.durationSec ? r : b));
  return routes.map((r) => {
    const tags: RouteTag[] = r.tags.filter((g) => g !== "fastest" && g !== "shortest");
    if (r === first) tags.unshift("fastest");
    if (r === short) tags.splice(r === first ? 1 : 0, 0, "shortest");
    return { ...r, tags };
  });
}
