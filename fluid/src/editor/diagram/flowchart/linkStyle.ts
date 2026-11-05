/**
 * `linkStyle` indices count the edges in mermaid's order, so an arrow added or
 * removed anywhere before an index makes that index mean another arrow. These
 * edits keep each index on its arrow and touch only the digits.
 */

import type { Span, TextEdit } from "../edits";
import type { FlowModel } from "./model";

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
 * The ranges that take the statements `gone` (by their `from`) out of a line that
 * keeps something else. Neighbours go as one run, with the `;` beside the run, and
 * no other statement and no accDescr `}` goes with them (R33).
 */
function statementDeletions(
  statements: readonly Span[],
  gone: ReadonlySet<number>,
): Span[] {
  const out: Span[] = [];
  for (let i = 0; i < statements.length; i++) {
    const first = statements[i];
    if (first === undefined || !gone.has(first.from)) {
      continue;
    }
    let end = i;
    while (gone.has(statements[end + 1]?.from ?? -1)) {
      end++;
    }
    const last = statements[end] ?? first;
    const next = statements[end + 1];
    const previous = statements[i - 1];
    out.push(
      next
        ? { from: first.from, to: next.from }
        : previous
          ? { from: previous.to, to: last.to }
          : { from: first.from, to: last.to },
    );
    i = end;
  }
  return out;
}

/**
 * Digit edits for every numeric `linkStyle` index `>= at`: each moves by `delta`,
 * and with `removed` by `delta` once per removed index below it, so one call
 * renumbers a removal of several edges. A removed index goes with one adjacent
 * comma. A line left with nothing goes with its line break, and when the line
 * holds anything else only the statements go. `linkStyle default` has no indices
 * and is never touched.
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
  /** Per line, the `from` of each statement whose indices are all removed. */
  const emptied = new Map<number, Set<number>>();
  for (const directive of model.directives) {
    if (directive.kind !== "linkStyle" || directive.indices.length === 0) {
      continue;
    }
    const indices = directive.indices;
    if (indices.every((index) => gone.has(index.n))) {
      const statements = emptied.get(directive.line) ?? new Set<number>();
      emptied.set(directive.line, statements.add(directive.span.from));
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
  const wholeLines: number[] = [];
  const parts: Span[] = [];
  for (const [index, statements] of emptied) {
    const line = model.scan.lines[index];
    if (
      line?.kind === "statement" &&
      line.statements.every((s) => statements.has(s.from))
    ) {
      wholeLines.push(index);
    } else {
      parts.push(...statementDeletions(line?.statements ?? [], statements));
    }
  }
  return [
    ...edits,
    ...[...lineDeletions(model, wholeLines), ...parts].map((span) => ({
      ...span,
      insert: "",
    })),
  ];
}
