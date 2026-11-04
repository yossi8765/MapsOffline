/**
 * Live navigation on the frontend side: starts / stops the backend session (src-tauri/src/nav),
 * mirrors its "nav-update" events into `store.navigation`, and reroutes when the backend says the
 * vehicle left the route (new route from the current position to the waypoints still ahead).
 *
 * Map camera follow (agent Map / MapView): read `useStore((s) => s.navigation?.update)` and, while
 * it is non-null, easeTo({ center: [snappedLon, snappedLat], bearing, pitch: 50, zoom: 17 }).
 */
import type { UnlistenFn } from "@tauri-apps/api/event";
import { api, type NavUpdate, type Route, type RouteRequest } from "./api";
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
}

let unlisten: UnlistenFn[] = [];

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
  const voice = s.settings.navVoice ?? true;
  useStore.setState({
    navigation: { routeId, route, request: s.lastRequest, simulate, update: null, voice, rerouting: false, error: null, arrived: false },
  });
  unlisten = await Promise.all([
    api.onNavUpdate(onUpdate),
    api.onNavError((msg) => patch({ error: msg })),
    api.onNavEnded(() => {
      const n = useStore.getState().navigation;
      // arrival keeps the card until the user closes it; a lost session closes it
      if (n && !n.arrived && !n.error) void stopNavigation();
    }),
  ]);
  try {
    if (simulate) await api.navSimulate(route, simSpeed(route.mode));
    else await api.navStart(route);
  } catch (e) {
    patch({ error: String(e) });
  }
}

export async function stopNavigation(): Promise<void> {
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
    patch({ route: r, routeId: r.id, request: req, rerouting: false });
  } catch (e) {
    console.warn("reroute failed", e);
    patch({ rerouting: false });
  }
}
