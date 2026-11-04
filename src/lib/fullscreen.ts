// F11 full screen through the Tauri window (the WebView's own F11 does nothing in Tauri).
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Last known state, so Escape can decide synchronously whether it has something to undo. */
let full = false;

export function isFullscreenNow(): boolean {
  return full;
}

export async function toggleFullscreen(): Promise<void> {
  try {
    const w = getCurrentWindow();
    full = !(await w.isFullscreen());
    await w.setFullscreen(full);
  } catch {
    /* outside Tauri (tests, browser preview) */
  }
}

export async function exitFullscreen(): Promise<void> {
  if (!full) return;
  try {
    await getCurrentWindow().setFullscreen(false);
  } catch {
    /* ignore */
  }
  full = false;
}
