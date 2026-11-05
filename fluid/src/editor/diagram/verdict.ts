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
 * trims labels). A db label need not be a string (`@{ label: 1 }` reads as
 * a number), so any value is taken and read as text.
 */
export function normalizeLabel(text: unknown): string {
  return (typeof text === "string" ? text : String(text))
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

function sameList(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

/** Each box's `style` entries in source order, as the db lists them. */
export function stylesOf(model: FlowModel): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const directive of model.directives) {
    const id = directive.targets[0]?.id;
    if (directive.kind === "style" && id !== undefined) {
      const entries = directive.props.map((p) =>
        model.body.slice(p.entry.from, p.entry.to),
      );
      out.set(id, [...(out.get(id) ?? []), ...entries]);
    }
  }
  return out;
}

/**
 * Each box's classes as mermaid's db collects them, in source order:
 * `setClass` skips an id with no vertex yet, so a `class` or `click` line
 * only counts after the box's first ref or `style` line. A `click` line adds
 * `clickable`.
 */
export function classesOf(model: FlowModel): Map<string, string[]> {
  const steps: { at: number; run: () => void }[] = [];
  const known = new Set<string>();
  const out = new Map<string, string[]>();
  const add = (id: string, name: string) => {
    if (known.has(id)) {
      out.set(id, [...(out.get(id) ?? []), name]);
    }
  };
  for (const chain of model.chains) {
    for (const group of chain.groups) {
      for (const ref of group.refs) {
        steps.push({
          at: ref.span.from,
          run: () => {
            known.add(ref.id);
            if (ref.classSpan) {
              add(
                ref.id,
                model.body.slice(ref.classSpan.from, ref.classSpan.to),
              );
            }
          },
        });
      }
    }
  }
  for (const directive of model.directives) {
    steps.push({
      at: directive.span.from,
      run: () => {
        for (const target of directive.targets) {
          if (directive.kind === "style") {
            known.add(target.id);
          } else if (directive.kind === "class" && directive.className) {
            add(target.id, directive.className.name);
          } else if (directive.kind === "click") {
            add(target.id, "clickable");
          }
        }
      },
    });
  }
  steps.sort((a, b) => a.at - b.at);
  for (const step of steps) {
    step.run();
  }
  return out;
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
 * label (through `normalizeLabel`, R6), type (the classic table, or the
 * `@{ shape }` name as written, R9), styles and classes.
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
  const styles = stylesOf(model);
  const classes = classesOf(model);
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
    if (
      !sameList(
        styles.get(node.id) ?? [],
        vertex.styles.map((entry) => entry.trim()),
      )
    ) {
      return `The styles of ${node.id} differ.`;
    }
    if (!sameList(classes.get(node.id) ?? [], vertex.classes)) {
      return `The classes of ${node.id} differ.`;
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

/** A verdict, or an answer that says nothing about the body (`final: false`). */
interface Decision {
  verdict: Verdict;
  final: boolean;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

async function decide(body: string, dark: boolean): Promise<Decision> {
  const refused = (reason: string): Decision => ({
    verdict: { ok: false, reason },
    final: true,
  });
  const model = parseFlowchart(body);
  if (!model.understood) {
    return refused(model.reasons[0] ?? "The parser does not understand it.");
  }
  const db = await flowDbOf(body, dark);
  if ("error" in db) {
    if (db.unavailable) {
      return {
        verdict: {
          ok: false,
          reason: `mermaid could not be loaded: ${messageOf(db.error)}`,
        },
        final: false,
      };
    }
    return refused(`mermaid refuses it: ${messageOf(db.error)}`);
  }
  const difference = agreesFlow(model, db);
  if (difference !== null) {
    return refused(difference);
  }
  const clusters = new Set(db.subgraphs.map((s) => s.id));
  const domIds = new Map(
    db.vertices
      .filter((v) => !clusters.has(v.id))
      .map((v) => [v.domId, v.id] as const),
  );
  return { verdict: { ok: true, kind: "flowchart", domIds }, final: true };
}

/**
 * The verdict for `body`: parse, mermaid's db (`flowDbOf` with the asking
 * view's `dark`) and `agreesFlow`. Cached by body; a request while one is
 * running for the same body gets the same promise. A check that could not
 * run (mermaid failing to load, an unexpected throw) answers `ok: false` with
 * the cause but is no verdict: it is not cached, and the next request tries
 * again.
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
    .catch((error: unknown): Decision => ({
      verdict: { ok: false, reason: `The check failed: ${messageOf(error)}` },
      final: false,
    }))
    .then(({ verdict, final }) => {
      pending.delete(body);
      if (final) {
        remember(body, verdict);
      }
      return verdict;
    });
  pending.set(body, promise);
  return promise;
}
