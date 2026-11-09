/**
 * The one place the shared pieces (the keys, the menus, the rename field) ask which
 * kind of diagram a body is and what its parts are. A body is a sequence diagram by
 * the same rule the verdict uses; every other body is read as a flowchart, whose
 * parser refuses what it does not know.
 */

import {
  decorateFlow,
  flowElementOf,
  flowLabelOf,
  flowStops,
  flowTargetOf,
} from "./flowchart/hit";
import type { FlowModel } from "./flowchart/model";
import { parseFlowchart } from "./flowchart/parse";
import type { TargetKey } from "./intent";
import {
  decorateSequence,
  seqElementOf,
  seqLabelOf,
  seqStopAt,
  seqStops,
  seqTargetOf,
} from "./sequence/hit";
import type { SeqModel } from "./sequence/model";
import { parseSequence } from "./sequence/parse";
import type { Verdict } from "./verdict";

export type DiagramModel = FlowModel | SeqModel;

const CANVAS: TargetKey = { kind: "canvas" };

/** The model of `body`, of the kind its header says, as the verdict reads it. */
export function parseDiagram(body: string): DiagramModel {
  if (body.includes("sequenceDiagram")) {
    const sequence = parseSequence(body);
    if (sequence.scan.keyword === "sequenceDiagram") {
      return sequence;
    }
  }
  return parseFlowchart(body);
}

/** Canvas, then the parts in the order the keys walk them. */
export function stopsOf(model: DiagramModel): TargetKey[] {
  return model.kind === "sequence" ? seqStops(model) : flowStops(model);
}

/** What a drawn element stands for in the source: the canvas when nothing maps. */
export function targetOfElement(
  el: Element,
  svgId: string,
  model: DiagramModel,
  verdict: Verdict,
): TargetKey {
  return model.kind === "sequence"
    ? seqTargetOf(el, svgId, model, verdict)
    : flowTargetOf(el, svgId, model, verdict);
}

/** The element of a part on screen; null for the canvas, or a part of another kind. */
export function elementOfTarget(
  svg: SVGSVGElement,
  svgId: string,
  target: TargetKey,
  model: DiagramModel,
  verdict: Verdict,
): SVGElement | null {
  if (model.kind === "sequence") {
    return seqElementOf(svg, svgId, target, model, verdict);
  }
  return target.kind === "participant" || target.kind === "message"
    ? null
    : flowElementOf(svg, svgId, target, model, verdict);
}

/** Makes the drawing's parts stops, named for a screen reader. */
export function decorateDrawing(
  svg: SVGSVGElement,
  svgId: string,
  model: DiagramModel,
  verdict: Verdict,
): void {
  if (model.kind === "sequence") {
    decorateSequence(svg, svgId, model, verdict);
  } else {
    decorateFlow(svg, svgId, model, verdict);
  }
}

/**
 * What the rename field shows for `target`; null when the source has no such part,
 * so a part a peer took away can be told.
 */
export function labelOfTarget(
  model: DiagramModel,
  target: TargetKey,
): string | null {
  if (model.kind === "sequence") {
    return seqLabelOf(model, target);
  }
  return target.kind === "participant" || target.kind === "message"
    ? null
    : flowLabelOf(model, target);
}

/** The stop whose source holds `offset` in the body; the canvas otherwise. */
export function stopAtOffset(model: DiagramModel, offset: number): TargetKey {
  if (model.kind === "sequence") {
    return seqStopAt(model, offset);
  }
  for (const id of model.nodeOrder) {
    const refs = model.nodes.get(id)?.refs ?? [];
    if (refs.some((r) => r.span.from <= offset && offset <= r.span.to)) {
      return { kind: "node", id };
    }
  }
  for (const edge of model.edges) {
    const gap = model.chains[edge.chain]?.links[edge.link]?.gap;
    if (gap !== undefined && gap.from <= offset && offset <= gap.to) {
      return {
        kind: "edge",
        key: { start: edge.start, end: edge.end, occurrence: edge.occurrence },
      };
    }
  }
  return CANVAS;
}

/**
 * What connect mode can stand on and choose, by its id: a box, or a participant.
 * Null for any other stop.
 */
export function connectIdOf(target: TargetKey): string | null {
  if (target.kind === "node") {
    return target.id;
  }
  return target.kind === "participant" ? target.name : null;
}

/** The stop for connect mode's `id` in `model`, or null when it is not there. */
export function connectStopOf(
  model: DiagramModel,
  id: string,
): TargetKey | null {
  if (model.kind === "sequence") {
    return model.participants.has(id)
      ? { kind: "participant", name: id }
      : null;
  }
  return model.nodes.has(id) ? { kind: "node", id } : null;
}

/** Whether connect mode can stand on `target`. */
export function connectable(target: TargetKey): boolean {
  return connectIdOf(target) !== null;
}

/**
 * Where a stop goes when a peer's edit leaves its line unclear: an arrow's start
 * box, a message's sender. Null for a stop that is no line.
 */
export function startOf(target: TargetKey): TargetKey | null {
  if (target.kind === "edge") {
    return { kind: "node", id: target.key.start };
  }
  return target.kind === "message"
    ? { kind: "participant", name: target.from }
    : null;
}

/** Whether `target` is a line held by its place in the text: an arrow or a message. */
export function isLine(
  target: TargetKey,
): target is Extract<TargetKey, { kind: "edge" | "message" }> {
  return target.kind === "edge" || target.kind === "message";
}
