/**
 * `linkStyle` indices count the edges in mermaid's order, so an arrow added or
 * removed anywhere before an index makes that index mean another arrow. These
 * edits keep each index on its arrow and touch only the digits.
 */

import type { Span, TextEdit } from "../edits";
import type { DirectiveLine, FlowModel } from "./model";

/** The ranges that delete whole lines with one line break each. */
export function lineDeletions(
  model: FlowModel,
  indices: readonly number[],
): Span[] {
  const lines = model.scan.lines;
  const sorted = [...indices].sort((a, b) => a - b);
  const out: Span[] = [];
  for (let i = 0; i < sorted.length; i++) {
    const first = sorted[i] ?? 0;
    let last = first;
    while (sorted[i + 1] === last + 1) {
      last++;
      i++;
    }
    const next = lines[last + 1];
    const from = next ? lines[first]?.from : lines[first - 1]?.to;
    const to = next ? next.from : lines[last]?.to;
    if (from !== undefined && to !== undefined) {
      out.push({ from, to });
    }
  }
  return out;
}

/**
 * The range that takes one statement out of a line that keeps something else:
 * its neighbour's `;` goes too, and no other statement and no accDescr `}` goes
 * with it (R33).
 */
function statementDeletion(model: FlowModel, directive: DirectiveLine): Span {
  const statements = model.scan.lines[directive.line]?.statements ?? [];
  const at = statements.findIndex((s) => s.from === directive.span.from);
  const next = statements[at + 1];
  const previous = statements[at - 1];
  return next
    ? { from: directive.span.from, to: next.from }
    : previous
      ? { from: previous.to, to: directive.span.to }
      : directive.span;
}

/**
 * Digit edits for every numeric `linkStyle` index `>= at`: each moves by `delta`,
 * and with `removed` by `delta` once per removed index below it, so one call
 * renumbers a removal of several edges. A removed index goes with one adjacent
 * comma. A line whose indices are all removed goes with its line break, and when
 * the line holds anything else only the statement goes. `linkStyle default` has
 * no indices and is never touched.
 */
export function shiftLinkStyles(
  model: FlowModel,
  at: number,
  delta: number,
  removed?: ReadonlySet<number>,
): TextEdit[] {
  const gone = removed ?? new Set<number>();
  const moved = (n: number): number => {
    if (n < at) {
      return n;
    }
    if (removed === undefined) {
      return n + delta;
    }
    return n + delta * [...gone].filter((r) => r < n).length;
  };
  const edits: TextEdit[] = [];
  const wholeLines: number[] = [];
  for (const directive of model.directives) {
    if (directive.kind !== "linkStyle" || directive.indices.length === 0) {
      continue;
    }
    const indices = directive.indices;
    if (indices.every((index) => gone.has(index.n))) {
      const line = model.scan.lines[directive.line];
      if (line?.kind === "statement" && line.statements.length === 1) {
        wholeLines.push(directive.line);
      } else {
        edits.push({ ...statementDeletion(model, directive), insert: "" });
      }
      continue;
    }
    for (let i = 0; i < indices.length; i++) {
      const index = indices[i];
      if (index === undefined) {
        continue;
      }
      if (gone.has(index.n)) {
        let last = i;
        while (gone.has(indices[last + 1]?.n ?? -1)) {
          last++;
        }
        const next = indices[last + 1];
        const previous = indices[i - 1];
        const end = indices[last];
        if (next) {
          edits.push({ from: index.span.from, to: next.span.from, insert: "" });
        } else if (previous && end) {
          edits.push({ from: previous.span.to, to: end.span.to, insert: "" });
        }
        i = last;
      } else if (moved(index.n) !== index.n) {
        edits.push({ ...index.span, insert: String(moved(index.n)) });
      }
    }
  }
  return [
    ...edits,
    ...lineDeletions(model, wholeLines).map((span) => ({
      ...span,
      insert: "",
    })),
  ];
}
