import dayjs from "dayjs";
import relativeTime from "dayjs/plugin/relativeTime";
import "dayjs/locale/he";
import type { Leg, Route, RouteTag } from "./api";
import { translate, type I18nKey } from "./i18n";
import { ask } from "@tauri-apps/plugin-dialog";

dayjs.extend(relativeTime);

export type T = (k: I18nKey) => string;

export function fmtDuration(sec: number, t: T): string {
  const min = Math.round(sec / 60);
  if (min < 1) return t("lessThanMinute");
  if (min < 60) return `${min} ${t("minutes")}`;
  const h = Math.floor(min / 60);
  const m = min % 60;
  // Hebrew: "שעה", "שעתיים", "3 שעות"; minutes join with "ו-" ("שעה ו-20 דק׳")
  const hs = h === 1 ? t("hourOne") : h === 2 ? t("hourTwo") : `${h} ${t("hoursPlural")}`;
  return m ? `${hs} ${t("durationAnd")}${m} ${t("minutes")}` : hs;
}

export function fmtDistance(m: number, t: T, units = "metric"): string {
  if (units === "imperial") {
    const ft = m * 3.28084;
    if (ft < 1000) return `${Math.round(ft)} ${t("ft")}`;
    const mi = m / 1609.344;
    return `${mi < 10 ? mi.toFixed(1) : Math.round(mi)} ${t("mi")}`;
  }
  if (m < 1000) return `${Math.round(m)} ${t("m")}`;
  const km = m / 1000;
  return `${km < 10 ? km.toFixed(1) : Math.round(km)} ${t("km")}`;
}

export function fmtTime(iso?: string): string {
  if (!iso) return "";
  const d = dayjs(iso);
  return d.isValid() ? d.format("HH:mm") : "";
}

/** Date and time: "03/11/2026 08:12" in Hebrew (the Israeli convention), the locale's own
 *  format through Intl otherwise ("Nov 3, 2026, 08:12"). */
export function fmtDateTime(iso?: string, lang = "he"): string {
  if (!iso) return "";
  const d = dayjs(iso);
  if (!d.isValid()) return "";
  if (lang === "he") return d.format("DD/MM/YYYY HH:mm");
  try {
    return new Intl.DateTimeFormat("en-IL", { dateStyle: "medium", timeStyle: "short", hour12: false }).format(d.toDate());
  } catch {
    return d.format("YYYY-MM-DD HH:mm");
  }
}

/** Date only: "03/11/2026" in Hebrew, "Nov 3, 2026" otherwise. */
export function fmtDate(iso?: string, lang = "he"): string {
  if (!iso) return "";
  const d = dayjs(iso);
  if (!d.isValid()) return "";
  if (lang === "he") return d.format("DD/MM/YYYY");
  try {
    return new Intl.DateTimeFormat("en-IL", { dateStyle: "medium" }).format(d.toDate());
  } catch {
    return d.format("YYYY-MM-DD");
  }
}

export function fmtAgo(iso: string, lang = "he"): string {
  const d = dayjs(iso).locale(lang === "he" ? "he" : "en");
  return d.isValid() ? d.fromNow() : "";
}

export function fmtBytes(b: number): string {
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(0)} KB`;
  if (b < 1024 * 1024 * 1024) return `${(b / 1048576).toFixed(1)} MB`;
  return `${(b / 1073741824).toFixed(2)} GB`;
}

export function fmtCoords(lon: number, lat: number): string {
  return `${lat.toFixed(5)}, ${lon.toFixed(5)}`;
}

export function tagLabel(tag: RouteTag, t: T): string {
  switch (tag) {
    case "fastest":
      return t("tagFastest");
    case "shortest":
      return t("tagShortest");
    case "fewest_transfers":
      return t("tagFewestTransfers");
    case "least_walking":
      return t("tagLeastWalking");
  }
}

/** Local datetime-local input value for a Date (no timezone). */
export function toLocalInput(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Round up to the next 5 minutes. */
export function nextFiveMinutes(from = new Date()): Date {
  const d = new Date(from);
  d.setSeconds(0, 0);
  d.setMinutes(Math.ceil((d.getMinutes() + 1) / 5) * 5);
  return d;
}

/** datetime-local value → ISO-8601 with local offset. */
export function localInputToIso(v: string): string {
  const d = new Date(v);
  return dayjs(d).format("YYYY-MM-DDTHH:mm:ssZ");
}

export function isoToLocalInput(iso?: string): string {
  if (!iso) return toLocalInput(nextFiveMinutes());
  const d = new Date(iso);
  return isNaN(d.getTime()) ? toLocalInput(nextFiveMinutes()) : toLocalInput(d);
}

// ------------------------------------------------------------- export
function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

export function routeToGpx(route: Route, name: string): string {
  const pts = route.geometry.map(([lon, lat]) => `      <trkpt lat="${lat.toFixed(6)}" lon="${lon.toFixed(6)}"/>`).join("\n");
  const wpts = route.legs
    .flatMap((l) => l.steps ?? [])
    .map((s) => {
      const p = route.geometry[Math.min(s.geometryStart, route.geometry.length - 1)];
      return p ? `  <wpt lat="${p[1].toFixed(6)}" lon="${p[0].toFixed(6)}"><name>${esc(s.instruction)}</name></wpt>` : "";
    })
    .join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="MapsOffline" xmlns="http://www.topografix.com/GPX/1/1">
  <metadata><name>${esc(name)}</name></metadata>
${wpts}
  <trk>
    <name>${esc(name)}</name>
    <trkseg>
${pts}
    </trkseg>
  </trk>
</gpx>
`;
}

export function routeToKml(route: Route, name: string): string {
  const legs = route.legs
    .map((l, i) => {
      const coords = l.geometry.map(([lon, lat]) => `${lon.toFixed(6)},${lat.toFixed(6)},0`).join(" ");
      const color = l.kind === "walk" ? "ff888888" : "ffdb6325";
      const legName = l.transit ? `${l.transit.routeShortName} ${l.transit.headsign}` : `${l.kind} ${i + 1}`;
      return `    <Placemark>
      <name>${esc(legName)}</name>
      <Style><LineStyle><color>${color}</color><width>4</width></LineStyle></Style>
      <LineString><tessellate>1</tessellate><coordinates>${coords}</coordinates></LineString>
    </Placemark>`;
    })
    .join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2">
  <Document>
    <name>${esc(name)}</name>
${legs}
  </Document>
</kml>
`;
}

export function routeToText(route: Route, name: string, lang: string): string {
  const t = (k: I18nKey) => translate(lang, k);
  const lines: string[] = [name, `${fmtDuration(route.durationSec, t)} · ${fmtDistance(route.distanceM, t)}`, ""];
  route.legs.forEach((l: Leg) => {
    if (l.transit) {
      const tr = l.transit;
      lines.push(
        `${fmtTime(tr.departIso)} ${tr.routeShortName} (${tr.agency}) ${tr.fromStop} ${lang === "en" ? "→" : "←"} ${tr.toStop} ${fmtTime(tr.arriveIso)} (${tr.numStops} ${t("stops")})`,
      );
    } else if (l.steps?.length) {
      l.steps.forEach((s) => lines.push(`• ${s.instruction} (${fmtDistance(s.distanceM, t)})`));
    } else {
      lines.push(`${t("walking")}: ${fmtDistance(l.distanceM, t)} · ${fmtDuration(l.durationSec, t)}`);
    }
  });
  return lines.join("\n");
}


export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  }
}

/** Yes/no question in a native Windows dialog with the app's own title and button labels
 *  (window.confirm shows "tauri.localhost says" and English OK/Cancel). */
export async function askConfirm(message: string, okLabel: string, cancelLabel: string, title: string): Promise<boolean> {
  try {
    return await ask(message, { title, kind: "warning", okLabel, cancelLabel });
  } catch {
    return window.confirm(message);
  }
}

/** Arrow between origin and destination in route names: "A ← B" reads right-to-left in Hebrew,
 *  "A → B" in English (also used for exported file names). */
export function routeArrow(lang: string): string {
  return lang === "he" || lang === "ar" ? "←" : "→";
}
