import type * as maplibregl from "maplibre-gl";
import { POI_CATEGORIES } from "../lib/poiCategories";
import { mapColors } from "./colors";

function devicePixelRatio(): number {
  return Math.max(1, Math.min(4, Math.ceil(window.devicePixelRatio || 1)));
}

export const BUS_ICON = "bus-stop-marker";
export const BUS_ICON_PX = 32; // logical size; rendered at device pixel ratio so it stays crisp

/** Bus-stop marker in the Google Maps style: blue rounded square, white bus glyph, thin white outline. */
export function drawBusIcon(dark: boolean): ImageData {
  const dpr = devicePixelRatio();
  const px = BUS_ICON_PX * dpr;
  const canvas = document.createElement("canvas");
  canvas.width = px;
  canvas.height = px;
  const c = canvas.getContext("2d")!;
  c.scale(dpr, dpr);
  const rr = (x: number, y: number, w: number, h: number, r: number) => {
    c.beginPath();
    c.roundRect(x, y, w, h, r);
  };
  const mc = mapColors(dark);
  // outline + blue square
  rr(2, 2, 28, 28, 8);
  c.fillStyle = mc.casing;
  c.fill();
  rr(3.5, 3.5, 25, 25, 7);
  c.fillStyle = mc.stop;
  c.fill();
  // bus body
  c.fillStyle = "#ffffff";
  rr(9, 7.5, 14, 15, 3);
  c.fill();
  // window band
  c.fillStyle = mc.stop;
  rr(11, 10, 10, 5.5, 1.2);
  c.fill();
  // lights
  c.beginPath();
  c.arc(12.5, 19, 1.25, 0, Math.PI * 2);
  c.arc(19.5, 19, 1.25, 0, Math.PI * 2);
  c.fill();
  // wheels
  c.fillStyle = "#ffffff";
  rr(10.5, 22, 3.2, 2.6, 1);
  c.fill();
  rr(18.3, 22, 3.2, 2.6, 1);
  c.fill();
  return c.getImageData(0, 0, px, px);
}

const POI_ICON_PX = 36;

/** Category marker: a badge in the category's colour (the same colour as its chip and its
 * label) with the white lucide glyph and a thin outline in the map's casing colour. */
export function poiIconSvg(lucideSvg: string, dark: boolean, color: string): string {
  const inner = lucideSvg.replace(/^[\s\S]*?<svg[^>]*>/, "").replace(/<\/svg>\s*$/, "");
  const card = color;
  const edge = mapColors(dark).casing;
  const ink = "#ffffff";
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" width="${POI_ICON_PX}" height="${POI_ICON_PX}" viewBox="0 0 36 36">` +
    `<rect x="2.5" y="2.5" width="31" height="31" rx="9" fill="${card}" stroke="${edge}" stroke-width="1.5"/>` +
    `<g transform="translate(9 9) scale(0.75)" fill="none" stroke="${ink}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${inner}</g>` +
    `</svg>`
  );
}

/** Rasterise every picker category's marker into the map's images (`poi-<id>`). */
export async function loadPoiIcons(map: maplibregl.Map, dark: boolean): Promise<void> {
  const dpr = devicePixelRatio();
  await Promise.all(
    POI_CATEGORIES.map(
      (c) =>
        new Promise<void>((resolve) => {
          const name = `poi-${c.id}`;
          if (map.hasImage(name)) return resolve();
          const img = new Image(POI_ICON_PX * dpr, POI_ICON_PX * dpr);
          img.onload = () => {
            try {
              const canvas = document.createElement("canvas");
              canvas.width = POI_ICON_PX * dpr;
              canvas.height = POI_ICON_PX * dpr;
              const ctx = canvas.getContext("2d")!;
              ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
              if (!map.hasImage(name)) map.addImage(name, ctx.getImageData(0, 0, canvas.width, canvas.height), { pixelRatio: dpr });
            } catch (e) {
              console.warn("poi icon", c.id, e);
            }
            resolve();
          };
          img.onerror = () => resolve();
          img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(poiIconSvg(c.svg, dark, c.color))}`;
        }),
    ),
  );
}

/** Israeli road-sign colours (hebrew-maps' palette). */
const SHIELD_STYLES = {
  fw: { bg: "#1D5BBF", fg: "#FFFFFF", frame: "#FFFFFF", edge: "#123C80" },
  nat: { bg: "#D6332B", fg: "#FFFFFF", frame: "#FFFFFF", edge: "#8F1D17" },
  reg: { bg: "#178246", fg: "#FFFFFF", frame: "#FFFFFF", edge: "#0D5A2F" },
  loc: { bg: "#FFFFFF", fg: "#1F2328", frame: "#1F2328", edge: "#6B7280" },
} as const;

/** Road-number shield `shield:<fw|nat|reg|loc>:<number>`: a rounded badge with the inner frame
 *  of the real signs and the number, drawn at the device pixel ratio. */
export function shieldImage(name: string): { data: ImageData; pixelRatio: number } | null {
  const m = /^shield:(fw|nat|reg|loc):(.+)$/.exec(name);
  if (!m) return null;
  const st = SHIELD_STYLES[m[1] as keyof typeof SHIELD_STYLES];
  const text = m[2].trim();
  const dpr = devicePixelRatio();
  const height = 18;
  const font = `700 ${Math.round(height * 0.64)}px "Segoe UI", "Noto Sans", Arial, sans-serif`;
  const probe = document.createElement("canvas").getContext("2d")!;
  probe.font = font;
  const w = Math.max(height + 2, Math.ceil(probe.measureText(text).width) + 12);
  const h = height;
  const canvas = document.createElement("canvas");
  canvas.width = Math.round((w + 2) * dpr);
  canvas.height = Math.round((h + 3) * dpr);
  const c = canvas.getContext("2d")!;
  c.scale(dpr, dpr);
  // body with a soft drop shadow
  c.shadowColor = "rgba(0,0,0,.28)";
  c.shadowBlur = 1.5;
  c.shadowOffsetY = 0.8;
  c.beginPath();
  c.roundRect(1, 1, w, h, 3.5);
  c.fillStyle = st.bg;
  c.fill();
  c.shadowColor = "transparent";
  c.lineWidth = 1;
  c.strokeStyle = st.edge;
  c.stroke();
  // inner frame, as on the real signs
  c.beginPath();
  c.roundRect(2.6, 2.6, w - 3.2, h - 3.2, 2.2);
  c.lineWidth = 1.1;
  c.strokeStyle = st.frame;
  c.stroke();
  c.font = font;
  c.fillStyle = st.fg;
  c.textAlign = "center";
  c.textBaseline = "middle";
  c.fillText(text, 1 + w / 2, 1 + h / 2 + 0.5);
  return { data: c.getImageData(0, 0, canvas.width, canvas.height), pixelRatio: dpr };
}
