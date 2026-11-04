import { useEffect, useRef, type RefObject } from "react";

const FOCUSABLE = [
  "a[href]",
  "area[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type=hidden])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "iframe",
  "[contenteditable]:not([contenteditable=false])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

/** Focusable elements inside `root`, in tab order (hidden ones skipped where layout is known). */
export function focusableIn(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => {
    if (el.hasAttribute("disabled") || el.getAttribute("aria-hidden") === "true") return false;
    // jsdom has no layout: offsetParent is always null there, so only trust it in a browser
    const hasLayout = typeof navigator !== "undefined" && !/jsdom/i.test(navigator.userAgent);
    return !hasLayout || el.offsetParent !== null || el.getClientRects().length > 0;
  });
}

export interface DialogOptions {
  /** Element to focus on open (default: the first focusable element, else the container). */
  initialFocus?: RefObject<HTMLElement | null>;
  /** Give the focus back to the element that had it before the dialog opened (default true). */
  restoreFocus?: boolean;
  /** Off: the hook does nothing (a dialog that is mounted but closed). Default true. */
  active?: boolean;
}

/**
 * Modal-dialog keyboard behaviour for the element in `ref`:
 * - on open moves the focus inside (`initialFocus` or the first focusable element);
 * - Tab / Shift+Tab cycle inside the dialog;
 * - Escape calls `onClose` (the event stops here, so App-level Escape handlers don't also fire);
 * - on close gives the focus back to the element that had it before.
 * Set `role="dialog"` and `aria-modal="true"` on the element yourself.
 */
export function useDialog(ref: RefObject<HTMLElement | null>, onClose: () => void, opts: DialogOptions = {}): void {
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  const { initialFocus, restoreFocus = true, active = true } = opts;

  useEffect(() => {
    if (!active) return;
    const root = ref.current;
    if (!root) return;
    const previous = document.activeElement as HTMLElement | null;

    const target = initialFocus?.current ?? focusableIn(root)[0] ?? root;
    if (target === root && !root.hasAttribute("tabindex")) root.setAttribute("tabindex", "-1");
    target.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        closeRef.current();
        return;
      }
      if (e.key !== "Tab") return;
      const items = focusableIn(root);
      if (!items.length) {
        e.preventDefault();
        root.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const cur = document.activeElement as HTMLElement | null;
      const inside = !!cur && root.contains(cur);
      if (e.shiftKey && (cur === first || !inside)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (cur === last || !inside)) {
        e.preventDefault();
        first.focus();
      }
    };
    // capture on the document: the dialog sees Escape before window-level shortcut handlers
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      if (restoreFocus && previous && typeof previous.focus === "function" && document.contains(previous)) previous.focus();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);
}

/**
 * Arrow-key navigation for a `role="menu"` element: ArrowDown / ArrowUp move between its
 * `[role=menuitem]` children (wrapping), Home / End jump to the first / last. Roving tabindex:
 * the focused item has tabindex 0, the others -1. Focuses the first item on mount when
 * `focusFirst` (default true).
 */
export function useMenuKeys(ref: RefObject<HTMLElement | null>, focusFirst = true): void {
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    const items = () => Array.from(root.querySelectorAll<HTMLElement>("[role=menuitem]:not([aria-disabled=true]):not([disabled])"));
    const focusAt = (i: number) => {
      const list = items();
      if (!list.length) return;
      const idx = ((i % list.length) + list.length) % list.length;
      list.forEach((el, j) => el.setAttribute("tabindex", j === idx ? "0" : "-1"));
      list[idx].focus();
    };
    const all = items();
    all.forEach((el, j) => el.setAttribute("tabindex", j === 0 ? "0" : "-1"));
    if (focusFirst && all.length) all[0].focus();

    const onKey = (e: KeyboardEvent) => {
      const list = items();
      const cur = list.indexOf(document.activeElement as HTMLElement);
      if (e.key === "ArrowDown") focusAt(cur + 1);
      else if (e.key === "ArrowUp") focusAt(cur < 0 ? list.length - 1 : cur - 1);
      else if (e.key === "Home") focusAt(0);
      else if (e.key === "End") focusAt(list.length - 1);
      else return;
      e.preventDefault();
    };
    root.addEventListener("keydown", onKey);
    return () => root.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}
