// @vitest-environment jsdom
// Component-level behaviour of the F1 stream: dialogs move / trap / restore focus, toasts merge
// and keep error details, route steps merge roundabouts, nearby prayers group by synagogue.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useStore } from "../lib/store";
import { ShortcutsDialog } from "./ShortcutsDialog";
import { mergeSteps, niceTicks } from "./RouteDetails";
import { groupNearby } from "./MinyanimCard";
import { matchingFavorites } from "./SearchBox";
import { filterActions } from "./CommandPalette";
import { filterHistory } from "./HistoryPanel";
import { batchToGeoJson } from "./BatchGeocodeDialog";
import { translate } from "../lib/i18n";
import type { Favorite, GeoResult, HistoryItem, NextMinyan, Step } from "../lib/api";

afterEach(cleanup);

describe("ShortcutsDialog", () => {
  beforeEach(() => useStore.setState({ shortcutsOpen: false }));
  it("takes the focus, closes on Escape and gives the focus back", () => {
    render(
      <>
        <button>opener</button>
        <ShortcutsDialog />
      </>,
    );
    const opener = screen.getByText("opener");
    opener.focus();
    act(() => useStore.getState().setShortcutsOpen(true));
    const dialog = screen.getByRole("dialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.contains(document.activeElement)).toBe(true);
    act(() => {
      fireEvent.keyDown(document, { key: "Escape" });
    });
    expect(useStore.getState().shortcutsOpen).toBe(false);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });
});

describe("toasts", () => {
  beforeEach(() => useStore.setState({ toasts: [] }));
  it("an error toast carries the translated title and the developer details, and stays", () => {
    useStore.getState().toastError({ code: "NETWORK", message: "error sending request for url (https://x)" });
    const [t] = useStore.getState().toasts;
    expect(t.kind).toBe("error");
    expect(t.text).toBe(translate("he", "err_NETWORK"));
    expect(t.details).toContain("error sending request");
  });
  it("the same message twice is one toast with a count", () => {
    useStore.getState().toast("info", "x");
    useStore.getState().toast("info", "x");
    const list = useStore.getState().toasts;
    expect(list).toHaveLength(1);
    expect(list[0].count).toBe(2);
  });
});

const step = (maneuver: Step["maneuver"], distanceM: number, extra: Partial<Step> = {}): Step => ({ instruction: maneuver, maneuver, distanceM, durationSec: 1, geometryStart: 0, ...extra });

describe("route steps", () => {
  it("a roundabout entry and its exit are one line with both distances", () => {
    const shown = mergeSteps([step("depart", 100), step("roundabout", 60, { street: "כביש 45" }), step("exit-roundabout", 652), step("arrive", 0)]);
    expect(shown.map((s) => s.step.maneuver)).toEqual(["depart", "roundabout", "arrive"]);
    expect(shown[1]).toMatchObject({ index: 1, count: 2, distanceM: 712 });
  });
  it("axis ticks are round numbers", () => {
    expect(niceTicks(0, 63, 4)).toEqual([0, 20, 40, 60]);
    expect(niceTicks(397, 1168, 3)).toEqual([500, 750, 1000]);
  });
});

const nm = (mosadId: string, time: string, inMinutes: number, name = mosadId): NextMinyan => ({ mosadId, name, lon: 34.9, lat: 31.9, distanceM: 120, prayer: "מנחה", time, inMinutes });

describe("nearby prayers", () => {
  it("group by synagogue, soonest first, duplicates once", () => {
    const g = groupNearby([nm("a", "15:00", 17), nm("b", "15:00", 17), nm("a", "15:40", 57), nm("a", "15:00", 17)]);
    expect(g.map((x) => x.key)).toEqual(["a", "b"]);
    expect(g[0].items.map((x) => x.time)).toEqual(["15:00", "15:40"]);
  });
});

const fav = (id: number, name: string, extra: Partial<Favorite> = {}): Favorite => ({ id, name, icon: "star", lon: 34.8, lat: 32, createdIso: "", kind: "address", ...extra });

describe("search and filters", () => {
  it("favorites matching the typed text come first, lines and routes excluded", () => {
    const favs = [fav(1, "בית"), fav(2, "עבודה", { address: "הרצל 12" }), fav(3, "קו 480", { kind: "line" }), fav(4, "בית ספר")];
    expect(matchingFavorites(favs, "בית").map((f) => f.id)).toEqual([1, 4]);
    expect(matchingFavorites(favs, "הרצל").map((f) => f.id)).toEqual([2]);
    expect(matchingFavorites(favs, "480")).toEqual([]);
  });
  it("history filter ignores final letters", () => {
    const h: HistoryItem[] = [{ id: 1, kind: "route", createdIso: "", label: "ירושלים ← שלום", payload: null }];
    expect(filterHistory(h, "route", "שלומ")).toHaveLength(1);
    expect(filterHistory(h, "search", "שלום")).toHaveLength(0);
  });
  it("the palette finds commands by their Hebrew label and English keywords", () => {
    const t = (k: Parameters<typeof translate>[1]) => translate("he", k);
    const s = useStore.getState();
    expect(filterActions("הגדר", t, s)[0].id).toBe("settings");
    expect(filterActions("gpx", t, s).map((a) => a.id)).toContain("import");
    // print only with a route on screen
    expect(filterActions("הדפס", t, { ...s, routes: [] }).map((a) => a.id)).not.toContain("print");
  });
});

describe("address list", () => {
  it("found rows become GeoJSON points with every column", () => {
    const r: GeoResult = { id: "1", name: "הרצל 12", kind: "address", precision: "exact", lon: 34.77, lat: 32.07, street: "הרצל", houseNumber: "12", city: "תל אביב" };
    const gj = JSON.parse(batchToGeoJson(["שם", "כתובת"], [["א", "הרצל 12"], ["ב", "לא קיים"]], [r, null]));
    expect(gj.features).toHaveLength(1);
    expect(gj.features[0].geometry.coordinates).toEqual([34.77, 32.07]);
    expect(gj.features[0].properties).toMatchObject({ שם: "א", כתובת: "הרצל 12", precision: "exact" });
  });
});
