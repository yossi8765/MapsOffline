import { cloneElement, useEffect, useId, useLayoutEffect, useRef, useState, type ReactElement } from "react";
import { createPortal } from "react-dom";

interface Props {
  /** Tooltip text (also the element's accessible description). */
  text: string;
  /** Keyboard shortcut shown after the text ("Ctrl+K"). */
  shortcut?: string;
  /** Preferred side; flips when there is no room. */
  side?: "top" | "bottom" | "start" | "end";
  children: ReactElement<Record<string, unknown>>;
}

const DELAY = 500;

/**
 * WinUI-style tooltip: appears after half a second of hover or on keyboard focus, Caption text,
 * shadow 8, with the shortcut in a muted kbd. Replaces the browser's `title=` (slow, unstyled,
 * no shortcut). The child keeps its own aria-label; the tooltip text becomes aria-describedby.
 */
export function Tip({ text, shortcut, side = "bottom", children }: Props) {
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const id = useId();

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const show = (el: HTMLElement, now = false) => {
    window.clearTimeout(timer.current);
    const go = () => setAnchor(el.getBoundingClientRect());
    if (now) go();
    else timer.current = window.setTimeout(go, DELAY);
  };
  const hide = () => {
    window.clearTimeout(timer.current);
    setAnchor(null);
  };

  const props = children.props as Record<string, ((e: unknown) => void) | undefined>;
  const child = cloneElement(children, {
    "aria-describedby": anchor ? id : undefined,
    onMouseEnter: (e: React.MouseEvent<HTMLElement>) => {
      props.onMouseEnter?.(e);
      show(e.currentTarget);
    },
    onMouseLeave: (e: React.MouseEvent<HTMLElement>) => {
      props.onMouseLeave?.(e);
      hide();
    },
    onFocus: (e: React.FocusEvent<HTMLElement>) => {
      props.onFocus?.(e);
      if (e.currentTarget.matches(":focus-visible")) show(e.currentTarget, true);
    },
    onBlur: (e: React.FocusEvent<HTMLElement>) => {
      props.onBlur?.(e);
      hide();
    },
    onMouseDown: (e: React.MouseEvent<HTMLElement>) => {
      props.onMouseDown?.(e);
      hide();
    },
  });

  // Placed after measuring itself: on the preferred side when it fits, else the opposite one,
  // and always inside the window (a "!" near the bottom of a panel used to open below the
  // window's edge, its text cut off).
  const tipRef = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  useLayoutEffect(() => {
    const tip = tipRef.current;
    if (!anchor || !tip) {
      setPos(null);
      return;
    }
    const w = tip.offsetWidth;
    const h = tip.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const gap = 6;
    const m = 8;
    const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(v, Math.max(lo, hi)));
    const rtl = document.documentElement.dir === "rtl";
    const s = side === "start" ? (rtl ? "right" : "left") : side === "end" ? (rtl ? "left" : "right") : side;
    let left: number;
    let top: number;
    if (s === "top" || s === "bottom") {
      const below = anchor.bottom + gap;
      const above = anchor.top - gap - h;
      const fitsBelow = below + h <= vh - m;
      const fitsAbove = above >= m;
      top = s === "bottom" ? (fitsBelow || !fitsAbove ? below : above) : fitsAbove || !fitsBelow ? above : below;
      left = anchor.left + anchor.width / 2 - w / 2;
    } else {
      const toLeft = anchor.left - gap - w;
      const toRight = anchor.right + gap;
      left = s === "left" ? (toLeft >= m || toRight + w > vw - m ? toLeft : toRight) : toRight + w <= vw - m || toLeft < m ? toRight : toLeft;
      top = anchor.top + anchor.height / 2 - h / 2;
    }
    setPos({ left: clamp(left, m, vw - w - m), top: clamp(top, m, vh - h - m) });
  }, [anchor, side, text]);

  return (
    <>
      {child}
      {anchor &&
        createPortal(
          <div
            ref={tipRef}
            id={id}
            role="tooltip"
            className="tip fade-in"
            style={pos ? { left: pos.left, top: pos.top } : { left: 0, top: 0, visibility: "hidden" }}
          >
            {text}
            {shortcut && (
              <span className="tip-key">
                <kbd dir="ltr">{shortcut}</kbd>
              </span>
            )}
          </div>,
          document.body,
        )}
    </>
  );
}
