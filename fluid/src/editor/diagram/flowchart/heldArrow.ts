/**
 * An arrow held by its place in the text, not by its occurrence, which a peer's
 * duplicate moves (R94). A ticket of `diagramAnchors` sits inside the arrow's token
 * and follows it through every change, so whoever acts on an arrow later (a field, a
 * menu, a key waiting for its verify) finds the arrow that was meant, or learns
 * that it cannot tell, and no label or delete lands on another occurrence.
 */

import type { EditorView } from "@codemirror/view";

import {
  diagramAnchors,
  trackAnchor,
  untrackAnchor,
} from "../../diagramAnchors";
import { nextAnchorTicket } from "../apply";
import type { TargetKey } from "../intent";
import { flowEdgeAt, flowEdgeSpanOf, flowPairCount } from "./hit";
import type { EdgeKey, FlowModel } from "./model";

export interface ArrowHold {
  ticket: number;
  key: EdgeKey;
  /** No duplicate has been seen; an arrow stays not sole once one was (R94). */
  sole: boolean;
}

/**
 * The arrow's key in `model`, by the place `arrow` holds. A box's target stands as
 * it is, and `flowLabelOf` says whether it is there.
 */
export function heldTarget(
  main: EditorView,
  target: TargetKey,
  arrow: number | null,
  sole: boolean,
  model: FlowModel,
  bodyFrom: number,
): TargetKey | "stale" | "gone" {
  if (target.kind !== "edge") {
    return target;
  }
  const pos =
    arrow === null
      ? undefined
      : main.state.field(diagramAnchors, false)?.get(arrow);
  const edge = flowEdgeAt(
    model,
    target.key,
    pos === undefined ? null : pos - bodyFrom,
    sole,
  );
  return typeof edge === "string" ? edge : { kind: "edge", key: edge };
}

/** Starts holding the arrow `key` of `model`, whose body starts at `bodyFrom`. */
export function holdArrow(
  main: EditorView,
  model: FlowModel,
  key: EdgeKey,
  bodyFrom: number,
): ArrowHold {
  const ticket = nextAnchorTicket();
  const span = flowEdgeSpanOf(model, key);
  if (span !== null) {
    // Inside the token, so a change right at its edge stays clear of the ticket.
    main.dispatch({
      effects: trackAnchor.of({ ticket, pos: bodyFrom + span + 1 }),
    });
  }
  return { ticket, key, sole: flowPairCount(model, key) <= 1 };
}

/** The held arrow in `model` now, and what `model` says about duplicates. */
export function heldEdge(
  main: EditorView,
  hold: ArrowHold,
  model: FlowModel,
  bodyFrom: number,
): EdgeKey | "stale" | "gone" {
  noteDuplicates(hold, model);
  const held = heldTarget(
    main,
    { kind: "edge", key: hold.key },
    hold.ticket,
    hold.sole,
    model,
    bodyFrom,
  );
  if (typeof held === "string") {
    return held;
  }
  return held.kind === "edge" ? held.key : "gone";
}

/** A duplicate seen once counts for good: its own going later hands nothing over. */
export function noteDuplicates(hold: ArrowHold, model: FlowModel): void {
  hold.sole &&= flowPairCount(model, hold.key) <= 1;
}

/** Stops holding: the ticket goes, whatever has become of the arrow. */
export function letGoOfArrow(main: EditorView, hold: ArrowHold): void {
  if (
    main.dom.isConnected &&
    main.state.field(diagramAnchors, false)?.has(hold.ticket)
  ) {
    main.dispatch({ effects: untrackAnchor.of(hold.ticket) });
  }
}
