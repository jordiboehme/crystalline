/**
 * The component tiers every screen draws from: exactly three button levels
 * plus a destructive variant, one icon-button shape, and one chip primitive
 * with semantic variants. Status colors are filled rather than outlined
 * because filled indicators scan faster; the mapping is guidance-shaped -
 * recommended values get semantic color, anything else is neutral, never an
 * error.
 */

import type { LucideIcon } from "lucide-react";
import type { ComponentPropsWithoutRef, ReactElement, ReactNode } from "react";

import { isRetired } from "../lifecycle";

/** The one focus treatment, everywhere. */
export const FOCUS_RING =
  "focus-visible:ring-2 focus-visible:ring-accent-600 focus-visible:outline-none dark:focus-visible:ring-accent-400";

// eslint-disable-next-line react-refresh/only-export-components
export const BUTTON = {
  primary: `rounded bg-accent-700 px-3 py-1 text-sm font-medium text-white hover:bg-accent-800 disabled:bg-slate-200 disabled:text-slate-500 dark:bg-accent-400 dark:text-accent-950 dark:hover:bg-accent-300 dark:disabled:bg-slate-800 dark:disabled:text-slate-500 ${FOCUS_RING}`,
  secondary: `rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800 ${FOCUS_RING}`,
  ghost: `rounded px-2 py-1 text-sm text-slate-600 hover:bg-slate-100 disabled:opacity-50 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING}`,
  destructive: `rounded border border-red-300 px-3 py-1 text-sm text-red-700 hover:bg-red-50 disabled:opacity-50 dark:border-red-800 dark:text-red-300 dark:hover:bg-red-950 ${FOCUS_RING}`,
} as const;

export type ButtonTier = keyof typeof BUTTON;

export interface IconButtonProps extends ComponentPropsWithoutRef<"button"> {
  /** The accessible name; also the tooltip. Mandatory by construction. */
  label: string;
  icon: LucideIcon;
}

export function IconButton({
  label,
  icon: Icon,
  className,
  ...rest
}: IconButtonProps): ReactElement {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={`inline-flex h-8 w-8 shrink-0 items-center justify-center rounded text-slate-600 hover:bg-slate-100 disabled:opacity-50 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING} ${className ?? ""}`}
      {...rest}
    >
      <Icon aria-hidden="true" size={16} strokeWidth={1.75} />
    </button>
  );
}

export type ChipVariant =
  | "neutral"
  | "positive"
  | "caution"
  | "retired"
  | "accent";

const CHIP_VARIANTS: Record<ChipVariant, string> = {
  neutral: "bg-slate-100 text-slate-700 dark:bg-slate-800 dark:text-slate-300",
  positive:
    "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300",
  caution:
    "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300",
  retired: "bg-slate-100 text-slate-500 dark:bg-slate-800 dark:text-slate-400",
  accent:
    "bg-accent-100 text-accent-800 dark:bg-accent-950 dark:text-accent-300",
};

export function Chip({
  variant = "neutral",
  mono = false,
  children,
}: {
  variant?: ChipVariant;
  mono?: boolean;
  children: ReactNode;
}): ReactElement {
  return (
    <span
      className={`inline-flex items-center rounded px-1.5 py-0.5 text-caption ${
        mono ? "font-mono" : ""
      } ${CHIP_VARIANTS[variant]}`}
    >
      {children}
    </span>
  );
}

const POSITIVE = new Set(["current", "stable", "implemented"]);
const CAUTION = new Set(["draft", "proposed", "idea", "poc"]);

/** Which chip a lifecycle status wears. Free-form values stay neutral. */
// eslint-disable-next-line react-refresh/only-export-components
export function statusVariant(status: string): ChipVariant {
  const lowered = status.toLowerCase();
  if (POSITIVE.has(lowered)) {
    return "positive";
  }
  if (CAUTION.has(lowered)) {
    return "caution";
  }
  if (isRetired(lowered)) {
    return "retired";
  }
  return "neutral";
}
