/**
 * What the eager side of the diagram layer shares with the lazy chunk: which
 * kinds of diagram the layer edits, how a fence body says its kind, and the
 * two facets the chunk, the preview layers and the code box talk through.
 * Eager, so it imports nothing of the chunk.
 */

import { Facet } from "@codemirror/state";

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
