// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

const pos = { lon: 0, lat: 0 };
const spoken: string[] = [];
vi.mock("./api", async (orig) => {
  const m = await orig<typeof import("./api")>();
  return {
    ...m,
    api: {
      ...m.api,
      locate: vi.fn(async () => ({ lon: pos.lon, lat: pos.lat, accuracyM: 10 })),
      speak: vi.fn(async (text: string) => void spoken.push(text)),
    },
  };
});

import { startTransitAlert, stopTransitAlert, ridesOf } from "./transitAlert";
import { useStore } from "./store";
import type { Route } from "./api";

const stop = (name: string, lon: number, code?: string) => ({ name, code, lon, lat: 32, timeIso: "2026-11-09T10:00:00+02:00" });
const leg = (line: string, stops: ReturnType<typeof stop>[], arriveIso: string) => ({
  kind: "transit" as const,
  durationSec: 600,
  distanceM: 5000,
  geometry: [],
  transit: { vehicle: "bus" as const, routeShortName: line, routeLongName: "", agency: "", headsign: "", fromStop: stops[0].name, toStop: stops[stops.length - 1].name, departIso: arriveIso, arriveIso, numStops: stops.length - 1, stops },
});
// about 1.1 km per 0.01° of longitude at 32°N... 0.001° ≈ 94 m
const route = {
  id: "transit-0",
  mode: "transit",
  legs: [
    leg("18", [stop("א", 34.0), stop("ב", 34.01), stop("ג", 34.02, "123")], "2099-01-01T10:00:00+02:00"),
    { kind: "walk", durationSec: 60, distanceM: 50, geometry: [] },
    leg("5", [stop("ד", 34.021), stop("ה", 34.04)], "2099-01-01T10:30:00+02:00"),
  ],
} as unknown as Route;

async function at(lon: number) {
  pos.lon = lon;
  pos.lat = 32;
  await vi.advanceTimersByTimeAsync(8_000);
}

afterEach(() => {
  stopTransitAlert();
  vi.useRealTimers();
  spoken.length = 0;
});

describe("transit stop alert", () => {
  it("lists the rides with the stop before the last", () => {
    const r = ridesOf(route);
    expect(r.map((x) => [x.line, x.prev?.name, x.alight.name])).toEqual([
      ["18", "ב", "ג"],
      ["5", "ד", "ה"],
    ]);
  });

  it("alerts at the stop before, moves to the next ride at the stop, ends after the last", async () => {
    vi.useFakeTimers();
    pos.lon = 34.0;
    startTransitAlert(route);
    await vi.advanceTimersByTimeAsync(0);
    expect(useStore.getState().transitAlert?.stop).toBe("ג (123)");
    expect(spoken).toEqual([]);
    await at(34.005); // between the first stops: quiet
    expect(spoken).toEqual([]);
    await at(34.0101); // at the stop before ג
    expect(spoken.length).toBe(1);
    expect(spoken[0]).toContain("ג");
    expect(useStore.getState().transitAlert?.warned).toBe(true);
    await at(34.02); // at ג: the second ride
    expect(useStore.getState().transitAlert?.stop).toBe("ה");
    expect(useStore.getState().transitAlert?.ride).toBe(2);
    await at(34.034); // ~560 m before ה, no stop in between: alert by distance
    expect(spoken.length).toBe(2);
    await at(34.04);
    expect(useStore.getState().transitAlert).toBeNull();
  });
});
