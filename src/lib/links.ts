/** Share links: our own `mapsoffline://map=…` and RFC 5870 `geo:` URIs. */

export interface ParsedMapLink {
  lon: number;
  lat: number;
  zoom?: number;
  label?: string;
  /** `geo:0,0?q=<text>`: an address to search for */
  query?: string;
}

/** `mapsoffline://map=<z>/<lat>/<lon>&q=<label>` (our share links) or RFC 5870
 *  `geo:<lat>,<lon>[,alt][;u=..][?z=<zoom>&q=<label>]`. `geo:0,0?q=<text>` (the Android form for
 *  "search this address") yields `query`; `q=<lat>,<lon>(<label>)` yields the label. */
export function parseMapLink(url: string): ParsedMapLink | null {
  const u = url.trim();
  let m = /^mapsoffline:\/*map=([\d.]+)\/(-?[\d.]+)\/(-?[\d.]+)(?:&q=(.*))?$/i.exec(u);
  if (m) {
    const [zoom, lat, lon] = [Number(m[1]), Number(m[2]), Number(m[3])];
    if (![zoom, lat, lon].every(Number.isFinite) || Math.abs(lat) > 90 || Math.abs(lon) > 180) return null;
    let label: string | undefined;
    try {
      label = m[4] ? decodeURIComponent(m[4]) : undefined;
    } catch {
      label = m[4];
    }
    return { lon, lat, zoom, label };
  }
  m = /^geo:(-?[\d.]+),(-?[\d.]+)(?:[;,][^?]*)?(?:\?(.*))?$/i.exec(u);
  if (m) {
    const lat = Number(m[1]);
    const lon = Number(m[2]);
    if (!Number.isFinite(lat) || !Number.isFinite(lon) || Math.abs(lat) > 90 || Math.abs(lon) > 180) return null;
    const q = new URLSearchParams(m[3] ?? "");
    const z = Number(q.get("z"));
    const zoom = Number.isFinite(z) && z > 0 ? z : undefined;
    const text = q.get("q")?.trim() || undefined;
    // q=<lat>,<lon>(<label>)
    const pin = text ? /^(-?[\d.]+)\s*,\s*(-?[\d.]+)\s*(?:\((.*)\))?$/.exec(text) : null;
    if (pin) return { lon: Number(pin[2]), lat: Number(pin[1]), zoom, label: pin[3] || undefined };
    if (lat === 0 && lon === 0) return text ? { lon, lat, zoom, query: text } : null;
    return { lon, lat, zoom, label: text };
  }
  return null;
}

/** Share link of a place (see parseMapLink). */
export function makeMapLink(lon: number, lat: number, zoom: number, label?: string): string {
  return `mapsoffline://map=${Math.round(zoom * 10) / 10}/${lat.toFixed(6)}/${lon.toFixed(6)}${label ? `&q=${encodeURIComponent(label)}` : ""}`;
}
