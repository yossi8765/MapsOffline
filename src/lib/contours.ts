/**
 * Contour lines built in the browser from the Terrarium elevation tiles that already drive the
 * hillshade (maplibre-contour, BSD-3): no extra data to download.
 *
 * maplibre-contour fetches DEM tiles from a {z}/{x}/{y} URL; our elevation data is one PMTiles
 * archive, so its tile manager is given a fetch function that reads the archive directly (and
 * answers a flat sea-level tile where the archive has none, e.g. over the sea).
 */
import * as maplibregl from "maplibre-gl";
import mlcontour from "maplibre-contour";
import { PMTiles } from "pmtiles";
import { TERRAIN_MAX_ZOOM } from "./mapStyle";

const PATTERN = "pmdem://{z}/{x}/{y}";

let current: { terrainUrl: string; tiles: string } | null = null;
let seq = 0;
let flat: Promise<Blob> | null = null;

/** A 512 px Terrarium tile at 0 m (r=128, g=0, b=0), the size of the Mapterhorn DEM tiles. */
function flatTile(): Promise<Blob> {
  if (!flat) {
    flat = new Promise((resolve, reject) => {
      const c = document.createElement("canvas");
      c.width = 512;
      c.height = 512;
      const ctx = c.getContext("2d")!;
      ctx.fillStyle = "rgb(128,0,0)";
      ctx.fillRect(0, 0, 512, 512);
      c.toBlob((b) => (b ? resolve(b) : reject(new Error("flat tile"))), "image/png");
    });
  }
  return flat;
}

/** Contour vector-tile URL template for the archive at `terrainUrl` (registered once per file). */
export function contourTilesUrl(terrainUrl: string): string {
  if (current?.terrainUrl === terrainUrl) return current.tiles;
  const archive = new PMTiles(terrainUrl);
  const id = `contour${++seq}`;
  const source = new mlcontour.DemSource({ url: PATTERN, encoding: "terrarium", maxzoom: TERRAIN_MAX_ZOOM, worker: false, id });
  source.manager = new mlcontour.LocalDemManager({
    demUrlPattern: PATTERN,
    cacheSize: 100,
    encoding: "terrarium",
    maxzoom: TERRAIN_MAX_ZOOM,
    timeoutMs: 10_000,
    getTile: async (url: string, abort: AbortController) => {
      const m = /(\d+)\/(\d+)\/(\d+)$/.exec(url);
      if (!m) throw new Error(`bad DEM url ${url}`);
      const r = await archive.getZxy(Number(m[1]), Number(m[2]), Number(m[3]), abort.signal);
      if (!r?.data) return { data: await flatTile() };
      // Mapterhorn tiles are lossless WebP ("RIFF"), the older DEM was PNG
      const b = new Uint8Array(r.data, 0, 4);
      const webp = b[0] === 0x52 && b[1] === 0x49 && b[2] === 0x46 && b[3] === 0x46;
      return { data: new Blob([r.data], { type: webp ? "image/webp" : "image/png" }) };
    },
  });
  source.setupMaplibre(maplibregl);
  const tiles = source.contourProtocolUrl({
    // [minor, major] interval in metres by zoom
    thresholds: { 10: [100, 500], 11: [50, 250], 12: [20, 100], 13: [10, 50], 15: [5, 25] },
    elevationKey: "ele",
    levelKey: "level",
    contourLayer: "contours",
  });
  current = { terrainUrl, tiles };
  return tiles;
}
