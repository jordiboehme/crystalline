/**
 * The self-check every flowchart op goes through before it answers: the edits
 * are applied, the result is parsed again, and it must be understood and differ
 * from the model in exactly the fields the op names. It lives apart from `ops.ts`
 * so that the op modules can use it without importing each other.
 *
 * Spans are read from `model.body`, the string they point into.
 */

import {
  type OpResult,
  type RefusalCode,
  type TextEdit,
  applyEdits,
  sortedEdits,
} from "../edits";
import { droppedSemicolons } from "../scan";
import { classesOf, stylesOf } from "../verdict";
import { decodeLabel } from "./labels";
import type { FlowEdge, FlowModel, FlowNode, Shape } from "./model";
import { parseFlowchart } from "./parse";

export const refused = (code: RefusalCode): OpResult<never> => ({
  refused: code,
});

export const COLOUR_KEYS = ["fill", "stroke", "color"] as const;

/** The direction a header value reads as: `TD`, `v` and none are `TB`. */
export function readDirection(value: string | undefined): string {
  return value === undefined
    ? "TB"
    : ({ TD: "TB", v: "TB", ">": "LR", "<": "RL", "^": "BT" }[value] ?? value);
}

/** The text of a label as written, as the rename field shows it. */
export function shownLabel(body: string, shape: Shape): string {
  return decodeLabel(
    body.slice(shape.label.from, shape.label.to),
    shape.quoted,
  );
}

/** What the shape field of a box reads: its name, or the `@{ }` name as written. */
function shownShape(body: string, node: FlowNode): string {
  const shape = node.defining?.shape;
  if (!shape) {
    return "";
  }
  return shape.atShape
    ? `at:${body.slice(shape.atShape.from, shape.atShape.to)}`
    : `${shape.name}`;
}

/** The fill, stroke and color a box draws with: the last value of each, as CSS reads them. */
function coloursOf(styles: readonly string[]): (string | null)[] {
  return COLOUR_KEYS.map((key) => {
    const entry = styles.findLast(
      (s) => s.slice(0, s.indexOf(":")).trim().toLowerCase() === key,
    );
    return entry === undefined
      ? null
      : entry.slice(entry.indexOf(":") + 1).trim();
  });
}

/** One row of the `arrows` field per edge, what a reading of the arrow must keep. */
export function arrowRows(model: FlowModel): unknown[][] {
  const body = model.body;
  return model.edges.map((edge) => {
    const arrow = model.chains[edge.chain]?.links[edge.link]?.arrow;
    return [
      edge.start,
      edge.end,
      edge.mermaidId,
      arrow?.line,
      arrow?.start,
      arrow?.end,
      arrow?.length,
      arrow?.label ? body.slice(arrow.label.from, arrow.label.to) : null,
    ];
  });
}

/** The `subgraphs` field, with `added` as one more member of the subgraph `into`. */
export function subgraphRows(
  model: FlowModel,
  into: string | null = null,
  added = "",
): string {
  return JSON.stringify(
    model.subgraphs.map((s) => [
      s.id,
      s.parent,
      (s.id === into ? [...s.members, added] : [...s.members]).sort(),
    ]),
  );
}

/** Which arrow each `linkStyle` index means, line by line. */
function linkStyleRows(model: FlowModel): unknown[] {
  return model.directives
    .filter((d) => d.kind === "linkStyle")
    .map((d) =>
      d.indices.map(({ n }) => {
        const edge = model.edges[n];
        return [edge?.start, edge?.end, edge?.occurrence];
      }),
    );
}

/** What an arrow reads as after an op, in the order of `arrowRows`' columns. */
export interface ArrowChange {
  start?: string;
  end?: string;
  line?: string;
  startHead?: string;
  endHead?: string;
  label?: string | null;
}

/**
 * What the `arrows` and `linkStyles` fields must read after an op that takes the
 * edges `gone` away and changes others as `change` says. A generated id follows
 * mermaid's count of that pair, so a later arrow of the pair takes the number of
 * the one that went; an explicit id stays exactly where it was. So an id that
 * would move to another arrow (the id of `A & B e1@--> C` without B) is refused.
 */
export function arrowsAfter(
  model: FlowModel,
  gone: ReadonlySet<number>,
  change: (edge: FlowEdge) => ArrowChange = () => ({}),
): { arrows: string; linkStyles: string } {
  const rows = arrowRows(model);
  const counts = new Map<string, number>();
  const kept: unknown[][] = [];
  const meaning = new Map<number, unknown[]>();
  for (const edge of model.edges) {
    const row = rows[edge.index];
    if (row === undefined || gone.has(edge.index)) {
      continue;
    }
    const wanted = change(edge);
    const start = wanted.start ?? edge.start;
    const end = wanted.end ?? edge.end;
    const pair = `${start}\u0000${end}`;
    const k = counts.get(pair) ?? 0;
    counts.set(pair, k + 1);
    const generated = isGeneratedId(edge);
    kept.push([
      start,
      end,
      generated ? `L_${start}_${end}_${k === 0 ? 0 : k + 1}` : edge.mermaidId,
      wanted.line ?? row[3],
      wanted.startHead ?? row[4],
      wanted.endHead ?? row[5],
      row[6],
      wanted.label === undefined ? row[7] : wanted.label,
    ]);
    meaning.set(edge.index, [start, end, k]);
  }
  const linkStyles = model.directives
    .filter((d) => d.kind === "linkStyle")
    // A line whose indices are all gone goes itself.
    .filter(
      (d) => d.indices.length === 0 || d.indices.some(({ n }) => !gone.has(n)),
    )
    .map((d) =>
      d.indices.filter(({ n }) => !gone.has(n)).map(({ n }) => meaning.get(n)),
    );
  return {
    arrows: JSON.stringify(kept),
    linkStyles: JSON.stringify(linkStyles),
  };
}

function isGeneratedId(edge: FlowEdge): boolean {
  const prefix = `L_${edge.start}_${edge.end}_`;
  return (
    edge.mermaidId.startsWith(prefix) &&
    /^\d+$/.test(edge.mermaidId.slice(prefix.length))
  );
}

/** A test for the `boxes` field: the same boxes, whatever their order, without `gone`. */
export function sameBoxes(
  model: FlowModel,
  gone: string | null = null,
): (value: string) => boolean {
  const wanted = JSON.stringify(
    model.nodeOrder.filter((id) => id !== gone).sort(),
  );
  return (value) =>
    JSON.stringify((JSON.parse(value) as string[]).sort()) === wanted;
}

/** The fields one box has, by name. */
const BOX_FIELDS = [
  "label",
  "markdown",
  "shape",
  "styles",
  "colours",
  "classes",
] as const;

/**
 * The model as named fields, the same parts `agreesFlow` compares, so that two
 * models can be compared field by field.
 */
function fieldsOf(model: FlowModel): Map<string, string> {
  const body = model.body;
  const styles = stylesOf(model);
  const classes = classesOf(model);
  const fields = new Map<string, string>([
    ["direction", readDirection(model.direction?.value)],
    ["boxes", JSON.stringify(model.nodeOrder)],
    ["arrows", JSON.stringify(arrowRows(model))],
    ["subgraphs", subgraphRows(model)],
    ["linkStyles", JSON.stringify(linkStyleRows(model))],
  ]);
  for (const [id, node] of model.nodes) {
    const shape = node.defining?.shape ?? null;
    fields.set(`${id} label`, shape ? shownLabel(body, shape) : id);
    fields.set(`${id} markdown`, String(shape?.markdown ?? false));
    fields.set(`${id} shape`, shownShape(body, node));
    fields.set(`${id} styles`, JSON.stringify(styles.get(id) ?? []));
    fields.set(
      `${id} colours`,
      JSON.stringify(coloursOf(styles.get(id) ?? [])),
    );
    fields.set(`${id} classes`, JSON.stringify(classes.get(id) ?? []));
  }
  return fields;
}

/** The offsets of the `;`s mermaid's `encodeEntities` drops from `model`, with the end of each one's line. */
function droppedIn(model: FlowModel): { at: number; lineEnd: number }[] {
  const out: { at: number; lineEnd: number }[] = [];
  for (const line of model.scan.lines) {
    if (
      line.kind === "statement" ||
      line.kind === "header" ||
      line.kind === "acc"
    ) {
      const text = model.body.slice(line.from, line.to);
      for (const i of droppedSemicolons(text)) {
        out.push({ at: line.from + i, lineEnd: line.to });
      }
    }
  }
  return out;
}

/** Where `at` of the edited text was before `edits`, null inside an inserted text. */
function beforeEdits(edits: readonly TextEdit[], at: number): number | null {
  let shift = 0;
  for (const edit of sortedEdits(edits, Number.MAX_SAFE_INTEGER)) {
    const from = edit.from + shift;
    if (at < from) {
      break;
    }
    if (at < from + edit.insert.length) {
      return null;
    }
    shift += edit.insert.length - (edit.to - edit.from);
  }
  return at - shift;
}

/**
 * Whether the edited text makes mermaid drop a `;` the body kept, with more
 * than space after it on the line. Nothing in the source says so and the db
 * shows it only in fields far from the edit: a rename to `style:#b` takes the
 * `;` out of a click URL, a tooltip or a subgraph title on the same line (R9).
 */
function dropsNewSemicolon(
  before: FlowModel,
  after: FlowModel,
  edits: readonly TextEdit[],
): boolean {
  const kept = new Set(droppedIn(before).map((d) => d.at));
  return droppedIn(after).some(({ at, lineEnd }) => {
    if (after.body.slice(at + 1, lineEnd).trim() === "") {
      return false;
    }
    const was = beforeEdits(edits, at);
    return was === null || !kept.has(was);
  });
}

/** What a changed field must hold: this value, any value (null), or what this test accepts. */
export type Wanted = string | null | ((value: string) => boolean);

function holds(wanted: Wanted, value: string): boolean {
  return typeof wanted === "function"
    ? wanted(value)
    : wanted === null || wanted === value;
}

/**
 * `edits`, when applying them leaves a body that is understood and differs from
 * `model` only in the fields `changes` names, each then holding what is named
 * there. A field that was not there before must be named. No field may go, except
 * the fields of the one box `goneBox`, and those must go. Otherwise `invalid`.
 */
export function checked(
  model: FlowModel,
  edits: TextEdit[],
  changes: Readonly<Record<string, Wanted>>,
  goneBox: string | null = null,
): OpResult<never> {
  if (edits.length === 0) {
    return { edits };
  }
  const after = parseFlowchart(applyEdits(model.body, edits));
  if (!after.understood || dropsNewSemicolon(model, after, edits)) {
    return refused("invalid");
  }
  const before = fieldsOf(model);
  const now = fieldsOf(after);
  const going = new Set(
    goneBox === null ? [] : BOX_FIELDS.map((field) => `${goneBox} ${field}`),
  );
  for (const field of before.keys()) {
    if (now.has(field) === !going.has(field)) {
      continue;
    }
    return refused("invalid");
  }
  for (const [field, value] of now) {
    const wanted = changes[field];
    if (
      wanted === undefined ? before.get(field) !== value : !holds(wanted, value)
    ) {
      return refused("invalid");
    }
  }
  return { edits };
}
