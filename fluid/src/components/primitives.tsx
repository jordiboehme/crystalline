/**
 * The component tiers every screen draws from: exactly three button levels
 * plus a destructive variant, one icon-button shape, and one chip primitive
 * with semantic variants. Status colors are filled rather than outlined
 * because filled indicators scan faster; the mapping is guidance-shaped -
 * recommended values get semantic color, anything else is neutral, never an
 * error.
 */

import type { LucideIcon } from "lucide-react";
import type { ComponentPropsWithRef, ReactElement, ReactNode } from "react";

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

/**
 * A two-state toggle's two faces, for a button that carries `aria-pressed`.
 *
 * The pressed face is SELF-CONTAINED rather than accent utilities layered on
 * top of `BUTTON.ghost`, because that layering does not work and fails
 * silently. Tailwind decides same-specificity conflicts by the order the
 * utilities are emitted into the stylesheet, not by the order of names in a
 * class attribute: `.text-slate-600` is written after `.text-accent-800`, so
 * ghost's own color wins and the pressed label never turns accent. Worse,
 * ghost's `hover:bg-slate-100` is a (0,2,0) selector and beats a plain
 * `bg-accent-50` (0,1,0), so a pressed toggle under the pointer would be
 * pixel-identical to an unpressed one. The two faces therefore share only
 * the geometry and the focus ring, and every color - background, text and
 * hover, in both schemes - is declared exactly once, by exactly one of them.
 *
 * The pressed face carries a border as well as a wash, because the wash alone
 * is not a state indicator: accent-100 against a white page is 1.13:1, well
 * under the 3:1 floor for non-text UI, while the border is 3.74:1 (accent-600
 * on white) and 10.84:1 (accent-400 on slate-950). The off face reserves the
 * same border in `transparent`, so pressing changes color and nothing moves.
 */
// eslint-disable-next-line react-refresh/only-export-components
export const TOGGLE = {
  off: `${BUTTON.ghost} border border-transparent`,
  on: `rounded border border-accent-600 bg-accent-100 px-2 py-1 text-sm text-accent-900 hover:bg-accent-200 dark:border-accent-400 dark:bg-accent-900 dark:text-accent-50 dark:hover:bg-accent-800 ${FOCUS_RING}`,
} as const;

/**
 * One height for every control that stands on a row beside another one, and
 * for the cells that are not controls at all.
 *
 * A row of an input, a select, a word and a handful of buttons gets a different
 * height for each from the browser. `h-8` is what the rest of the app's
 * controls stand at (`IconButton`, the filter bars), so a row reads as one line
 * rather than as six things that happen to be next to each other.
 */
export const CONTROL_HEIGHT = "h-8";

/**
 * The one text-input face: the admin forms' fields, the settings screen's, and
 * the domain dialog's.
 *
 * Width is the caller's, because that is the one thing that differs between
 * them - a login name is not as wide as a personal access token - and the rest
 * is shared to the character. Opaque rather than transparent, so a field inside
 * a dialog reads as a field rather than as a rectangle drawn on the panel.
 */
export const FIELD = `${CONTROL_HEIGHT} rounded border border-slate-300 bg-white px-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 dark:border-slate-700 dark:bg-slate-900`;

/**
 * One labelled field, with what the field is called kept apart from what is
 * worth knowing about it.
 *
 * The parenthetical labels used to carry - "Tags (optional, comma separated)" -
 * was help wearing a label's clothes: it made the name a reader hears longer
 * than the thing it names, and it made every label a different length for no
 * reason a reader could see. The helper is a description instead, tied on with
 * `aria-describedby` by the caller, so the name stays the word and the advice
 * still reaches a screen reader - after the name rather than inside it.
 */
export function Field({
  id,
  label,
  helper,
  children,
}: {
  id: string;
  label: string;
  helper?: string;
  children: ReactNode;
}): ReactElement {
  return (
    <div className="flex flex-col gap-1 text-sm">
      <label htmlFor={id}>{label}</label>
      {helper !== undefined && (
        <p
          id={`${id}-help`}
          className="text-caption text-slate-500 dark:text-slate-400"
        >
          {helper}
        </p>
      )}
      {children}
    </div>
  );
}

/**
 * Every button attribute, `ref` included: a caller that has to move the
 * keyboard onto one of these - a control that replaces the control that was
 * pressed - needs the element itself, and React hands a function component's
 * `ref` through with the rest of its props.
 */
export interface IconButtonProps extends ComponentPropsWithRef<"button"> {
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
  "neutral" | "positive" | "caution" | "retired" | "accent";

const CHIP_VARIANTS: Record<ChipVariant, string> = {
  neutral: "bg-slate-100 text-slate-700 dark:bg-slate-800 dark:text-slate-300",
  positive:
    "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300",
  caution: "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300",
  retired: "bg-slate-100 text-slate-600 dark:bg-slate-800 dark:text-slate-400",
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
