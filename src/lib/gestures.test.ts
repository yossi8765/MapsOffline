// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { isMouseWheel } from "./gestures";

function wheel(init: WheelEventInit): WheelEvent {
  return new WheelEvent("wheel", init);
}

describe("isMouseWheel", () => {
  it("line / page delta modes are a mouse", () => {
    expect(isMouseWheel(wheel({ deltaY: 3, deltaMode: WheelEvent.DOM_DELTA_LINE }))).toBe(true);
  });
  it("Windows notches (100 / 120 px) are a mouse", () => {
    expect(isMouseWheel(wheel({ deltaY: 100 }))).toBe(true);
    expect(isMouseWheel(wheel({ deltaY: -240 }))).toBe(true);
  });
  it("small or sideways pixel deltas are a touchpad", () => {
    expect(isMouseWheel(wheel({ deltaY: 12 }))).toBe(false);
    expect(isMouseWheel(wheel({ deltaY: 100, deltaX: 3 }))).toBe(false);
    expect(isMouseWheel(wheel({ deltaY: 57 }))).toBe(false);
  });
});
