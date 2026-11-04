// Text matching shared by every local filter (search box, history, line picker, categories,
// command palette): case-insensitive, Hebrew final letters folded to their regular form,
// niqqud / cantillation and quote marks ignored.

const FINALS: Record<string, string> = { ך: "כ", ם: "מ", ן: "נ", ף: "פ", ץ: "צ" };

/** Lower case, final letters → regular ("שלום" → "שלומ"), niqqud and quote marks removed. */
export function fold(s: string): string {
  let out = "";
  for (const ch of s.toLowerCase()) {
    const c = ch.codePointAt(0)!;
    // niqqud and cantillation marks (U+0591–U+05C7, except maqaf U+05BE which reads as "-")
    if (c >= 0x0591 && c <= 0x05c7 && c !== 0x05be) continue;
    if (ch === '"' || ch === "'" || ch === "׳" || ch === "״") continue;
    out += FINALS[ch] ?? ch;
  }
  return out;
}

/** Every whitespace-separated word of `query` occurs in `text` (folded); empty query matches. */
export function matchesAll(text: string, query: string): boolean {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const f = fold(text);
  return words.every((w) => f.includes(w));
}

/** Rank for sorting filter results: 0 = starts with the query, 1 = a word starts with it, 2 = contains. */
export function matchRank(text: string, query: string): number {
  const f = fold(text);
  const q = fold(query.trim());
  if (!q) return 2;
  if (f.startsWith(q)) return 0;
  if (f.split(/[\s,\-–־]+/).some((w) => w.startsWith(q))) return 1;
  return 2;
}
