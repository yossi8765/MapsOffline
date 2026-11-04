import { useEffect, useRef } from "react";
import { listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Subscribe to a backend event while the component is mounted.
 *
 * `listen()` resolves asynchronously; when the component unmounts first (React StrictMode mounts
 * twice in dev) the plain `.then(u => un = u)` pattern never removes the first listener and every
 * event is handled twice. Here a `dead` flag unlistens as soon as the promise resolves.
 * The handler may change between renders without re-subscribing.
 */
export function useTauriEvent<T>(name: string, handler: (payload: T) => void, enabled = true): void {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    if (!enabled) return;
    return subscribe<T>(name, (e) => ref.current(e.payload));
  }, [name, enabled]);
}

/** The same race-free subscription outside React: returns a synchronous unsubscribe. */
export function subscribe<T>(name: string, cb: EventCallback<T>): () => void {
  let dead = false;
  let un: UnlistenFn | undefined;
  listen<T>(name, cb)
    .then((u) => {
      if (dead) u();
      else un = u;
    })
    .catch(() => {
      /* not running inside Tauri (tests, headless preview) */
    });
  return () => {
    dead = true;
    un?.();
  };
}

/** Wrap an `api.onX(cb)` style subscription (Promise<UnlistenFn>) the same way. */
export function subscribeWith(start: () => Promise<UnlistenFn>): () => void {
  let dead = false;
  let un: UnlistenFn | undefined;
  start()
    .then((u) => {
      if (dead) u();
      else un = u;
    })
    .catch(() => {});
  return () => {
    dead = true;
    un?.();
  };
}
