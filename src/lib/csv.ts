// Minimal CSV for the address-list dialog: quoted fields ("a, b" and "" escapes), the delimiter
// guessed from the header (comma, semicolon or tab – Hebrew Excel exports often use ";"), a
// UTF-8 BOM dropped. Output CSV is written with a BOM so Excel opens Hebrew correctly.

export type Delimiter = "," | ";" | "\t";

export function guessDelimiter(firstLine: string): Delimiter {
  const counts: [Delimiter, number][] = [",", ";", "\t"].map((d) => [d as Delimiter, firstLine.split(d).length - 1]);
  counts.sort((a, b) => b[1] - a[1]);
  return counts[0][1] > 0 ? counts[0][0] : ",";
}

export function parseCsv(text: string, delimiter?: Delimiter): string[][] {
  const src = text.replace(/^﻿/, "");
  const d = delimiter ?? guessDelimiter(src.split(/\r?\n/, 1)[0] ?? "");
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  for (let i = 0; i < src.length; i++) {
    const c = src[i];
    if (quoted) {
      if (c === '"') {
        if (src[i + 1] === '"') {
          field += '"';
          i++;
        } else quoted = false;
      } else field += c;
      continue;
    }
    if (c === '"' && field === "") quoted = true;
    else if (c === d) {
      row.push(field);
      field = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && src[i + 1] === "\n") i++;
      row.push(field);
      field = "";
      if (row.some((x) => x !== "")) rows.push(row);
      row = [];
    } else field += c;
  }
  row.push(field);
  if (row.some((x) => x !== "")) rows.push(row);
  return rows;
}

function quote(v: string, d: Delimiter): string {
  return /["\r\n]/.test(v) || v.includes(d) ? `"${v.replace(/"/g, '""')}"` : v;
}

export function toCsv(rows: string[][], d: Delimiter = ","): string {
  return "﻿" + rows.map((r) => r.map((v) => quote(v, d)).join(d)).join("\r\n") + "\r\n";
}

/** The column that most looks like an address (header words), else the first one. */
export function guessAddressColumn(header: string[]): number {
  const words = ["כתובת", "address", "רחוב", "street", "מיקום", "location"];
  const i = header.findIndex((h) => words.some((w) => h.toLowerCase().includes(w)));
  return i >= 0 ? i : 0;
}
