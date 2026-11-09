/**
 * A line of a diagram held by its place in the text: a flowchart's arrow, or a
 * sequence diagram's message. An arrow's occurrence and a message's move with every
 * duplicate a peer adds or removes before it, and its token does not, so a ticket of
 * `diagramAnchors` inside the token says which one was meant. A box or a
 * participant is named by its id, which no peer's line moves: it needs no hold.
 *
 * `flowchart/heldArrow.ts` holds arrows. This module holds messages the same way and
 * gives the shared pieces one answer for both.
 */

import type { StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

import { diagramAnchors, trackAnchor, untrackAnchor } from "../diagramAnchors";
import { nextAnchorTicket } from "./apply";
import type { ArrowHold } from "./flowchart/heldArrow";
import { holdEffect, letGoOfArrow, resolveArrow } from "./flowchart/heldArrow";
import type { TargetKey } from "./intent";
import type { DiagramModel } from "./kinds";
import { seqMessageSpanOf } from "./sequence/hit";
import type { SeqModel, SeqTarget } from "./sequence/model";
import type { SeqOp } from "./sequence/ops";
import { heldSeqOp, seqMessageNow } from "./sequence/ops";
import type { RefusalCode } from "./edits";

type MessageTarget = SeqTarget & { kind: "message" };

export interface MessageHold {
  /** The ticket inside the message's arrow token; null when none was asked for. */
  ticket: number | null;
  message: MessageTarget;
}

export type LineHold = ArrowHold | MessageHold;

const isMessageHold = (hold: LineHold): hold is MessageHold =>
  "message" in hold;

/** The offset in the body of a hold's ticket: null when it lost its place. */
export function ticketOffset(
  main: EditorView,
  ticket: number,
  bodyFrom: number,
): number | null {
  const pos = main.state.field(diagramAnchors, false)?.get(ticket);
  return pos === undefined ? null : pos - bodyFrom;
}

/**
 * The message a hold names now. Once a ticket was asked for it is the message the
 * ticket stands in, if that still says what was chosen, or nothing: `stale` while a
 * message of its ends or its text is left, which may be it. With no ticket only a
 * message with no equal answers for itself.
 */
export function resolveMessage(
  main: EditorView,
  hold: MessageHold,
  model: SeqModel,
  bodyFrom: number,
): MessageTarget | "stale" | "gone" {
  return seqMessageNow(
    model,
    hold.message,
    hold.ticket === null
      ? undefined
      : ticketOffset(main, hold.ticket, bodyFrom),
    undefined,
  );
}

/** Where a ticket for a line of `model` goes, and the hold that names it. */
export function lineHoldEffect(
  model: DiagramModel,
  target: TargetKey,
  bodyFrom: number,
): { hold: LineHold; effect: StateEffect<unknown> | null } | null {
  if (target.kind === "edge" && model.kind !== "sequence") {
    return holdEffect(model, target.key, bodyFrom);
  }
  if (target.kind === "message" && model.kind === "sequence") {
    const span = seqMessageSpanOf(model, target);
    const ticket = nextAnchorTicket();
    return {
      hold: { ticket, message: target },
      effect:
        span === null
          ? null
          : trackAnchor.of({ ticket, pos: bodyFrom + span + 1 }),
    };
  }
  return null;
}

/** A hold with no ticket, for a line no reading of the fence could place. */
export function ticketless(
  target: Extract<TargetKey, { kind: "edge" | "message" }>,
): LineHold {
  return target.kind === "edge"
    ? { ticket: null, key: target.key, sole: true }
    : { ticket: null, message: target };
}

/** Starts holding `target` when it is a line; null for any other stop. */
export function holdLine(
  main: EditorView,
  model: DiagramModel,
  target: TargetKey,
  bodyFrom: number,
): LineHold | null {
  const made = lineHoldEffect(model, target, bodyFrom);
  if (made === null) {
    return null;
  }
  if (made.effect !== null) {
    main.dispatch({ effects: made.effect });
  }
  return made.hold;
}

/** Stops holding: the ticket goes, whatever has become of the line. */
export function letGoOfLine(main: EditorView, hold: LineHold): void {
  if (!isMessageHold(hold)) {
    letGoOfArrow(main, hold);
    return;
  }
  if (
    hold.ticket !== null &&
    main.dom.isConnected &&
    main.state.field(diagramAnchors, false)?.has(hold.ticket)
  ) {
    main.dispatch({ effects: untrackAnchor.of(hold.ticket) });
  }
}

/** Whether `hold` names `target`'s line, whatever its occurrence is now. */
export function holdsLine(hold: LineHold, target: TargetKey): boolean {
  if (isMessageHold(hold)) {
    return (
      target.kind === "message" &&
      target.from === hold.message.from &&
      target.to === hold.message.to &&
      target.text === hold.message.text
    );
  }
  return (
    target.kind === "edge" &&
    target.key.start === hold.key.start &&
    target.key.end === hold.key.end
  );
}

/**
 * `target` now. A box, a participant or the canvas stands as it is (the label check
 * says whether it is there); a line is found through its hold, or with none, as a
 * hold with no ticket would find it.
 */
export function resolveHeldTarget(
  main: EditorView,
  target: TargetKey,
  hold: LineHold | null,
  model: DiagramModel,
  bodyFrom: number,
): TargetKey | "stale" | "gone" {
  if (target.kind === "edge") {
    if (model.kind === "sequence") {
      return "gone";
    }
    const edge = resolveArrow(
      main,
      hold !== null && !isMessageHold(hold)
        ? hold
        : { ticket: null, key: target.key, sole: true },
      model,
      bodyFrom,
    );
    return typeof edge === "string" ? edge : { kind: "edge", key: edge };
  }
  if (target.kind === "message") {
    if (model.kind !== "sequence") {
      return "gone";
    }
    return resolveMessage(
      main,
      hold !== null && isMessageHold(hold)
        ? hold
        : { ticket: null, message: target },
      model,
      bodyFrom,
    );
  }
  return target;
}

/** The stop a hold names, as it named it last. */
export function heldTargetOf(hold: LineHold): TargetKey {
  return isMessageHold(hold) ? hold.message : { kind: "edge", key: hold.key };
}

/** What a message hold names, for a caller that builds an op from it. */
export function heldMessage(hold: LineHold): MessageTarget | null {
  return isMessageHold(hold) ? hold.message : null;
}

/** Points a message hold at the message it found, its occurrence as it reads now. */
export function followLine(hold: LineHold, now: TargetKey): void {
  if (isMessageHold(hold)) {
    if (now.kind === "message") {
      hold.message = now;
    }
  } else if (now.kind === "edge") {
    hold.key = now.key;
  }
}

/**
 * The op on a held message, worked out on each read of the fence: through
 * `heldSeqOp` with the ticket's place, so the message holding the ticket is the one
 * meant. A hold with no ticket, or none at all, finds the message by its words and
 * holds only one with no equal. One resolver serves one `runDiagramOp` call.
 */
export function heldMessageOp(
  main: EditorView,
  hold: LineHold | null,
  target: MessageTarget,
  make: (message: MessageTarget) => SeqOp,
  read?: SeqModel,
): (model: SeqModel, bodyFrom: number) => SeqOp | { refused: RefusalCode } {
  const message = hold === null ? null : heldMessage(hold);
  const ticket = hold?.ticket ?? null;
  if (message !== null && ticket !== null) {
    return heldSeqOp(make(message), read, (bodyFrom) =>
      ticketOffset(main, ticket, bodyFrom),
    );
  }
  return (model, bodyFrom) => {
    const now = resolveMessage(
      main,
      { ticket: null, message: message ?? target },
      model,
      bodyFrom,
    );
    return typeof now === "string" ? { refused: now } : make(now);
  };
}
