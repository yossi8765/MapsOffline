import { forwardRef } from "react";
import clsx from "clsx";
import { FolderOpen, PackageOpen, X } from "lucide-react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Icon } from "./Icon";
import type { ProgressPhase } from "../lib/api";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";

export interface PackageDl {
  phase: ProgressPhase;
  done: number;
  total: number;
  message: string;
}

export function packageActive(p: PackageDl | null): boolean {
  return !!p && p.phase !== "done" && p.phase !== "failed" && p.phase !== "cancelled";
}

/**
 * Progress of the full ready-made package download (event `package-download`), shown where the
 * download was started: at the top of the Data panel (first run) or in "transfer to an offline
 * computer". After it finished: install it / show it in its folder.
 */
export const PackageProgress = forwardRef<HTMLDivElement, { pkg: PackageDl | null; file: string | null; installing?: boolean; onInstall?: () => void }>(function PackageProgress(
  { pkg, file, installing, onInstall },
  ref,
) {
  const t = useT();
  const active = packageActive(pkg);
  const pct = pkg && pkg.total > 0 ? Math.min(100, Math.round((pkg.done / pkg.total) * 100)) : null;
  const updating = useStore((s) => !!s.dataStatus?.updating);
  if (!active && !file && pkg?.phase !== "failed") return null;
  return (
    <div ref={ref} className="flex flex-col gap-1.5 fade-in" aria-live="polite">
      {active && (
        <>
          <div className="flex items-center gap-2 text-xs text-muted">
            <span className="flex-1 min-w-0 truncate">
              <bdi>{pkg?.message || t("downloadPackage")}</bdi>
            </span>
            {pct !== null && <span className="num shrink-0">{pct}%</span>}
            <button className="btn btn-ghost btn-icon btn-sm text-muted shrink-0" aria-label={t("cancel")} onClick={() => void useStore.getState().cancelUpdate()}>
              <Icon as={X} size={14} />
            </button>
          </div>
          <div className="h-1 rounded-full bg-surface-2 overflow-hidden" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct ?? undefined} aria-label={t("downloadPackage")}>
            <div className={clsx("h-full bg-accent rounded-full transition-[width] duration-300 ease-out", pct === null && "animate-pulse w-1/3")} style={pct !== null ? { width: `${pct}%` } : undefined} />
          </div>
        </>
      )}
      {!active && pkg?.phase === "failed" && <p className="m-0 text-xs text-danger selectable break-words">{pkg.message}</p>}
      {!active && file && (
        <div className="flex gap-2">
          {onInstall && (
            <button className="btn btn-sm flex-1" disabled={updating || installing} onClick={onInstall}>
              <Icon as={PackageOpen} size={14} /> {t("installDownloaded")}
            </button>
          )}
          <button className="btn btn-sm flex-1" onClick={() => void revealItemInDir(file).catch((e) => useStore.getState().toastError(e))}>
            <Icon as={FolderOpen} size={14} /> {t("showInFolder")}
          </button>
        </div>
      )}
    </div>
  );
});
