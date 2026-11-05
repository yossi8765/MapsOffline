import { useEffect, useState } from "react";
import { Trash2 } from "lucide-react";
import { api, DATA_COMPONENTS } from "../lib/api";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { askConfirm, fmtBytes } from "../lib/format";
import { Icon } from "./Icon";
import { InfoTip } from "./InfoTip";

/** Labels of the deletable targets that are not components (components use their card names). */
const EXTRA_LABEL: Record<string, I18nKey> = {
  regions: "delRegions",
  areas: "delAreas",
  downloads: "delDownloads",
  cache: "delCache",
  personal: "delPersonal",
};

/**
 * Data panel → advanced: delete chosen parts of the data folder, or all of it. Personal data
 * (favorites, history, own layers) goes only when picked explicitly.
 */
export function DeleteDataSection({ componentLabel, busy }: { componentLabel: (c: string) => string; busy: boolean }) {
  const t = useT();
  const [usage, setUsage] = useState<{ target: string; bytes: number }[] | null>(null);
  const [picked, setPicked] = useState<string[]>([]);
  const [working, setWorking] = useState(false);
  const [tick, setTick] = useState(0);
  const dataStatus = useStore((s) => s.dataStatus);

  useEffect(() => {
    let alive = true;
    api
      .dataUsage()
      .then((u) => alive && setUsage(u))
      .catch(() => alive && setUsage([]));
    return () => {
      alive = false;
    };
  }, [tick, dataStatus]);

  const label = (target: string) => (EXTRA_LABEL[target] ? t(EXTRA_LABEL[target]) : componentLabel(target));
  // components in the Data panel's order, then the rest
  const order = [...DATA_COMPONENTS, "regions", "areas", "downloads", "cache", "personal"] as string[];
  const rows = (usage ?? []).slice().sort((a, b) => order.indexOf(a.target) - order.indexOf(b.target));
  const bytesOf = (targets: string[]) => rows.filter((r) => targets.includes(r.target)).reduce((n, r) => n + r.bytes, 0);
  const everything = rows.map((r) => r.target).filter((x) => x !== "personal");

  const run = async (targets: string[]) => {
    if (!targets.length) return;
    const size = fmtBytes(bytesOf(targets));
    const personal = targets.includes("personal");
    const msg = `${t("delConfirm")} ${targets.map(label).join(", ")} (${size}).${personal ? ` ${t("delConfirmPersonal")}` : ""} ${t("delConfirmTail")}`;
    if (!(await askConfirm(msg, t("delete"), t("cancel"), t("appName")))) return;
    setWorking(true);
    const st = useStore.getState();
    try {
      const r = await api.dataDelete(targets);
      st.toast("success", `${t("delDone")} ${fmtBytes(r.freedBytes)}${r.pending.length ? `. ${t("delPending")}` : ""}`);
      if (targets.includes("areas")) useStore.setState((s) => ({ settings: { ...s.settings, satelliteAreas: [] } }));
      if (personal) {
        void st.loadFavorites();
        void st.loadHistory();
        void st.loadUserLayers();
      }
    } catch (e) {
      st.toastError(e);
    } finally {
      setWorking(false);
      setPicked([]);
      setTick((n) => n + 1);
      await st.refreshDataStatus();
      await st.refreshTiles();
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <h2 className="section-label m-0 inline-flex items-center gap-1.5">
        {t("delTitle")}
        <InfoTip text={t("delHint")} />
      </h2>
      <div className="group-list">
        <div className="px-3 py-2.5 flex flex-col gap-2">
          {usage === null ? (
            <span className="text-xs text-muted">{t("regSizeComputing")}</span>
          ) : rows.length === 0 ? (
            <span className="text-xs text-muted">{t("delNothing")}</span>
          ) : (
            <div className="flex flex-wrap gap-1.5">
              {rows.map((r) => {
                const on = picked.includes(r.target);
                return (
                  <button
                    key={r.target}
                    type="button"
                    className="choice"
                    aria-pressed={on}
                    disabled={busy || working}
                    onClick={() => setPicked((p) => (on ? p.filter((x) => x !== r.target) : [...p, r.target]))}
                  >
                    {label(r.target)}{" "}
                    <bdi dir="ltr" className="num text-muted">
                      {fmtBytes(r.bytes)}
                    </bdi>
                  </button>
                );
              })}
            </div>
          )}
          <div className="flex items-center gap-2">
            <button className="btn btn-sm btn-danger flex-1" disabled={busy || working || picked.length === 0} onClick={() => void run(picked)}>
              <Icon as={Trash2} size={14} /> {t("delSelected")}
              {picked.length > 0 && (
                <bdi dir="ltr" className="num">
                  {" "}
                  ({fmtBytes(bytesOf(picked))})
                </bdi>
              )}
            </button>
            <button className="btn btn-sm flex-1" disabled={busy || working || everything.length === 0} onClick={() => void run(everything)}>
              {t("delAll")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
