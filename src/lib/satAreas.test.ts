import { describe, expect, it } from "vitest";
import { boxAround, fmtEstimate, newAreaId, withoutArea, zoomLabel } from "./satAreas";

const t = ((k: string) =>
  ({
    satAbout: "כ-",
    satLessThanMinute: "פחות מדקה",
    satAboutMinutes: "{n} דקות",
    satAboutHours: "{n} שעות",
    satZoom17: "חד מאוד",
  })[k] ?? k) as never;

describe("satellite areas", () => {
  it("builds a box around a point", () => {
    const b = boxAround([34.78, 32.08], 5);
    expect(b[0]).toBeLessThan(34.78);
    expect(b[2]).toBeGreaterThan(34.78);
    // 10 km tall
    expect(Math.round((b[3] - b[1]) * 110.57)).toBe(10);
    // and about 10 km wide at this latitude
    expect(Math.round((b[2] - b[0]) * 111.32 * Math.cos((32.08 * Math.PI) / 180))).toBe(10);
  });

  it("makes ids the backend accepts", () => {
    expect(newAreaId(123456789)).toMatch(/^[a-z0-9-]{1,40}$/);
  });

  it("formats estimates in minutes or hours", () => {
    expect(fmtEstimate({ tiles: 1, bytes: 2 * 1048576, minutes: 12.4, km2: 1, ok: true }, t)).toBe("כ- 2.0 MB · 12 דקות");
    expect(fmtEstimate({ tiles: 1, bytes: 3 * 1073741824, minutes: 150, km2: 1, ok: true }, t)).toBe("כ- 3.00 GB · 2.5 שעות");
    expect(fmtEstimate({ tiles: 1, bytes: 1024, minutes: 0.2, km2: 1, ok: true }, t)).toBe("כ- 1 KB · פחות מדקה");
  });

  it("names zooms and removes areas", () => {
    expect(zoomLabel(17, t)).toBe("חד מאוד");
    expect(zoomLabel(15, t)).toBe("z15");
    const a = [{ id: "a", name: "A", bbox: [0, 0, 1, 1] as [number, number, number, number], zoom: 16 }, { id: "b", name: "B", bbox: [0, 0, 1, 1] as [number, number, number, number], zoom: 17 }];
    expect(withoutArea(a, "a").map((x) => x.id)).toEqual(["b"]);
  });
});
