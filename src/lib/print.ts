/** Print the current page in the light theme (dark colours waste ink and lose contrast on paper). */
export function printPage(): void {
  const root = document.documentElement;
  const theme = root.getAttribute("data-theme");
  const restore = () => {
    if (theme) root.setAttribute("data-theme", theme);
    window.removeEventListener("afterprint", restore);
  };
  window.addEventListener("afterprint", restore);
  root.setAttribute("data-theme", "light");
  // let the theme switch paint before the print dialog snapshots the page
  window.setTimeout(() => window.print(), 50);
}
