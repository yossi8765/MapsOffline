// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { useRef } from "react";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { useDialog, useMenuKeys } from "./useDialog";

afterEach(cleanup);

function Dialog({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useDialog(ref, onClose);
  return (
    <div ref={ref} role="dialog" aria-modal="true">
      <button>first</button>
      <input aria-label="middle" />
      <button>last</button>
    </div>
  );
}

function Host({ open, onClose }: { open: boolean; onClose: () => void }) {
  return (
    <>
      <button>opener</button>
      {open && <Dialog onClose={onClose} />}
    </>
  );
}

describe("useDialog", () => {
  it("moves focus in, traps Tab, closes on Escape, restores focus", () => {
    const onClose = vi.fn();
    const { getByText, rerender } = render(<Host open={false} onClose={onClose} />);
    const opener = getByText("opener");
    opener.focus();
    rerender(<Host open onClose={onClose} />);
    expect(document.activeElement).toBe(getByText("first"));

    // Tab from the last element wraps to the first, Shift+Tab from the first to the last
    getByText("last").focus();
    fireEvent.keyDown(document.activeElement!, { key: "Tab" });
    expect(document.activeElement).toBe(getByText("first"));
    fireEvent.keyDown(document.activeElement!, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(getByText("last"));

    // Escape closes and does not reach window-level handlers
    const windowEsc = vi.fn();
    window.addEventListener("keydown", windowEsc);
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    window.removeEventListener("keydown", windowEsc);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(windowEsc).not.toHaveBeenCalled();

    rerender(<Host open={false} onClose={onClose} />);
    expect(document.activeElement).toBe(opener);
  });
});

function Menu() {
  const ref = useRef<HTMLDivElement>(null);
  useMenuKeys(ref);
  return (
    <div ref={ref} role="menu">
      <button role="menuitem">one</button>
      <button role="menuitem">two</button>
      <button role="menuitem">three</button>
    </div>
  );
}

describe("useMenuKeys", () => {
  it("arrows, Home and End move the focus with roving tabindex", () => {
    const { getByText } = render(<Menu />);
    expect(document.activeElement).toBe(getByText("one"));
    act(() => void fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" }));
    expect(document.activeElement).toBe(getByText("two"));
    expect(getByText("two").getAttribute("tabindex")).toBe("0");
    expect(getByText("one").getAttribute("tabindex")).toBe("-1");
    fireEvent.keyDown(document.activeElement!, { key: "End" });
    expect(document.activeElement).toBe(getByText("three"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(getByText("one"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    expect(document.activeElement).toBe(getByText("three"));
    fireEvent.keyDown(document.activeElement!, { key: "Home" });
    expect(document.activeElement).toBe(getByText("one"));
  });
});
