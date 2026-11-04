// Vehicle markers for car navigation (Settings → navigation → vehicle, like Waze's "my car"):
// clean top-down shapes pointing north, in the accent colour with a white outline and a soft
// shadow, drawn at the device pixel ratio so they stay sharp. The map rotates them with the
// route bearing (icon-rotate, map-aligned).

export type VehicleKind = "arrow" | "car" | "suv" | "van" | "truck" | "motorbike" | "bus";
export const VEHICLE_KINDS: VehicleKind[] = ["arrow", "car", "suv", "van", "truck", "motorbike", "bus"];

/** Body colour choice (Settings → navigation); "accent" = the colour scheme's accent. */
export type VehicleColor = "accent" | "white" | "silver" | "black" | "gray" | "blue" | "red" | "yellow";
export const VEHICLE_COLORS: VehicleColor[] = ["accent", "white", "silver", "black", "gray", "blue", "red", "yellow"];

/** Paint of a body colour: fill, outline (dark for light bodies, light for dark ones, so the
 *  vehicle stays visible on light and dark maps), roof and glass. */
export interface VehiclePaint {
  fill: string;
  outline: string;
  roof: string;
  glass: string;
}

const BODY: Record<Exclude<VehicleColor, "accent">, VehiclePaint> = {
  white: { fill: "#f5f6f8", outline: "#3a4250", roof: "#e3e6ea", glass: "rgba(20, 28, 40, 0.85)" },
  silver: { fill: "#c4c9d0", outline: "#2f3540", roof: "#dde1e6", glass: "rgba(20, 28, 40, 0.85)" },
  black: { fill: "#202329", outline: "#eef1f5", roof: "#3a3f48", glass: "rgba(120, 140, 165, 0.75)" },
  gray: { fill: "#6b7280", outline: "#ffffff", roof: "#8b929e", glass: "rgba(20, 28, 40, 0.85)" },
  blue: { fill: "#1f5fd1", outline: "#ffffff", roof: "#4a83e6", glass: "rgba(20, 28, 40, 0.85)" },
  red: { fill: "#d42d2d", outline: "#ffffff", roof: "#e5605f", glass: "rgba(20, 28, 40, 0.85)" },
  yellow: { fill: "#f5c518", outline: "#3d3a2a", roof: "#f8d95e", glass: "rgba(20, 28, 40, 0.85)" },
};

/** The paint of a colour choice; `accent` is the current accent colour ("#rrggbb"). */
export function vehiclePaint(choice: VehicleColor | undefined, accent: string): VehiclePaint {
  if (choice && choice !== "accent") return BODY[choice];
  return { fill: accent, outline: "#ffffff", roof: shade(accent, 0.35), glass: "rgba(20, 28, 40, 0.82)" };
}

/** Swatch colour of a choice (Settings / picker). */
export function vehicleSwatch(choice: VehicleColor, accent: string): string {
  return vehiclePaint(choice, accent).fill;
}

/** Logical size of the square canvas (css px). */
export const VEHICLE_PX = 56;

function dpr(): number {
  return typeof window === "undefined" ? 1 : Math.max(1, Math.min(4, Math.ceil(window.devicePixelRatio || 1)));
}

/** Darker / lighter version of a "#rrggbb" colour (k < 0 darker). */
function shade(hex: string, k: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return hex;
  const n = parseInt(m[1], 16);
  const ch = (v: number) => Math.round(k < 0 ? v * (1 + k) : v + (255 - v) * k);
  const r = ch((n >> 16) & 255),
    g = ch((n >> 8) & 255),
    b = ch(n & 255);
  return `rgb(${r}, ${g}, ${b})`;
}

/** Draw `kind` on a new canvas (north up) in `paint` (or a plain body colour string). */
export function drawVehicle(kind: VehicleKind, paint: VehiclePaint | string, ratio = dpr()): HTMLCanvasElement {
  const p: VehiclePaint = typeof paint === "string" ? vehiclePaint("accent", paint) : paint;
  const color = p.fill;
  const S = VEHICLE_PX;
  const canvas = document.createElement("canvas");
  canvas.width = S * ratio;
  canvas.height = S * ratio;
  const c = canvas.getContext("2d")!;
  c.scale(ratio, ratio);
  c.translate(S / 2, S / 2);
  c.lineJoin = "round";
  const glass = p.glass;
  const dark = shade(color, -0.35);
  const light = p.roof;

  // shared: a body with outline + shadow
  const body = (path: () => void) => {
    c.save();
    c.shadowColor = "rgba(0, 0, 0, 0.35)";
    c.shadowBlur = 5;
    c.shadowOffsetY = 1.5;
    path();
    c.fillStyle = color;
    c.fill();
    c.restore();
    path();
    c.lineWidth = 2;
    c.strokeStyle = p.outline;
    c.stroke();
  };
  const rr = (x: number, y: number, w: number, h: number, r: number | number[]) => () => {
    c.beginPath();
    c.roundRect(x, y, w, h, r);
  };
  const fillRR = (x: number, y: number, w: number, h: number, r: number | number[], style: string) => {
    rr(x, y, w, h, r)();
    c.fillStyle = style;
    c.fill();
  };

  switch (kind) {
    case "arrow": {
      body(() => {
        c.beginPath();
        c.moveTo(0, -19);
        c.lineTo(14, 16);
        c.lineTo(0, 9);
        c.lineTo(-14, 16);
        c.closePath();
      });
      break;
    }
    case "car": {
      body(rr(-9.5, -19, 19, 38, [7, 7, 6, 6]));
      fillRR(-7.5, -10, 15, 6.5, [3, 3, 1.5, 1.5], glass); // windshield
      fillRR(-7, 8, 14, 4.5, [1.5, 1.5, 3, 3], glass); // rear window
      fillRR(-6.5, -3, 13, 10, 2, light); // roof
      break;
    }
    case "suv": {
      body(rr(-11, -21, 22, 42, [6, 6, 5, 5]));
      fillRR(-9, -12, 18, 7, [3, 3, 1.5, 1.5], glass);
      fillRR(-8.5, 11, 17, 5, [1.5, 1.5, 3, 3], glass);
      fillRR(-8, -4, 16, 14, 2, light);
      c.strokeStyle = dark; // roof rails
      c.lineWidth = 1.4;
      c.beginPath();
      c.moveTo(-6, -3);
      c.lineTo(-6, 9);
      c.moveTo(6, -3);
      c.lineTo(6, 9);
      c.stroke();
      break;
    }
    case "van": {
      body(rr(-11, -22, 22, 44, [8, 8, 3, 3]));
      fillRR(-9, -16, 18, 6, [3, 3, 1.5, 1.5], glass);
      fillRR(-8.5, -8, 17, 27, 2, light); // long roof
      break;
    }
    case "truck": {
      // cab + a separate trailer
      body(rr(-10, -23, 20, 13, [6, 6, 2, 2]));
      fillRR(-8, -19, 16, 5, [2.5, 2.5, 1, 1], glass);
      body(rr(-11.5, -8, 23, 31, 2.5));
      c.strokeStyle = dark;
      c.lineWidth = 1.2;
      for (const y of [0, 8, 16]) {
        c.beginPath();
        c.moveTo(-9, y);
        c.lineTo(9, y);
        c.stroke();
      }
      break;
    }
    case "motorbike": {
      body(rr(-4.5, -20, 9, 40, 4.5));
      // handlebar
      c.strokeStyle = p.outline;
      c.lineWidth = 3.5;
      c.lineCap = "round";
      c.beginPath();
      c.moveTo(-10, -9);
      c.lineTo(10, -9);
      c.stroke();
      c.strokeStyle = dark;
      c.lineWidth = 2;
      c.stroke();
      // rider's helmet
      c.beginPath();
      c.arc(0, 1, 5.5, 0, Math.PI * 2);
      c.fillStyle = glass;
      c.fill();
      c.lineWidth = 1.5;
      c.strokeStyle = p.outline;
      c.stroke();
      break;
    }
    case "bus": {
      body(rr(-11, -24, 22, 48, [5, 5, 3, 3]));
      fillRR(-9, -21, 18, 5, [2.5, 2.5, 1, 1], glass); // windshield
      // side windows along the roof
      for (let y = -13; y <= 15; y += 7) {
        fillRR(-9, y, 3, 5, 1, glass);
        fillRR(6, y, 3, 5, 1, glass);
      }
      fillRR(-5, -12, 10, 30, 2, light); // roof
      break;
    }
  }
  return canvas;
}

/** Image data of `kind` for map.addImage (with its pixel ratio). */
export function vehicleImage(kind: VehicleKind, paint: VehiclePaint | string): { data: ImageData; pixelRatio: number } {
  const r = dpr();
  const cv = drawVehicle(kind, paint, r);
  return { data: cv.getContext("2d")!.getImageData(0, 0, cv.width, cv.height), pixelRatio: r };
}

/** A data URL of `kind` for previews in Settings / the navigation picker. */
export function vehiclePreview(kind: VehicleKind, paint: VehiclePaint | string): string {
  return drawVehicle(kind, paint, 2).toDataURL("image/png");
}
