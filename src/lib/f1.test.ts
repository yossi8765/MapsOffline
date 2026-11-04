// Pure helpers added in the F1 (look / UX / accessibility) stream.
import { describe, expect, it } from "vitest";
import { fold, matchesAll, matchRank } from "./text";
import { isBrowserKey, matchShortcut, SHORTCUTS, shortcutHint, shortcutRows } from "./shortcuts";
import { CREDITS } from "./credits";
import { guessAddressColumn, guessDelimiter, parseCsv, toCsv } from "./csv";
import { encodeQr, qrPath } from "./qr";
import { translate, type I18nKey } from "./i18n";

const key = (code: string, mods: Partial<{ ctrlKey: boolean; shiftKey: boolean; altKey: boolean; metaKey: boolean }> = {}) => ({
  code,
  key: code,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});

describe("text.fold", () => {
  it("folds final letters, case, niqqud and quote marks", () => {
    expect(fold("שלום")).toBe("שלומ");
    expect(fold("Tel Aviv")).toBe("tel aviv");
    expect(fold("שָׁלוֹם")).toBe("שלומ");
    expect(fold('צה"ל')).toBe("צהל");
    expect(fold("רמב״ם")).toBe("רמבמ");
  });
  it("matchesAll needs every word, in any order", () => {
    expect(matchesAll("ירושלים ← הרצל 12, תל אביב", "תל הרצל")).toBe(true);
    expect(matchesAll("שלום עליכם", "שלומ")).toBe(true);
    expect(matchesAll("Tel Aviv", "tel haifa")).toBe(false);
    expect(matchesAll("anything", "  ")).toBe(true);
  });
  it("matchRank prefers a prefix, then a word start", () => {
    expect(matchRank("הרצל 12", "הרצ")).toBe(0);
    expect(matchRank("רחוב הרצל", "הרצ")).toBe(1);
    expect(matchRank("שהרצל", "הרצ")).toBe(2);
  });
});

describe("shortcuts table", () => {
  it("plain keys never fire while typing; Ctrl ones do", () => {
    expect(matchShortcut(key("KeyL"), false)?.id).toBe("locate");
    expect(matchShortcut(key("KeyL"), true)).toBeUndefined();
    expect(matchShortcut(key("KeyF", { ctrlKey: true }), true)?.id).toBe("search");
    expect(matchShortcut(key("KeyK", { ctrlKey: true }), false)?.id).toBe("palette");
  });
  it("tells Ctrl+D (nothing), Ctrl+Shift+D (data) and D (route) apart", () => {
    expect(matchShortcut(key("KeyD"), false)?.id).toBe("routePanel");
    expect(matchShortcut(key("KeyD", { ctrlKey: true, shiftKey: true }), false)?.id).toBe("data");
    expect(matchShortcut(key("KeyD", { ctrlKey: true }), false)).toBeUndefined();
  });
  it("? (Shift+/) is the help, / alone is search", () => {
    expect(matchShortcut(key("Slash", { shiftKey: true }), false)?.id).toBe("help");
    expect(matchShortcut(key("Slash"), false)?.id).toBe("searchSlash");
  });
  it("swallows reload and history navigation", () => {
    expect(isBrowserKey(key("F5"))).toBe(true);
    expect(isBrowserKey(key("KeyR", { ctrlKey: true, shiftKey: true }))).toBe(true);
    expect(isBrowserKey(key("ArrowLeft", { altKey: true }))).toBe(true);
    expect(isBrowserKey(key("KeyR", { ctrlKey: true }))).toBe(false); // new route
  });
  it("no two shortcuts share the same key combination", () => {
    const combos = SHORTCUTS.filter((s) => !s.displayOnly).flatMap((s) => s.codes.map((c) => `${s.ctrl ? "C" : ""}${s.shift ? "S" : ""}${c}`));
    expect(new Set(combos).size).toBe(combos.length);
  });
  it("the dialog rows merge keys per label and every label is translated", () => {
    const rows = shortcutRows();
    const search = rows.find((r) => r.label === "shortcutSearch");
    expect(search?.keys).toEqual(["Ctrl+F", "/"]);
    for (const r of rows) {
      expect(translate("he", r.label)).not.toBe(r.label);
      expect(translate("en", r.label)).not.toBe(r.label);
    }
    expect(shortcutHint("palette")).toBe("Ctrl+K");
  });
});

describe("credits", () => {
  it("ids are unique and every use is translated", () => {
    expect(new Set(CREDITS.map((c) => c.id)).size).toBe(CREDITS.length);
    for (const c of CREDITS) {
      expect(translate("he", c.use as I18nKey)).not.toBe(c.use);
      expect(translate("en", c.use as I18nKey)).not.toBe(c.use);
    }
  });
});

describe("csv", () => {
  it("guesses the delimiter and parses quoted fields", () => {
    expect(guessDelimiter("שם;כתובת;עיר")).toBe(";");
    expect(guessDelimiter("a\tb")).toBe("\t");
    const rows = parseCsv('﻿שם,כתובת\n"בית ""גדול""","הרצל 12, תל אביב"\r\n\nא,ב\n');
    expect(rows).toEqual([
      ["שם", "כתובת"],
      ['בית "גדול"', "הרצל 12, תל אביב"],
      ["א", "ב"],
    ]);
  });
  it("round-trips through toCsv", () => {
    const rows = [["a", 'b "c"', "d,e"], ["1", "2\n3", ""]];
    expect(parseCsv(toCsv(rows))).toEqual(rows.map((r) => (r.every((x) => x === "") ? [] : r)));
  });
  it("finds the address column by its header", () => {
    expect(guessAddressColumn(["שם", "כתובת מלאה", "טלפון"])).toBe(1);
    expect(guessAddressColumn(["Name", "Street address"])).toBe(1);
    expect(guessAddressColumn(["x", "y"])).toBe(0);
  });
});

describe("qr", () => {
  it("picks the smallest version and draws the finder patterns", () => {
    const q = encodeQr("hello");
    expect(q.version).toBe(1);
    expect(q.size).toBe(21);
    // finder pattern corners: dark ring, light ring, dark centre
    for (const [x, y] of [
      [0, 0],
      [q.size - 7, 0],
      [0, q.size - 7],
    ]) {
      expect(q.modules[y][x]).toBe(true);
      expect(q.modules[y + 1][x + 1]).toBe(false);
      expect(q.modules[y + 3][x + 3]).toBe(true);
    }
    // timing pattern alternates
    expect(q.modules[6].slice(8, 13)).toEqual([true, false, true, false, true]);
  });
  it("grows with the text (a long route link needs a bigger symbol)", () => {
    const q = encodeQr("https://www.google.com/maps/dir/?api=1&origin=31.7683,35.2137&destination=32.0779,34.7741&travelmode=driving");
    expect(q.version).toBeGreaterThanOrEqual(6);
    expect(q.size).toBe(q.version * 4 + 17);
    expect(qrPath(q)).toMatch(/^M\d+ \d+h1v1h-1z/);
  });
  it("encodes UTF-8 (Hebrew labels in geo: links)", () => {
    expect(() => encodeQr("geo:32.07,34.77?q=הרצל 12")).not.toThrow();
  });
});
