/**
 * The live-preview decoration layer: a read-model over the buffer, never a
 * write-model. Syntax markers (heading #s, emphasis stars, backticks, link
 * brackets and targets, list, task and quote prefixes) and HTML comments,
 * which the reading page never draws, fold away wherever the hidden-mark
 * model says a line hides its marks (`marks/hiddenMarks.ts`): every line by
 * default, and with the setting on every line but the ones a selection
 * touches, which show exactly what is written. Nothing here rewrites the
 * document: a decoration is a view over the bytes, and the bytes come back out
 * of the buffer exactly as they went in.
 *
 * The one exception is deliberate and is an ordinary edit rather than a
 * decoration: a task marker is drawn as the checkbox it means, and toggling it
 * dispatches a transaction over the marker's own range, taken from the
 * model's mark offsets. Those offsets are positions in the document, not
 * indexes into the file's string, and they never leave this module as
 * anything else.
 *
 * The frontmatter block is left entirely alone - the parser misreads its
 * fences as thematic breaks, and the form panel is that region's structured
 * view.
 */

import type { Extension, Range } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";

import { frontmatterRegion } from "./frontmatterRegion";
import { marksOnLine } from "./marks/hiddenMarks";
import { markModeChanged } from "./marks/mode";
import { parkingChanged } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";

/** A task marker drawn as the checkbox it means. The click IS a text edit. */
class CheckboxWidget extends WidgetType {
  // Assigned in the body rather than declared as constructor parameters:
  // the build erases types, it does not run a TypeScript transform, and
  // parameter properties are syntax that would need one.
  readonly checked: boolean;
  readonly from: number;
  readonly to: number;

  constructor(checked: boolean, from: number, to: number) {
    super();
    this.checked = checked;
    this.from = from;
    this.to = to;
  }

  override eq(other: CheckboxWidget): boolean {
    return (
      other.checked === this.checked &&
      other.from === this.from &&
      other.to === this.to
    );
  }

  /**
   * Flip the marker text in place. The range is the parsed marker's own span,
   * so what is replaced is exactly `[ ]` or `[x]` and nothing around it; the
   * decorations rebuild off the new document and this widget is replaced by
   * one that no longer compares equal.
   */
  private toggle(view: EditorView): void {
    view.dispatch({
      changes: {
        from: this.from,
        to: this.to,
        insert: this.checked ? "[ ]" : "[x]",
      },
    });
  }

  override toDOM(view: EditorView): HTMLElement {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = this.checked;
    box.className = "cm-task-toggle";
    box.setAttribute(
      "aria-label",
      this.checked ? "Mark task open" : "Mark task done",
    );
    box.addEventListener("mousedown", (event) => {
      // The editor must not also read the press as a click into the hidden
      // marker text and move the selection there.
      event.preventDefault();
      this.toggle(view);
    });
    // Keyboard parity: a focused checkbox answers to Space, and the raw
    // marker is always reachable by putting the cursor on the line, where
    // the widget folds back into the `[ ]` it stands for.
    box.addEventListener("keydown", (event) => {
      if (event.key === " " || event.key === "Enter") {
        event.preventDefault();
        this.toggle(view);
      }
    });
    return box;
  }

  override ignoreEvent(): boolean {
    // The widget owns its events; the editor must not also treat the click
    // as a selection change on the hidden marker text.
    return true;
  }
}

/**
 * The not-applicable marker `[~]`, drawn as a struck box. It is no checkbox
 * and toggles nothing: the third state is written by hand, and a click is a
 * click into the text, which puts the cursor on the line (and, with the
 * setting on, shows `[~]` as written).
 */
class NotApplicableWidget extends WidgetType {
  override eq(): boolean {
    return true;
  }

  override toDOM(): HTMLElement {
    const box = document.createElement("span");
    box.className = "cm-task-na";
    box.setAttribute("role", "img");
    box.setAttribute("aria-label", "Not applicable");
    return box;
  }

  override ignoreEvent(): boolean {
    return false;
  }
}

/**
 * The hidden marks of the visible lines, from the model (`marks/hiddenMarks.ts`):
 * each one folds away as a plain replace, except a task box, which is drawn as
 * the box it means with the space after it folded beside it.
 */
function buildDecorations(view: EditorView): DecorationSet {
  const { state } = view;
  const doc = state.doc;
  const decorations: Range<Decoration>[] = [];
  // A line two visible ranges both reach is drawn once.
  let drawnTo = 0;
  for (const { from, to } of view.visibleRanges) {
    let number = Math.max(doc.lineAt(from).number, drawnTo + 1);
    const last = doc.lineAt(to).number;
    for (; number <= last; number += 1) {
      for (const mark of marksOnLine(state, number)) {
        if (mark.kind !== "task") {
          decorations.push(Decoration.replace({}).range(mark.from, mark.to));
          continue;
        }
        const box = mark.from + 3;
        const text = doc.sliceString(mark.from, box);
        decorations.push(
          Decoration.replace({
            widget:
              text === "[~]"
                ? new NotApplicableWidget()
                : new CheckboxWidget(
                    text.toLowerCase() === "[x]",
                    mark.from,
                    box,
                  ),
          }).range(mark.from, box),
        );
        if (box < mark.to) {
          decorations.push(Decoration.replace({}).range(box, mark.to));
        }
      }
    }
    drawnTo = last;
  }
  return Decoration.set(decorations, true);
}

const hiddenMarkPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.selectionSet ||
        update.viewportChanged ||
        parseAdvanced(update) ||
        parkingChanged(update) ||
        markModeChanged(update)
      ) {
        this.decorations = buildDecorations(update.view);
      }
    }
  },
  {
    decorations: (value) => value.decorations,
    // Hidden text is not a place a cursor may sit: arrowing through a folded
    // mark steps over it rather than into the middle of it.
    provide: (plugin) =>
      EditorView.atomicRanges.of(
        (view) => view.plugin(plugin)?.decorations ?? Decoration.none,
      ),
  },
);

/** A quiet tint over the frontmatter block, so the region reads as metadata. */
const frontmatterTint = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = this.tint(view);
    }

    update(update: ViewUpdate) {
      if (update.docChanged) {
        this.decorations = this.tint(update.view);
      }
    }

    tint(view: EditorView): DecorationSet {
      const region = frontmatterRegion(view.state.doc);
      if (!region) {
        return Decoration.none;
      }
      const lines: Range<Decoration>[] = [];
      const last = view.state.doc.lineAt(region.to).number;
      for (let number = 1; number <= last; number += 1) {
        const line = view.state.doc.line(number);
        lines.push(
          Decoration.line({ class: "cm-frontmatter" }).range(line.from),
        );
      }
      return Decoration.set(lines);
    }
  },
  { decorations: (value) => value.decorations },
);

const previewTheme = EditorView.baseTheme({
  ".cm-frontmatter": { color: "var(--color-slate-500)", fontSize: "0.85em" },
  ".cm-task-toggle": { verticalAlign: "middle", margin: "0 0.15em" },
  // A box the size of the checkbox beside it, struck through once.
  ".cm-task-na": {
    display: "inline-block",
    width: "0.8em",
    height: "0.8em",
    margin: "0 0.2em",
    verticalAlign: "-0.05em",
    border: "1px solid var(--color-slate-400)",
    borderRadius: "2px",
    background:
      "linear-gradient(var(--color-slate-500), var(--color-slate-500)) center / 100% 1px no-repeat",
  },
});

/**
 * The hidden marks drawn, for `hiddenMarkLayer()`: outside `livePreview()`, so
 * the table cell editor, which reads the prose layers, does not get it yet.
 */
export const hiddenMarkDrawing: Extension = hiddenMarkPlugin;

/** The whole layer, handed to the caller's on/off compartment. */
export function livePreview(): Extension {
  return [frontmatterTint, previewTheme];
}
