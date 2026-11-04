// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { buildStyle, firstSymbolLayer, type StyleOptions } from "./mapStyle";

const base: StyleOptions = {
  tilesUrl: "http://asset.localhost/israel.pmtiles",
  satelliteUrl: "http://asset.localhost/satellite.pmtiles",
  satellite: false,
  terrainUrl: "http://asset.localhost/dem.pmtiles",
  hillshade: false,
  terrain3d: false,
  flavor: "light",
  lang: "he",
  poi: true,
  buildings3d: false,
};

const fonts = (style: ReturnType<typeof buildStyle>) => JSON.stringify(style.layers.map((l) => (l.type === "symbol" ? l.layout?.["text-font"] : null)));

describe("buildStyle", () => {
  it("starts with the background", () => {
    expect(buildStyle(base).layers[0].id).toBe("background");
  });

  it("satellite sits right after the background", () => {
    const s = buildStyle({ ...base, satellite: true });
    expect(s.layers[1].type).toBe("raster");
    expect((s.layers[1] as { source: string }).source).toBe("satellite");
  });

  it("no layer uses the Italic face (it has no Hebrew)", () => {
    for (const flavor of ["light", "dark"] as const) {
      expect(fonts(buildStyle({ ...base, flavor, worldUrl: "http://asset.localhost/world.pmtiles" }))).not.toContain("Italic");
    }
  });

  it("hillshade is drawn below the first label", () => {
    const s = buildStyle({ ...base, hillshade: true });
    const ids = s.layers.map((l) => l.id);
    const hs = s.layers.findIndex((l) => l.type === "hillshade");
    const firstLabel = ids.indexOf(firstSymbolLayer(s)!);
    expect(hs).toBeGreaterThan(0);
    expect(hs).toBeLessThan(firstLabel);
  });

  it("overlays inserted before firstSymbolLayer stay under the detailed map's labels, not the world's", () => {
    const s = buildStyle({ ...base, worldUrl: "http://asset.localhost/world.pmtiles" });
    const first = firstSymbolLayer(s);
    expect(first).toBeDefined();
    expect(first!.startsWith("world-")).toBe(false);
  });

  it("without tiles the style is just the background", () => {
    const s = buildStyle({ ...base, tilesUrl: null, satelliteUrl: null, terrainUrl: null });
    expect(s.layers.every((l) => l.type === "background" || !("source" in l) || l.source !== "protomaps")).toBe(true);
  });

  it("Hebrew labels come from font-faces", () => {
    const s = buildStyle(base) as unknown as { "font-faces": Record<string, unknown> };
    expect(Object.keys(s["font-faces"])).toContain("Noto Sans Regular");
  });
});
