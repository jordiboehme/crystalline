/**
 * Preview mode's answer to a frontmatter block and a frontmatter form both on
 * screen: the block folds to one summary chip and the form is the metadata
 * surface. Unfolding is one click for hand edits; Raw shows everything
 * unconditionally (the fold lives in the preview layer set, which the Raw
 * toggle empties).
 *
 * The fold is atomic, and the accepted price of that is a Backspace at its
 * trailing edge - where the left-arrow walk parks the caret - deleting the
 * whole block in one keypress, because the delete command pulls its target out
 * of an atomic range to the range's start. Accepted rather than worked around:
 * it is exactly what CodeMirror's own folded ranges do, it is loud (the chip
 * goes and no yaml appears in its place, and the rail form drops to its
 * no-block state), one undo restores it on both surfaces, and a document with
 * no frontmatter fails the save gate rather than landing on disk.
 *
 * A decoration source, so the standing guard question is answered here: it
 * decorates exactly the `frontmatterRegion` range and reads no markdown prose
 * context, so `inCompletableProse` is not applicable by construction.
 */

import type { EditorState, Extension } from "@codemirror/state";
import { StateEffect, StateField } from "@codemirror/state";
import type { DecorationSet } from "@codemirror/view";
import { Decoration, EditorView, WidgetType } from "@codemirror/view";

import { readScalar, readTagList } from "./frontmatterFields";
import { frontmatterRegion } from "./frontmatterRegion";
import { docText } from "./setup";

/** Dispatched by the chip; the fold stays open for the rest of the session. */
export const unfoldEffect = StateEffect.define<boolean>();

/** What the chip says the block holds, in the fields a reader scans for. */
function summaryOf(state: EditorState): string {
  const doc = docText(state);
  const type = readScalar(doc, "type");
  const status = readScalar(doc, "status");
  const tags = readTagList(doc);
  const parts = [
    type,
    status,
    tags.length > 0
      ? `${String(tags.length)} ${tags.length === 1 ? "tag" : "tags"}`
      : null,
  ].filter((part): part is string => part !== null);
  return parts.length > 0 ? `Frontmatter: ${parts.join(", ")}` : "Frontmatter";
}

class SummaryChip extends WidgetType {
  readonly summary: string;

  constructor(summary: string) {
    super();
    this.summary = summary;
  }

  override eq(other: SummaryChip): boolean {
    return other.summary === this.summary;
  }

  override toDOM(view: EditorView): HTMLElement {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "cm-frontmatter-chip";
    // A collapsed disclosure, named by what it shows plus what it does: the
    // visible summary is a prefix of the accessible name, so a voice-control
    // user can say what they read (WCAG 2.5.3). There is no expanded
    // counterpart because unfolding removes the control - the yaml itself is
    // then on screen - and `aria-controls` has nothing to point at either:
    // the revealed lines are CodeMirror's own, with no stable id.
    button.setAttribute("aria-expanded", "false");
    button.setAttribute("aria-label", `${this.summary}, show the frontmatter`);
    button.textContent = this.summary;
    button.onclick = () => {
      // The caret lands on the first yaml line and the buffer takes focus:
      // this control is about to be removed from the DOM, and focus that
      // fell to the body would strand a keyboard user at the top of the page
      // instead of in the text they just asked to see.
      const region = frontmatterRegion(view.state.doc);
      view.dispatch({
        effects: unfoldEffect.of(true),
        ...(region === null
          ? {}
          : { selection: { anchor: view.state.doc.line(2).from } }),
      });
      view.focus();
    };
    return button;
  }

  override ignoreEvent(): boolean {
    // The widget owns its events; the editor must not also treat the click as
    // a selection change on the text hidden behind the chip. Same reasoning
    // as the preview widgets next door.
    return true;
  }
}

function folded(state: EditorState): DecorationSet {
  const region = frontmatterRegion(state.doc);
  if (region === null) {
    return Decoration.none;
  }
  return Decoration.set([
    Decoration.replace({
      widget: new SummaryChip(summaryOf(state)),
      block: true,
    }).range(region.from, region.to),
  ]);
}

const foldField = StateField.define<DecorationSet>({
  create: (state) => folded(state),
  update(deco, tr) {
    for (const effect of tr.effects) {
      if (effect.is(unfoldEffect)) {
        return Decoration.none;
      }
    }
    // An empty set is the unfolded state, and it is deliberately terminal:
    // once the yaml is on screen - because the chip was clicked, because the
    // document never had a block, or because a hand edit removed the closing
    // fence - nothing folds it away again under a person who is editing it.
    if (!tr.docChanged || deco === Decoration.none) {
      return deco;
    }
    // Recompute rather than map: a form edit inside the folded region moves
    // the fences and rewrites the summary, and the region is cheap to re-read.
    return folded(tr.state);
  },
  provide: (field) => [
    EditorView.decorations.from(field),
    // Atomic, so cursor motion steps OVER the folded block: without this,
    // arrow keys walk the caret into the invisible yaml line by line and the
    // editor appears stuck to anyone keyboarding past the chip.
    EditorView.atomicRanges.of((view) => view.state.field(field)),
  ],
});

/**
 * The chip's face.
 *
 * A base theme rather than a plain one, and on a class of its own: a plain
 * `EditorView.theme` appended late loses to an equal-specificity rule listed
 * earlier, because the view mounts its style modules reversed. `.cm-frontmatter-chip`
 * collides with nothing in `editorTheme` - it is not a `.cm-line` and not the
 * scroller - and the base theme's own rewriting puts a scope class in front of
 * both selectors, so the `&dark` variant is one class deep as well and wins
 * over the plain rule by mount order within this one module.
 */
const foldTheme = EditorView.baseTheme({
  ".cm-frontmatter-chip": {
    display: "inline-flex",
    alignItems: "center",
    margin: "0.25rem 0.75rem",
    padding: "0.125rem 0.5rem",
    borderRadius: "0.25rem",
    border: "1px solid var(--color-slate-300)",
    background: "var(--color-slate-100)",
    color: "var(--color-slate-600)",
    fontSize: "0.75rem",
    fontFamily: "var(--font-mono, ui-monospace, monospace)",
    cursor: "pointer",
  },
  "&dark .cm-frontmatter-chip": {
    border: "1px solid var(--color-slate-700)",
    background: "var(--color-slate-800)",
    color: "var(--color-slate-300)",
  },
});

/** The frontmatter block as one summary chip, until somebody unfolds it. */
export function frontmatterFold(): Extension {
  return [foldField, foldTheme];
}
