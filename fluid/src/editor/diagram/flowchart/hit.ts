/**
 * What a part of a drawn flowchart is in the source, and back. Elements are told
 * apart by the ids mermaid gives them and never by their label text: a box by
 * its `g.node` id through `verdict.domIds`, an arrow by the exact `data-id` the
 * model computes. Whatever does not map (a cluster, an emptied subgraph's
 * `g.node`) is the canvas (R4, R60), so no key or click can reach a part of the
 * source it does not mean.
 */

import type { FlowTarget } from "../intent";
import type { Verdict } from "../verdict";
import { decodeLabel } from "./labels";
import type { EdgeKey, FlowEdge, FlowModel } from "./model";

const CANVAS: FlowTarget = { kind: "canvas" };
const SVG_NS = "http://www.w3.org/2000/svg";

const edgeKey = (edge: FlowEdge): EdgeKey => ({
  start: edge.start,
  end: edge.end,
  occurrence: edge.occurrence,
});

const sameEdge = (edge: FlowEdge, key: EdgeKey): boolean =>
  edge.start === key.start &&
  edge.end === key.end &&
  edge.occurrence === key.occurrence;

/** Canvas, then boxes in `nodeOrder`, then arrows in index order. */
export function flowStops(model: FlowModel): FlowTarget[] {
  const clusters = new Set(model.subgraphs.map((s) => s.id));
  return [
    CANVAS,
    ...model.nodeOrder
      .filter((id) => !clusters.has(id))
      .map((id): FlowTarget => ({ kind: "node", id })),
    ...model.edges.map((edge): FlowTarget => ({
      kind: "edge",
      key: edgeKey(edge),
    })),
  ];
}

/** The box a `g.node` draws, or undefined for one that is no box. */
function nodeIdOf(el: Element, svgId: string, verdict: Verdict) {
  if (!verdict.ok || !el.id.startsWith(`${svgId}-`)) {
    return undefined;
  }
  return verdict.domIds.get(el.id.slice(svgId.length + 1));
}

/** What `el` stands for in the source: the canvas when nothing maps. */
export function flowTargetOf(
  el: Element,
  svgId: string,
  model: FlowModel,
  verdict: Verdict,
): FlowTarget {
  const node = el.closest("g.node");
  if (node !== null) {
    const id = nodeIdOf(node, svgId, verdict);
    return id === undefined ? CANVAS : { kind: "node", id };
  }
  const part = el.closest(
    'path[data-et="edge"], g.label[data-id], [data-hit-for]',
  );
  const edgeId =
    part?.getAttribute("data-id") ?? part?.getAttribute("data-hit-for");
  const edge = model.edges.find((e) => e.mermaidId === edgeId);
  return edge === undefined ? CANVAS : { kind: "edge", key: edgeKey(edge) };
}

/** The element of a box or arrow on screen; null for the canvas or no element. */
export function flowElementOf(
  svg: SVGSVGElement,
  svgId: string,
  target: FlowTarget,
  model: FlowModel,
  verdict: Verdict,
): SVGElement | null {
  if (target.kind === "node") {
    for (const el of svg.querySelectorAll<SVGElement>("g.node")) {
      if (nodeIdOf(el, svgId, verdict) === target.id) {
        return el;
      }
    }
    return null;
  }
  if (target.kind === "edge") {
    const edge = model.edges.find((e) => sameEdge(e, target.key));
    if (edge !== undefined) {
      for (const el of svg.querySelectorAll<SVGElement>(
        'path[data-et="edge"]',
      )) {
        if (el.getAttribute("data-id") === edge.mermaidId) {
          return el;
        }
      }
    }
  }
  return null;
}

/** A box's or a subgraph's label as it is drawn; its id when it has none. */
function boxLabel(model: FlowModel, id: string): string {
  const title = model.subgraphs.find((s) => s.id === id)?.title ?? null;
  if (title !== null) {
    return decodeLabel(title, title.startsWith('"')) || id;
  }
  const shape = model.nodes.get(id)?.defining?.shape ?? null;
  if (shape === null) {
    return id;
  }
  const raw = model.body.slice(shape.label.from, shape.label.to);
  return decodeLabel(raw, shape.quoted) || id;
}

function arrowLabel(model: FlowModel, edge: FlowEdge): string {
  const arrow = model.chains[edge.chain]?.links[edge.link]?.arrow;
  const span = arrow?.label ?? null;
  const text =
    span === null || arrow === undefined
      ? ""
      : decodeLabel(model.body.slice(span.from, span.to), arrow.labelQuoted);
  const ends = `Arrow from ${boxLabel(model, edge.start)} to ${boxLabel(model, edge.end)}`;
  return text === "" ? ends : `${ends}: ${text}`;
}

/**
 * What the rename field shows for `target`, which is what is drawn: a box with no
 * label of its own shows its id, an arrow with none is empty. Null when the source
 * has no such box or arrow, so a target a peer took away can be told.
 */
export function flowLabelOf(
  model: FlowModel,
  target: FlowTarget,
): string | null {
  if (target.kind === "node") {
    return model.nodes.has(target.id) ? boxLabel(model, target.id) : null;
  }
  if (target.kind === "edge") {
    const edge = model.edges.find((e) => sameEdge(e, target.key));
    const arrow = edge && model.chains[edge.chain]?.links[edge.link]?.arrow;
    if (arrow === undefined) {
      return null;
    }
    return arrow.label === null
      ? ""
      : decodeLabel(
          model.body.slice(arrow.label.from, arrow.label.to),
          arrow.labelQuoted,
        );
  }
  return null;
}

/**
 * Makes the drawing's boxes and arrows stops: out of the tab order until the
 * focus code gives one `tabindex="0"`, named for a screen reader, and each arrow
 * with a wide transparent path beside it, so a thin line can be hit.
 */
export function decorateFlow(
  svg: SVGSVGElement,
  svgId: string,
  model: FlowModel,
  verdict: Verdict,
): void {
  for (const el of svg.querySelectorAll<SVGElement>("g.node")) {
    const id = nodeIdOf(el, svgId, verdict);
    if (id !== undefined) {
      el.setAttribute("tabindex", "-1");
      el.setAttribute("role", "button");
      el.setAttribute("aria-haspopup", "menu");
      el.setAttribute("aria-label", `Box: ${boxLabel(model, id)}`);
    }
  }
  for (const el of svg.querySelectorAll<SVGElement>('path[data-et="edge"]')) {
    const edgeId = el.getAttribute("data-id");
    const edge = model.edges.find((e) => e.mermaidId === edgeId);
    if (edge === undefined || edgeId === null) {
      continue;
    }
    el.setAttribute("tabindex", "-1");
    el.setAttribute("role", "button");
    el.setAttribute("aria-haspopup", "menu");
    el.setAttribute("aria-label", arrowLabel(model, edge));
    const hit = document.createElementNS(SVG_NS, "path");
    hit.setAttribute("d", el.getAttribute("d") ?? "");
    hit.setAttribute("data-hit-for", edgeId);
    hit.setAttribute("aria-hidden", "true");
    hit.style.fill = "none";
    hit.style.stroke = "transparent";
    hit.style.strokeWidth = "14px";
    hit.style.pointerEvents = "stroke";
    el.after(hit);
  }
}
