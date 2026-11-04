import { useMemo } from "react";
import clsx from "clsx";
import { useStore } from "../lib/store";
import { useT, type I18nKey } from "../lib/i18n";
import { VEHICLE_COLORS, VEHICLE_KINDS, vehiclePaint, vehiclePreview, vehicleSwatch, type VehicleKind } from "../map/vehicles";
import { cssVar } from "../map/colors";

/** The vehicle drawn while navigating by car (like Waze's "my car"): a row of top-down previews
 *  and a row of body colours; saved in settings.navVehicle / settings.navVehicleColor. */
export function VehiclePicker({ onPicked }: { onPicked?: () => void }) {
  const t = useT();
  const current = useStore((s) => s.settings.navVehicle ?? "arrow");
  const color = useStore((s) => s.settings.navVehicleColor ?? "accent");
  const accentSetting = useStore((s) => s.settings.accent);
  const scheme = useStore((s) => s.settings.colorScheme);
  // previews follow the chosen colour (or the accent / colour scheme, like the marker on the map)
  const accent = useMemo(
    () => cssVar("--map-me", "#1a66d9"),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [accentSetting, scheme],
  );
  const previews = useMemo(() => {
    const paint = vehiclePaint(color, accent);
    return Object.fromEntries(VEHICLE_KINDS.map((k) => [k, vehiclePreview(k, paint)])) as Record<VehicleKind, string>;
  }, [color, accent]);
  return (
    <div className="flex flex-col gap-2">
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
      <div className="flex flex-wrap items-center gap-2" role="radiogroup" aria-label={t("navVehicleColor")}>
        <span className="text-xs text-muted">{t("navVehicleColor")}</span>
        {VEHICLE_COLORS.map((c) => {
          const on = color === c;
          const label = t(`vcolor_${c}` as I18nKey);
          return (
            <button
              key={c}
              type="button"
              role="radio"
              aria-checked={on}
              aria-label={label}
              title={label}
              className={clsx(
                "w-7 h-7 rounded-full border transition-transform duration-150 hover:scale-110",
                on ? "border-fg border-2 scale-110" : "border-line-strong",
              )}
              style={{
                background:
                  c === "accent"
                    ? // the scheme's colour: half accent, half white, so it reads as "follows the colours"
                      `linear-gradient(135deg, ${accent} 0 50%, #ffffff 50% 100%)`
                    : vehicleSwatch(c, accent),
              }}
              onClick={() => void useStore.getState().updateSettings({ navVehicleColor: c })}
            />
          );
        })}
      </div>
    </div>
  );
}
