import { api, errorText, type AppUpdate } from "../api";
import type { SliceCreator } from "./types";

/** In-app program updates (signed GitHub releases, GitLab fallback, src-tauri/src/app_update.rs). */
export interface AppUpdateSlice {
  appUpdate: AppUpdate | null;
  appUpdateBusy: "check" | "download" | null;
  appUpdateProgress: { done: number; total: number; message: string } | null;

  /** Ask GitHub (then GitLab) for a newer signed release; `quiet` = startup check: a newer version is offered
   *  with an "install now" button – nothing is installed without the user's go. */
  checkAppUpdate: (quiet?: boolean) => Promise<void>;
  /** Download, verify, start the installer (passive, update mode – data is never deleted) and quit;
   *  the installer opens the new version when it is done. */
  installAppUpdate: () => Promise<void>;
}

export const createAppUpdateSlice: SliceCreator<AppUpdateSlice> = (set, get) => {
  const offer = (version: string) =>
    get().toast("info", `${get().t("updateAvailableToast")} ${version}`, {
      duration: 0,
      action: { label: get().t("updateNow"), run: () => void get().installAppUpdate() },
    });

  return {
    appUpdate: null,
    appUpdateBusy: null,
    appUpdateProgress: null,

    checkAppUpdate: async (quiet = false) => {
      set({ appUpdateBusy: "check" });
      let found: AppUpdate | null = null;
      try {
        const u = await api.appUpdateCheck();
        set({ appUpdate: u });
        if (u.available) found = u;
      } catch (e) {
        const msg = errorText(e);
        set({ appUpdate: { current: get().appVersion ?? "", latest: "", available: false, pageUrl: "", notes: "", checkedIso: new Date().toISOString(), error: msg } });
        if (!quiet) get().toastError(e);
      } finally {
        set({ appUpdateBusy: null });
      }
      if (!found || !quiet) return;
      // never installs by itself: the message stays until the user presses "update now"
      // (or closes it); an update would cut a navigation or a data download short
      offer(found.latest);
    },

    installAppUpdate: async () => {
      if (get().appUpdateBusy === "download") return;
      const t = get().t;
      set({ appUpdateBusy: "download", appUpdateProgress: { done: 0, total: 0, message: "" } });
      try {
        const path = await api.appUpdateDownload();
        set({ appUpdateProgress: { done: 1, total: 1, message: t("updateStarting") } });
        await api.appUpdateInstall(path);
      } catch (e) {
        set({ appUpdateBusy: null, appUpdateProgress: null });
        get().toast("error", t("updateFailed"), {
          details: errorText(e),
          duration: 0,
          action: { label: t("updateRetry"), run: () => void get().installAppUpdate() },
        });
      }
    },
  };
};
