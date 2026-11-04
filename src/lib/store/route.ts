import { api, apiX, type Avoid, type LonLat, type Mode, type Route, type RouteRequest, type RouteSource, type ServiceNotice, type TimeSpec, type TripStop } from "../api";
import { describeError } from "../errors";
import type { NavState } from "../nav";
import { sortTransit } from "../transitSort";
import { avoidDefaults, DEFAULT_SETTINGS } from "./settings";
import type { HighlightState, RoutePoint, SliceCreator } from "./types";

export interface RouteSlice {
  waypoints: (RoutePoint | null)[]; // [from, ...via, to]
  mode: Mode;
  time: TimeSpec;
  avoid: Avoid;
  alternativesCount: number;
  routes: Route[];
  selectedRouteId: string | null;
  /** The selected route card is expanded to show its navigation steps. */
  detailsOpen: boolean;
  routeLoading: boolean;
  routeError: string | null;
  routeSource: RouteSource | null;
  routeProvider: string | null;
  routeTick: number;
  fitRouteTick: number;
  /** Every transit result of the last search, in the preference's order (the list shows the
   *  first `alternativesCount`); re-sorted when the preference changes. */
  transitAll: Route[];
  highlight: HighlightState | null;
  lastRequest: RouteRequest | null;
  /** live navigation session (src/lib/nav.ts), null when not navigating */
  navigation: NavState | null;
  /** Why transit found little or nothing (Shabbat, holiday, night), from the last response. */
  routeNotice: ServiceNotice | null;
  /** Truck dimensions (mode "truck"). */
  vehicle: { heightM?: number; weightT?: number; widthM?: number };
  /** Transit: wheelchair-accessible trips and stops only. */
  accessible: boolean;
  /** Trip planner: stay / time window per waypoint (aligned with `waypoints`). */
  tripStops: TripStop[];
  /** Planned arrival times per waypoint after "plan by times". */
  tripArrivals: string[] | null;

  setWaypoint: (index: number, p: RoutePoint | null) => void;
  setFrom: (p: RoutePoint) => void;
  setTo: (p: RoutePoint) => void;
  addVia: (p?: RoutePoint | null) => void;
  removeWaypoint: (index: number) => void;
  moveWaypoint: (index: number, dir: -1 | 1) => void;
  swapEnds: () => void;
  setMode: (m: Mode) => void;
  setTime: (t: TimeSpec) => void;
  setAvoid: (a: Partial<Avoid>) => void;
  setAlternativesCount: (n: number) => void;
  computeRoute: () => Promise<void>;
  /** start live navigation along one of the current routes (simulate: drive it without GPS) */
  startNavigation: (routeId: string, simulate?: boolean) => Promise<void>;
  stopNavigation: () => Promise<void>;
  selectRoute: (id: string | null) => void;
  /** Click on a route card: select it (opening its steps), or toggle the steps when already selected. */
  toggleRoute: (id: string) => void;
  clearRoute: () => void;
  setHighlight: (h: HighlightState | null) => void;
  loadRequest: (req: RouteRequest, labels?: string[]) => void;
  /** Reorder the via stops for the shortest trip (origin fixed; destination fixed when `fixedEnd`). */
  optimizeOrder: (fixedEnd: boolean) => Promise<void>;
  /** Plan the order with stays and time windows (up to 25 stops). */
  optimizeTrip: (fixedEnd: boolean) => Promise<void>;
  setTripStop: (index: number, patch: Partial<TripStop>) => void;
  setVehicle: (v: Partial<RouteSlice["vehicle"]>) => void;
  setAccessible: (on: boolean) => void;
  /** "Avoid this road": the road nearest the point is left out; recomputes. Undo via the toast. */
  avoidRoadAt: (at: LonLat) => void;
  /** Most waypoints the current settings allow (25 with one route, else 7). */
  maxWaypoints: () => number;
}

let routeSeq = 0;

/** Every field of a route result: cleared together so nothing of an old result lingers
 *  (transitAll, the source line, the request) when the points change or a request fails. */
const EMPTY_RESULT = {
  routes: [] as Route[],
  transitAll: [] as Route[],
  selectedRouteId: null,
  routeError: null,
  routeSource: null,
  routeProvider: null,
  routeNotice: null,
  highlight: null,
  lastRequest: null,
  detailsOpen: false,
};

export const createRouteSlice: SliceCreator<RouteSlice> = (set, get) => ({
  waypoints: [null, null],
  mode: "car",
  time: { kind: "now" },
  avoid: avoidDefaults(DEFAULT_SETTINGS),
  alternativesCount: 3,
  routes: [],
  selectedRouteId: null,
  detailsOpen: false,
  routeLoading: false,
  routeError: null,
  routeSource: null,
  routeProvider: null,
  routeTick: 0,
  fitRouteTick: 0,
  transitAll: [],
  highlight: null,
  lastRequest: null,
  navigation: null,
  routeNotice: null,
  vehicle: {},
  accessible: false,
  tripStops: [],
  tripArrivals: null,

  startNavigation: async (routeId, simulate) => {
    const m = await import("../nav");
    await m.startNavigation(routeId, simulate);
  },
  stopNavigation: async () => {
    const m = await import("../nav");
    await m.stopNavigation();
  },

  setWaypoint: (index, p) => {
    const wps = [...get().waypoints];
    wps[index] = p;
    set({ waypoints: wps });
    void get().computeRoute();
  },
  setFrom: (p) => get().setWaypoint(0, p),
  setTo: (p) => get().setWaypoint(get().waypoints.length - 1, p),
  addVia: (p) => {
    const wps = [...get().waypoints];
    if (wps.length >= get().maxWaypoints()) return;
    wps.splice(wps.length - 1, 0, p ?? null);
    const stops = [...get().tripStops];
    if (stops.length) stops.splice(wps.length - 2, 0, {});
    set({ waypoints: wps, tripStops: stops, tripArrivals: null });
    if (p) void get().computeRoute();
  },
  removeWaypoint: (index) => {
    const wps = [...get().waypoints];
    const stops = [...get().tripStops];
    if (wps.length <= 2) {
      wps[index] = null;
    } else {
      wps.splice(index, 1);
      stops.splice(index, 1);
    }
    set({ waypoints: wps, tripStops: stops, tripArrivals: null });
    void get().computeRoute();
  },
  moveWaypoint: (index, dir) => {
    const wps = [...get().waypoints];
    const j = index + dir;
    if (j < 0 || j >= wps.length) return;
    [wps[index], wps[j]] = [wps[j], wps[index]];
    const stops = [...get().tripStops];
    if (stops.length > Math.max(index, j)) [stops[index], stops[j]] = [stops[j], stops[index]];
    set({ waypoints: wps, tripStops: stops, tripArrivals: null });
    void get().computeRoute();
  },
  swapEnds: () => {
    const wps = [...get().waypoints].reverse();
    set({ waypoints: wps });
    void get().computeRoute();
  },
  setMode: (mode) => {
    set({ mode });
    void get().computeRoute();
  },
  setTime: (time) => {
    set({ time });
    void get().computeRoute();
  },
  setAvoid: (a) => {
    set({ avoid: { ...get().avoid, ...a } });
    void get().computeRoute();
  },
  setAlternativesCount: (n) => {
    set({ alternativesCount: n });
    void get().computeRoute();
  },
  computeRoute: async () => {
    const s = get();
    const pts = s.waypoints;
    if (pts.some((p) => !p) || pts.length < 2) {
      routeSeq++;
      set({ ...EMPTY_RESULT, routeLoading: false });
      return;
    }
    const req: RouteRequest = {
      waypoints: pts.map((p) => ({ lon: p!.lon, lat: p!.lat, name: p!.label })),
      mode: s.mode,
      time: s.time.kind === "now" ? { kind: "now" } : s.time,
      avoid: s.avoid,
      alternatives: s.mode === "transit" ? Math.max(s.alternativesCount, 3) : s.alternativesCount,
      ...(s.mode === "truck" ? { vehicle: s.vehicle } : {}),
      ...(s.mode === "transit" && s.accessible ? { accessible: true } : {}),
    };
    const seq = ++routeSeq;
    set({ routeLoading: true, routeError: null, routeNotice: null, lastRequest: req });
    try {
      const resp = await api.route(req);
      if (seq !== routeSeq) return;
      // transit asks for at least 3 so the preference has something to choose from; show only as
      // many as the user picked in "מספר מסלולים"
      const sorted = req.mode === "transit" ? sortTransit(resp.routes, get().settings.transitPreference ?? "fastest", req.time.kind) : resp.routes;
      const routes = sorted.slice(0, Math.max(1, get().alternativesCount));
      set({
        transitAll: req.mode === "transit" ? sorted : [],
        routes,
        routeSource: resp.source ?? "offline",
        routeProvider: resp.provider ?? null,
        selectedRouteId: routes[0]?.id ?? null,
        detailsOpen: false,
        routeLoading: false,
        routeError: routes.length ? null : get().t("noRoute"),
        routeTick: get().routeTick + 1,
        fitRouteTick: get().fitRouteTick + 1,
        highlight: null,
        routeNotice: resp.notice ?? null,
      });
      if (routes.length) {
        const label = `${pts[0]!.label} ${get().settings.language === "en" ? "→" : "←"} ${pts[pts.length - 1]!.label}`;
        const r = routes[0];
        void get().addHistory("route", label, {
          request: req,
          labels: pts.map((p) => p!.label),
          summary: { durationSec: r.durationSec, distanceM: r.distanceM, mode: r.mode, transfers: r.transfers },
        });
      }
    } catch (e) {
      if (seq !== routeSeq) return;
      // the code's own message; an unclassified failure shows its text as it came
      const d = describeError(e, get().t);
      // NO_SERVICE carries the notice (Shabbat / holiday / night) as JSON in details
      let notice: ServiceNotice | null = null;
      if (d.code === "NO_SERVICE" && d.details) {
        try {
          const m = /\{[\s\S]*\}/.exec(d.details);
          if (m) notice = JSON.parse(m[0]) as ServiceNotice;
        } catch {
          /* plain text details */
        }
      }
      set({ ...EMPTY_RESULT, routeLoading: false, routeNotice: notice, routeError: notice ? null : d.known ? d.title : (d.details ?? d.title) });
    }
  },
  selectRoute: (selectedRouteId) => set({ selectedRouteId, highlight: null }),
  toggleRoute: (id) => {
    const s = get();
    if (s.selectedRouteId === id) set({ detailsOpen: !s.detailsOpen, highlight: null });
    else set({ selectedRouteId: id, detailsOpen: true, highlight: null });
  },
  clearRoute: () => {
    routeSeq++;
    const a = get().avoid;
    set({
      ...EMPTY_RESULT,
      waypoints: [null, null],
      routeLoading: false,
      tripStops: [],
      tripArrivals: null,
      // roads / areas avoided for this trip only
      avoid: { ...a, points: undefined, polygons: undefined },
    });
  },
  setHighlight: (highlight) => set({ highlight }),
  loadRequest: (req, labels) => {
    const wps: RoutePoint[] = req.waypoints.map((w, i) => ({
      lon: w.lon,
      lat: w.lat,
      label: labels?.[i] ?? w.name ?? `${w.lat.toFixed(5)}, ${w.lon.toFixed(5)}`,
    }));
    set({
      waypoints: wps,
      mode: req.mode,
      time: req.time ?? { kind: "now" },
      avoid: req.avoid,
      alternativesCount: req.alternatives || 3,
      panel: "route",
      sidebarCollapsed: false,
    });
    void get().computeRoute();
  },
  optimizeOrder: async (fixedEnd) => {
    const s = get();
    const pts = s.waypoints;
    if (pts.some((p) => !p) || pts.length < 3) return;
    const req: RouteRequest = {
      waypoints: pts.map((p) => ({ lon: p!.lon, lat: p!.lat, name: p!.label })),
      mode: s.mode,
      time: { kind: "now" },
      avoid: s.avoid,
      alternatives: 0,
    };
    set({ routeLoading: true });
    try {
      const order = await api.optimizeOrder(req, fixedEnd);
      const changed = order.some((o, i) => o !== i);
      const stops = get().tripStops;
      set({ waypoints: order.map((i) => pts[i]), tripStops: stops.length ? order.map((i) => stops[i] ?? {}) : stops, tripArrivals: null });
      if (!changed) get().toast("info", get().t("orderAlreadyBest"));
      await get().computeRoute();
    } catch (e) {
      set({ routeLoading: false });
      get().toastError(e);
    }
  },
  optimizeTrip: async (fixedEnd) => {
    const s = get();
    const pts = s.waypoints;
    if (pts.some((p) => !p) || pts.length < 3) return;
    const req: RouteRequest = {
      waypoints: pts.map((p) => ({ lon: p!.lon, lat: p!.lat, name: p!.label })),
      mode: s.mode,
      time: s.time.kind === "now" ? { kind: "depart", iso: new Date().toISOString() } : s.time,
      avoid: s.avoid,
      alternatives: 1,
    };
    const stops = pts.map((_, i) => s.tripStops[i] ?? {});
    set({ routeLoading: true });
    try {
      const plan = await apiX.optimizeTrip(req, fixedEnd, stops);
      set({ waypoints: plan.order.map((i) => pts[i]), tripStops: plan.order.map((i) => stops[i]), tripArrivals: plan.arrivals });
      await get().computeRoute();
    } catch (e) {
      set({ routeLoading: false });
      get().toastError(e);
    }
  },
  setTripStop: (index, patch) => {
    const stops = get().waypoints.map((_, i) => get().tripStops[i] ?? {});
    stops[index] = { ...stops[index], ...patch };
    set({ tripStops: stops, tripArrivals: null });
  },
  setVehicle: (v) => {
    set({ vehicle: { ...get().vehicle, ...v } });
    if (get().mode === "truck") void get().computeRoute();
  },
  setAccessible: (accessible) => {
    set({ accessible });
    if (get().mode === "transit") void get().computeRoute();
  },
  avoidRoadAt: (at) => {
    const before = get().avoid.points ?? [];
    set({ avoid: { ...get().avoid, points: [...before, at] } });
    void get().computeRoute();
    get().toast("info", get().t("roadAvoided"), {
      action: {
        label: get().t("undo"),
        run: () => {
          set({ avoid: { ...get().avoid, points: before.length ? before : undefined } });
          void get().computeRoute();
        },
      },
    });
  },
  maxWaypoints: () => (get().alternativesCount === 1 ? 25 : 7),
});
