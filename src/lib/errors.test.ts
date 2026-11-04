import { describe, expect, it, vi } from "vitest";

// i18n.ts imports the store (useT); keep the store out of these tests
vi.mock("./store", () => ({ useStore: () => "he" }));

const { errorCode, errorText, toApiError } = await import("./api");
const { describeError, errorMessage } = await import("./errors");
const { translate } = await import("./i18n");

describe("toApiError", () => {
  it("passes the backend object through", () => {
    expect(toApiError({ code: "NO_TRANSIT_DATA", message: "NO_TRANSIT_DATA: transit.bin missing" })).toEqual({ code: "NO_TRANSIT_DATA", message: "NO_TRANSIT_DATA: transit.bin missing" });
    expect(toApiError({ code: "BLOCKED_HOST", message: "HTTP 418", details: "gtfs.mot.gov.il" })).toEqual({ code: "BLOCKED_HOST", message: "HTTP 418", details: "gtfs.mot.gov.il" });
  });

  it("reads a leading code from a legacy string", () => {
    expect(toApiError("NO_ROUTING_DATA")).toEqual({ code: "NO_ROUTING_DATA", message: "NO_ROUTING_DATA" });
    expect(toApiError("ONLINE_FAILED: timeout")).toEqual({ code: "ONLINE_FAILED", message: "ONLINE_FAILED: timeout", details: "timeout" });
    expect(toApiError("NETWORK: connection reset").code).toBe("NETWORK");
  });

  it("does not take an ordinary capitalised word for a code", () => {
    expect(toApiError("OSRM request failed").code).toBe("INTERNAL");
    expect(errorCode("OSRM request failed")).toBeNull();
  });

  it("finds a known code inside a longer legacy message", () => {
    expect(toApiError("route failed: NO_ROUTE_AVOID_AREAS").code).toBe("NO_ROUTE_AVOID_AREAS");
    expect(toApiError("this is not implemented yet").code).toBe("NOT_IMPLEMENTED");
  });

  it("handles Error objects", () => {
    expect(toApiError(new Error("UPDATE_RUNNING")).code).toBe("UPDATE_RUNNING");
  });

  it("errorCode / errorText", () => {
    expect(errorCode({ code: "LOCATION_DENIED", message: "x" })).toBe("LOCATION_DENIED");
    expect(errorCode({ code: "INTERNAL", message: "x" })).toBe("INTERNAL");
    expect(errorText({ code: "SAVE_FAILED", message: "SAVE_FAILED: denied", details: "C:\\x.gpx" })).toBe("SAVE_FAILED: denied (C:\\x.gpx)");
    expect(errorText("plain")).toBe("plain");
  });
});

describe("describeError", () => {
  const t = (k: Parameters<typeof translate>[1]) => translate("he", k);

  it("known code → its Hebrew message, developer text as details", () => {
    const d = describeError({ code: "NO_TRANSIT_DATA", message: "NO_TRANSIT_DATA: transit.bin missing" }, t);
    expect(d.known).toBe(true);
    expect(d.title).toBe(t("err_NO_TRANSIT_DATA"));
    expect(d.details).toBe("NO_TRANSIT_DATA: transit.bin missing");
  });

  it("blocked host is named in the message", () => {
    const d = describeError({ code: "BLOCKED_HOST", message: "HTTP 418 from gtfs.mot.gov.il", details: "gtfs.mot.gov.il" }, "he");
    expect(d.title).toContain("gtfs.mot.gov.il");
    expect(d.title).not.toContain("{host}");
  });

  it("unclassified error → generic title + raw text", () => {
    const d = describeError("OSRM request failed", t);
    expect(d.known).toBe(false);
    expect(d.title).toBe(t("errGeneric"));
    expect(d.details).toBe("OSRM request failed");
    expect(errorMessage("OSRM request failed", t)).toBe(`${t("errGeneric")}: OSRM request failed`);
  });

  it("unknown code → generic title too", () => {
    expect(describeError({ code: "SOMETHING_NEW", message: "boom" }, "en").known).toBe(false);
  });

  it("every listed code has a message in both languages", async () => {
    const { ERROR_CODES } = await import("./api");
    for (const c of ERROR_CODES) {
      if (c === "INTERNAL") continue;
      for (const lang of ["he", "en"]) {
        const key = `err_${c}` as Parameters<typeof translate>[1];
        expect(translate(lang, key), `${lang} ${key}`).not.toBe(key);
      }
    }
  });
});
