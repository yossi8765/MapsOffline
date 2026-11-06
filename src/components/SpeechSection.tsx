import { useCallback, useEffect, useRef, useState } from "react";
import { Download, Mic, Trash2, X } from "lucide-react";
import { Icon } from "./Icon";
import { InfoTip } from "./InfoTip";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { askConfirm, fmtBytes } from "../lib/format";
import { useTauriEvent } from "../lib/useTauriEvent";
import { speechCancel, speechDelete, speechInstall, speechStatus, type SpeechProgress, type SpeechStatus } from "../lib/speech";

/**
 * Data panel: the offline speech-recognition model for voice search (ivrit-ai Whisper, ~550 MB).
 * Download with progress, size when installed, delete.
 */
export function SpeechSection() {
  const t = useT();
  const [st, setSt] = useState<SpeechStatus | null>(null);
  const [prog, setProg] = useState<SpeechProgress | null>(null);

  const refresh = useCallback(() => {
    void speechStatus().then(setSt, () => undefined);
  }, []);
  useEffect(refresh, [refresh]);
  useTauriEvent<SpeechProgress>("speech-progress", (p) => {
    setProg(p);
    if (p.phase === "done" || p.phase === "failed" || p.phase === "cancelled") refresh();
  });

  // a download the user cancelled ends with an error from the backend: not worth a message
  const cancelled = useRef(false);
  const cancel = () => {
    cancelled.current = true;
    void speechCancel();
  };
  const install = async () => {
    cancelled.current = false;
    setProg({ phase: "queued", done: 0, total: st?.downloadBytes ?? 0, message: "" });
    try {
      await speechInstall();
    } catch (e) {
      if (!cancelled.current) useStore.getState().toastError(e);
    } finally {
      setProg(null);
      refresh();
    }
  };
  const remove = async () => {
    if (!(await askConfirm(t("speechDeleteConfirm"), t("delete"), t("cancel"), t("appName")))) return;
    try {
      await speechDelete();
    } catch (e) {
      useStore.getState().toastError(e);
    }
    refresh();
  };

  if (!st) return null;
  const busy = st.busy || !!prog;
  const pct = prog && prog.total > 0 ? Math.min(100, Math.round((prog.done / prog.total) * 100)) : null;
  return (
    <div className="flex flex-col gap-2">
      <h2 className="section-label m-0 inline-flex items-center gap-1.5">
        {t("speechSection")}
        <InfoTip text={t("speechSectionHint")} />
      </h2>
      <div className="group-list">
        <div className="px-3 py-2.5 flex items-center gap-3">
          <Icon as={Mic} className="text-muted shrink-0" />
          <div className="flex-1 min-w-0 flex flex-col gap-0.5">
            <span className="font-medium">{t("speechModel")}</span>
            <span className="text-xs text-muted num">
              {busy
                ? `${t("speechDownloading")}${pct != null ? ` ${pct}%` : ""}`
                : st.installed
                  ? `${t("speechInstalled")} · ${fmtBytes(st.sizeBytes)}`
                  : `${t("speechNotInstalled")} · ${fmtBytes(st.downloadBytes)}`}
            </span>
            {busy && pct != null && (
              <div className="h-1 rounded-full bg-hover overflow-hidden mt-1">
                <div className="h-full bg-accent transition-[width] duration-300" style={{ width: `${pct}%` }} />
              </div>
            )}
          </div>
          {busy ? (
            <button className="btn btn-sm" onClick={cancel}>
              <Icon as={X} size={14} /> {t("cancel")}
            </button>
          ) : st.installed ? (
            <button className="btn btn-sm btn-ghost text-muted" onClick={() => void remove()} aria-label={t("delete")}>
              <Icon as={Trash2} size={14} />
            </button>
          ) : (
            <button className="btn btn-sm btn-primary" onClick={() => void install()}>
              <Icon as={Download} size={14} /> {t("downloadComponent")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
