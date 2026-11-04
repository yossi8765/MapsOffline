import { describe, expect, it } from "vitest";
import { formatCoord, fromItm, toItm, toUtm } from "./coords";

describe("ITM", () => {
  // reference values from src-tauri/src/data/orthophoto.rs tests
  it("matches the Rust reference points", () => {
    const [e, n] = toItm(34.774082, 32.077885);
    expect(Math.abs(e - 178824.11042624438)).toBeLessThan(0.01);
    expect(Math.abs(n - 665036.0136441475)).toBeLessThan(0.01);
    const [e2, n2] = toItm(35.2045169444444, 31.7343936111111);
    expect(Math.abs(e2 - 219463.54798758472)).toBeLessThan(0.01);
    expect(Math.abs(n2 - 626866.4265986377)).toBeLessThan(0.01);
  });
  it("round-trips", () => {
    for (const [lon, lat] of [
      [34.78, 32.08],
      [35.57, 33.2],
      [34.95, 29.55],
    ]) {
      const [e, n] = toItm(lon, lat);
      const [lo, la] = fromItm(e, n);
      expect(Math.abs(lo - lon)).toBeLessThan(1e-7);
      expect(Math.abs(la - lat)).toBeLessThan(1e-7);
    }
  });
});

describe("UTM", () => {
  it("Tel Aviv is in zone 36S", () => {
    const u = toUtm(34.78, 32.08);
    expect(u.zone).toBe(36);
    expect(u.band).toBe("S");
    expect(Math.round(u.easting / 1000)).toBe(668);
    expect(Math.round(u.northing / 1000)).toBe(3551);
  });
});

describe("formatCoord", () => {
  it("formats every kind", () => {
    expect(formatCoord(34.78, 32.08, "dd")).toBe("32.08000, 34.78000");
    expect(formatCoord(34.78, 32.08, "dms")).toBe("32°04′48.0″N 34°46′48.0″E");
    expect(formatCoord(34.774082, 32.077885, "itm")).toBe("178824 665036");
    expect(formatCoord(34.78, 32.08, "utm")).toMatch(/^36S \d+ \d+$/);
  });
});
