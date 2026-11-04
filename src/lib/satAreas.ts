// Sharp aerial-imagery areas: pure helpers for the Data panel (src/components/SatelliteAreas.tsx).
import type { LonLat, SatelliteArea, SatelliteAreaEstimate } from "./api";
import type { T } from "./format";
import { fmtBytes } from "./format";

export type Box = [number, number, number, number];

/** Zooms the user picks from, with their practical names. */
export const AREA_ZOOMS: { zoom: number; label: "satZoom16" | "satZoom17" | "satZoom18" }[] = [
  { zoom: 16, label: "satZoom16" },
  { zoom: 17, label: "satZoom17" },
  { zoom: 18, label: "satZoom18" },
];

export const RADII_KM = [2, 5, 10];

/** A square-ish box of `km` around a point (half-width `km`). */
export function boxAround([lon, lat]: LonLat, km: number): Box {
  const dLat = km / 110.57;
  const dLon = km / (111.32 * Math.cos((lat * Math.PI) / 180));
  const r = (v: number) => Math.round(v * 1e5) / 1e5;
  return [r(lon - dLon), r(lat - dLat), r(lon + dLon), r(lat + dLat)];
}

/** A new area id (lower-case letters, digits and "-", as the backend accepts). */
export function newAreaId(now = Date.now()): string {
  return `a-${now.toString(36)}`;
}

/** "about 1.2 GB · about 3 hours" (or minutes) for an estimate. */
export function fmtEstimate(e: SatelliteAreaEstimate, t: T): string {
  const size = fmtBytes(e.bytes);
  const time =
    e.minutes < 1
      ? t("satLessThanMinute")
      : e.minutes < 90
        ? t("satAboutMinutes").replace("{n}", String(Math.max(1, Math.round(e.minutes))))
        : t("satAboutHours").replace("{n}", (Math.round((e.minutes / 60) * 10) / 10).toString());
  return `${t("satAbout")} ${size} · ${time}`;
}

/** The label of an area's zoom. */
export function zoomLabel(zoom: number, t: T): string {
  const z = AREA_ZOOMS.find((x) => x.zoom === zoom);
  return z ? t(z.label) : `z${zoom}`;
}

/** Areas without the one removed. */
export function withoutArea(areas: SatelliteArea[], id: string): SatelliteArea[] {
  return areas.filter((a) => a.id !== id);
}
