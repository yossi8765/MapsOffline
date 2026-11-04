import type { LucideIcon, LucideProps } from "lucide-react";

/** The app's icon stroke: one width everywhere (Fluent icons are thin; lucide's default 2 is heavy). */
export const ICON_STROKE = 1.75;

/** A lucide icon with the app's stroke width and size 16 unless given. Decorative by default
 *  (aria-hidden); pass `label` for an icon that carries meaning on its own. */
export function Icon({ as: C, size = 16, strokeWidth = ICON_STROKE, label, ...rest }: { as: LucideIcon; label?: string } & LucideProps) {
  return <C size={size} strokeWidth={strokeWidth} aria-hidden={label ? undefined : true} aria-label={label} role={label ? "img" : undefined} {...rest} />;
}
