import { api, errorText, type AppUpdate } from "../api";
import type { SliceCreator } from "./types";

/** In-app program updates (signed GitLab releases, src-tauri/src/app_update.rs). */
export interface AppUpdateSlice {
  appUpdate: AppUpdate | null;
  appUpdateBusy: "check" | "download" | null;
  appUpdateProgress: { done: number; total: number; message: string } | null;

  /** Ask GitLab for a newer signed release; `quiet` = startup check (toast only when newer). */
  checkAppUpdate: (quiet?: boolean) => Promise<void>;
  /** Download, verify, start the installer and quit. */
  installAppUpdate: () => Promise<void>;
}

export const createAppUpdateSlice: SliceCreator<AppUpdateSlice> = (set, get) => ({
  appUpdate: null,
  appUpdateBusy: null,
  appUpdateProgress: null,

  checkAppUpdate: async (quiet = false) => {
    const t = get().t;
    set({ appUpdateBusy: "check" });
    try {
      const u = await api.appUpdateCheck();
      set({ appUpdate: u });
      if (u.available && quiet) get().toast("info", `${t("updateAvailableToast")} ${u.latest}`);
    } catch (e) {
      const msg = errorText(e);
      set({ appUpdate: { current: get().appVersion ?? "", latest: "", available: false, pageUrl: "", notes: "", checkedIso: new Date().toISOString(), error: msg } });
      if (!quiet) get().toastError(e);
    } finally {
      set({ appUpdateBusy: null });
    }
  },
  installAppUpdate: async () => {
    const t = get().t;
    set({ appUpdateBusy: "download", appUpdateProgress: { done: 0, total: 0, message: "" } });
    try {
      const path = await api.appUpdateDownload();
      set({ appUpdateProgress: { done: 1, total: 1, message: t("updateStarting") } });
      await api.appUpdateInstall(path);
    } catch (e) {
      get().toastError(e);
      set({ appUpdateBusy: null, appUpdateProgress: null });
    }
  },
});
