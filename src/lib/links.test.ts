import { describe, expect, it } from "vitest";
import { makeMapLink, parseMapLink } from "./links";

describe("map links", () => {
  it("round-trips our own link with a Hebrew label", () => {
    const url = makeMapLink(34.78123456, 32.0812345, 15.26, "הרצל 12, תל אביב");
    expect(url).toBe("mapsoffline://map=15.3/32.081235/34.781235&q=%D7%94%D7%A8%D7%A6%D7%9C%2012%2C%20%D7%AA%D7%9C%20%D7%90%D7%91%D7%99%D7%91");
    expect(parseMapLink(url)).toEqual({ lon: 34.781235, lat: 32.081235, zoom: 15.3, label: "הרצל 12, תל אביב" });
  });

  it("accepts a link without a label", () => {
    expect(parseMapLink("mapsoffline://map=12/31.7/35.2")).toEqual({ lon: 35.2, lat: 31.7, zoom: 12, label: undefined });
  });

  it("rejects out-of-range coordinates", () => {
    expect(parseMapLink("mapsoffline://map=12/95/35")).toBeNull();
    expect(parseMapLink("geo:31,200")).toBeNull();
  });

  it("parses RFC 5870 geo: URIs", () => {
    expect(parseMapLink("geo:31.77,35.21")).toEqual({ lon: 35.21, lat: 31.77, zoom: undefined, label: undefined });
    expect(parseMapLink("geo:31.77,35.21;u=30?z=14&q=Kotel")).toEqual({ lon: 35.21, lat: 31.77, zoom: 14, label: "Kotel" });
  });

  it("geo:0,0?q=<text> is an address search", () => {
    expect(parseMapLink("geo:0,0?q=Herzl 12 Tel Aviv")).toEqual({ lon: 0, lat: 0, zoom: undefined, query: "Herzl 12 Tel Aviv" });
    expect(parseMapLink("geo:0,0")).toBeNull();
  });

  it("q=<lat>,<lon>(<label>) wins over the path", () => {
    expect(parseMapLink("geo:0,0?q=32.1,34.8(Home)")).toEqual({ lon: 34.8, lat: 32.1, zoom: undefined, label: "Home" });
  });

  it("ignores other schemes", () => {
    expect(parseMapLink("https://example.com")).toBeNull();
  });
});
