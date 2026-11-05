/**
 * What an op did to the diagram, as mermaid's own db sees it. The property
 * suites compare the db of the body before and after an op, so a change the
 * op did not mean shows up even when our own parser reads past it.
 */

import { type FlowDbView, flowDbOf } from "../editor/diagram/mermaidDb";

/** Mermaid's db for `body`; a refusal is a throw, which fails the test. */
export async function flowDbFor(body: string): Promise<FlowDbView> {
  const view = await flowDbOf(body, false);
  if ("error" in view) {
    throw new Error(`mermaid refused: ${String(view.error)}\n${body}`);
  }
  return view;
}

/**
 * How a label is read: mermaid's `text` and `string` are a label without and
 * with quotes, and only `markdown` draws differently.
 */
function readAs(labelType: string | undefined): string {
  return labelType === "markdown" ? "markdown" : "text";
}

/**
 * Every difference between two dbs, one string per difference, in a fixed
 * order, so a suite can assert the exact list:
 *
 * - `direction`
 * - `vertex <id> added`, `vertex <id> removed`, `vertex order`
 * - `vertex <id> text`, `type`, `styles`, `classes`, `labelType`, `link`,
 *   `linkTarget`, `tooltip`
 * - `edge <n> added`, `edge <n> removed` (by position in the edge list)
 * - `edge <n> id`, `start`, `end`, `text`, `type`, `stroke`, `length`,
 *   `labelType`
 * - `subgraph <id> added`, `removed`, `title`, `nodes`, `dir`, `labelType`
 * - `classDef <id> added`, `removed`, `styles`, `textStyles`
 *
 * A vertex's `domId` is left out on purpose: its number counts every
 * `addVertex` call, so any new `style` line moves the ones after it, and the
 * drawing maps through the db, never by computing it.
 */
export function dbDelta(before: FlowDbView, after: FlowDbView): string[] {
  const out: string[] = [];
  if (before.direction !== after.direction) {
    out.push("direction");
  }

  const was = new Map(before.vertices.map((v) => [v.id, v]));
  const now = new Map(after.vertices.map((v) => [v.id, v]));
  for (const id of was.keys()) {
    if (!now.has(id)) {
      out.push(`vertex ${id} removed`);
    }
  }
  for (const [id, vertex] of now) {
    const old = was.get(id);
    if (old === undefined) {
      out.push(`vertex ${id} added`);
      continue;
    }
    if (old.text !== vertex.text) {
      out.push(`vertex ${id} text`);
    }
    if (old.type !== vertex.type) {
      out.push(`vertex ${id} type`);
    }
    if (old.styles.join("\n") !== vertex.styles.join("\n")) {
      out.push(`vertex ${id} styles`);
    }
    if (old.classes.join("\n") !== vertex.classes.join("\n")) {
      out.push(`vertex ${id} classes`);
    }
    if (readAs(old.labelType) !== readAs(vertex.labelType)) {
      out.push(`vertex ${id} labelType`);
    }
    for (const field of ["link", "linkTarget", "tooltip"] as const) {
      if (old[field] !== vertex[field]) {
        out.push(`vertex ${id} ${field}`);
      }
    }
  }
  const kept = (list: FlowDbView["vertices"], other: Map<string, unknown>) =>
    list.filter((v) => other.has(v.id)).map((v) => v.id);
  if (
    kept(before.vertices, now).join("\n") !==
    kept(after.vertices, was).join("\n")
  ) {
    out.push("vertex order");
  }

  const edges = Math.max(before.edges.length, after.edges.length);
  for (let n = 0; n < edges; n++) {
    const old = before.edges[n];
    const edge = after.edges[n];
    if (old === undefined) {
      out.push(`edge ${n} added`);
    } else if (edge === undefined) {
      out.push(`edge ${n} removed`);
    } else {
      for (const field of [
        "id",
        "start",
        "end",
        "text",
        "type",
        "stroke",
        "length",
      ] as const) {
        if (old[field] !== edge[field]) {
          out.push(`edge ${n} ${field}`);
        }
      }
      if (readAs(old.labelType) !== readAs(edge.labelType)) {
        out.push(`edge ${n} labelType`);
      }
    }
  }

  const oldGroups = new Map(before.subgraphs.map((s) => [s.id, s]));
  const newGroups = new Map(after.subgraphs.map((s) => [s.id, s]));
  for (const id of oldGroups.keys()) {
    if (!newGroups.has(id)) {
      out.push(`subgraph ${id} removed`);
    }
  }
  for (const [id, group] of newGroups) {
    const old = oldGroups.get(id);
    if (old === undefined) {
      out.push(`subgraph ${id} added`);
      continue;
    }
    if (old.title !== group.title) {
      out.push(`subgraph ${id} title`);
    }
    if (old.nodes.join("\n") !== group.nodes.join("\n")) {
      out.push(`subgraph ${id} nodes`);
    }
    if (old.dir !== group.dir) {
      out.push(`subgraph ${id} dir`);
    }
    if (readAs(old.labelType) !== readAs(group.labelType)) {
      out.push(`subgraph ${id} labelType`);
    }
  }

  const oldDefs = new Map(before.classDefs.map((c) => [c.id, c]));
  const newDefs = new Map(after.classDefs.map((c) => [c.id, c]));
  for (const id of oldDefs.keys()) {
    if (!newDefs.has(id)) {
      out.push(`classDef ${id} removed`);
    }
  }
  for (const [id, def] of newDefs) {
    const old = oldDefs.get(id);
    if (old === undefined) {
      out.push(`classDef ${id} added`);
      continue;
    }
    if (old.styles.join("\n") !== def.styles.join("\n")) {
      out.push(`classDef ${id} styles`);
    }
    if (old.textStyles.join("\n") !== def.textStyles.join("\n")) {
      out.push(`classDef ${id} textStyles`);
    }
  }
  return out;
}

/** `dbDelta` of the two bodies. Throws when mermaid refuses either. */
export async function flowDelta(
  before: string,
  after: string,
): Promise<string[]> {
  return dbDelta(await flowDbFor(before), await flowDbFor(after));
}
