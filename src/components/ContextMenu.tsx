import { useEffect, useRef } from "react";
import { Copy, ExternalLink, Flag, Info, MapPin, Navigation, Star } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../lib/api";
import { useStore, type ContextMenuState } from "../lib/store";
import { useT } from "../lib/i18n";
import { copyText, fmtCoords } from "../lib/format";

export function ContextMenu({ state }: { state: ContextMenuState }) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const close = () => useStore.getState().setContextMenu(null);
  const point = { lon: state.lon, lat: state.lat, label: fmtCoords(state.lon, state.lat) };

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close();
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, []);

  // keep inside viewport
  const style: React.CSSProperties = { left: state.x, top: state.y };
  if (typeof window !== "undefined") {
    if (state.x + 220 > window.innerWidth) style.left = state.x - 220;
    if (state.y + 260 > window.innerHeight) style.top = state.y - 260;
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
      icon: <Navigation size={16} strokeWidth={1.75} />,
      label: t("ctxFromHere"),
      run: async () => {
        const label = await resolveLabel();
        useStore.getState().setFrom({ ...point, label });
        useStore.getState().setPanel("route");
      },
    },
    {
      group: 0,
      icon: <Flag size={16} strokeWidth={1.75} />,
      label: t("ctxToHere"),
      run: async () => {
        const label = await resolveLabel();
        useStore.getState().setTo({ ...point, label });
        useStore.getState().setPanel("route");
      },
    },
    {
      group: 0,
      icon: <MapPin size={16} strokeWidth={1.75} />,
      label: t("ctxAddVia"),
      run: async () => {
        const label = await resolveLabel();
        useStore.getState().addVia({ ...point, label });
        useStore.getState().setPanel("route");
      },
    },
    {
      group: 1,
      icon: <Info size={16} strokeWidth={1.75} />,
      label: t("ctxWhatHere"),
      run: async () => {
        const s = useStore.getState();
        try {
          const r = await api.geocodeReverse(state.lon, state.lat);
          s.setMarker({ lonlat: [state.lon, state.lat], label: r ? `${r.name}${r.city && !r.name.includes(r.city) ? `, ${r.city}` : ""}` : t("unknownPlace") });
        } catch (e) {
          s.setMarker({ lonlat: [state.lon, state.lat], label: point.label });
          s.toastError(e);
        }
      },
    },
    {
      group: 2,
      icon: <Copy size={16} strokeWidth={1.75} />,
      label: t("ctxCopyCoords"),
      run: async () => {
        if (await copyText(point.label)) useStore.getState().toast("success", t("copied"));
      },
    },
    ...(online
      ? [
          {
            group: 2,
            icon: <ExternalLink size={16} strokeWidth={1.75} />,
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
      group: 2,
      icon: <Star size={16} strokeWidth={1.75} />,
      label: t("ctxAddFavorite"),
      run: async () => {
        const s = useStore.getState();
        const label = await resolveLabel();
        try {
          await api.favoritesAdd(label, "star", state.lon, state.lat, label);
          await s.loadFavorites();
          s.toast("success", `${t("addFavorite")}: ${label}`);
        } catch (e) {
          s.toastError(e);
        }
      },
    },
  ];

  return (
    <div
      ref={ref}
      role="menu"
      className="absolute z-30 popover rounded-[var(--radius-card)] p-1 w-[220px] fade-in no-print"
      style={style}
      onContextMenu={(e) => e.preventDefault()}
    >
      <div className="px-2.5 pt-1.5 pb-2 text-xs text-subtle num" dir="ltr" style={{ textAlign: "start" }}>
        {point.label}
      </div>
      {items.map((it, i) => (
        <div key={it.label}>
          {(i === 0 || items[i - 1].group !== it.group) && <div className="h-px bg-line my-1 -mx-1" />}
          <button
            role="menuitem"
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
