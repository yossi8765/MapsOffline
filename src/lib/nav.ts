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
 *  GPS one when following); shows it on the map and says it when the voice is on. */
export function previewStep(delta: number): void {
  const n = useStore.getState().navigation;
  if (!n) return;
  const steps = flatSteps(n.route);
  if (!steps.length) return;
  const from = n.manualStep ?? Math.min(n.update?.stepIndex ?? 0, steps.length - 1);
  const to = Math.max(0, Math.min(steps.length - 1, from + delta));
  patch({ manualStep: to });
  if (n.voice) void api.speak(steps[to].step.instruction).catch(() => undefined);
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
function simSpeed(mode: Route["mode"]): number {
  return mode === "car" ? 60 : mode === "bike" ? 18 : 5;
}

export async function startNavigation(routeId: string, simulate = false): Promise<void> {
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
  useStore.setState({
    navigation: { routeId, route, request: s.lastRequest, simulate, update: null, voice, rerouting: false, error: null, arrived: false, manualStep: null },
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
    if (simulate) await api.navSimulate(route, simSpeed(route.mode));
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
