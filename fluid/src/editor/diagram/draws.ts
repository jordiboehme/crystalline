/**
 * What the eager side of the diagram layer shares with the lazy chunk: which
 * kinds of diagram the layer edits, how a fence body says its kind, and the
 * two facets the chunk, the preview layers and the code box talk through.
 * Eager, so it imports nothing of the chunk.
 */

import type { EditorState } from "@codemirror/state";
import { Facet } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

/**
 * The diagram kinds the layer draws and edits. 4b adds `sequenceDiagram`.
 * `flowchart-elk` stays out: no layout loader is registered for it and no
 * recorded SVG pins its element ids, so nothing proves they map.
 */
export const EDITABLE_KINDS: readonly string[] = ["flowchart", "graph"];

/**
 * The first keyword of a fence body once the frontmatter, the directives, the
 * comments and the blank lines are past (the lines mermaid removes before it
 * reads a header), or null when nothing else is there. Written the way the
 * body is: `flowchart-elk TD` is `flowchart-elk`, `graph LR;` is `graph`.
 */
export function diagramKindOf(body: string): string | null {
  const lines = body.split("\n");
  let index = 0;
  // Frontmatter only from the first line, closed by the same indent and `---`.
  const first = /^([ \t]*)---[ \t]*$/.exec(lines[0] ?? "");
  if (first !== null) {
    const close = lines.findIndex(
      (line, at) =>
        at > 0 && line.replace(/[ \t]+$/, "") === `${first[1] ?? ""}---`,
    );
    if (close > 0) {
      index = close + 1;
    }
  }
  while (index < lines.length) {
    const line = lines[index] ?? "";
    if (line.trim() === "") {
      index += 1;
    } else if (/^\s*%%\{/.test(line)) {
      // A directive runs to the next `}%%`, across lines.
      while (index < lines.length && !(lines[index] ?? "").includes("}%%")) {
        index += 1;
      }
      index += 1;
    } else if (/^\s*%%/.test(line)) {
      index += 1;
    } else {
      return /^\s*([A-Za-z][\w-]*)/.exec(line)?.[1] ?? null;
    }
  }
  return null;
}

/**
 * Whether the diagram chunk is loaded and in this state. Provided by the
 * chunk's `diagramExtension()`.
 */
export const diagramLoaded = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

function sameNumbers(a: readonly number[], b: readonly number[]): boolean {
  return a.length === b.length && a.every((value, at) => value === b[at]);
}

/**
 * The `from` positions of the fences the diagram layer draws, in document
 * order. The chunk computes it from its field; `fencePreviews` reads it to
 * leave a drawn fence without the read-only preview. `compare` keeps the
 * value (and so the reference) when a rebuild finds the same positions, so a
 * reader asking `startState !== state` sees only real changes.
 */
export const drawnDiagrams = Facet.define<readonly number[], readonly number[]>(
  {
    combine: (values) => values.flat(),
    compare: sameNumbers,
  },
);

/** The lines a drawn fence takes: its first line's start, its last line's end. */
export interface DrawnLines {
  from: number;
  to: number;
}

/** The drawn fences by their lines, in document order. The chunk computes it. */
export const drawnFenceLines = Facet.define<
  readonly DrawnLines[],
  readonly DrawnLines[]
>({
  combine: (values) => values.flat(),
  compare: (a, b) =>
    a.length === b.length &&
    a.every((v, at) => v.from === b[at]?.from && v.to === b[at]?.to),
});

/** Provided by the chunk: takes the keyboard to the canvas of the fence at `pos`. */
export const enterDrawnFence = Facet.define<
  (view: EditorView, pos: number) => void,
  ((view: EditorView, pos: number) => void) | null
>({ combine: (values) => values[0] ?? null });

/**
 * The drawn fence right across the line edge `pos`: on the next line going
 * `forward` from a line's end, on the line before going back from its start.
 */
export function drawnFenceBeside(
  state: EditorState,
  pos: number,
  forward: boolean,
): DrawnLines | null {
  const line = state.doc.lineAt(pos);
  if (pos !== (forward ? line.to : line.from)) {
    return null;
  }
  return (
    state
      .facet(drawnFenceLines)
      .find((f) =>
        forward ? f.from === line.to + 1 : f.to === line.from - 1,
      ) ?? null
  );
}

/**
 * The hand-off from the hidden-mark keys (R13): a key at the line edge `pos`
 * that would cross into a drawn fence takes the keyboard to its canvas and
 * changes nothing. False when no drawn fence is there.
 */
export function stepIntoDrawnFence(
  view: EditorView,
  pos: number,
  forward: boolean,
): boolean {
  const { state } = view;
  const enter = state.facet(enterDrawnFence);
  const fence = enter === null ? null : drawnFenceBeside(state, pos, forward);
  if (enter === null || fence === null) {
    return false;
  }
  enter(view, fence.from);
  return true;
}

/**
 * What the chunk's open rename field does when the room closes: writes its text
 * into the document if it can, and answers with what it could not keep.
 */
export const fieldSettlers = new WeakMap<
  EditorView,
  () => Promise<string | null>
>();

/**
 * Settles the view's open rename field, or null at once with none open. The answer
 * is the text that could not be kept, null when nothing was lost (R100).
 */
export function settleFieldDraft(
  view: EditorView,
): Promise<string | null> | null {
  return fieldSettlers.get(view)?.() ?? null;
}
