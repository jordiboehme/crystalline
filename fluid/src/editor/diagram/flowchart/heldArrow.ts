/**
 * An arrow held by its place in the text, not by its occurrence, which a peer's
 * duplicate moves (R94). A ticket of `diagramAnchors` sits inside the arrow's token
 * and follows it through every change, so whoever acts on an arrow later (a field, a
 * menu, a key waiting for its verify) finds the arrow that was meant, or learns
 * that it cannot tell, and no label or delete lands on another occurrence.
 */

import type { StateEffect } from "@codemirror/state";
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
  /**
   * The ticket inside the arrow's token; null when no ticket was asked for, as for
   * a field brought back with no reading of its fence.
   */
  ticket: number | null;
  key: EdgeKey;
  /** With no ticket: no duplicate has been seen, and one seen counts for good. */
  sole: boolean;
}

/**
 * The one answer to "which arrow is this now?", for every action on an arrow: a menu
 * item, a key, the rename field, a stop. Connect mode has no arrow to ask about: it
 * holds a box by its id.
 *
 * Once a ticket was asked for, the answer is the arrow the ticket stands in, or
 * nothing: a ticket a deletion ran across, or one in no token of this pair, is "stale"
 * while the pair has an arrow, whatever is left of it, as that arrow may be somebody
 * else's (R94). It costs a "Try again" when a peer rewrites a whole token. Only with
 * no ticket asked for does a sole arrow answer for itself.
 */
export function resolveArrow(
  main: EditorView,
  hold: ArrowHold,
  model: FlowModel,
  bodyFrom: number,
): EdgeKey | "stale" | "gone" {
  if (hold.ticket === null) {
    hold.sole &&= flowPairCount(model, hold.key) <= 1;
    return flowEdgeAt(model, hold.key, null, hold.sole);
  }
  const pos = main.state.field(diagramAnchors, false)?.get(hold.ticket);
  return flowEdgeAt(
    model,
    hold.key,
    pos === undefined ? null : pos - bodyFrom,
    false,
  );
}

/** `target` now: a box stands as it is, and `flowLabelOf` says whether it is there. */
export function resolveTarget(
  main: EditorView,
  target: TargetKey,
  hold: ArrowHold | null,
  model: FlowModel,
  bodyFrom: number,
): TargetKey | "stale" | "gone" {
  if (target.kind !== "edge") {
    return target;
  }
  const edge = resolveArrow(
    main,
    hold ?? { ticket: null, key: target.key, sole: true },
    model,
    bodyFrom,
  );
  return typeof edge === "string" ? edge : { kind: "edge", key: edge };
}

/** Where a ticket for `key` of `model` goes: inside the token, clear of its edges. */
export function holdEffect(
  model: FlowModel,
  key: EdgeKey,
  bodyFrom: number,
): { hold: ArrowHold; effect: StateEffect<unknown> | null } {
  const span = flowEdgeSpanOf(model, key);
  const ticket = nextAnchorTicket();
  return {
    hold: { ticket, key, sole: flowPairCount(model, key) <= 1 },
    effect:
      span === null
        ? null
        : trackAnchor.of({ ticket, pos: bodyFrom + span + 1 }),
  };
}

/** Starts holding the arrow `key` of `model`, whose body starts at `bodyFrom`. */
export function holdArrow(
  main: EditorView,
  model: FlowModel,
  key: EdgeKey,
  bodyFrom: number,
): ArrowHold {
  const { hold, effect } = holdEffect(model, key, bodyFrom);
  if (effect !== null) {
    main.dispatch({ effects: effect });
  }
  return hold;
}

/** Stops holding: the ticket goes, whatever has become of the arrow. */
export function letGoOfArrow(main: EditorView, hold: ArrowHold): void {
  if (
    hold.ticket !== null &&
    main.dom.isConnected &&
    main.state.field(diagramAnchors, false)?.has(hold.ticket)
  ) {
    main.dispatch({ effects: untrackAnchor.of(hold.ticket) });
  }
}
