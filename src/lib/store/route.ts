import { api, type Avoid, type Mode, type Route, type RouteRequest, type RouteSource, type TimeSpec } from "../api";
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
}

let routeSeq = 0;

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
    if (wps.length >= 7) return;
    wps.splice(wps.length - 1, 0, p ?? null);
    set({ waypoints: wps });
    if (p) void get().computeRoute();
  },
  removeWaypoint: (index) => {
    const wps = [...get().waypoints];
    if (wps.length <= 2) {
      wps[index] = null;
    } else {
      wps.splice(index, 1);
    }
    set({ waypoints: wps });
    void get().computeRoute();
  },
  moveWaypoint: (index, dir) => {
    const wps = [...get().waypoints];
    const j = index + dir;
    if (j < 0 || j >= wps.length) return;
    [wps[index], wps[j]] = [wps[j], wps[index]];
    set({ waypoints: wps });
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
      set({ routes: [], selectedRouteId: null, routeError: null, highlight: null });
      return;
    }
    const req: RouteRequest = {
      waypoints: pts.map((p) => ({ lon: p!.lon, lat: p!.lat, name: p!.label })),
      mode: s.mode,
      time: s.time.kind === "now" ? { kind: "now" } : s.time,
      avoid: s.avoid,
      alternatives: s.mode === "transit" ? Math.max(s.alternativesCount, 3) : s.alternativesCount,
    };
    const seq = ++routeSeq;
    set({ routeLoading: true, routeError: null, lastRequest: req });
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
      set({ routes: [], selectedRouteId: null, routeLoading: false, routeError: d.known ? d.title : (d.details ?? d.title) });
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
    set({
      waypoints: [null, null],
      routes: [],
      selectedRouteId: null,
      routeError: null,
      routeLoading: false,
      highlight: null,
      lastRequest: null,
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
      set({ waypoints: order.map((i) => pts[i]) });
      if (!changed) get().toast("info", get().t("orderAlreadyBest"));
      await get().computeRoute();
    } catch (e) {
      set({ routeLoading: false });
      get().toastError(e);
    }
  },
});
