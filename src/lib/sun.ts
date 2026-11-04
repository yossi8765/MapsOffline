/** Theme resolution: "auto" follows Windows, "night" follows the sun over Israel. */

export function resolveTheme(theme: string): "light" | "dark" {
  if (theme === "light" || theme === "dark") return theme;
  if (theme === "night") return isNightInIsrael() ? "dark" : "light";
  return typeof window !== "undefined" && window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/**
 * Dark between sunset and sunrise, computed for the centre of Israel (31.8°N, 35.0°E) with the
 * standard NOAA approximation (accurate to a few minutes, plenty for a theme switch).
 */
export function isNightInIsrael(now = new Date()): boolean {
  const lat = 31.8;
  const lon = 35.0;
  const rad = Math.PI / 180;
  const start = Date.UTC(now.getUTCFullYear(), 0, 0);
  const doy = Math.floor((now.getTime() - start) / 86_400_000);
  const g = ((2 * Math.PI) / 365) * (doy - 1 + (now.getUTCHours() - 12) / 24);
  const eqTime = 229.18 * (0.000075 + 0.001868 * Math.cos(g) - 0.032077 * Math.sin(g) - 0.014615 * Math.cos(2 * g) - 0.040849 * Math.sin(2 * g));
  const decl = 0.006918 - 0.399912 * Math.cos(g) + 0.070257 * Math.sin(g) - 0.006758 * Math.cos(2 * g) + 0.000907 * Math.sin(2 * g) - 0.002697 * Math.cos(3 * g) + 0.00148 * Math.sin(3 * g);
  const cosHa = (Math.cos(90.833 * rad) - Math.sin(lat * rad) * Math.sin(decl)) / (Math.cos(lat * rad) * Math.cos(decl));
  if (cosHa >= 1) return true; // polar night (never at this latitude)
  if (cosHa <= -1) return false;
  const ha = Math.acos(cosHa) / rad; // degrees
  const solarNoonUtcMin = 720 - 4 * lon - eqTime;
  const sunriseUtcMin = solarNoonUtcMin - 4 * ha;
  const sunsetUtcMin = solarNoonUtcMin + 4 * ha;
  const nowUtcMin = now.getUTCHours() * 60 + now.getUTCMinutes();
  return nowUtcMin < sunriseUtcMin || nowUtcMin > sunsetUtcMin;
}
