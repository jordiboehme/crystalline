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
  sortedEdits,
} from "../edits";
import { droppedSemicolons } from "../scan";
import { classesOf, stylesOf } from "../verdict";
import {
  COLOUR_PRESETS,
  cannotDraw,
  type ColourPreset,
  SHAPES,
  decodeLabel,
  encodeAtLabel,
  encodeLabel,
  encodeMarkdownLabel,
  tidyLabel,
} from "./labels";
import {
  type DirectiveLine,
  type EdgeKey,
  type FlowModel,
  type FlowNode,
  type Shape,
  type ShapeName,
} from "./model";
import { lineDeletions, shiftLinkStyles } from "./linkStyle";
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
function arrowRows(model: FlowModel): unknown[][] {
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
function subgraphRows(
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
type Wanted = string | null | ((value: string) => boolean);

function holds(wanted: Wanted, value: string): boolean {
  return typeof wanted === "function"
    ? wanted(value)
    : wanted === null || wanted === value;
}

/**
 * `edits`, when applying them leaves a body that is understood and differs from
 * `model` only in the fields `changes` names, each then holding what is named
 * there. A field that was not there before must be named, and no field may go.
 * Otherwise `invalid`.
 */
export function checked(
  model: FlowModel,
  edits: TextEdit[],
  changes: Readonly<Record<string, Wanted>>,
): OpResult {
  if (edits.length === 0) {
    return { edits };
  }
  const after = parseFlowchart(applyEdits(model.body, edits));
  if (!after.understood || dropsNewSemicolon(model, after, edits)) {
    return refused("invalid");
  }
  const before = fieldsOf(model);
  const now = fieldsOf(after);
  for (const field of before.keys()) {
    if (!now.has(field)) {
      return refused("invalid");
    }
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

/** The label to write for `text` in a place that holds `shape`. */
function encodedFor(text: string, shape: Shape): string {
  if (shape.markdown) {
    return encodeMarkdownLabel(text);
  }
  if (shape.form === "at") {
    return encodeAtLabel(text, shape.quoted);
  }
  return encodeLabel(text, shape.quoted);
}

function rename(model: FlowModel, node: FlowNode, text: string): OpResult {
  const typed = tidyLabel(text);
  if (typed === "" || cannotDraw(typed)) {
    return refused("invalid");
  }
  const shape = node.defining?.shape ?? null;
  const ref = node.refs[0];
  if (ref === undefined) {
    return refused("unsupported");
  }
  const field = `${node.id} label`;
  if (shape === null) {
    if (typed === node.id) {
      return { edits: [] };
    }
    const at = ref.idSpan.to;
    return checked(
      model,
      [{ from: at, to: at, insert: `[${encodeLabel(typed, false)}]` }],
      { [field]: typed, [`${node.id} shape`]: "rect" },
    );
  }
  if (typed === tidyLabel(shownLabel(model.body, shape))) {
    return { edits: [] };
  }
  return checked(
    model,
    [
      {
        from: shape.label.from,
        to: shape.label.to,
        insert: encodedFor(typed, shape),
      },
    ],
    { [field]: typed },
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
  const colours = `${node.id} colours`;
  const landed = JSON.stringify([preset.fill, preset.stroke, preset.color]);
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
        [colours]: landed,
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
  return checked(model, edits, { [field]: null, [colours]: landed });
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
  return checked(model, edits, {
    [`${node.id} styles`]: null,
    [`${node.id} colours`]: JSON.stringify([null, null, null]),
  });
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

/** The label of a box an op makes. Task 10 opens the rename field on it. */
const NEW_BOX_LABEL = "New box";

/**
 * The id for a new box. It never takes an id the source already gives a meaning
 * to: a subgraph, an arrow, or a `class` or `click` line that would style or link
 * the box (those two make no box of their own, so the id is not among the nodes).
 * Neither one letter nor `n<k>` is a reserved word.
 */
export function newNodeId(model: FlowModel): string {
  const ids = [...model.nodes.keys()];
  const taken = new Set<string>([
    ...ids,
    ...model.subgraphs.map((s) => s.id),
    ...model.edges.map((e) => e.mermaidId),
    ...model.chains.flatMap((chain) =>
      chain.links.flatMap(({ arrow }) =>
        arrow.idSpan
          ? [model.body.slice(arrow.idSpan.from, arrow.idSpan.to)]
          : [],
      ),
    ),
    ...model.directives.flatMap((d) =>
      d.kind === "classDef" ? [] : d.targets.map((t) => t.id),
    ),
  ]);
  if (ids.every((id) => /^[A-Z]$/.test(id))) {
    const first = "A".charCodeAt(0);
    const highest = Math.max(first - 1, ...ids.map((id) => id.charCodeAt(0)));
    for (let code = highest + 1; code <= "Z".charCodeAt(0); code++) {
      const letter = String.fromCharCode(code);
      if (!taken.has(letter)) {
        return letter;
      }
    }
  }
  for (let k = 1; ; k++) {
    if (!taken.has(`n${String(k)}`)) {
      return `n${String(k)}`;
    }
  }
}

/** Where a new line goes: after the line `line`, inside the subgraph `group`. */
interface Place {
  line: number;
  group: string | null;
}

/**
 * The subgraph a new line after `line` would sit in. `subgraphs` is in close
 * order, so the first that spans the line is the innermost. The opening line
 * counts as inside and the `end` line as outside, whatever else those lines hold.
 */
function scopeAfter(model: FlowModel, line: number): string | null {
  return (
    model.subgraphs.find((s) => s.openLine <= line && line < s.endLine)?.id ??
    null
  );
}

/** Every line that names the box: its refs and its `style`, `class` and `click` lines. */
function linesNaming(model: FlowModel, node: FlowNode): number[] {
  return [
    ...node.refs.map((ref) => ref.line),
    ...model.directives
      .filter(
        (d) => d.kind !== "classDef" && d.targets.some((t) => t.id === node.id),
      )
      .map((d) => d.line),
  ];
}

function endOfBody(model: FlowModel): number {
  return model.scan.lines.findLastIndex((line) => line.kind !== "blank");
}

/** Just before the `end` of `group`, or the end of the body for the top level. */
function closingPlace(model: FlowModel, group: string | null): Place {
  const subgraph = model.subgraphs.find((s) => s.id === group);
  return {
    line: subgraph ? subgraph.endLine - 1 : endOfBody(model),
    group,
  };
}

/** After the last line that names `node` and lies in its own subgraph. */
function placeBy(model: FlowModel, node: FlowNode): Place {
  const own = linesNaming(model, node).filter(
    (line) => scopeAfter(model, line) === node.subgraph,
  );
  return own.length > 0
    ? { line: Math.max(...own), group: node.subgraph }
    : closingPlace(model, node.subgraph);
}

/** The subgraphs around `group`, innermost first, `group` itself included. */
function scopesAround(model: FlowModel, group: string | null): string[] {
  const out: string[] = [];
  let at = model.subgraphs.find((s) => s.id === group);
  while (at !== undefined) {
    out.push(at.id);
    const parent = at.parent;
    at = model.subgraphs.find((s) => s.id === parent);
  }
  return out;
}

function indentFor(model: FlowModel, place: Place): string {
  const lines = model.scan.lines;
  const opening = model.subgraphs.find((s) => s.id === place.group)?.openLine;
  const inGroup = (index: number, kind: string): boolean =>
    kind === "statement" &&
    index !== opening &&
    scopeAfter(model, index) === place.group;
  const followed = lines[place.line];
  if (followed && inGroup(followed.index, followed.kind)) {
    return followed.indent;
  }
  const first = (test: (index: number, kind: string) => boolean) =>
    lines.find((line) => test(line.index, line.kind))?.indent;
  return (
    (place.group !== null ? first(inGroup) : undefined) ??
    first((_, kind) => kind === "statement") ??
    "    "
  );
}

/** What a new line adds, besides itself. */
interface Addition {
  box: string | null;
  edge: { start: string; end: string } | null;
}

/** The fields a new box adds, each as it must read. */
function newBoxFields(
  model: FlowModel,
  id: string,
  group: string | null,
): Record<string, Wanted> {
  const without = JSON.stringify(model.nodeOrder);
  return {
    boxes: (value) =>
      JSON.stringify(
        (JSON.parse(value) as string[]).filter((box) => box !== id),
      ) === without,
    subgraphs: subgraphRows(model, group, id),
    [`${id} label`]: NEW_BOX_LABEL,
    [`${id} markdown`]: "false",
    [`${id} shape`]: "rect",
    [`${id} styles`]: "[]",
    [`${id} colours`]: JSON.stringify([null, null, null]),
    [`${id} classes`]: "[]",
  };
}

/** The `arrows` field with one plain new arrow at `index`. */
function arrowsWith(
  model: FlowModel,
  edge: { start: string; end: string },
  index: number,
): string {
  const earlier = model.edges
    .slice(0, index)
    .filter((e) => e.start === edge.start && e.end === edge.end).length;
  const id = `L_${edge.start}_${edge.end}_${earlier === 0 ? 0 : earlier + 1}`;
  const rows = arrowRows(model);
  rows.splice(index, 0, [
    edge.start,
    edge.end,
    id,
    "solid",
    "none",
    "arrow",
    1,
    null,
  ]);
  return JSON.stringify(rows);
}

/**
 * The new line `text` at `place`, with the `linkStyle` digits that must follow
 * when a new arrow lands before the last one (mermaid numbers edges in source
 * order).
 */
function insertLine(
  model: FlowModel,
  place: Place,
  text: string,
  { box, edge }: Addition,
): OpResult {
  const line = model.scan.lines[place.line];
  if (line === undefined) {
    return refused("invalid");
  }
  const edits: TextEdit[] = [
    {
      from: line.to,
      to: line.to,
      insert: `\n${indentFor(model, place)}${text}`,
    },
  ];
  const changes: Record<string, Wanted> = {};
  if (box !== null) {
    Object.assign(changes, newBoxFields(model, box, place.group));
  }
  if (edge !== null) {
    const index = model.edges.filter(
      (e) => (model.chains[e.chain]?.line ?? 0) <= place.line,
    ).length;
    if (index < model.edges.length) {
      edits.push(...shiftLinkStyles(model, index, 1));
    }
    changes.arrows = arrowsWith(model, edge, index);
  }
  return checked(model, edits, changes);
}

function withBox(result: OpResult, id: string): OpResult {
  return "edits" in result
    ? { ...result, created: { kind: "node", id } }
    : result;
}

/** A new box's ref: the id and the label through the rename encoder. */
function newBoxRef(id: string): string {
  return `${id}[${encodeLabel(NEW_BOX_LABEL, false)}]`;
}

/** The first parent in source order: a box, so no subgraph (R4), and not `node` itself. */
function firstParent(model: FlowModel, node: FlowNode): FlowNode | null {
  for (const edge of model.edges) {
    if (edge.end === node.id && edge.start !== node.id) {
      const parent = model.nodes.get(edge.start);
      if (parent !== undefined) {
        return parent;
      }
    }
  }
  return null;
}

function addUnder(model: FlowModel, parent: FlowNode): OpResult {
  const id = newNodeId(model);
  return withBox(
    insertLine(
      model,
      placeBy(model, parent),
      `${parent.id} --> ${newBoxRef(id)}`,
      { box: id, edge: { start: parent.id, end: id } },
    ),
    id,
  );
}

function addSibling(model: FlowModel, node: FlowNode): OpResult {
  const parent = firstParent(model, node);
  if (parent !== null) {
    return addUnder(model, parent);
  }
  const id = newNodeId(model);
  return withBox(
    insertLine(model, placeBy(model, node), newBoxRef(id), {
      box: id,
      edge: null,
    }),
    id,
  );
}

/**
 * `from --> to` after the later of the two. Inside a subgraph the line would make
 * an end that no earlier subgraph holds a member of that subgraph, so when the
 * self-check refuses that, the arrow goes in the nearest subgraph around both.
 */
function connect(model: FlowModel, from: FlowNode, to: FlowNode): OpResult {
  const a = placeBy(model, from);
  const b = placeBy(model, to);
  const place = a.line >= b.line ? a : b;
  const addition = { box: null, edge: { start: from.id, end: to.id } };
  const text = `${from.id} --> ${to.id}`;
  const result = insertLine(model, place, text, addition);
  if ("edits" in result || from.subgraph === to.subgraph) {
    return result;
  }
  const around = scopesAround(model, to.subgraph);
  const common =
    scopesAround(model, from.subgraph).find((s) => around.includes(s)) ?? null;
  return common === place.group
    ? result
    : insertLine(model, closingPlace(model, common), text, addition);
}

function addBox(model: FlowModel): OpResult {
  const id = newNodeId(model);
  return withBox(
    insertLine(model, closingPlace(model, null), newBoxRef(id), {
      box: id,
      edge: null,
    }),
    id,
  );
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
    case "addSibling": {
      const node = model.nodes.get(op.id);
      if (node === undefined) {
        return refused("gone");
      }
      return op.op === "addChild"
        ? addUnder(model, node)
        : addSibling(model, node);
    }
    case "connect": {
      const from = model.nodes.get(op.from);
      const to = model.nodes.get(op.to);
      if (from === undefined || to === undefined) {
        return refused("gone");
      }
      return connect(model, from, to);
    }
    case "addBox":
      return addBox(model);
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
