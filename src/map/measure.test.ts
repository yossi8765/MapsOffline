import { describe, expect, it } from "vitest";
import { areaM2, climb, densify, distanceM, fmtArea, pathLengthM } from "./measure";
import { containsBbox, padBbox, bboxOf } from "./bbox";
import { gridStep, itmGrid, zoomForScale } from "./printGrid";
import { validateCustomLayer } from "./customLayers";
import { dataTag } from "../lib/dataTag";

describe("geodesic measuring", () => {
  it("one degree of latitude ≈ 111.2 km", () => {
    expect(distanceM([34.8, 32], [34.8, 33]) / 1000).toBeCloseTo(111.19, 1);
  });
  it("Tel Aviv – Jerusalem ≈ 54 km as the crow flies", () => {
    const d = distanceM([34.7818, 32.0853], [35.2137, 31.7683]);
    expect(d).toBeGreaterThan(53_000);
    expect(d).toBeLessThan(55_000);
  });
  it("path length adds the segments; closed adds the way back", () => {
    const pts: [number, number][] = [
      [35, 31],
      [35, 31.01],
      [35.01, 31.01],
    ];
    const open = pathLengthM(pts);
    expect(open).toBeCloseTo(distanceM(pts[0], pts[1]) + distanceM(pts[1], pts[2]), 6);
    expect(pathLengthM(pts, true)).toBeCloseTo(open + distanceM(pts[2], pts[0]), 6);
  });
  it("area of a 0.01° square near Israel ≈ 1.11 km × 0.95 km", () => {
    const sq: [number, number][] = [
      [35, 31],
      [35.01, 31],
      [35.01, 31.01],
      [35, 31.01],
    ];
    const expected = 1112 * (1112 * Math.cos((31.005 * Math.PI) / 180));
    expect(Math.abs(areaM2(sq) - expected) / expected).toBeLessThan(0.01);
    // closed ring or reversed order: same area
    expect(areaM2([...sq, sq[0]])).toBeCloseTo(areaM2(sq), 3);
    expect(areaM2([...sq].reverse())).toBeCloseTo(areaM2(sq), 3);
    expect(areaM2(sq.slice(0, 2))).toBe(0);
  });
  it("climb ignores gaps and 1 m jitter", () => {
    expect(climb([100, 100.5, 110, null, 105, 120])).toEqual({ up: 25, down: 5 });
  });
  it("densify keeps the ends and caps the count", () => {
    const pts: [number, number][] = [
      [35, 31],
      [35, 32],
    ];
    const d = densify(pts, 1000, 50);
    expect(d[0]).toEqual(pts[0]);
    expect(d[d.length - 1]).toEqual(pts[1]);
    expect(d.length).toBeLessThanOrEqual(52);
  });
  it("area text: m², dunam, km²", () => {
    expect(fmtArea(500, true)).toBe("500 מ״ר");
    expect(fmtArea(25_000, true)).toBe("25.0 דונם");
    expect(fmtArea(3_500_000, false)).toBe("3.50 km²");
  });
});

describe("viewport boxes", () => {
  it("pad grows every side by the share of the size", () => {
    expect(padBbox([0, 0, 2, 4], 0.25)).toEqual([-0.5, -1, 2.5, 5]);
  });
  it("containment: inside, touching edges, poking out", () => {
    const outer: [number, number, number, number] = [0, 0, 10, 10];
    expect(containsBbox(outer, [1, 1, 9, 9])).toBe(true);
    expect(containsBbox(outer, [0, 0, 10, 10])).toBe(true);
    expect(containsBbox(outer, [-0.1, 1, 9, 9])).toBe(false);
    expect(containsBbox(outer, [1, 1, 9, 10.1])).toBe(false);
  });
  it("a small pan stays inside the padded box, a long one leaves it", () => {
    const view: [number, number, number, number] = [34.7, 32, 34.9, 32.1];
    const fetched = padBbox(view, 0.25);
    expect(containsBbox(fetched, [34.72, 32.01, 34.92, 32.11])).toBe(true);
    expect(containsBbox(fetched, [34.8, 32, 35.0, 32.1])).toBe(false);
  });
  it("bboxOf", () => {
    expect(bboxOf([])).toBeNull();
    expect(
      bboxOf([
        [1, 5],
        [-2, 3],
        [4, -1],
      ]),
    ).toEqual([-2, -1, 4, 5]);
  });
});

describe("printout scale and grid", () => {
  it("1:25,000 at 32° N is about zoom 14.3, 1:50,000 one level less", () => {
    const z25 = zoomForScale(25000, 32);
    expect(z25).toBeGreaterThan(14);
    expect(z25).toBeLessThan(14.6);
    expect(zoomForScale(50000, 32)).toBeCloseTo(z25 - 1, 6);
  });
  it("grid step: 1 km at 1:25,000, 2 km at 1:50,000, ~8 lines when fitted", () => {
    expect(gridStep(25000, 0)).toBe(1000);
    expect(gridStep(50000, 0)).toBe(2000);
    expect(gridStep("fit", 40_000)).toBe(5000);
    expect(gridStep("fit", 3_000)).toBe(500);
  });
  it("ITM grid lines are labelled in km and cover the box", () => {
    const fc = itmGrid([34.76, 32.06, 34.8, 32.09], 1000);
    const labels = fc.features.map((f) => f.properties?.label);
    expect(labels).toContain("179");
    expect(labels).toContain("665");
    expect(fc.features.length).toBeGreaterThan(6);
  });
});

describe("custom tile layers", () => {
  it("validates name, URL and the XYZ template", () => {
    expect(validateCustomLayer({ kind: "xyz", name: "", url: "https://a/{z}/{x}/{y}.png" })).toBe("name");
    expect(validateCustomLayer({ kind: "xyz", name: "a", url: "ftp://a/{z}/{x}/{y}" })).toBe("url");
    expect(validateCustomLayer({ kind: "xyz", name: "a", url: "https://a/tiles.png" })).toBe("template");
    expect(validateCustomLayer({ kind: "xyz", name: "a", url: "https://a/{z}/{x}/{-y}.png" })).toBeNull();
    expect(validateCustomLayer({ kind: "wms", name: "a", url: "https://a/wms" })).toBeNull();
    expect(validateCustomLayer({ kind: "pmtiles", name: "a", url: "" })).toBe("url");
    expect(validateCustomLayer({ kind: "mbtiles", name: "a", url: "C:\\maps\\a.mbtiles" })).toBeNull();
  });
});

describe("dataTag", () => {
  it("same inputs → same tag; any change → another", () => {
    expect(dataTag("2026-11-04", "v1", 10)).toBe(dataTag("2026-11-04", "v1", 10));
    expect(dataTag("2026-11-04", "v1", 10)).not.toBe(dataTag("2026-11-05", "v1", 10));
    expect(dataTag(undefined, null)).toBe(dataTag("", ""));
    expect(dataTag("a")).toMatch(/^[0-9a-z]+$/);
  });
});
