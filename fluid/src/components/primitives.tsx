/**
 * The component tiers every screen draws from: exactly three button levels
 * plus a destructive variant, one icon-button shape, and one chip primitive
 * with semantic variants. Status colors are filled rather than outlined
 * because filled indicators scan faster; the mapping is guidance-shaped -
 * recommended values get semantic color, anything else is neutral, never an
 * error.
 */

import type { LucideIcon } from "lucide-react";
import { Tooltip as TooltipPrimitive } from "radix-ui";
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
 * `TOGGLE`'s two faces at `IconButton`'s size, for a two-state switch whose
 * name is its tooltip rather than its text.
 *
 * Self-contained for exactly the reason spelled out above `TOGGLE`, and doubly
 * so here: `IconButton` bakes its own `text-slate-600 hover:bg-slate-100` into
 * the element, so a pressed face passed to it through `className` would lose to
 * it in the emitted stylesheet and press silently. These are whole strings for
 * a plain button instead, and every color in them is one of the pairs `TOGGLE`
 * already carries - no new pair enters the app through this constant.
 */
// eslint-disable-next-line react-refresh/only-export-components
export const ICON_TOGGLE = {
  off: `inline-flex h-8 w-8 shrink-0 items-center justify-center rounded border border-transparent text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING}`,
  on: `inline-flex h-8 w-8 shrink-0 items-center justify-center rounded border border-accent-600 bg-accent-100 text-accent-900 hover:bg-accent-200 dark:border-accent-400 dark:bg-accent-900 dark:text-accent-50 dark:hover:bg-accent-800 ${FOCUS_RING}`,
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
 * How long the pointer has to rest on a control before it says its name.
 *
 * Long enough that crossing a row of icons on the way somewhere else says
 * nothing at all, short enough that pausing on one is answered rather than
 * waited out. The keyboard does not wait: focus opens a tooltip immediately,
 * because a control that was deliberately moved to has already been asked
 * about.
 */
const TOOLTIP_DELAY_MS = 600;

/**
 * The whole app's tooltip group, mounted once at the root.
 *
 * Once, rather than one per tooltip, for the thing only a shared group can
 * do: after one tooltip has been open, a neighbor within the skip window
 * opens instantly instead of making a reader who is scanning the row wait out
 * the delay again. Every `IconButton` and every `Tooltip` in the tree needs
 * this above it - Radix throws by name without it - so a test that mounts one
 * of those in isolation mounts this around it.
 */
export function Tooltips({ children }: { children: ReactNode }): ReactElement {
  return (
    <TooltipPrimitive.Provider delayDuration={TOOLTIP_DELAY_MS}>
      {children}
    </TooltipPrimitive.Provider>
  );
}

/**
 * The tooltip surface: a menu's face, sized for a word.
 *
 * Deliberately not `MENU_CLASSES` itself plus overrides. That constant carries
 * `min-w-48` and `p-1` because a menu is a column of rows, and a tooltip
 * holding two words is neither; overriding them from a second class string
 * would be the layering trap `TOGGLE` documents above - same specificity,
 * decided by the order Tailwind happens to emit the two utilities in. So the
 * border, the background, the radius, the shadow and the stacking are copied
 * exactly, the sizing is the tooltip's own, and nothing here is fighting
 * anything.
 */
const TOOLTIP_CLASSES =
  "text-caption z-50 max-w-64 rounded border border-slate-200 bg-white px-2 py-1 shadow-lg dark:border-slate-700 dark:bg-slate-900";

/**
 * A name for a control that has no room for one, on hover and on focus.
 *
 * No arrow: an arrow is for a surface a reader has to connect to a target
 * across distance, and this one opens under the control it names. Below by
 * default for the same reason - the row above a control is usually where the
 * page's own content is, and a label that covers what you were reading to
 * tell you what a button is called has traded down.
 *
 * The label is NOT the accessible name. `aria-label` on the control itself is
 * that, and it stays; this is the same words drawn for a pointer, tied on with
 * `aria-describedby` by Radix, so a screen reader hears the name once.
 */
export function Tooltip({
  label,
  side = "bottom",
  children,
}: {
  label: string;
  side?: "top" | "right" | "bottom" | "left";
  children: ReactNode;
}): ReactElement {
  return (
    <TooltipPrimitive.Root>
      <TooltipPrimitive.Trigger asChild>{children}</TooltipPrimitive.Trigger>
      <TooltipPrimitive.Portal>
        <TooltipPrimitive.Content
          side={side}
          sideOffset={6}
          className={TOOLTIP_CLASSES}
        >
          {label}
        </TooltipPrimitive.Content>
      </TooltipPrimitive.Portal>
    </TooltipPrimitive.Root>
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

/**
 * One icon, one name, said two ways.
 *
 * The name is `aria-label` for anything that reads the page and a `Tooltip`
 * for anything that looks at it, and there is no `title` beside them: the
 * browser's own tooltip would be a second label for the same button, drawn a
 * beat later, in a font the page does not choose. One label, one delay, one
 * surface.
 */
export function IconButton({
  label,
  icon: Icon,
  className,
  ...rest
}: IconButtonProps): ReactElement {
  return (
    <Tooltip label={label}>
      <button
        type="button"
        aria-label={label}
        className={`inline-flex h-8 w-8 shrink-0 items-center justify-center rounded text-slate-600 hover:bg-slate-100 disabled:opacity-50 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING} ${className ?? ""}`}
        {...rest}
      >
        <Icon aria-hidden="true" size={16} strokeWidth={1.75} />
      </button>
    </Tooltip>
  );
}

export type ChipVariant =
  "neutral" | "positive" | "caution" | "danger" | "retired" | "accent";

const CHIP_VARIANTS: Record<ChipVariant, string> = {
  neutral: "bg-slate-100 text-slate-700 dark:bg-slate-800 dark:text-slate-300",
  positive:
    "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300",
  caution: "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300",
  // The face for what was refused rather than merely held back, kept apart
  // from `caution` so a reader can tell "not written because something is
  // already there" from "cannot be written at all". The alert red one shade
  // deeper, because a chip is a filled block rather than a panel: red-800 on
  // red-100 is 6.85:1 and red-200 on red-950 is 11.12:1, both clear of the
  // 4.5:1 floor for text this size.
  danger: "bg-red-100 text-red-800 dark:bg-red-950 dark:text-red-200",
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
