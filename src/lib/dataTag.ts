/**
 * Version tag of installed data behind a tile URL (report ד-2). The pmtiles protocol caches
 * archive headers and directories per URL, so a replaced file needs a new URL; the tag changes
 * only when the file did (install time / version / size), so a refresh that changed nothing keeps
 * the same URL and the map style is not rebuilt. FNV-1a over the parts, base 36.
 */
export function dataTag(...parts: (string | number | boolean | undefined | null)[]): string {
  const raw = parts.map((p) => (p === undefined || p === null ? "" : String(p))).join("|");
  let h = 2166136261;
  for (let i = 0; i < raw.length; i++) {
    h ^= raw.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return (h >>> 0).toString(36);
}
