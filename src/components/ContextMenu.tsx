import { useEffect, useRef } from "react";
import { Ban, Copy, ExternalLink, FileUp, Flag, ImageDown, Info, MapPin, Navigation, Radar, Ruler, Star, Timer } from "lucide-react";
import { Icon } from "./Icon";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../lib/api";
import { findFavorite, useStore, type ContextMenuState } from "../lib/store";
import { useT } from "../lib/i18n";
import { copyText, fmtCoords } from "../lib/format";
import { formatCoord } from "../lib/coords";
import { useMenuKeys } from "../lib/useDialog";
import type { IsochroneMode } from "../lib/store/map";

export function ContextMenu({ state }: { state: ContextMenuState }) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const close = () => useStore.getState().setContextMenu(null);
  const point = { lon: state.lon, lat: state.lat, label: fmtCoords(state.lon, state.lat) };
  // the coordinates as shown / copied: in the format chosen in Settings (decimal, DMS, ITM, UTM)
  const coordFormat = useStore((s) => s.settings.coordFormat ?? "dd");
  const coordText = formatCoord(state.lon, state.lat, coordFormat);
  // arrow keys / Home / End move between the items (report ב-3)
  useMenuKeys(ref);
  const routeMode = useStore((s) => s.mode);
  const isoMode: IsochroneMode = routeMode === "foot" || routeMode === "bike" || routeMode === "transit" || routeMode === "wheelchair" ? routeMode : "car";

  useEffect(() => {
    // the focus goes back where it was (map canvas, a button) when the menu closes
    const previous = document.activeElement as HTMLElement | null;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close();
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
      if (previous && typeof previous.focus === "function" && document.contains(previous)) previous.focus();
    };
  }, []);

  // keep inside viewport
  const style: React.CSSProperties = { left: state.x, top: state.y };
  if (typeof window !== "undefined") {
    if (state.x + 230 > window.innerWidth) style.left = state.x - 230;
    if (state.y + 470 > window.innerHeight) style.top = Math.max(8, window.innerHeight - 470);
  }

  const resolveLabel = async () => {
    try {
      const r = await api.geocodeReverse(state.lon, state.lat);
      return r?.name ?? point.label;
    } catch {
      return point.label;
    }
  };

  const online = useStore((s) => s.online);
  const items = [
    {
      group: 0,
      icon: <Icon as={Navigation} size={16} />,
      label: t("ctxFromHere"),
      run: async () => {
        const label = await resolveLabel();
        useStore.getState().setFrom({ ...point, label });
        useStore.getState().setPanel("route");
      },
    },
    {
      group: 0,
      icon: <Icon as={Flag} size={16} />,
      label: t("ctxToHere"),
      run: async () => {
        const label = await resolveLabel();
        useStore.getState().setTo({ ...point, label });
        useStore.getState().setPanel("route");
      },
    },
    {
      group: 0,
      icon: <Icon as={MapPin} size={16} />,
      label: t("ctxAddVia"),
      run: async () => {
        const label = await resolveLabel();
        useStore.getState().addVia({ ...point, label });
        useStore.getState().setPanel("route");
      },
    },
    {
      group: 1,
      icon: <Icon as={Info} size={16} />,
      label: t("ctxWhatHere"),
      run: async () => {
        // the place card of what is here (nearest address / place), at the clicked point
        const s = useStore.getState();
        try {
          const r = await api.geocodeReverse(state.lon, state.lat);
          const label = r ? `${r.name}${r.city && !r.name.includes(r.city) ? `, ${r.city}` : ""}` : t("unknownPlace");
          s.pickPlace({ lon: state.lon, lat: state.lat, label, result: r ? { ...r, lon: state.lon, lat: state.lat } : undefined }, Math.max(s.mapZoom, 16));
        } catch (e) {
          s.pickPlace({ lon: state.lon, lat: state.lat, label: point.label });
          s.toastError(e);
        }
      },
    },
    {
      group: 1,
      icon: <Icon as={Radar} size={16} />,
      label: t("ctxNearby"),
      run: async () => useStore.getState().showNearby(state.lon, state.lat),
    },
    {
      group: 2,
      icon: <Icon as={Ruler} size={16} />,
      label: t("ctxMeasure"),
      run: async () => useStore.getState().startMeasure([state.lon, state.lat]),
    },
    {
      group: 2,
      icon: <Icon as={Timer} size={16} />,
      label: t("ctxIsochrone"),
      run: async () => useStore.getState().showIsochrone([state.lon, state.lat], isoMode),
    },
    {
      group: 2,
      icon: <Icon as={Ban} size={16} />,
      label: t("ctxAvoidArea"),
      run: async () => {
        const s = useStore.getState();
        s.startDrawing("avoid");
        s.addDrawPoint([state.lon, state.lat]);
      },
    },
    {
      group: 2,
      icon: <Icon as={ImageDown} size={16} />,
      label: t("ctxSharpArea"),
      run: async () => useStore.getState().startSatAreaAt([state.lon, state.lat]),
    },
    {
      group: 2,
      icon: <Icon as={FileUp} size={16} />,
      label: t("ctxImport"),
      run: async () => useStore.getState().openImport(),
    },
    {
      group: 3,
      icon: <Icon as={Copy} size={16} />,
      label: t("ctxCopyCoords"),
      run: async () => {
        if (await copyText(coordText)) useStore.getState().toast("success", t("copied"));
      },
    },
    ...(online
      ? [
          {
            group: 3,
            icon: <Icon as={ExternalLink} size={16} />,
            label: t("ctxOpenGoogle"),
      run: async () => {
        // Google's universal "search" URL centres the map on the exact coordinate with a pin
        const q = `${state.lat.toFixed(6)},${state.lon.toFixed(6)}`;
        try {
          await openUrl(`https://www.google.com/maps/search/?api=1&query=${q}`);
        } catch (e) {
          useStore.getState().toastError(e);
        }
      },
          },
        ]
      : []),
    {
      group: 3,
      icon: <Icon as={Star} size={16} />,
      label: t("ctxAddFavorite"),
      run: async () => {
        // through toggleFavorite: no "added" toast for a spot that is already saved (5.61)
        const s = useStore.getState();
        const existing = findFavorite(s.favorites, state.lon, state.lat);
        if (existing) {
          s.toast("info", `${t("savedPlace")}: ${existing.name}`);
          return;
        }
        const label = await resolveLabel();
        await s.toggleFavorite({ lon: state.lon, lat: state.lat, label });
      },
    },
  ];

  return (
    <div
      ref={ref}
      role="menu"
      aria-label={t("ctxMenuLabel")}
      className="absolute z-30 popover rounded-[var(--radius-card)] p-1 w-[230px] fade-in no-print"
      style={style}
      onContextMenu={(e) => e.preventDefault()}
    >
      <div className="px-2.5 pt-1.5 pb-2 text-xs text-subtle num selectable">
        <bdi dir="ltr">{coordText}</bdi>
      </div>
      {items.map((it, i) => (
        <div key={it.label}>
          {(i === 0 || items[i - 1].group !== it.group) && <div className="h-px bg-line my-1 -mx-1" />}
          <button
            role="menuitem"
            tabIndex={-1}
            className="w-full flex items-center gap-2.5 px-2.5 h-8 rounded text-start text-sm hover:bg-hover focus-visible:bg-hover transition-colors duration-100"
            onClick={() => {
              close();
              void it.run();
            }}
          >
            <span className="text-muted">{it.icon}</span>
            <span className="truncate">{it.label}</span>
          </button>
        </div>
      ))}
    </div>
  );
}
