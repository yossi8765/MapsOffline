// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { getLastView, getNearbyPref, getPrefJson, getRememberView, setLastView, setNearbyPref, setRememberView } from "./prefs";
import { DEFAULT_LAYERS, validateLayers } from "./store/map";

beforeEach(() => localStorage.clear());

describe("prefs", () => {
  it("last view round-trip, rejects garbage", () => {
    expect(getLastView()).toBeNull();
    setLastView({ lon: 34.8, lat: 32.1, zoom: 12 });
    expect(getLastView()).toEqual({ lon: 34.8, lat: 32.1, zoom: 12 });
    localStorage.setItem("mapsoffline.lastView", "{not json");
    expect(getLastView()).toBeNull();
    localStorage.setItem("mapsoffline.lastView", JSON.stringify({ lon: "x", lat: 1, zoom: 2 }));
    expect(getLastView()).toBeNull();
  });

  it("remember view defaults to on and keeps the old 1/0 format", () => {
    expect(getRememberView()).toBe(true);
    setRememberView(false);
    expect(localStorage.getItem("mapsoffline.rememberView")).toBe("0");
    expect(getRememberView()).toBe(false);
  });

  it("nearby keeps only valid fields", () => {
    setNearbyPref({ radius: 800, cat: "synagogue" });
    expect(getNearbyPref()).toEqual({ radius: 800, cat: "synagogue" });
    localStorage.setItem("mapsoffline.nearby", JSON.stringify({ radius: "far", cat: 3 }));
    expect(getNearbyPref()).toEqual({});
  });

  it("saved layers are checked key by key", () => {
    localStorage.setItem("mapsoffline.layers", JSON.stringify({ satellite: true, poiCats: "nope", flavor: "dark", bogus: 1 }));
    const l = getPrefJson("mapsoffline.layers", validateLayers, DEFAULT_LAYERS);
    expect(l.satellite).toBe(true);
    expect(l.flavor).toBe("dark");
    expect(l.poiCats).toEqual([]);
    expect("bogus" in l).toBe(false);
    localStorage.setItem("mapsoffline.layers", "null");
    expect(getPrefJson("mapsoffline.layers", validateLayers, DEFAULT_LAYERS)).toEqual(DEFAULT_LAYERS);
  });
});
