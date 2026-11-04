import { cloneElement, useEffect, useId, useRef, useState, type ReactElement } from "react";
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

  let style: React.CSSProperties | undefined;
  if (anchor) {
    const rtl = document.documentElement.dir === "rtl";
    const gap = 6;
    const s = side === "start" ? (rtl ? "right" : "left") : side === "end" ? (rtl ? "left" : "right") : side;
    if (s === "top") style = { left: anchor.left + anchor.width / 2, top: anchor.top - gap, transform: "translate(-50%, -100%)" };
    else if (s === "bottom") style = { left: anchor.left + anchor.width / 2, top: anchor.bottom + gap, transform: "translateX(-50%)" };
    else if (s === "left") style = { left: anchor.left - gap, top: anchor.top + anchor.height / 2, transform: "translate(-100%, -50%)" };
    else style = { left: anchor.right + gap, top: anchor.top + anchor.height / 2, transform: "translateY(-50%)" };
  }

  return (
    <>
      {child}
      {anchor &&
        createPortal(
          <div id={id} role="tooltip" className="tip fade-in" style={style}>
            {text}
            {shortcut && <kbd dir="ltr">{shortcut}</kbd>}
          </div>,
          document.body,
        )}
    </>
  );
}
