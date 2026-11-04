import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import App from "./App";
import { MapOnlyApp, isMapWindow } from "./components/MapOnlyApp";
import "./styles.css";
import { installPageZoomGuard } from "./lib/gestures";
import { useStore } from "./lib/store";

installPageZoomGuard();

/** mapsoffline:// and geo: links: the one that started the app, and those a second start hands
 *  over (single instance). The store parses and shows them. */
function openLink(url: string | null | undefined) {
  if (!url) return;
  const open = (useStore.getState() as { openLink?: (u: string) => void }).openLink;
  if (open) open(url);
  else console.warn("no link handler for", url);
}
void listen<string>("open-link", (e) => openLink(e.payload));
// give the store a moment to load settings before showing the link
setTimeout(() => {
  void invoke<string | null>("take_pending_link").then(openLink, () => undefined);
}, 1200);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isMapWindow() ? <MapOnlyApp /> : <App />}
  </React.StrictMode>,
);
