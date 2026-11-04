// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import type { LayerSpecification, StyleSpecification } from "maplibre-gl";
import { keepOverlays, setVisible } from "./styleDiff";
import { buildStyle, trailLayers, TRAILS_MINZOOM, type StyleOptions } from "../lib/mapStyle";

const L = (id: string, source?: string, type = "line"): LayerSpecification => (source ? ({ id, type, source } as LayerSpecification) : ({ id, type: "background" } as LayerSpecification));

function style(layers: LayerSpecification[], sources: string[]): StyleSpecification {
  return { version: 8, sources: Object.fromEntries(sources.map((s) => [s, { type: "geojson", data: { type: "FeatureCollection", features: [] } }])), layers } as StyleSpecification;
}

describe("keepOverlays", () => {
  const prev = style(
    [L("background"), L("water", "protomaps"), L("route-sel", "route-selected"), L("route-hl", "route-highlight"), L("labels", "protomaps", "symbol"), L("poi-layer", "poi-layer", "symbol"), L("my-location-dot", "my-location", "circle")],
    ["protomaps", "route-selected", "route-highlight", "poi-layer", "my-location"],
  );

  it("carries the overlays into the new style at the same places", () => {
    const next = style([L("background"), L("water", "protomaps"), L("hillshade", "dem"), L("labels", "protomaps", "symbol")], ["protomaps", "dem"]);
    const out = keepOverlays(prev, next);
    expect(out.layers.map((l) => l.id)).toEqual(["background", "water", "hillshade", "route-sel", "route-hl", "labels", "poi-layer", "my-location-dot"]);
    expect(Object.keys(out.sources).sort()).toEqual(["dem", "my-location", "poi-layer", "protomaps", "route-highlight", "route-selected"]);
    // the overlay keeps its data object (no reload of the GeoJSON)
    expect(out.sources["route-selected"]).toBe(prev.sources["route-selected"]);
  });

  it("anchor gone: the overlay goes to the top; base sources never carried", () => {
    const next = style([L("background"), L("water", "protomaps")], ["protomaps"]);
    const out = keepOverlays(prev, next);
    expect(out.layers.map((l) => l.id)).toEqual(["background", "water", "route-sel", "route-hl", "poi-layer", "my-location-dot"]);
    const withTrails = style([L("background"), L("t", "trails")], ["trails", "custom-x"]);
    const out2 = keepOverlays(withTrails, style([L("background")], []));
    expect(Object.keys(out2.sources)).toEqual([]);
  });

  it("no previous style: the new one as is", () => {
    const next = style([L("background")], []);
    expect(keepOverlays(undefined, next)).toBe(next);
  });
});

describe("setVisible", () => {
  it("sets visibility only on existing layers", () => {
    const calls: unknown[][] = [];
    const map = { getLayer: (id: string) => (id === "a" ? {} : undefined), setLayoutProperty: (...a: unknown[]) => calls.push(a) };
    setVisible(map, ["a", "b"], false);
    expect(calls).toEqual([["a", "visibility", "none"]]);
  });
});

describe("trails and custom layers in the style", () => {
  const base: StyleOptions = { tilesUrl: "http://asset.localhost/i.pmtiles", satelliteUrl: null, satellite: false, terrainUrl: null, hillshade: false, terrain3d: false, flavor: "light", lang: "he", poi: true, buildings3d: false };
  it("only long trails below zoom 11, half transparent", () => {
    const ls = trailLayers();
    const long = ls.find((l) => l.id === "trails-long")!;
    expect(long.minzoom).toBe(9);
    expect(long.maxzoom).toBe(TRAILS_MINZOOM);
    expect((long as { paint: Record<string, unknown> }).paint["line-opacity"]).toBe(0.5);
    for (const id of ["trails", "trails-casing", "trails-bike"]) expect(ls.find((l) => l.id === id)!.minzoom).toBe(TRAILS_MINZOOM);
  });
  it("visible custom layers: raster under the first line, hidden ones left out", () => {
    const s = buildStyle({
      ...base,
      customLayers: [
        { id: "a", name: "A", kind: "xyz", url: "https://x/{z}/{x}/{y}", opacity: 0.6, visible: true },
        { id: "b", name: "B", kind: "wms", url: "https://y/wms", opacity: 1, visible: false },
      ],
    });
    expect(s.sources["custom-a"]).toMatchObject({ type: "raster", tiles: ["http://mo-user.localhost/a/{z}/{x}/{y}"] });
    expect(s.sources["custom-b"]).toBeUndefined();
    const i = s.layers.findIndex((l) => l.id === "custom-a");
    const firstLine = s.layers.findIndex((l) => l.type === "line" || l.type === "symbol");
    expect(i).toBeGreaterThan(0);
    expect(i).toBeLessThan(firstLine);
  });
  it("dark flavour: minor roads lighter than the stock #3d3d3d", () => {
    const s = buildStyle({ ...base, flavor: "dark" });
    const minor = s.layers.find((l) => l.id === "roads_minor") as { paint?: Record<string, unknown> } | undefined;
    expect(JSON.stringify(minor?.paint)).toContain("#50535a");
  });
});
