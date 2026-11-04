/**
 * The one call every command that acts from outside the editor's DOM makes
 * first (D60): the sticky toolbar, the floating bar, a popover's commit and
 * a menu. Their presses never reach the capture listeners the composition
 * module keeps on the editor, so a composition still waiting to be judged
 * is judged here, in a step of its own that joins the composition's undo
 * step, before the command reads the text. One undo then takes only the
 * command.
 *
 * The judge itself lives in `composition.ts`, which provides it through
 * this facet; the commands import only this module, so the composition
 * module and the format commands it reads need not import each other.
 */

import { Facet } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

/** What judges a waiting composition on a view before a command. */
export const commandJudge = Facet.define<(view: EditorView) => void>();

/** Judges a waiting composition on `view`, the view the command acts on. */
export function judgeBeforeCommand(view: EditorView): void {
  for (const judge of view.state.facet(commandJudge)) judge(view);
}
