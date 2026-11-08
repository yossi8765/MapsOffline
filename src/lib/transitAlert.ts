/**
 * "Wake me before my stop": while riding a public-transport route, follow the position and give
 * a spoken + vibrating alert when the stop to get off at is next (at the stop before it, or about
 * 700 m before it), then the same for the next ride. Without a position fix (no GPS indoors in a
 * bus, location off) it falls back to the timetable: the alert comes two minutes before the
 * scheduled arrival. The screen stays on while it runs (Android: MoAndroid via android.ts).
 */
import { api, type LonLat, type Route, type TransitStopTime } from "./api";
import { useStore } from "./store";

/** Alert when this close to the stop before the one to get off at… */
const PREV_STOP_M = 250;
/** …or this close to the stop itself (long stretches between stops). */
const ALIGHT_M = 700;
/** The ride counts as done this close to its last stop (or after its scheduled arrival + DONE_LATE_MS). */
const DONE_M = 120;
const DONE_LATE_MS = 5 * 60_000;
/** Timetable fallback: the alert this long before the scheduled arrival. */
const SCHEDULE_LEAD_MS = 2 * 60_000;
const POLL_MS = 8_000;

interface Ride {
  line: string;
  alight: TransitStopTime;
  prev?: TransitStopTime;
  arriveAt: number;
}

let timer: number | null = null;
let rides: Ride[] = [];
let idx = 0;
let warned = false;
let failures = 0;
let wake: { release(): Promise<void> } | null = null;

const R = 6371000;
function distM(a: LonLat, b: LonLat): number {
  const rad = Math.PI / 180;
  const dLat = (b[1] - a[1]) * rad;
  const dLon = (b[0] - a[0]) * rad;
  const h = Math.sin(dLat / 2) ** 2 + Math.cos(a[1] * rad) * Math.cos(b[1] * rad) * Math.sin(dLon / 2) ** 2;
  return 2 * R * Math.asin(Math.min(1, Math.sqrt(h)));
}

const stopLabel = (s: TransitStopTime) => (s.code ? `${s.name} (${s.code})` : s.name);

function publish(dist: number | null): void {
  const r = rides[idx];
  const s = useStore.getState();
  if (!r || !s.transitAlert) return;
  useStore.setState({ transitAlert: { ...s.transitAlert, stop: stopLabel(r.alight), line: r.line, distM: dist, warned, ride: idx + 1, rides: rides.length } });
}

function alert(r: Ride): void {
  const s = useStore.getState();
  const text = s.t("transitAlertSpeech").replace("{stop}", r.alight.name);
  warned = true;
  s.toast("info", text, { duration: 15_000 });
  try {
    navigator.vibrate?.([500, 250, 500, 250, 800]);
  } catch {
    /* no vibration motor */
  }
  void api.speak(text, s.settings.navVoiceNiqqud ?? true).catch(() => undefined);
}

function nextRide(): void {
  idx++;
  warned = false;
  if (idx >= rides.length) {
    const s = useStore.getState();
    s.toast("success", s.t("transitAlertDone"));
    stopTransitAlert();
  } else publish(null);
}

async function tick(): Promise<void> {
  const r = rides[idx];
  if (!r) return;
  let here: LonLat | null = null;
  try {
    const loc = await api.locate();
    here = [loc.lon, loc.lat];
    failures = 0;
    useStore.setState({ myLocation: { ...loc, at: Date.now() } });
  } catch {
    failures++;
  }
  if (!useStore.getState().transitAlert) return; // stopped meanwhile
  const now = Date.now();
  const dAlight = here ? distM(here, [r.alight.lon, r.alight.lat]) : null;
  const dPrev = here && r.prev ? distM(here, [r.prev.lon, r.prev.lat]) : null;
  if (!warned) {
    const near = (dPrev != null && dPrev < PREV_STOP_M) || (dAlight != null && dAlight < ALIGHT_M);
    const bySchedule = failures >= 2 && now >= r.arriveAt - SCHEDULE_LEAD_MS;
    if (near || bySchedule) alert(r);
  }
  if ((warned && dAlight != null && dAlight < DONE_M) || now > r.arriveAt + DONE_LATE_MS) {
    if (!warned) alert(r);
    nextRide();
    return;
  }
  publish(dAlight);
}

/** Rides of a public-transport route: the stop to get off at, the one before it, the arrival. */
export function ridesOf(route: Route): Ride[] {
  return route.legs.flatMap((l) => {
    const t = l.transit;
    if (!t || !t.stops.length) return [];
    const alight = t.stops[t.stops.length - 1];
    return [{ line: t.vehicle === "rail" ? "" : t.routeShortName, alight, prev: t.stops.length > 1 ? t.stops[t.stops.length - 2] : undefined, arriveAt: Date.parse(t.arriveIso) || Date.now() + l.durationSec * 1000 }];
  });
}

export function startTransitAlert(route: Route): void {
  stopTransitAlert();
  rides = ridesOf(route);
  if (!rides.length) return;
  idx = 0;
  warned = false;
  failures = 0;
  useStore.setState({ transitAlert: { routeId: route.id, stop: stopLabel(rides[0].alight), line: rides[0].line, distM: null, warned: false, ride: 1, rides: rides.length } });
  const nav = navigator as unknown as { wakeLock?: { request(type: "screen"): Promise<{ release(): Promise<void> }> } };
  void nav.wakeLock
    ?.request("screen")
    .then((l) => (wake = l))
    .catch(() => undefined);
  void tick();
  timer = window.setInterval(() => void tick(), POLL_MS);
}

export function stopTransitAlert(): void {
  if (timer != null) window.clearInterval(timer);
  timer = null;
  rides = [];
  const l = wake;
  wake = null;
  void l?.release().catch(() => undefined);
  if (useStore.getState().transitAlert) useStore.setState({ transitAlert: null });
}
