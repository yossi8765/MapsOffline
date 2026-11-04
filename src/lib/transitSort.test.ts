import { describe, expect, it } from "vitest";
import type { Leg, Route } from "./api";
import { sortTransit } from "./transitSort";

function leg(kind: Leg["kind"], distanceM = 0): Leg {
  return { kind, durationSec: 0, distanceM, geometry: [] } as unknown as Leg;
}

function route(id: string, depart: string, arrive: string, legs: Leg[], tags: Route["tags"] = []): Route {
  const durationSec = (Date.parse(arrive) - Date.parse(depart)) / 1000;
  return { id, mode: "transit", durationSec, distanceM: 0, geometry: [], legs, tags, departIso: depart, arriveIso: arrive } as unknown as Route;
}

const d = (hm: string) => `2026-11-04T${hm}:00+02:00`;

// a: leaves 08:00 arrives 09:00 (60 min, 1 ride)
// b: leaves 08:30 arrives 09:10 (40 min, 2 rides)
// c: leaves 08:10 arrives 09:30 (80 min, 1 ride, little walking)
const a = route("a", d("08:00"), d("09:00"), [leg("walk", 800), leg("transit"), leg("walk", 400)]);
const b = route("b", d("08:30"), d("09:10"), [leg("walk", 300), leg("transit"), leg("transit"), leg("walk", 100)]);
const c = route("c", d("08:10"), d("09:30"), [leg("walk", 50), leg("transit")], ["fewest_transfers"]);

describe("sortTransit", () => {
  it("fastest = earliest arrival", () => {
    expect(sortTransit([b, c, a], "fastest", "depart").map((r) => r.id)).toEqual(["a", "b", "c"]);
  });

  it("shortest = shortest trip", () => {
    expect(sortTransit([a, b, c], "shortest", "depart").map((r) => r.id)).toEqual(["b", "a", "c"]);
  });

  it("fewest transfers, then by time", () => {
    expect(sortTransit([b, c, a], "fewestTransfers", "depart").map((r) => r.id)).toEqual(["a", "c", "b"]);
  });

  it("least walking", () => {
    expect(sortTransit([a, b, c], "leastWalking", "depart").map((r) => r.id)).toEqual(["c", "b", "a"]);
  });

  it("arrive-by prefers the latest departure", () => {
    expect(sortTransit([a, c, b], "fastest", "arrive").map((r) => r.id)).toEqual(["b", "c", "a"]);
  });

  it("tags follow the preference rules and keep the provider's other tags", () => {
    const out = sortTransit([a, b, c], "fastest", "depart");
    const tags = Object.fromEntries(out.map((r) => [r.id, r.tags]));
    expect(tags.a).toEqual(["fastest"]);
    expect(tags.b).toEqual(["shortest"]);
    expect(tags.c).toEqual(["fewest_transfers"]);
  });

  it("does not mutate the input", () => {
    const input = [b, a];
    sortTransit(input, "fastest", "depart");
    expect(input.map((r) => r.id)).toEqual(["b", "a"]);
    expect(a.tags).toEqual([]);
  });
});
