import { useEffect, useState } from "react";
import { Navigation } from "lucide-react";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";

/**
 * An on-screen keyboard is open: the visible height dropped well below the tallest seen at this
 * width (focus alone says nothing: after a search the field keeps it with the keyboard closed).
 */
function useKeyboardOpen(): boolean {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const vv = window.visualViewport;
    const tallest = new Map<number, number>();
    const check = () => {
      const h = vv?.height ?? window.innerHeight;
      const w = Math.round(window.innerWidth);
      const max = Math.max(tallest.get(w) ?? 0, h);
      tallest.set(w, max);
      setOpen(h < max * 0.8);
    };
    check();
    (vv ?? window).addEventListener("resize", check);
    return () => (vv ?? window).removeEventListener("resize", check);
  }, []);
  return open;
}

/**
 * Driving routes found: a floating blue square in the bottom left corner of the screen; one tap
 * starts navigating the fastest of them (no need to open the route card first).
 */
export function QuickNavButton() {
  const t = useT();
  const routes = useStore((s) => s.routes);
  const navigating = useStore((s) => !!s.navigation);
  const keyboard = useKeyboardOpen();
  const drive = routes.length > 0 && (routes[0].mode === "car" || routes[0].mode === "truck");
  const visible = drive && !navigating && !keyboard;
  // the map's scale / attribution share the corner: they move up while the square is there
  useEffect(() => {
    document.documentElement.classList.toggle("quick-nav-shown", visible);
    return () => document.documentElement.classList.remove("quick-nav-shown");
  }, [visible]);
  if (!visible) return null;
  const fastest = routes.reduce((a, b) => (b.durationSec < a.durationSec ? b : a));
  return (
    <div className="fixed left-3 z-40 no-print fade-in" style={{ bottom: "calc(12px + env(safe-area-inset-bottom, 0px))" }} data-quick-nav>
      <Tip text={t("navQuickStart")}>
        <button
          className="w-14 h-14 rounded-[var(--radius)] bg-[var(--accent-solid)] text-[var(--accent-fg)] shadow-lg flex items-center justify-center hover:opacity-90 active:scale-95 transition"
          aria-label={t("navQuickStart")}
          onClick={() => void useStore.getState().startNavigation(fastest.id)}
        >
          <Icon as={Navigation} size={26} className="fill-current" />
        </button>
      </Tip>
    </div>
  );
}
