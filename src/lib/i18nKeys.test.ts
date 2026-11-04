// The dictionaries are edited by several people / streams at once: no key may appear twice in a
// language, and Hebrew and English must have exactly the same keys. (TypeScript also rejects
// duplicates in one literal, but this test names them and checks both directions.)
import { describe, expect, it } from "vitest";
import src from "./i18n.ts?raw";

/** Top-level keys (two-space indent) of the object literal that starts at `start`. */
function keysOf(start: string): string[] {
  const from = src.indexOf(start);
  expect(from, `dictionary "${start}" not found`).toBeGreaterThanOrEqual(0);
  const body = src.slice(from + start.length);
  const end = body.search(/^};/m);
  const keys: string[] = [];
  for (const line of body.slice(0, end).split(/\r?\n/)) {
    const m = /^ {2}([A-Za-z_][A-Za-z0-9_]*)\s*:/.exec(line);
    if (m) keys.push(m[1]);
  }
  return keys;
}

const dupes = (keys: string[]) => keys.filter((k, i) => keys.indexOf(k) !== i);

describe("i18n dictionaries", () => {
  const he = keysOf("const he = {");
  const en = keysOf("const en: Record<I18nKey, string> = {");

  it("parses both", () => {
    expect(he.length).toBeGreaterThan(500);
    expect(en.length).toBeGreaterThan(500);
  });
  it("has no duplicate keys", () => {
    expect(dupes(he)).toEqual([]);
    expect(dupes(en)).toEqual([]);
  });
  it("has the same keys in Hebrew and English", () => {
    const h = new Set(he);
    const e = new Set(en);
    expect(he.filter((k) => !e.has(k))).toEqual([]);
    expect(en.filter((k) => !h.has(k))).toEqual([]);
  });
});
