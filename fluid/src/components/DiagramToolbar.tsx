/**
 * What a reader can ask of a diagram, offered on the diagram itself.
 *
 * They float in its top-right corner and stay out of the way until they are
 * wanted: hidden by opacity while nobody is near, shown on hover and on focus
 * within the diagram's frame, and shown outright where there is no pointer to
 * hover with. Opacity rather than `hidden` or `invisible` on purpose - both of
 * those take the buttons out of the focus order, and a control a keyboard
 * cannot reach is not a control.
 *
 * Plain buttons rather than the frame's `IconButton`: that one carries a Radix
 * tooltip and so needs a provider above it, which a diagram inside a rendered
 * document has no business requiring. The name lives on `aria-label` and is
 * repeated as `title`, which is the browser's own tooltip and costs nothing.
 *
 * Both names are verbs that say what pressing will do, and the first one
 * changes with the state rather than sitting there as a label of it, so the
 * full-width button reads "Show at reading width" once the diagram is wide.
 */

import { FoldHorizontal, UnfoldHorizontal } from "lucide-react";
import type { ComponentType, ReactElement } from "react";

import { FOCUS_RING } from "./primitives";

/**
 * One action's face: the app's icon geometry over a solid ground, because
 * these sit ON the drawing rather than beside it and a bare glyph over a
 * diagram's own lines is unreadable.
 */
const ACTION = `inline-flex h-8 w-8 shrink-0 items-center justify-center rounded border border-slate-200 bg-white/90 text-slate-600 hover:bg-slate-100 dark:border-slate-700 dark:bg-slate-900/90 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING}`;

export interface DiagramToolbarProps {
  /** Whether the diagram is currently showing at full width. */
  fullWidth: boolean;
  onToggleFullWidth: () => void;
}

function Action({
  label,
  icon: Icon,
  onClick,
}: {
  label: string;
  icon: ComponentType<{ size?: number; strokeWidth?: number }>;
  onClick: () => void;
}): ReactElement {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={ACTION}
      onClick={onClick}
    >
      <Icon size={16} strokeWidth={1.75} />
    </button>
  );
}

export default function DiagramToolbar({
  fullWidth,
  onToggleFullWidth,
}: DiagramToolbarProps): ReactElement {
  return (
    <div
      className="absolute top-1 right-1 flex gap-1 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100 [@media(hover:none)]:opacity-100"
      data-testid="diagram-toolbar"
    >
      <Action
        label={fullWidth ? "Show at reading width" : "Show at full width"}
        icon={fullWidth ? FoldHorizontal : UnfoldHorizontal}
        onClick={onToggleFullWidth}
      />
    </div>
  );
}
