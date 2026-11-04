import { describe, expect, it } from "vitest";
import { isNightInIsrael, resolveTheme } from "./sun";

// Jerusalem area, 4 Nov 2026: sunrise ≈ 06:00, sunset ≈ 16:50 (UTC+2) → 04:00 / 14:50 UTC.
// 21 Jun 2026: sunrise ≈ 05:35, sunset ≈ 19:50 (UTC+3) → 02:35 / 16:50 UTC.
describe("isNightInIsrael", () => {
  it("noon is day, midnight is night", () => {
    expect(isNightInIsrael(new Date("2026-11-04T10:00:00Z"))).toBe(false);
    expect(isNightInIsrael(new Date("2026-11-04T22:00:00Z"))).toBe(true);
  });

  it("switches around sunset in November", () => {
    expect(isNightInIsrael(new Date("2026-11-04T14:30:00Z"))).toBe(false);
    expect(isNightInIsrael(new Date("2026-11-04T15:15:00Z"))).toBe(true);
  });

  it("long summer evening is still day", () => {
    expect(isNightInIsrael(new Date("2026-06-21T16:30:00Z"))).toBe(false);
    expect(isNightInIsrael(new Date("2026-06-21T17:15:00Z"))).toBe(true);
  });

  it("before sunrise is night", () => {
    expect(isNightInIsrael(new Date("2026-11-04T03:30:00Z"))).toBe(true);
    expect(isNightInIsrael(new Date("2026-11-04T04:30:00Z"))).toBe(false);
  });
});

describe("resolveTheme", () => {
  it("explicit themes pass through", () => {
    expect(resolveTheme("light")).toBe("light");
    expect(resolveTheme("dark")).toBe("dark");
  });

  it("auto without a window is light", () => {
    expect(resolveTheme("auto")).toBe("light");
  });
});
