import { useMemo, useRef, useState } from "react";
import { Check, Copy, X } from "lucide-react";
import { Icon } from "./Icon";
import { makeMapLink, useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { copyText } from "../lib/format";
import { encodeQr, qrPath } from "../lib/qr";
import { useDialog } from "../lib/useDialog";
import type { ShareTarget } from "../lib/store/types";

interface Links {
  title: string;
  /** opens in MapsOffline */
  app: string;
  /** opens in any phone map app (geo: URI, RFC 5870) or Google Maps for a route */
  phone: string;
  /** Google Maps / OpenStreetMap links for people without the app */
  web: { label: string; url: string }[];
}

function linksFor(target: ShareTarget, zoom: number): Links | null {
  if (target.kind === "place") {
    const { lon, lat, label } = target;
    const ll = `${lat.toFixed(6)},${lon.toFixed(6)}`;
    return {
      title: label,
      app: makeMapLink(lon, lat, Math.max(zoom, 15), label),
      phone: `geo:${ll}?q=${ll}(${encodeURIComponent(label)})`,
      web: [
        { label: "Google Maps", url: `https://www.google.com/maps/search/?api=1&query=${ll}` },
        { label: "OpenStreetMap", url: `https://www.openstreetmap.org/?mlat=${lat.toFixed(6)}&mlon=${lon.toFixed(6)}#map=17/${lat.toFixed(5)}/${lon.toFixed(5)}` },
      ],
    };
  }
  const s = useStore.getState();
  const route = s.routes.find((r) => r.id === target.routeId);
  const pts = s.waypoints.filter((p): p is NonNullable<typeof p> => !!p);
  if (!route || pts.length < 2) return null;
  const ll = (p: { lon: number; lat: number }) => `${p.lat.toFixed(6)},${p.lon.toFixed(6)}`;
  const travel = route.mode === "transit" ? "transit" : route.mode === "foot" || route.mode === "wheelchair" ? "walking" : route.mode === "bike" ? "bicycling" : "driving";
  const via = pts.slice(1, -1).map(ll).join("|");
  const google = `https://www.google.com/maps/dir/?api=1&origin=${ll(pts[0])}&destination=${ll(pts[pts.length - 1])}&travelmode=${travel}${via ? `&waypoints=${encodeURIComponent(via)}` : ""}`;
  const osmEngine = travel === "walking" ? "fossgis_osrm_foot" : travel === "bicycling" ? "fossgis_osrm_bike" : "fossgis_osrm_car";
  const osm = `https://www.openstreetmap.org/directions?engine=${osmEngine}&route=${encodeURIComponent(`${ll(pts[0])};${ll(pts[pts.length - 1])}`)}`;
  const title = `${pts[0].label} – ${pts[pts.length - 1].label}`;
  const dest = pts[pts.length - 1];
  return {
    title,
    app: makeMapLink(dest.lon, dest.lat, 15, dest.label),
    phone: google,
    web: [
      { label: "Google Maps", url: google },
      ...(travel !== "transit" ? [{ label: "OpenStreetMap", url: osm }] : []),
    ],
  };
}

function CopyRow({ label, value }: { label: string; value: string }) {
  const t = useT();
  const [done, setDone] = useState(false);
  return (
    <div className="flex flex-col gap-1">
      <span className="text-xs text-muted">{label}</span>
      <div className="flex items-center gap-1.5">
        <input readOnly className="input h-9 flex-1 min-w-0 text-xs selectable" dir="ltr" value={value} onFocus={(e) => e.currentTarget.select()} aria-label={label} />
        <button
          className="btn btn-sm shrink-0"
          onClick={async () => {
            if (await copyText(value)) {
              setDone(true);
              window.setTimeout(() => setDone(false), 1500);
            }
          }}
          aria-label={`${t("copy")}: ${label}`}
        >
          <Icon as={done ? Check : Copy} size={14} />
          {done ? t("copied") : t("copy")}
        </button>
      </div>
    </div>
  );
}

function Dialog({ target }: { target: ShareTarget }) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const zoom = useStore((s) => s.mapZoom);
  const close = () => useStore.getState().setShare(null);
  useDialog(ref, close);
  const links = useMemo(() => linksFor(target, zoom), [target, zoom]);
  const qr = useMemo(() => {
    if (!links) return null;
    try {
      return encodeQr(links.phone);
    } catch {
      return null;
    }
  }, [links]);
  const n = qr ? qr.size + 8 : 0;
  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center dialog-backdrop no-print" onClick={close}>
      <div ref={ref} role="dialog" aria-modal="true" aria-labelledby="share-title" className="dialog w-[420px] max-w-[calc(100%-32px)] p-5 flex flex-col gap-4 fade-in" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between gap-2">
          <h2 id="share-title" className="text-xl font-semibold m-0 truncate-dir">
            {t("shareTitle")}
          </h2>
          <button className="btn btn-ghost btn-icon btn-sm text-muted shrink-0" onClick={close} aria-label={t("close")}>
            <Icon as={X} />
          </button>
        </div>
        {!links ? (
          <p className="m-0 text-muted">{t("shareNothing")}</p>
        ) : (
          <>
            <div className="font-medium truncate-dir -mt-2">{links.title}</div>
            {qr && (
              <figure className="m-0 flex flex-col items-center gap-2">
                {/* always dark on white: phones read it in either theme */}
                <svg viewBox={`0 0 ${n} ${n}`} className="w-48 h-48 rounded-[var(--radius-sm)] bg-white" shapeRendering="crispEdges" role="img" aria-label={t("shareQrLabel")}>
                  <path d={qrPath(qr)} fill="#000" />
                </svg>
                <figcaption className="text-xs text-muted text-center">{t("shareQrHint")}</figcaption>
              </figure>
            )}
            <CopyRow label={t("shareAppLink")} value={links.app} />
            {links.web.map((w) => (
              <CopyRow key={w.label} label={w.label} value={w.url} />
            ))}
          </>
        )}
      </div>
    </div>
  );
}

/** Share a place or the selected route: an app link, web links and a QR code for the phone. */
export function ShareDialog() {
  const target = useStore((s) => s.share);
  return target ? <Dialog target={target} /> : null;
}
