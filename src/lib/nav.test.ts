import { describe, expect, it } from "vitest";
import { flatSteps } from "./nav";
import type { Route } from "./api";

const step = (instruction: string, geometryStart: number) => ({
  instruction,
  maneuver: "straight" as const,
  distanceM: 100,
  durationSec: 10,
  geometryStart,
});

describe("flatSteps", () => {
  it("lists every leg's steps in order with their map point and heading", () => {
    const route = {
      id: "r",
      mode: "car",
      durationSec: 0,
      distanceM: 0,
      geometry: [[35, 31]],
      tags: [],
      transfers: 0,
      walkM: 0,
      legs: [
        { kind: "drive", durationSec: 0, distanceM: 0, geometry: [[35, 31], [35, 31.01]], steps: [step("a", 0), step("b", 1)] },
        { kind: "drive", durationSec: 0, distanceM: 0, geometry: [[35.01, 31.01], [35.02, 31.01]], steps: [step("c", 0)] },
      ],
    } as unknown as Route;
    const s = flatSteps(route);
    expect(s.map((x) => x.step.instruction)).toEqual(["a", "b", "c"]);
    expect(s.map((x) => x.legIndex)).toEqual([0, 0, 1]);
    expect(s[0].at).toEqual([35, 31]);
    expect(Math.round(s[0].bearing)).toBe(0); // north
    expect(Math.round(s[2].bearing)).toBe(90); // east
  });
});
