/**
 * The ops on one arrow: its label, its line, its heads and its direction. Each
 * rewrites only the arrow's own bytes, or the two refs' texts for `reverse`.
 *
 * A link with a group on either side draws several arrows from one token, so no op
 * can change one of them on its own, and each refuses `group`.
 *
 * Every result goes through `checked` with the exact row each arrow must read as,
 * ends included. That is what stops the arrow token trap (proposal 11): `A-->xray`
 * with no heads would be `A---xray`, which mermaid and our parser both read as a
 * cross arrow to `ray`. Mermaid's db could not tell that from a real edit.
 */

import type { OpResult, Span, TextEdit } from "../edits";
import {
  type ArrowChange,
  arrowsAfter,
  checked,
  refused,
  sameBoxes,
} from "./checked";
import { cannotDraw, decodeLabel, encodeEdgeLabel, tidyLabel } from "./labels";
import type { Arrow, Chain, EdgeKey, FlowEdge, FlowModel, Head } from "./model";
import { edgeByKey } from "./parse";

type Line = Arrow["line"];

const HEAD_AT_START: Record<Head, string> = {
  none: "",
  arrow: "<",
  circle: "o",
  cross: "x",
};

const HEAD_AT_END: Record<Head, string> = {
  none: "",
  arrow: ">",
  circle: "o",
  cross: "x",
};

/**
 * The line of a plain token, without its heads. Mermaid reads a solid or thick
 * line's length from the characters before the last one, so a token with no head
 * at its end has one character more: `-->` and `---` are both length 1.
 */
function tokenLine(line: Line, length: number, end: Head): string {
  const extra = end === "none" ? 2 : 1;
  switch (line) {
    case "solid":
      return "-".repeat(length + extra);
    case "thick":
      return "=".repeat(length + extra);
    case "dotted":
      return `-${".".repeat(length)}-`;
    case "invisible":
      return "~".repeat(length + 2);
  }
}

/** The plain token for a line, heads and length. */
function plainToken(
  line: Line,
  start: Head,
  end: Head,
  length: number,
): string {
  return `${HEAD_AT_START[start]}${tokenLine(line, length, end)}${HEAD_AT_END[end]}`;
}

/** The opening token of the text form. */
function openerToken(line: Line, start: Head): string {
  const opener = { solid: "--", thick: "==", dotted: "-.", invisible: "~~" };
  return `${HEAD_AT_START[start]}${opener[line]}`;
}

/** The arrow's edge, or why an op on that one arrow cannot be made. */
function oneArrow(
  model: FlowModel,
  key: EdgeKey,
):
  | { edge: FlowEdge; chain: Chain; arrow: Arrow }
  | { refused: "gone" | "group" } {
  const edge = edgeByKey(model, key);
  const chain = model.chains[edge?.chain ?? -1];
  const link = chain?.links[edge?.link ?? -1];
  if (edge === null || chain === undefined || link === undefined) {
    return { refused: "gone" };
  }
  return link.edges.length > 1
    ? { refused: "group" }
    : { edge, chain, arrow: link.arrow };
}

function textOf(model: FlowModel, span: Span): string {
  return model.body.slice(span.from, span.to);
}

/** Replaces a span, leaving out an edit that changes nothing. */
function replacing(model: FlowModel, span: Span, insert: string): TextEdit[] {
  return textOf(model, span) === insert ? [] : [{ ...span, insert }];
}

/** The edits that give `arrow` this line and these heads, its length kept. */
function tokenEdits(
  model: FlowModel,
  arrow: Arrow,
  line: Line,
  start: Head,
  end: Head,
): TextEdit[] {
  if (arrow.form !== "text" || !arrow.opener || !arrow.closer) {
    return replacing(
      model,
      arrow.span,
      plainToken(line, start, end, arrow.length),
    );
  }
  const closer = textOf(model, arrow.closer);
  // `-. x .->` and `-. x -.->` read the same. Keep the form that is written.
  const written =
    arrow.line === "dotted" && line === "dotted" && closer.startsWith(".")
      ? plainToken(line, "none", end, arrow.length).slice(1)
      : plainToken(line, "none", end, arrow.length);
  return [
    ...replacing(model, arrow.opener, openerToken(line, start)),
    ...replacing(model, arrow.closer, written),
  ];
}

function changed(
  model: FlowModel,
  index: number,
  edits: TextEdit[],
  change: ArrowChange,
): OpResult {
  return checked(
    model,
    edits,
    arrowsAfter(model, new Set(), (edge) =>
      edge.index === index ? change : {},
    ),
  );
}

export function lineStyle(
  model: FlowModel,
  key: EdgeKey,
  line: "solid" | "dotted" | "thick",
): OpResult {
  const found = oneArrow(model, key);
  if ("refused" in found) {
    return found;
  }
  const { edge, arrow } = found;
  if (arrow.line === line) {
    return { edits: [] };
  }
  // An invisible link has no heads, and a line one can see needs one.
  const start = arrow.start;
  const end = arrow.line === "invisible" ? "arrow" : arrow.end;
  return changed(
    model,
    edge.index,
    tokenEdits(model, arrow, line, start, end),
    {
      line,
      startHead: start,
      endHead: end,
    },
  );
}

export function arrowhead(
  model: FlowModel,
  key: EdgeKey,
  head: "arrow" | "none" | "both",
): OpResult {
  const found = oneArrow(model, key);
  if ("refused" in found) {
    return found;
  }
  const { edge, arrow } = found;
  const start: Head = head === "both" ? "arrow" : "none";
  const end: Head = head === "none" ? "none" : "arrow";
  if (arrow.start === start && arrow.end === end) {
    return { edits: [] };
  }
  if (arrow.line === "invisible") {
    return refused("unsupported");
  }
  return changed(
    model,
    edge.index,
    tokenEdits(model, arrow, arrow.line, start, end),
    { startHead: start, endHead: end },
  );
}

/** Whether a label as written is a markdown string. */
function isMarkdownLabel(raw: string): boolean {
  return raw.startsWith('"`') && raw.endsWith('`"') && raw.length >= 4;
}

function removeLabel(model: FlowModel, index: number, arrow: Arrow): OpResult {
  if (arrow.label === null) {
    return { edits: [] };
  }
  const edits =
    arrow.form === "text" && arrow.opener && arrow.closer
      ? [
          {
            from: arrow.opener.from,
            to: arrow.closer.to,
            insert: plainToken(
              arrow.line,
              arrow.start,
              arrow.end,
              arrow.length,
            ),
          },
        ]
      : [{ from: arrow.label.from - 1, to: arrow.label.to + 1, insert: "" }];
  return changed(model, index, edits, { label: null });
}

/**
 * The label to try, and quoted as the second try: a text form ends at the first
 * thing that reads as its closer, and `-- a--b -->` or a tight `-- foo-->` only
 * reads as written in quotes.
 */
function labelForms(arrow: Arrow, typed: string, markdown: boolean): string[] {
  const first = encodeEdgeLabel(typed, arrow.labelQuoted, markdown);
  const second = encodeEdgeLabel(typed, true, markdown);
  return first === second ? [first] : [first, second];
}

export function edgeLabel(
  model: FlowModel,
  key: EdgeKey,
  text: string,
): OpResult {
  const found = oneArrow(model, key);
  if ("refused" in found) {
    return found;
  }
  const { edge, arrow } = found;
  const typed = tidyLabel(text);
  if (typed === "") {
    return removeLabel(model, edge.index, arrow);
  }
  if (cannotDraw(typed)) {
    return refused("invalid");
  }
  const raw = arrow.label ? textOf(model, arrow.label) : null;
  if (
    raw !== null &&
    tidyLabel(decodeLabel(raw, arrow.labelQuoted)) === typed
  ) {
    return { edits: [] };
  }
  const markdown = raw !== null && isMarkdownLabel(raw);
  let result: OpResult = refused("invalid");
  for (const written of labelForms(arrow, typed, markdown)) {
    if (tidyLabel(decodeLabel(written, written.startsWith('"'))) !== typed) {
      continue;
    }
    const edits: TextEdit[] = arrow.label
      ? [{ ...arrow.label, insert: written }]
      : [{ from: arrow.span.to, to: arrow.span.to, insert: `|${written}|` }];
    result = changed(model, edge.index, edits, { label: written });
    if ("edits" in result) {
      return result;
    }
  }
  return result;
}

export function reverse(model: FlowModel, key: EdgeKey): OpResult {
  const edge = edgeByKey(model, key);
  const chain = model.chains[edge?.chain ?? -1];
  if (edge === null || chain === undefined) {
    return refused("gone");
  }
  if (chain.links.length > 1) {
    return refused("chain");
  }
  const left = chain.groups[0];
  const right = chain.groups[1];
  const leftRef = left?.refs[0];
  const rightRef = right?.refs[0];
  if (
    leftRef === undefined ||
    rightRef === undefined ||
    left?.refs.length !== 1 ||
    right?.refs.length !== 1
  ) {
    return refused("group");
  }
  if (leftRef.id === rightRef.id) {
    return { edits: [] };
  }
  const edits: TextEdit[] = [
    { ...leftRef.span, insert: textOf(model, rightRef.span) },
    { ...rightRef.span, insert: textOf(model, leftRef.span) },
  ];
  return checked(model, edits, {
    // Mermaid meets the two boxes in the other order now.
    boxes: sameBoxes(model),
    ...arrowsAfter(model, new Set(), (other) =>
      other.index === edge.index ? { start: edge.end, end: edge.start } : {},
    ),
  });
}
