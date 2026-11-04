/** Map overview for the printout: MapView registers a function that renders the selected route
 *  north-up in the light style and returns it as an image (data URL), or null. */
import { useStore } from "./store";

/** Printed map scale: fitted to the route, or a fixed topographic scale (report ג-32). */
export type PrintScale = "fit" | 25000 | 50000;

export interface PrintOptions {
  scale: PrintScale;
  /** ITM kilometre grid over the map */
  grid: boolean;
}

export const DEFAULT_PRINT_OPTIONS: PrintOptions = { scale: "fit", grid: false };

let mapImageProvider: ((o: PrintOptions) => Promise<string | null>) | null = null;
export function setPrintMapProvider(fn: ((o: PrintOptions) => Promise<string | null>) | null): void {
  mapImageProvider = fn;
}

/** Listeners of PrintSheet: told the image (or null) and the options before the dialog opens. */
const imageListeners = new Set<(img: string | null, o: PrintOptions) => void>();
export function onPrintMapImage(fn: (img: string | null, o: PrintOptions) => void): () => void {
  imageListeners.add(fn);
  return () => imageListeners.delete(fn);
}

/** The print-options dialog (PrintSheet registers it): resolves with the choice, or null = cancel. */
let askOptions: ((current: PrintOptions) => Promise<PrintOptions | null>) | null = null;
export function setPrintOptionsAsker(fn: ((current: PrintOptions) => Promise<PrintOptions | null>) | null): void {
  askOptions = fn;
}

let lastOptions: PrintOptions = DEFAULT_PRINT_OPTIONS;
let printing = false;

/** Rendering a big map can take a while (cold tiles): wait this long before printing without it. */
const MAP_IMAGE_TIMEOUT_MS = 15_000;

/** "Preparing the map…" over the page while the overview renders (plain DOM: it must show even
 *  when React is busy). */
function showPreparing(): () => void {
  const el = document.createElement("div");
  el.className = "print-preparing no-print";
  el.setAttribute("role", "status");
  el.innerHTML = '<span class="print-spinner" aria-hidden="true"></span><span></span>';
  (el.lastChild as HTMLElement).textContent = useStore.getState().t("printPreparing");
  document.body.appendChild(el);
  return () => el.remove();
}

/** Print the current page in the light theme (dark colours waste ink and lose contrast on paper),
 *  with a map overview of the route when the map can provide one. Asks for the scale / grid first
 *  when a route is shown. */
export async function printPage(): Promise<void> {
  if (printing) return;
  printing = true;
  const root = document.documentElement;
  const theme = root.getAttribute("data-theme");
  const restore = () => {
    if (theme) root.setAttribute("data-theme", theme);
    window.removeEventListener("afterprint", restore);
    imageListeners.forEach((f) => f(null, lastOptions));
    printing = false;
  };
  let opts = lastOptions;
  const hasRoute = useStore.getState().routes.length > 0;
  if (hasRoute && askOptions) {
    const chosen = await askOptions(lastOptions);
    if (!chosen) {
      printing = false;
      return;
    }
    opts = lastOptions = chosen;
  }
  let img: string | null = null;
  if (mapImageProvider && hasRoute) {
    const hide = showPreparing();
    try {
      img = await Promise.race([mapImageProvider(opts), new Promise<null>((r) => window.setTimeout(() => r(null), MAP_IMAGE_TIMEOUT_MS))]);
    } catch {
      img = null;
    } finally {
      hide();
    }
  }
  imageListeners.forEach((f) => f(img, opts));
  window.addEventListener("afterprint", restore);
  root.setAttribute("data-theme", "light");
  // let the theme switch and the image paint before the print dialog snapshots the page
  window.setTimeout(() => window.print(), 80);
}
