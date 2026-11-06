import { useEffect, useState } from "react";
import { Navigation } from "lucide-react";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";

/**
 * Driving routes found: a floating blue square at the bottom left of the screen; one tap starts
 * navigating the fastest of them (no need to open the route card first).
 */
export function QuickNavButton() {
  const t = useT();
  const routes = useStore((s) => s.routes);
  const navigating = useStore((s) => !!s.navigation);
  // typing (a phone keyboard shrinks the screen and would push the button over the fields)
  const [typing, setTyping] = useState(false);
  useEffect(() => {
    const check = () => {
      const a = document.activeElement;
      setTyping(!!a && (a.tagName === "INPUT" || a.tagName === "TEXTAREA"));
    };
    // focusout fires before the next element takes the focus: look once it has
    const later = () => window.setTimeout(check, 0);
    document.addEventListener("focusin", check);
    document.addEventListener("focusout", later);
    return () => {
      document.removeEventListener("focusin", check);
      document.removeEventListener("focusout", later);
    };
  }, []);
  if (typing || navigating || routes.length === 0) return null;
  if (routes[0].mode !== "car" && routes[0].mode !== "truck") return null;
  const fastest = routes.reduce((a, b) => (b.durationSec < a.durationSec ? b : a));
  return (
    <div className="fixed left-4 bottom-24 z-40 no-print fade-in" data-quick-nav>
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
