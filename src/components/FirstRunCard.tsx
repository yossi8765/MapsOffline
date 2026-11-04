import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ListChecks, PackageCheck } from "lucide-react";
import { Icon } from "./Icon";
import { useStore } from "../lib/store";
import { useT } from "../lib/i18n";
import { fmtBytes } from "../lib/format";

/** Size / date of the weekly ready-made package (R1's `package_info`, HEAD on the package). */
interface PackageInfo {
  sizeBytes: number;
  version?: string;
}

/** Fallback when the backend has no `package_info` yet or is offline. */
const PACKAGE_BYTES_FALLBACK = 420e6;

/**
 * First start (nothing installed): one card, two clearly explained ways in. The recommended one
 * downloads the ready-made package and installs it in one go; the other opens the component
 * list ("what to download?").
 */
export function FirstRunCard({ onPackage, onChoose }: { onPackage: () => void; onChoose: () => void }) {
  const t = useT();
  const online = useStore((s) => s.online);
  const [info, setInfo] = useState<PackageInfo | null>(null);
  useEffect(() => {
    let dead = false;
    invoke<PackageInfo>("package_info")
      .then((i) => !dead && i && typeof i.sizeBytes === "number" && setInfo(i))
      .catch(() => {});
    return () => {
      dead = true;
    };
  }, []);
  const size = fmtBytes(info?.sizeBytes ?? PACKAGE_BYTES_FALLBACK);
  return (
    <section className="card p-4 flex flex-col gap-3 fade-in" aria-labelledby="first-run-title">
      <h2 id="first-run-title" className="text-lg font-semibold m-0">
        {t("firstRunTitle")}
      </h2>
      <p className="m-0 text-muted">{t("firstRunBody")}</p>
      <button className="flex items-start gap-3 text-start rounded-[var(--radius)] border border-accent bg-accent-soft/50 p-3 hover:bg-accent-soft transition-colors" onClick={onPackage} disabled={!online}>
        <span className="tile bg-accent-soft text-accent">
          <Icon as={PackageCheck} size={18} />
        </span>
        <span className="flex-1 min-w-0">
          <span className="block font-semibold">
            {t("firstRunPackage")} <span className="chip ms-1">{t("recommended")}</span>
          </span>
          <span className="block text-xs text-muted mt-0.5">
            {t("firstRunPackageHint").replace("{size}", "")}
            <bdi dir="ltr" className="num">
              ~{size}
            </bdi>
          </span>
        </span>
      </button>
      <button className="flex items-start gap-3 text-start rounded-[var(--radius)] border border-line p-3 hover:bg-hover transition-colors" onClick={onChoose} disabled={!online}>
        <span className="tile">
          <Icon as={ListChecks} size={18} />
        </span>
        <span className="flex-1 min-w-0">
          <span className="block font-semibold">{t("firstRunChoose")}</span>
          <span className="block text-xs text-muted mt-0.5">{t("firstRunChooseHint")}</span>
        </span>
      </button>
      {!online && <p className="m-0 text-xs text-warning">{t("firstRunOffline")}</p>}
    </section>
  );
}
