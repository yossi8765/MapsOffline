import { describe, expect, it, vi } from "vitest";

// format.ts imports the Tauri dialog plugin for askConfirm; not needed here
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn() }));
// i18n.ts imports the store (useT); keep the store out of these tests
vi.mock("./store", () => ({ useStore: () => "he" }));

const { fmtDuration, fmtDistance, fmtBytes, fmtCoords, localInputToIso, toLocalInput } = await import("./format");
const { translate } = await import("./i18n");

const he = (k: Parameters<typeof translate>[1]) => translate("he", k);
const en = (k: Parameters<typeof translate>[1]) => translate("en", k);

describe("fmtDuration", () => {
  it("under a minute", () => {
    expect(fmtDuration(20, he)).toBe(he("lessThanMinute"));
  });
  it("minutes", () => {
    expect(fmtDuration(25 * 60, he)).toBe(`25 ${he("minutes")}`);
  });
  it("Hebrew hours: שעה, שעתיים, 3 שעות, joined with ו-", () => {
    expect(fmtDuration(3600, he)).toBe(he("hourOne"));
    expect(fmtDuration(2 * 3600, he)).toBe(he("hourTwo"));
    expect(fmtDuration(3600 + 20 * 60, he)).toBe(`${he("hourOne")} ${he("durationAnd")}20 ${he("minutes")}`);
    expect(fmtDuration(3 * 3600, he)).toBe(`3 ${he("hoursPlural")}`);
  });
  it("rounds to the nearest minute", () => {
    expect(fmtDuration(89, en)).toBe(`1 ${en("minutes")}`);
  });
});

describe("fmtDistance", () => {
  it("metres and kilometres", () => {
    expect(fmtDistance(450.4, he)).toBe(`450 ${he("m")}`);
    expect(fmtDistance(2340, he)).toBe(`2.3 ${he("km")}`);
    expect(fmtDistance(63_400, he)).toBe(`63 ${he("km")}`);
  });
  it("imperial", () => {
    expect(fmtDistance(100, en, "imperial")).toBe(`328 ${en("ft")}`);
    expect(fmtDistance(5000, en, "imperial")).toBe(`3.1 ${en("mi")}`);
  });
});

describe("misc formatting", () => {
  it("bytes", () => {
    expect(fmtBytes(512 * 1024)).toBe("512 KB");
    expect(fmtBytes(177.5 * 1024 * 1024)).toBe("177.5 MB");
    expect(fmtBytes(2.5 * 1024 ** 3)).toBe("2.50 GB");
  });
  it("coordinates are lat, lon with 5 decimals", () => {
    expect(fmtCoords(34.76995, 32.06231)).toBe("32.06231, 34.76995");
  });
  it("local datetime input round-trip", () => {
    const d = new Date(2026, 10, 4, 8, 15);
    const v = toLocalInput(d);
    expect(v).toBe("2026-11-04T08:15");
    expect(Date.parse(localInputToIso(v))).toBe(d.getTime());
  });
});
