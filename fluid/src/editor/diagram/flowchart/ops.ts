/**
 * Flowchart ops compiled to minimal text edits: an op changes only the bytes
 * it means, every other line stays as written, and when that cannot be
 * promised it refuses.
 *
 * Every compile checks itself before it answers: the edits are applied, the
 * result is parsed again, and it must be understood and differ from the model
 * in exactly the field the op names. A shape change that would turn `A(/x/)`
 * into the lean shape `A[/x/]`, or a rename that would put a `direction LR` in
 * a line (R7), comes back `invalid` here, before mermaid is asked. The apply
 * layer then asks mermaid's db as well.
 *
 * Spans are read from `model.body`, the string they point into.
 */

import {
  type OpResult,
  type RefusalCode,
  type Span,
  type TextEdit,
  applyEdits,
} from "../edits";
import { droppedSemicolons } from "../scan";
import { classesOf, stylesOf } from "../verdict";
import {
  COLOUR_PRESETS,
  type ColourPreset,
  SHAPES,
  decodeLabel,
  encodeLabel,
  encodeMarkdownLabel,
} from "./labels";
import {
  type DirectiveLine,
  type EdgeKey,
  type FlowModel,
  type FlowNode,
  type Shape,
  type ShapeName,
} from "./model";
import { parseFlowchart } from "./parse";

export type FlowOp =
  | { op: "rename"; id: string; text: string }
  | { op: "shape"; id: string; shape: ShapeName }
  | { op: "colour"; id: string; colour: ColourPreset | null }
  | { op: "direction"; value: "TD" | "LR" }
  | { op: "addChild"; id: string }
  | { op: "addSibling"; id: string }
  | { op: "connect"; from: string; to: string }
  | { op: "addBox" }
  | { op: "deleteNode"; id: string }
  | { op: "deleteEdge"; edge: EdgeKey }
  | { op: "edgeLabel"; edge: EdgeKey; text: string }
  | { op: "lineStyle"; edge: EdgeKey; line: "solid" | "dotted" | "thick" }
  | { op: "arrowhead"; edge: EdgeKey; head: "arrow" | "none" | "both" }
  | { op: "reverse"; edge: EdgeKey };

const refused = (code: RefusalCode): OpResult => ({ refused: code });

const COLOUR_KEYS = ["fill", "stroke", "color"] as const;

function isColourKey(key: string): boolean {
  return (COLOUR_KEYS as readonly string[]).includes(key.toLowerCase());
}

/** The direction a header value reads as: `TD`, `v` and none are `TB`. */
function readDirection(value: string | undefined): string {
  return value === undefined
    ? "TB"
    : ({ TD: "TB", v: "TB", ">": "LR", "<": "RL", "^": "BT" }[value] ?? value);
}

/** The text of a label as written, as the rename field shows it. */
function shownLabel(body: string, shape: Shape): string {
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
    [
      "arrows",
      JSON.stringify(
        model.edges.map((edge) => {
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
        }),
      ),
    ],
    [
      "subgraphs",
      JSON.stringify(
        model.subgraphs.map((s) => [s.id, s.parent, [...s.members].sort()]),
      ),
    ],
  ]);
  for (const [id, node] of model.nodes) {
    const shape = node.defining?.shape ?? null;
    fields.set(`${id} label`, shape ? shownLabel(body, shape) : id);
    fields.set(`${id} markdown`, String(shape?.markdown ?? false));
    fields.set(`${id} shape`, shownShape(body, node));
    fields.set(`${id} styles`, JSON.stringify(styles.get(id) ?? []));
    fields.set(`${id} classes`, JSON.stringify(classes.get(id) ?? []));
  }
  return fields;
}

/**
 * Whether mermaid's `encodeEntities` would drop a `;` inside a box or arrow of
 * `model`: a label that spells `style a:#b; c` loses its `;` before the parse,
 * so mermaid would store other text than the source says (R9).
 */
function dropsInsideChain(model: FlowModel): boolean {
  return model.chains.some((chain) => {
    const line = model.scan.lines[chain.line];
    if (line === undefined) {
      return false;
    }
    const text = model.body.slice(line.from, line.to);
    return [...droppedSemicolons(text)].some((i) => {
      const at = line.from + i;
      return at >= chain.span.from && at < chain.span.to;
    });
  });
}

/**
 * `edits`, when applying them leaves a body that is understood and differs from
 * `model` only in the fields `changes` names, each then holding the value named
 * there (null: any value). Otherwise `invalid`.
 */
function checked(
  model: FlowModel,
  edits: TextEdit[],
  changes: Readonly<Record<string, string | null>>,
): OpResult {
  if (edits.length === 0) {
    return { edits };
  }
  const after = parseFlowchart(applyEdits(model.body, edits));
  if (!after.understood || dropsInsideChain(after)) {
    return refused("invalid");
  }
  const before = fieldsOf(model);
  const now = fieldsOf(after);
  if (before.size !== now.size) {
    return refused("invalid");
  }
  for (const [field, value] of now) {
    const wanted = changes[field];
    if (
      wanted === undefined
        ? before.get(field) !== value
        : wanted !== null && wanted !== value
    ) {
      return refused("invalid");
    }
  }
  return { edits };
}

/** The label to write for `text` in a place that holds `shape`. */
function encodedFor(text: string, shape: Shape): string {
  if (shape.markdown) {
    return encodeMarkdownLabel(text);
  }
  if (shape.form === "at") {
    // The parser's bare `@{ }` value, the only one YAML cannot read as something else.
    const bare =
      /^[A-Za-z][A-Za-z0-9 _.-]*$/.test(text) &&
      !/^\s|\s$|^(?:true|false|null)$/i.test(text);
    return encodeLabel(text, shape.quoted || !bare);
  }
  return encodeLabel(text, shape.quoted);
}

function rename(model: FlowModel, node: FlowNode, text: string): OpResult {
  if (text.trim() === "") {
    return refused("invalid");
  }
  const shape = node.defining?.shape ?? null;
  const ref = node.refs[0];
  if (ref === undefined) {
    return refused("unsupported");
  }
  const field = `${node.id} label`;
  if (shape === null) {
    const encoded = encodeLabel(text, false);
    const shown = decodeLabel(encoded, encoded.startsWith('"'));
    if (shown === node.id) {
      return { edits: [] };
    }
    const at = ref.idSpan.to;
    return checked(model, [{ from: at, to: at, insert: `[${encoded}]` }], {
      [field]: shown,
      [`${node.id} shape`]: "rect",
    });
  }
  const encoded = encodedFor(text, shape);
  const shown = decodeLabel(encoded, encoded.startsWith('"'));
  if (shown === shownLabel(model.body, shape)) {
    return { edits: [] };
  }
  return checked(
    model,
    [{ from: shape.label.from, to: shape.label.to, insert: encoded }],
    { [field]: shown },
  );
}

function changeShape(
  model: FlowModel,
  node: FlowNode,
  name: ShapeName,
): OpResult {
  const target = SHAPES.find((s) => s.name === name);
  const ref = node.refs[0];
  if (target === undefined) {
    return refused("invalid");
  }
  if (ref === undefined) {
    return refused("unsupported");
  }
  const shape = node.defining?.shape ?? null;
  const field = `${node.id} shape`;
  if (shape === null) {
    // A bare box already draws as a rectangle (mermaid's `case void 0`).
    if (name === "rect") {
      return { edits: [] };
    }
    const at = ref.idSpan.to;
    return checked(
      model,
      [
        {
          from: at,
          to: at,
          insert: `${target.open}${encodeLabel(node.id, false)}${target.close}`,
        },
      ],
      { [field]: name },
    );
  }
  if (shape.name === name) {
    return { edits: [] };
  }
  // Mermaid's lexer reads `(-` as the ellipse's opener wherever a `(` ends the
  // opener, as in `A((-a))` (R5), so that label never goes after one.
  if (
    !shape.atShape &&
    target.open.endsWith("(") &&
    model.body.startsWith("-", shape.label.from)
  ) {
    return refused("invalid");
  }
  if (shape.atShape) {
    return checked(
      model,
      [{ from: shape.atShape.from, to: shape.atShape.to, insert: target.at }],
      { [field]: `at:${target.at}` },
    );
  }
  return checked(
    model,
    [
      { from: shape.open.from, to: shape.open.to, insert: target.open },
      { from: shape.close.from, to: shape.close.to, insert: target.close },
    ],
    { [field]: name },
  );
}

/** Where a new `style` line for `node` goes, as the end of the line it follows. */
function styleLineAnchor(model: FlowModel, node: FlowNode): number {
  const lines = model.scan.lines;
  const firstRef = node.refs[0]?.line ?? -1;
  let anchor = -1;
  for (const directive of model.directives) {
    if (["style", "classDef", "class"].includes(directive.kind)) {
      anchor = Math.max(anchor, directive.line);
    }
  }
  // Above the box's first ref it would create the vertex early and move the
  // box in mermaid's order, so it goes to the end of the body instead.
  if (anchor < firstRef || anchor < 0) {
    anchor = lines.findLastIndex((line) => line.kind !== "blank");
  }
  return lines[anchor]?.to ?? model.body.length;
}

function setColour(
  model: FlowModel,
  node: FlowNode,
  styleLines: readonly DirectiveLine[],
  name: ColourPreset,
): OpResult {
  const preset = COLOUR_PRESETS[name];
  const field = `${node.id} styles`;
  const last = styleLines.at(-1);
  if (last === undefined) {
    const indent =
      model.scan.lines.find((line) => line.kind === "statement")?.indent ??
      "    ";
    const at = styleLineAnchor(model, node);
    const line = `style ${node.id} fill:${preset.fill},stroke:${preset.stroke},color:${preset.color}`;
    return checked(
      model,
      [{ from: at, to: at, insert: `\n${indent}${line}` }],
      {
        [field]: null,
      },
    );
  }
  const edits: TextEdit[] = [];
  let appended = "";
  for (const key of COLOUR_KEYS) {
    const entries = last.props.filter((p) => p.key.toLowerCase() === key);
    if (entries.length === 0) {
      appended += `,${key}:${preset[key]}`;
    }
    for (const entry of entries) {
      if (model.body.slice(entry.value.from, entry.value.to) !== preset[key]) {
        edits.push({
          from: entry.value.from,
          to: entry.value.to,
          insert: preset[key],
        });
      }
    }
  }
  const end = last.props.at(-1)?.entry.to ?? last.span.to;
  if (appended !== "") {
    edits.push({ from: end, to: end, insert: appended });
  }
  return checked(model, edits, { [field]: null });
}

/** The ranges that take the colour entries out of a line that keeps others. */
function entryDeletions(line: DirectiveLine): Span[] {
  const props = line.props;
  const out: Span[] = [];
  for (let i = 0; i < props.length; i++) {
    if (!isColourKey(props[i]?.key ?? "")) {
      continue;
    }
    let end = i;
    while (isColourKey(props[end + 1]?.key ?? "")) {
      end++;
    }
    const first = props[i];
    const last = props[end];
    const next = props[end + 1];
    const previous = props[i - 1];
    if (first && last) {
      if (next) {
        out.push({ from: first.entry.from, to: next.entry.from });
      } else if (previous) {
        out.push({ from: previous.entry.to, to: last.entry.to });
      }
    }
    i = end;
  }
  return out;
}

/** The ranges that delete whole lines with one line break each. */
function lineDeletions(model: FlowModel, indices: readonly number[]): Span[] {
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

function clearColour(
  model: FlowModel,
  node: FlowNode,
  styleLines: readonly DirectiveLine[],
): OpResult {
  const whole: number[] = [];
  const parts: Span[] = [];
  for (const directive of styleLines) {
    if (!directive.props.some((p) => isColourKey(p.key))) {
      continue;
    }
    if (directive.props.some((p) => !isColourKey(p.key))) {
      parts.push(...entryDeletions(directive));
      continue;
    }
    // The whole statement goes; only a line that holds nothing else goes with
    // it, so no other statement and no accDescr `}` is ever taken along (R33).
    const line = model.scan.lines[directive.line];
    if (line?.kind !== "statement" || line.statements.length !== 1) {
      return refused("unsupported");
    }
    whole.push(directive.line);
  }
  const edits = [...lineDeletions(model, whole), ...parts].map((span) => ({
    ...span,
    insert: "",
  }));
  return checked(model, edits, { [`${node.id} styles`]: null });
}

function colour(
  model: FlowModel,
  node: FlowNode,
  name: ColourPreset | null,
): OpResult {
  const styleLines = model.directives.filter(
    (d) => d.kind === "style" && d.targets[0]?.id === node.id,
  );
  return name === null
    ? clearColour(model, node, styleLines)
    : setColour(model, node, styleLines, name);
}

function direction(model: FlowModel, value: "TD" | "LR"): OpResult {
  const written = model.direction;
  if (readDirection(written?.value) === readDirection(value)) {
    return { edits: [] };
  }
  const changes = { direction: readDirection(value) };
  if (written !== null) {
    return checked(
      model,
      [{ from: written.span.from, to: written.span.to, insert: value }],
      changes,
    );
  }
  const header = model.scan.header?.statements[0];
  if (header === undefined) {
    return refused("unsupported");
  }
  const at = header.from + model.keyword.length;
  return checked(model, [{ from: at, to: at, insert: ` ${value}` }], changes);
}

/**
 * The edits for `op`, or why there are none: a body it does not understand is
 * `unsupported`, a box that is not there `gone`, and a result that would not
 * read back as exactly the change `invalid`. A subgraph id is no box (R4).
 * No edits at all means the source already says this.
 */
export function compileFlowOp(model: FlowModel, op: FlowOp): OpResult {
  if (!model.understood) {
    return refused("unsupported");
  }
  switch (op.op) {
    case "rename":
    case "shape":
    case "colour": {
      const node = model.nodes.get(op.id);
      if (node === undefined) {
        return refused("gone");
      }
      return op.op === "rename"
        ? rename(model, node, op.text)
        : op.op === "shape"
          ? changeShape(model, node, op.shape)
          : colour(model, node, op.colour);
    }
    case "direction":
      return direction(model, op.value);
    case "addChild":
    case "addSibling":
    case "connect":
    case "addBox":
    case "deleteNode":
    case "deleteEdge":
    case "edgeLabel":
    case "lineStyle":
    case "arrowhead":
    case "reverse":
      return refused("unsupported");
    default: {
      const unknown: never = op;
      return unknown;
    }
  }
}
