import { describe, expect, it } from "vitest";
import { recentDestinations } from "./QuickDestinations";
import type { HistoryItem } from "../lib/api";

const route = (id: number, labels: string[], pts: [number, number][]): HistoryItem => ({
  id,
  kind: "route",
  createdIso: "",
  label: labels.join(" ← "),
  payload: { request: { waypoints: pts.map(([lon, lat]) => ({ lon, lat })) }, labels },
});
const search = (id: number, name: string, lon: number, lat: number): HistoryItem => ({ id, kind: "search", createdIso: "", label: name, payload: { name, lon, lat } });

describe("recentDestinations", () => {
  it("takes route ends and searches, newest first, without repeats, home or my location", () => {
    const h = [
      route(1, ["המיקום שלי", "דיזנגוף 50"], [[34.77, 32.08], [34.774, 32.078]]),
      route(2, ["המיקום שלי", "דיזנגוף 50, תל אביב"], [[34.77, 32.08], [34.77401, 32.07801]]), // same spot
      search(3, "הבית", 34.8, 32.1), // home
      route(4, ["דיזנגוף 50", "המיקום שלי"], [[34.774, 32.078], [34.77, 32.08]]),
      search(5, "קניון עזריאלי", 34.79, 32.074),
      route(6, ["א", "הרצליה"], [[34.7, 32.0], [34.84, 32.16]]),
      search(7, "עוד אחד", 35.0, 31.7),
    ];
    const r = recentDestinations(h, [{ lon: 34.8, lat: 32.1 }], "המיקום שלי");
    expect(r.map((d) => d.label)).toEqual(["דיזנגוף 50", "קניון עזריאלי", "הרצליה"]);
  });
});
