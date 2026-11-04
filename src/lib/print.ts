/** Map overview for the printout: MapView registers a function that renders the selected route
 *  north-up in the light style and returns it as an image (data URL), or null. */
let mapImageProvider: (() => Promise<string | null>) | null = null;
export function setPrintMapProvider(fn: (() => Promise<string | null>) | null): void {
  mapImageProvider = fn;
}

/** Listeners of PrintSheet: told the image (or null) before the dialog opens. */
const imageListeners = new Set<(img: string | null) => void>();
export function onPrintMapImage(fn: (img: string | null) => void): () => void {
  imageListeners.add(fn);
  return () => imageListeners.delete(fn);
}

let printing = false;

/** Print the current page in the light theme (dark colours waste ink and lose contrast on paper),
 *  with a map overview of the route when the map can provide one (at most 3 s). */
export async function printPage(): Promise<void> {
  if (printing) return;
  printing = true;
  const root = document.documentElement;
  const theme = root.getAttribute("data-theme");
  const restore = () => {
    if (theme) root.setAttribute("data-theme", theme);
    window.removeEventListener("afterprint", restore);
    imageListeners.forEach((f) => f(null));
    printing = false;
  };
  let img: string | null = null;
  if (mapImageProvider) {
    try {
      img = await Promise.race([mapImageProvider(), new Promise<null>((r) => window.setTimeout(() => r(null), 3000))]);
    } catch {
      img = null;
    }
  }
  imageListeners.forEach((f) => f(img));
  window.addEventListener("afterprint", restore);
  root.setAttribute("data-theme", "light");
  // let the theme switch and the image paint before the print dialog snapshots the page
  window.setTimeout(() => window.print(), 80);
}
