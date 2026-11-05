/**
 * Whether a fence may be drawn as an editable diagram, and the map from
 * mermaid's element ids back to our node ids for one that may.
 *
 * A fence is drawable only when our parser understands every line and
 * mermaid's own db agrees with our model (`agreesFlow`). The db is read
 * asynchronously (`flowDbOf` loads mermaid on demand), so the verdict is
 * asked for with `requestVerdict` and read back synchronously with
 * `verdictFor` once it has settled. Both are keyed by the body text and
 * kept in one LRU of 256 entries.
 *
 * This module is lazy: nothing eager imports it, so the parser and the
 * agreement check stay out of the main bundle.
 */

import { parseFlowchart } from "./flowchart/parse";
import type { FlowModel } from "./flowchart/model";
import { CLASSIC_SHAPES } from "./flowchart/model";
import { type FlowDbView, flowDbOf } from "./mermaidDb";

export type Verdict =
  | {
      ok: true;
      kind: "flowchart" | "sequence";
      /** db domId (without the `<svg id>-` prefix) to node id; boxes only, no clusters. */
      domIds: Map<string, string>;
    }
  | { ok: false; reason: string };

/** The diagram keywords that have an editor. */
const FLOW_KEYWORDS = new Set(["flowchart", "graph"]);

const CACHE_SIZE = 256;
const settled = new Map<string, Verdict>();
const pending = new Map<string, Promise<Verdict>>();

/**
 * The one label normalizer both sides of the label agreement pass through
 * (R6): mermaid's db keeps `encodeEntities`' markers (`#quot;` arrives as
 * `\uFB02\xB0quot\xB6\xDF`, `#35;` as `\uFB02\xB0\xB035\xB6\xDF`), so
 * they are mapped back to the `#name;` form the source uses; every way of
 * writing a line break (`<br/>`, `<br />`, `<BR>`) becomes `<br>`, the form
 * mermaid's sanitizer stores; and outer whitespace is dropped (mermaid
 * trims labels).
 */
export function normalizeLabel(text: string): string {
  return text
    .replace(/\uFB02\xB0\xB0(\w+)\xB6\xDF/g, "#$1;")
    .replace(/\uFB02\xB0(\w+)\xB6\xDF/g, "#$1;")
    .replace(/<br\s*\/?>/gi, "<br>")
    .trim();
}

/** The label text inside `span`, without the quotes or markdown wrapper. */
function labelText(
  body: string,
  span: { from: number; to: number },
  quoted: boolean,
  markdown: boolean,
): string {
  const raw = body.slice(span.from, span.to).trim();
  if (markdown) {
    return raw.slice(2, -2);
  }
  return quoted ? raw.slice(1, -1) : raw;
}

const DB_TYPE = new Map(CLASSIC_SHAPES.map((s) => [s.name, s.dbType]));
const STROKES: Record<string, string> = {
  solid: "normal",
  dotted: "dotted",
  thick: "thick",
  invisible: "invisible",
};

/** The db edge type for a pair of heads, as mermaid's `destructLink` names it. */
function edgeType(start: string, end: string): string {
  const name = {
    none: "open",
    arrow: "point",
    circle: "circle",
    cross: "cross",
  }[end];
  return start === "none" ? `arrow_${name}` : `double_arrow_${name}`;
}

function sameSet(a: Iterable<string>, b: Iterable<string>): boolean {
  const left = new Set(a);
  const right = new Set(b);
  return left.size === right.size && [...left].every((x) => right.has(x));
}

/**
 * Whether `model` and mermaid's `db` describe the same diagram: null when
 * they agree, else the first difference in words.
 *
 * Compared: the direction; the vertex ids (a db vertex whose id is a
 * subgraph id is the cluster, never a box, R4); the edge sequence with each
 * edge's start, end, id, type, stroke, length and label; the subgraph ids
 * and member sets (the db lists members in reverse, R9); and each box's
 * label (through `normalizeLabel`, R6) and type (the classic table, or the
 * `@{ shape }` name as written, R9).
 */
export function agreesFlow(model: FlowModel, db: FlowDbView): string | null {
  if (!model.understood) {
    return model.reasons[0] ?? "The parser does not understand the diagram.";
  }
  const body = model.body;
  const written = model.direction?.value;
  const direction =
    written === undefined
      ? "TB"
      : ({ TD: "TB", "<": "RL", ">": "LR", "^": "BT", v: "TB" }[written] ??
        written);
  if (direction !== (db.direction ?? "TB")) {
    return `The direction is ${direction} here and ${db.direction ?? "none"} in mermaid.`;
  }

  const clusters = new Set(db.subgraphs.map((s) => s.id));
  if (
    !sameSet(
      clusters,
      model.subgraphs.map((s) => s.id),
    )
  ) {
    return "The subgraph ids differ.";
  }
  for (const subgraph of model.subgraphs) {
    const theirs = db.subgraphs.find((s) => s.id === subgraph.id);
    if (!theirs || !sameSet(subgraph.members, theirs.nodes)) {
      return `The subgraph ${subgraph.id} holds different members.`;
    }
  }

  const boxes = db.vertices.filter((v) => !clusters.has(v.id));
  if (
    !sameSet(
      boxes.map((v) => v.id),
      model.nodes.keys(),
    )
  ) {
    return "The box ids differ.";
  }
  for (const vertex of boxes) {
    const node = model.nodes.get(vertex.id);
    if (!node) {
      return `The box ${vertex.id} is missing.`;
    }
    const shape = node.defining?.shape ?? null;
    const text = shape
      ? labelText(body, shape.label, shape.quoted, shape.markdown)
      : node.id;
    if (normalizeLabel(text) !== normalizeLabel(vertex.text)) {
      return `The label of ${node.id} differs.`;
    }
    const type = !shape
      ? undefined
      : shape.form === "at"
        ? body.slice(shape.atShape?.from ?? 0, shape.atShape?.to ?? 0)
        : shape.name === null
          ? undefined
          : DB_TYPE.get(shape.name);
    if (type !== vertex.type) {
      return `The shape of ${node.id} differs.`;
    }
  }

  if (model.edges.length !== db.edges.length) {
    return `There are ${model.edges.length} arrows here and ${db.edges.length} in mermaid.`;
  }
  for (const [i, edge] of model.edges.entries()) {
    const theirs = db.edges[i];
    if (
      theirs === undefined ||
      theirs.start !== edge.start ||
      theirs.end !== edge.end ||
      theirs.id !== edge.mermaidId
    ) {
      return `Arrow ${i} (${edge.start} to ${edge.end}) differs.`;
    }
    const arrow = model.chains[edge.chain]?.links[edge.link]?.arrow;
    if (!arrow) {
      return `Arrow ${i} has no source.`;
    }
    if (
      theirs.type !== edgeType(arrow.start, arrow.end) ||
      theirs.stroke !== STROKES[arrow.line] ||
      theirs.length !== Math.min(arrow.length, 10)
    ) {
      return `Arrow ${i} (${edge.start} to ${edge.end}) is drawn differently.`;
    }
    const raw = arrow.label
      ? body.slice(arrow.label.from, arrow.label.to).trim()
      : "";
    const label = arrow.label
      ? labelText(
          body,
          arrow.label,
          arrow.labelQuoted,
          raw.startsWith('"`') && raw.endsWith('`"'),
        )
      : "";
    if (normalizeLabel(label) !== normalizeLabel(theirs.text)) {
      return `The label of arrow ${i} (${edge.start} to ${edge.end}) differs.`;
    }
  }
  return null;
}

function remember(body: string, verdict: Verdict): void {
  settled.delete(body);
  settled.set(body, verdict);
  while (settled.size > CACHE_SIZE) {
    const oldest = settled.keys().next().value;
    if (oldest === undefined) {
      break;
    }
    settled.delete(oldest);
  }
}

/** The settled verdict for `body`, or undefined while none is known. */
export function verdictFor(body: string): Verdict | undefined {
  const verdict = settled.get(body);
  if (verdict !== undefined) {
    remember(body, verdict);
  }
  return verdict;
}

async function decide(body: string, dark: boolean): Promise<Verdict> {
  const model = parseFlowchart(body);
  if (!FLOW_KEYWORDS.has(model.keyword)) {
    return { ok: false, reason: "This kind of diagram has no editor yet." };
  }
  if (!model.understood) {
    return {
      ok: false,
      reason: model.reasons[0] ?? "The parser does not understand it.",
    };
  }
  const db = await flowDbOf(body, dark);
  if ("error" in db) {
    const error = db.error;
    return {
      ok: false,
      reason: `mermaid refuses it: ${error instanceof Error ? error.message : String(error)}`,
    };
  }
  const difference = agreesFlow(model, db);
  if (difference !== null) {
    return { ok: false, reason: difference };
  }
  const clusters = new Set(db.subgraphs.map((s) => s.id));
  const domIds = new Map(
    db.vertices
      .filter((v) => !clusters.has(v.id))
      .map((v) => [v.domId, v.id] as const),
  );
  return { ok: true, kind: "flowchart", domIds };
}

/**
 * The verdict for `body`: parse, mermaid's db (`flowDbOf` with the asking
 * view's `dark`) and `agreesFlow`. Cached by body; a request while one is
 * running for the same body gets the same promise.
 */
export function requestVerdict(body: string, dark: boolean): Promise<Verdict> {
  const known = verdictFor(body);
  if (known !== undefined) {
    return Promise.resolve(known);
  }
  const running = pending.get(body);
  if (running !== undefined) {
    return running;
  }
  const promise = decide(body, dark)
    .catch((error: unknown): Verdict => ({
      ok: false,
      reason: `The check failed: ${error instanceof Error ? error.message : String(error)}`,
    }))
    .then((verdict) => {
      pending.delete(body);
      remember(body, verdict);
      return verdict;
    });
  pending.set(body, promise);
  return promise;
}
