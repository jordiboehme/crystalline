/**
 * What a reader can ask of a picture, offered on the picture itself.
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
 * Both names are verbs that say what pressing will do, and the width one
 * changes with the state rather than sitting there as a label of it, so the
 * full-width button reads "Show at reading width" once the diagram is wide.
 * The width action is the one a caller may leave out: an image is drawn at
 * the width its own directive asked for, so there is no second width for a
 * button to fold it back to, and the full window is all it offers.
 */

import { Maximize2, UnfoldHorizontal, FoldHorizontal } from "lucide-react";
import type { ComponentType, ReactElement, RefObject } from "react";

import { FOCUS_RING } from "./primitives";

/**
 * One action's face: the app's icon geometry over a solid ground, because
 * these sit ON the drawing rather than beside it and a bare glyph over a
 * diagram's own lines is unreadable.
 */
const ACTION = `inline-flex h-8 w-8 shrink-0 items-center justify-center rounded border border-slate-200 bg-white/90 text-slate-600 hover:bg-slate-100 dark:border-slate-700 dark:bg-slate-900/90 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING}`;

export interface DiagramToolbarProps {
  /** The width action, for a drawing that has one; an image has none. */
  width?: { fullWidth: boolean; onToggle: () => void };
  onOpenFullWindow: () => void;
  /**
   * The full-window button, so the overlay it opens can hand the keyboard
   * back to it on the way out.
   */
  fullWindowRef: RefObject<HTMLButtonElement | null>;
}

function Action({
  label,
  icon: Icon,
  onClick,
  buttonRef,
}: {
  label: string;
  icon: ComponentType<{ size?: number; strokeWidth?: number }>;
  onClick: () => void;
  buttonRef?: RefObject<HTMLButtonElement | null> | undefined;
}): ReactElement {
  return (
    <button
      type="button"
      ref={buttonRef}
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
  width,
  onOpenFullWindow,
  fullWindowRef,
}: DiagramToolbarProps): ReactElement {
  return (
    <div
      // `pointer-events-none` until it is revealed: an invisible element is
      // still a hit target, and two 32px squares over the drawing's top-right
      // corner would otherwise swallow clicks on whatever is under them - a
      // node with a link in it, say. The reveal is the container's hover, not
      // the toolbar's own, so handing the pointer back only once the group is
      // hovered or focused within costs the reveal nothing.
      //
      // `print:hidden` belongs with the rest: a printed page has no pointer, so
      // a browser that reads `hover: none` in print media would otherwise put
      // two buttons in the corner of every printed diagram. Chromium does not
      // (measured), and this is what makes that not a question.
      className="pointer-events-none absolute top-1 right-1 flex gap-1 opacity-0 transition-opacity group-focus-within:pointer-events-auto group-focus-within:opacity-100 group-hover:pointer-events-auto group-hover:opacity-100 print:hidden [@media(hover:none)]:pointer-events-auto [@media(hover:none)]:opacity-100"
      data-testid="diagram-toolbar"
    >
      {width !== undefined && (
        <Action
          label={
            width.fullWidth ? "Show at reading width" : "Show at full width"
          }
          icon={width.fullWidth ? FoldHorizontal : UnfoldHorizontal}
          onClick={width.onToggle}
        />
      )}
      <Action
        label="Open in full window"
        icon={Maximize2}
        onClick={onOpenFullWindow}
        buttonRef={fullWindowRef}
      />
    </div>
  );
}
