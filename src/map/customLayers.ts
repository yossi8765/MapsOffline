import type { CustomLayer } from "../lib/api";

/** Checks of a new user tile layer: an online kind needs an http(s) URL (an XYZ template needs
 *  {z}, {x} and {y} or {-y}), a file kind a path. Returns what is wrong, or null. */
export function validateCustomLayer(c: Pick<CustomLayer, "kind" | "url" | "name">): "name" | "url" | "template" | null {
  if (!c.name.trim()) return "name";
  const u = c.url.trim();
  if (c.kind === "xyz" || c.kind === "wms") {
    if (!/^https?:\/\//i.test(u)) return "url";
    if (c.kind === "xyz" && !(u.includes("{z}") && u.includes("{x}") && (u.includes("{y}") || u.includes("{-y}")))) return "template";
    return null;
  }
  return u ? null : "url";
}
