/**
 * Live navigation on the frontend side: starts / stops the backend session (src-tauri/src/nav),
 * mirrors its "nav-update" events into `store.navigation`, and reroutes when the backend says the
 * vehicle left the route (new route from the current position to the waypoints still ahead).
 *
 * Map camera follow (agent Map / MapView): read `useStore((s) => s.navigation?.update)` and, while
 * it is non-null, easeTo({ center: [snappedLon, snappedLat], bearing, pitch: 50, zoom: 17 }).
 */
import type { UnlistenFn } from "@tauri-apps/api/event";
import { api, errorText, type LonLat, type NavUpdate, type Route, type RouteRequest, type Step } from "./api";
import { useStore } from "./store";

export interface NavState {
  routeId: string;
  route: Route;
  /** request the route came from (waypoints ahead are taken from it when rerouting) */
  request: RouteRequest | null;
  simulate: boolean;
  update: NavUpdate | null;
  voice: boolean;
  rerouting: boolean;
  /** "LOCATION_DENIED" / "LOCATION_UNAVAILABLE" / other error text */
  error: string | null;
  arrived: boolean;
  /** Manual step browsing (the GPS is stuck or the driver looks ahead): index into
   *  `flatSteps(route)`; null = follow the GPS. */
  manualStep: number | null;
  /** simulated drive: its speed in km/h (0 = as the road allows) */
  simSpeedKmh: number;
  /** simulated drive stopped by the pause button */
  simPaused?: boolean;
}

/** One maneuver of the route in driving order (all legs), with where it happens on the map. */
export interface FlatStep {
  step: Step;
  legIndex: number;
  at: LonLat;
  /** direction of travel just after the maneuver, degrees from north */
  bearing: number;
}

function bearingDeg(a: LonLat, b: LonLat): number {
  const r = Math.PI / 180;
  const y = Math.sin((b[0] - a[0]) * r) * Math.cos(b[1] * r);
  const x = Math.cos(a[1] * r) * Math.sin(b[1] * r) - Math.sin(a[1] * r) * Math.cos(b[1] * r) * Math.cos((b[0] - a[0]) * r);
  return ((Math.atan2(y, x) / r) + 360) % 360;
}

/** Every step of every leg, in order: the same order (and index) as the backend's `stepIndex`. */
export function flatSteps(route: Route): FlatStep[] {
  const out: FlatStep[] = [];
  route.legs.forEach((leg, legIndex) => {
    for (const step of leg.steps ?? []) {
      const i = Math.min(Math.max(step.geometryStart, 0), Math.max(leg.geometry.length - 1, 0));
      const at = leg.geometry[i] ?? route.geometry[0];
      if (!at) continue;
      const next = leg.geometry[Math.min(i + 1, leg.geometry.length - 1)] ?? at;
      out.push({ step, legIndex, at, bearing: next === at ? 0 : bearingDeg(at, next) });
    }
  });
  return out;
}

/** Browse the maneuvers by hand: one step forward (+1) or back (-1) from the shown one (the
 *  GPS one when following); shows it on the map and says it when the voice is on. In a
 *  simulated drive the vehicle itself moves to just before that maneuver and drives on (no
 *  browsing, no "back to GPS"). */
export function previewStep(delta: number): void {
  const n = useStore.getState().navigation;
  if (!n) return;
  const steps = flatSteps(n.route);
  if (!steps.length) return;
  if (n.simulate) {
    const cur = Math.min(n.update?.stepIndex ?? 0, steps.length - 1);
    const to = Math.max(0, Math.min(steps.length - 1, cur + delta));
    if (n.manualStep != null) patch({ manualStep: null });
    void api.navSimJump(to).catch(() => undefined);
    return;
  }
  const from = n.manualStep ?? Math.min(n.update?.stepIndex ?? 0, steps.length - 1);
  const to = Math.max(0, Math.min(steps.length - 1, from + delta));
  patch({ manualStep: to });
  if (n.voice) void api.speak(steps[to].step.instruction, true).catch(() => undefined);
}

/** Back to following the GPS position. */
export function resumeGps(): void {
  patch({ manualStep: null });
}

let unlisten: UnlistenFn[] = [];
/** Bumped by every start / stop: a start that was overtaken while it awaited (double click)
 *  drops the listeners it registered instead of leaving them running next to the newer ones. */
let session = 0;

const patch = (p: Partial<NavState>) => {
  const n = useStore.getState().navigation;
  if (n) useStore.setState({ navigation: { ...n, ...p } });
};

/** Default simulator speed for the mode (km/h). */
export function simSpeed(mode: Route["mode"]): number {
  return mode === "car" || mode === "truck" ? 60 : mode === "bike" ? 18 : 5;
}

/** Speeds offered for a simulated drive (km/h; 0 = as the road allows). */
export const SIM_SPEEDS = [0, 30, 50, 80, 110];
/** Speeds offered for a simulated walk (km/h). */
export const SIM_WALK_SPEEDS = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

/** On foot (walking / wheelchair): the simulator walks at 1–10 km/h, never "as the road allows". */
export const isWalkMode = (m: Route["mode"]): boolean => m === "foot" || m === "wheelchair";

/** Change the speed of the running simulation (and remember it for the next one of its kind). */
export function setSimSpeed(kmh: number): void {
  const n = useStore.getState().navigation;
  const walk = !!n && isWalkMode(n.route.mode);
  const v = walk ? Math.max(1, Math.min(10, Math.round(kmh))) : kmh <= 0 ? 0 : Math.max(5, Math.min(300, Math.round(kmh)));
  patch({ simSpeedKmh: v });
  void api.navSetSimSpeed(v).catch(() => undefined);
  void useStore.getState().updateSettings(walk ? { navSimWalkKmh: v } : { navSimSpeedKmh: v });
}

// ---- the screen stays on while navigating (Screen Wake Lock; the system takes it back when the
// window is hidden, so it is asked for again when the window comes back)
let wakeLock: { release(): Promise<void> } | null = null;
async function keepAwake(on: boolean): Promise<void> {
  const wl = (navigator as unknown as { wakeLock?: { request(type: "screen"): Promise<{ release(): Promise<void> }> } }).wakeLock;
  try {
    if (on && !wakeLock && wl) wakeLock = await wl.request("screen");
    if (!on && wakeLock) {
      const l = wakeLock;
      wakeLock = null;
      await l.release();
    }
  } catch {
    wakeLock = null;
  }
}
if (typeof document !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible" && useStore.getState().navigation) {
      wakeLock = null;
      void keepAwake(true);
    }
  });
  useStore.subscribe((s, prev) => {
    if (!!s.navigation !== !!prev.navigation) void keepAwake(!!s.navigation);
  });
}

let navigatingTo = false;

/**
 * "Navigate" in a place card: from where the device is to that place, straight into navigation on
 * the fastest route (no stop in the route panel). With no route the route panel shows the reason.
 */
export async function navigateTo(to: { lon: number; lat: number; label: string; result?: import("./api").GeoResult }): Promise<void> {
  if (navigatingTo) return;
  navigatingTo = true;
  try {
    const s = useStore.getState();
    const recent = s.myLocation && Date.now() - s.myLocation.at < 60_000 ? s.myLocation : null;
    const loc = recent ?? (await s.locate(false));
    if (!loc) return;
    s.toast("info", s.t("computing"));
    useStore.setState({
      waypoints: [{ lon: loc.lon, lat: loc.lat, label: s.t("myLocation") }, to],
      // public transport has no turn-by-turn navigation: drive instead
      ...(s.mode === "transit" ? { mode: "car" as const } : {}),
    });
    await useStore.getState().computeRoute();
    const st = useStore.getState();
    if (st.routes.length === 0) {
      st.setPanel("route");
      return;
    }
    const fastest = st.routes.reduce((a, b) => (b.durationSec < a.durationSec ? b : a));
    await startNavigation(fastest.id);
  } finally {
    navigatingTo = false;
  }
}

/** Pause / resume the running simulated drive. */
export function setSimPaused(paused: boolean): void {
  patch({ simPaused: paused });
  void api.navSetSimPaused(paused).catch(() => undefined);
}

/** `speedKmh` (simulation only): km/h, 0 = as the road allows; default: the remembered speed,
 *  else the mode's usual one. */
export async function startNavigation(routeId: string, simulate = false, speedKmh?: number): Promise<void> {
  const s = useStore.getState();
  const route = s.routes.find((r) => r.id === routeId) ?? (s.navigation?.routeId === routeId ? s.navigation.route : undefined);
  if (!route) return;
  if (route.mode === "transit") {
    s.toast("info", s.t("navTransitUnsupported"));
    return;
  }
  await stopNavigation();
  const mine = ++session;
  const voice = s.settings.navVoice ?? true;
  const simSpeedKmh = speedKmh ?? (isWalkMode(route.mode) ? (s.settings.navSimWalkKmh ?? simSpeed(route.mode)) : (s.settings.navSimSpeedKmh ?? simSpeed(route.mode)));
  useStore.setState({
    navigation: { routeId, route, request: s.lastRequest, simulate, update: null, voice, rerouting: false, error: null, arrived: false, manualStep: null, simSpeedKmh },
  });
  const subs = await Promise.all([
    api.onNavUpdate(onUpdate),
    api.onNavError((msg) => patch({ error: msg })),
    api.onNavEnded(() => {
      const n = useStore.getState().navigation;
      // arrival keeps the card until the user closes it; a lost session closes it
      if (n && !n.arrived && !n.error) void stopNavigation();
    }),
  ]);
  if (mine !== session) {
    subs.forEach((u) => u());
    return;
  }
  unlisten = subs;
  try {
    if (simulate) await api.navSimulate(route, simSpeedKmh);
    else await api.navStart(route);
  } catch (e) {
    patch({ error: errorText(e) });
  }
}

export async function stopNavigation(): Promise<void> {
  session++;
  unlisten.forEach((u) => u());
  unlisten = [];
  try {
    await api.navStop();
  } catch {
    /* not running */
  }
  if (useStore.getState().navigation) useStore.setState({ navigation: null });
}

export async function setNavVoice(on: boolean): Promise<void> {
  patch({ voice: on });
  try {
    await api.navSetVoice(on);
  } catch {
    /* ignore */
  }
  void useStore.getState().updateSettings({ navVoice: on });
}

function onUpdate(u: NavUpdate) {
  const n = useStore.getState().navigation;
  if (!n) return;
  patch({ update: u, arrived: u.arrived, error: null });
  if (u.rerouteNeeded && !n.rerouting) void reroute(u);
}

/** Request from the current position to the waypoints still ahead. */
function requestFromHere(n: NavState, u: NavUpdate, alternatives: number): RouteRequest | null {
  if (!n.request) return null;
  const ahead = n.request.waypoints.slice(Math.min(u.legIndex + 1, n.request.waypoints.length - 1));
  return {
    ...n.request,
    waypoints: [{ lon: u.lon, lat: u.lat, name: useStore.getState().t("myLocation") }, ...ahead],
    time: { kind: "now" },
    alternatives,
  };
}

/** Change route while navigating: up to 3 routes from the current position to what is still ahead. */
export async function routesFromHere(): Promise<{ routes: Route[]; request: RouteRequest } | null> {
  const n = useStore.getState().navigation;
  const u = n?.update;
  if (!n || !u) return null;
  const req = requestFromHere(n, u, 3);
  if (!req) return null;
  const resp = await api.route(req);
  return { routes: resp.routes, request: req };
}

/**
 * While navigating, a place found in the search: `stop` = drive there first and then on to what
 * was ahead; `destination` = drive there instead. Takes the fastest route from the current position.
 */
export async function navigateVia(p: { lon: number; lat: number; label: string }, kind: "stop" | "destination"): Promise<void> {
  const s = useStore.getState();
  const n = s.navigation;
  const u = n?.update;
  if (!n?.request) throw new Error(s.t("navChangeRouteNone"));
  const here = u ? { lon: u.lon, lat: u.lat } : (s.myLocation ?? null);
  if (!here) throw new Error(s.t("navWaiting"));
  const ahead = n.request.waypoints.slice(Math.min((u?.legIndex ?? 0) + 1, n.request.waypoints.length - 1));
  const place = { lon: p.lon, lat: p.lat, name: p.label };
  const req: RouteRequest = {
    ...n.request,
    waypoints: [{ lon: here.lon, lat: here.lat, name: s.t("myLocation") }, ...(kind === "stop" ? [place, ...ahead] : [place])],
    time: { kind: "now" },
    alternatives: 1,
  };
  const resp = await api.route(req);
  if (resp.routes.length === 0) throw new Error(s.t("navChangeRouteNone"));
  const fastest = resp.routes.reduce((a, b) => (b.durationSec < a.durationSec ? b : a));
  await switchRoute(fastest, req);
  // the route panel shows the same trip afterwards
  useStore.setState({ waypoints: req.waypoints.map((w) => ({ lon: w.lon, lat: w.lat, label: w.name ?? "" })) });
}

/** Drive on `r` from now on (picked in "change route"). */
export async function switchRoute(r: Route, request: RouteRequest): Promise<void> {
  if (!useStore.getState().navigation) return;
  await api.navSetRoute(r);
  useStore.setState({ routes: [r], selectedRouteId: r.id, routeTick: useStore.getState().routeTick + 1 });
  patch({ route: r, routeId: r.id, request, rerouting: false, manualStep: null });
}

/** New route from the current position to the waypoints after the current leg. */
async function reroute(u: NavUpdate) {
  const n = useStore.getState().navigation;
  if (!n?.request) return;
  patch({ rerouting: true });
  const ahead = n.request.waypoints.slice(Math.min(u.legIndex + 1, n.request.waypoints.length - 1));
  const req: RouteRequest = {
    ...n.request,
    waypoints: [{ lon: u.lon, lat: u.lat, name: useStore.getState().t("myLocation") }, ...ahead],
    time: { kind: "now" },
    alternatives: 1,
  };
  try {
    const resp = await api.route(req);
    const r = resp.routes[0];
    const cur = useStore.getState().navigation;
    if (!r || !cur) return;
    await api.navSetRoute(r);
    useStore.setState({ routes: [r], selectedRouteId: r.id, routeTick: useStore.getState().routeTick + 1 });
    patch({ route: r, routeId: r.id, request: req, rerouting: false, manualStep: null });
  } catch (e) {
    console.warn("reroute failed", e);
    patch({ rerouting: false });
  }
}
