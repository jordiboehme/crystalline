/**
 * `linkStyle` indices count the edges in mermaid's order, so an arrow added or
 * removed anywhere before an index makes that index mean another arrow. These
 * edits keep each index on its arrow and touch only the digits.
 */

import type { Span, TextEdit } from "../edits";
import type { FlowModel } from "./model";

/** The ranges that delete whole lines with one line break each. */
export function lineDeletions(
  model: Pick<FlowModel, "scan">,
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

/** Per line, the `from` of each statement that goes. */
export type GoneStatements = Map<number, Set<number>>;

/** Marks one statement of a line as going. */
export function markGone(
  gone: GoneStatements,
  line: number,
  statementFrom: number,
): void {
  gone.set(line, (gone.get(line) ?? new Set<number>()).add(statementFrom));
}

/**
 * The ranges that take every statement of `gone` out of the body. A line left
 * with nothing goes with its line break, through one `lineDeletions` call so that
 * neighbouring lines cannot overlap. When the line holds anything else, only the
 * statements go.
 */
export function goneStatementRanges(
  model: FlowModel,
  gone: GoneStatements,
): Span[] {
  const wholeLines: number[] = [];
  const parts: Span[] = [];
  for (const [index, statements] of gone) {
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
  return [...lineDeletions(model, wholeLines), ...parts];
}

/**
 * The digit edits of `shiftLinkStyles`. A statement whose indices are all removed
 * is added to `gone` instead, so a caller that removes other statements too can
 * take them all out in one go.
 */
export function shiftedIndices(
  model: FlowModel,
  at: number,
  delta: number,
  removed: ReadonlySet<number> | undefined,
  gone: GoneStatements,
): TextEdit[] {
  const removedIndices = removed ?? new Set<number>();
  const moved = (n: number): number => {
    if (n < at) {
      return n;
    }
    if (removed === undefined) {
      return n + delta;
    }
    return n + delta * [...removedIndices].filter((r) => r < n).length;
  };
  const edits: TextEdit[] = [];
  for (const directive of model.directives) {
    if (directive.kind !== "linkStyle" || directive.indices.length === 0) {
      continue;
    }
    const indices = directive.indices;
    if (indices.every((index) => removedIndices.has(index.n))) {
      markGone(gone, directive.line, directive.span.from);
      continue;
    }
    for (let i = 0; i < indices.length; i++) {
      const index = indices[i];
      if (index === undefined) {
        continue;
      }
      if (removedIndices.has(index.n)) {
        let last = i;
        while (removedIndices.has(indices[last + 1]?.n ?? -1)) {
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
  return edits;
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
  const gone: GoneStatements = new Map();
  const edits = shiftedIndices(model, at, delta, removed, gone);
  return [
    ...edits,
    ...goneStatementRanges(model, gone).map((span) => ({
      ...span,
      insert: "",
    })),
  ];
}
