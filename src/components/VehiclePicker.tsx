import { useMemo } from "react";
import clsx from "clsx";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { VEHICLE_KINDS, vehiclePreview, type VehicleKind } from "../map/vehicles";
import { cssVar } from "../map/colors";

/** The vehicle drawn while navigating by car (like Waze's "my car"): a row of top-down previews
 *  in the accent colour; the choice is saved in settings.navVehicle. */
export function VehiclePicker({ onPicked }: { onPicked?: () => void }) {
  const t = useT();
  const current = useStore((s) => s.settings.navVehicle ?? "arrow");
  const accent = useStore((s) => s.settings.accent);
  const scheme = useStore((s) => s.settings.colorScheme);
  // previews follow the accent / colour scheme like the marker on the map
  const previews = useMemo(() => {
    const color = cssVar("--map-me", "#1a66d9");
    return Object.fromEntries(VEHICLE_KINDS.map((k) => [k, vehiclePreview(k, color)])) as Record<VehicleKind, string>;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accent, scheme]);
  return (
    <div className="flex flex-wrap gap-1.5" role="radiogroup" aria-label={t("navVehicle")}>
      {VEHICLE_KINDS.map((k) => {
        const on = current === k;
        const label = t(`vehicle_${k}` as I18nKey);
        return (
          <button
            key={k}
            type="button"
            role="radio"
            aria-checked={on}
            aria-label={label}
            title={label}
            className={clsx(
              "w-14 h-16 rounded-[var(--radius-sm)] border flex flex-col items-center justify-center gap-0.5 transition-colors",
              on ? "border-accent bg-accent-soft" : "border-line hover:bg-hover",
            )}
            onClick={() => {
              void useStore.getState().updateSettings({ navVehicle: k });
              onPicked?.();
            }}
          >
            <img src={previews[k]} alt="" className="w-9 h-9" draggable={false} />
            <span className="text-xs leading-none">{label}</span>
          </button>
        );
      })}
    </div>
  );
}
