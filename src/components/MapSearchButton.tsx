import { Search } from "lucide-react";
import { Icon } from "./Icon";
import { Tip } from "./Tip";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";

/** The panel folded away: a round search button at the bottom of the map opens the address search. */
export function MapSearchButton() {
  const t = useT();
  const collapsed = useStore((s) => s.sidebarCollapsed);
  const navigating = useStore((s) => !!s.navigation);
  if (!collapsed || navigating) return null;
  return (
    <div className="absolute left-1/2 -translate-x-1/2 bottom-4 z-30 no-print fade-in" data-map-search>
      <Tip text={t("searchAddress")}>
        <button
          className="w-14 h-14 rounded-full bg-[var(--accent-solid)] text-[var(--accent-fg)] shadow-lg flex items-center justify-center hover:opacity-90 active:scale-95 transition"
          aria-label={t("searchAddress")}
          onClick={() => useStore.getState().focusSearch()}
        >
          <Icon as={Search} size={26} />
        </button>
      </Tip>
    </div>
  );
}
