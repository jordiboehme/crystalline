/**
 * What the menus of a drawn sequence diagram offer. Every item is a call of
 * `runDiagramOp`, so one undo step and one verify, or of a thing that makes those
 * calls (the rename field, adding with a name, connect mode, removing the fence).
 *
 * The menus open, close and follow a peer's edit in `flowchart/actions.ts`, which
 * runs the menus of both kinds; this module only says what is in them.
 *
 * What an item acts on is read when it runs: a participant by its name, a message by
 * a ticket inside its arrow token, through `heldSeqOp`. A message whose place
 * cannot be told answers "stale", and the menu opens again on what stands now.
 *
 * A delete that would leave a block with nothing in it, or an activation open, is
 * offered disabled with the sentence that says why. "Remove block" takes away the
 * block around a message and keeps what it holds, which is how such a delete is
 * then made.
 */

import type { EditorView } from "@codemirror/view";

import {
  diagramAnchors,
  trackAnchor,
  untrackAnchor,
} from "../../diagramAnchors";
import { fenceToAct, placeNow } from "../act";
import { nextAnchorTicket, runDiagramOp, seqKind } from "../apply";
import type { RefusalCode } from "../edits";
import { NOTICES } from "../edits";
import { sayAt } from "../focus";
import type { LineHold } from "../held";
import {
  heldMessageOp,
  letGoOfLine,
  resolveHeldTarget,
  ticketOffset,
} from "../held";
import type { TargetKey } from "../intent";
import {
  addWithName,
  fenceNow,
  modelNow,
  openLabelEditor,
} from "../labelEditor";
import type { MenuItem } from "../menu";
import { seqMessageOf } from "./hit";
import type { SeqHead, SeqMessage, SeqModel, SeqTarget } from "./model";
import type { SeqOp } from "./ops";
import { blocksOf, compileSeqOp, heldSeqOp } from "./ops";

type MessageTarget = SeqTarget & { kind: "message" };

/** What the shared menu code lends a sequence menu. */
export interface SeqMenuActs {
  /** Starts connect mode from the participant `name`. */
  connect(anchor: number, name: string): void;
  /** Removes the whole fence. */
  deleteDiagram(anchor: number): void;
  /** The message's hold for the menu's items, kept by the shared menu code. */
  hold(target: MessageTarget): LineHold;
  /** Forgets the hold: an item's run now owns it, and lets it go when it is done. */
  release(): void;
}

/** The prompt connect mode shows while it waits for the receiver. */
export const SEQ_CONNECT_PROMPT =
  "Choose the participant to send the message to. Press Escape to cancel.";

/** Sentence for "Remove block" on a message that stands in no block. */
const NO_BLOCK = "This message is not in a block.";

type Resolver = (
  model: SeqModel,
  bodyFrom: number,
) => SeqOp | { refused: RefusalCode };

/** Runs one op on the fence, says a refusal, and lets the hold go when done. */
async function runSeqOp(
  main: EditorView,
  anchor: number,
  hold: LineHold | null,
  resolver: () => Resolver,
): Promise<void> {
  try {
    const place = placeNow(main, anchor);
    if (fenceToAct(main, place) === null) {
      return;
    }
    const outcome = await runDiagramOp(main, place, seqKind, resolver());
    if ("refused" in outcome) {
      sayAt(main, placeNow(main, place), outcome.refused);
    }
  } finally {
    if (hold !== null) {
      letGoOfLine(main, hold);
    }
  }
}

/** Why `op` could not be made, for an item that says so before it is chosen. */
function blockedBy(model: SeqModel, op: SeqOp): Partial<MenuItem> {
  const result = compileSeqOp(model, op);
  return "refused" in result
    ? { disabled: true, description: NOTICES[result.refused] }
    : {};
}

function participantMenu(
  main: EditorView,
  anchor: number,
  name: string,
  model: SeqModel,
  acts: SeqMenuActs,
): MenuItem[] {
  const remove: SeqOp = { op: "deleteParticipant", name };
  return [
    {
      label: "Rename",
      run: () => {
        openLabelEditor(
          main,
          placeNow(main, anchor),
          { kind: "participant", name },
          { selectAll: true },
        );
      },
    },
    {
      label: "Add participant",
      run: () => {
        void addWithName(main, placeNow(main, anchor), {
          op: "addParticipant",
        });
      },
    },
    {
      label: "Add message to...",
      run: () => {
        acts.connect(placeNow(main, anchor), name);
      },
    },
    {
      label: "Delete",
      run: () => {
        void runSeqOp(main, anchor, null, () => heldSeqOp(remove));
      },
      ...blockedBy(model, remove),
    },
  ];
}

/** The innermost block around `message`, by its number among `blocksOf`; -1 for none. */
export function blockAround(model: SeqModel, message: SeqMessage): number {
  let found = -1;
  let start = -1;
  blocksOf(model).forEach((block, index) => {
    const first = block[0];
    const last = block.at(-1);
    if (
      first !== undefined &&
      last !== undefined &&
      last.kind === "blockEnd" &&
      first.span.from < message.span.from &&
      message.span.to <= last.span.from &&
      first.span.from > start
    ) {
      found = index;
      start = first.span.from;
    }
  });
  return found;
}

/**
 * Removes the block around the held message as it stands now. The block is held by a
 * ticket in its first keyword while the op waits, so a peer's block put above is
 * not the one removed.
 */
async function removeBlockAround(
  main: EditorView,
  anchor: number,
  hold: LineHold,
  target: MessageTarget,
): Promise<void> {
  const place = placeNow(main, anchor);
  const fence = fenceToAct(main, place);
  const model = fence && modelNow(fence);
  if (fence === null || model === null || model.kind !== "sequence") {
    letGoOfLine(main, hold);
    return;
  }
  const now = resolveHeldTarget(main, target, hold, model, fence.bodyFrom);
  letGoOfLine(main, hold);
  if (typeof now === "string" || now.kind !== "message") {
    sayAt(main, place, typeof now === "string" ? now : "gone");
    return;
  }
  const message = seqMessageOf(model, now);
  const block = message === undefined ? -1 : blockAround(model, message);
  const start = blocksOf(model)[block]?.[0];
  if (start === undefined) {
    sayAt(main, place, "gone");
    return;
  }
  const ticket = nextAnchorTicket();
  main.dispatch({
    effects: trackAnchor.of({
      ticket,
      pos: fence.bodyFrom + start.span.from + 1,
    }),
  });
  try {
    await runSeqOp(main, place, null, () =>
      heldSeqOp({ op: "removeBlock", block }, model, (bodyFrom) =>
        ticketOffset(main, ticket, bodyFrom),
      ),
    );
  } finally {
    if (
      main.dom.isConnected &&
      main.state.field(diagramAnchors, false)?.has(ticket)
    ) {
      main.dispatch({ effects: untrackAnchor.of(ticket) });
    }
  }
}

/** The field opens on the message the menu was opened on, wherever it is now. */
function editText(
  main: EditorView,
  anchor: number,
  hold: LineHold,
  target: MessageTarget,
): void {
  const place = placeNow(main, anchor);
  const fence = fenceNow(main, place);
  const model = fence && modelNow(fence);
  const now =
    fence === null || model === null
      ? "gone"
      : resolveHeldTarget(main, target, hold, model, fence.bodyFrom);
  letGoOfLine(main, hold);
  if (typeof now === "string") {
    sayAt(main, place, now);
    return;
  }
  openLabelEditor(main, place, now, { selectAll: true });
}

const HEADS: readonly [string, SeqHead | "both"][] = [
  ["Arrow", "arrow"],
  ["Open", "open"],
  ["Cross", "cross"],
  ["Async", "async"],
  ["Both ends", "both"],
];

function messageMenu(
  main: EditorView,
  anchor: number,
  target: MessageTarget,
  model: SeqModel,
  acts: SeqMenuActs,
): MenuItem[] {
  const hold = acts.hold(target);
  const message = seqMessageOf(model, target);
  const op = (make: (at: MessageTarget) => SeqOp) => () => {
    acts.release();
    void runSeqOp(main, anchor, hold, () =>
      heldMessageOp(main, hold, target, make, model),
    );
  };
  const head =
    message === undefined
      ? null
      : message.arrow.both
        ? "both"
        : message.arrow.head;
  const block = message === undefined ? -1 : blockAround(model, message);
  return [
    {
      label: "Edit text",
      run: () => {
        acts.release();
        editText(main, anchor, hold, target);
      },
    },
    {
      label: "Line",
      submenu: (
        [
          ["Solid", "solid"],
          ["Dotted", "dotted"],
        ] as const
      ).map(([label, line]) => ({
        label,
        checked: message?.arrow.line === line,
        run: op((at) => ({ op: "arrowStyle", target: at, line })),
      })),
    },
    {
      label: "Arrowhead",
      submenu: HEADS.map(([label, value]) => ({
        label,
        checked: head === value,
        run: op((at) => ({ op: "arrowStyle", target: at, head: value })),
      })),
    },
    {
      label: "Delete",
      run: op((at) => ({ op: "deleteMessage", target: at })),
      ...blockedBy(model, { op: "deleteMessage", target }),
    },
    {
      label: "Remove block",
      run: () => {
        acts.release();
        void removeBlockAround(main, anchor, hold, target);
      },
      ...(block < 0 ? { disabled: true, description: NO_BLOCK } : {}),
    },
  ];
}

function diagramMenu(
  main: EditorView,
  anchor: number,
  acts: SeqMenuActs,
): MenuItem[] {
  return [
    {
      label: "Add participant",
      run: () => {
        void addWithName(main, placeNow(main, anchor), {
          op: "addParticipant",
        });
      },
    },
    {
      label: "Delete diagram",
      run: () => {
        acts.deleteDiagram(placeNow(main, anchor));
      },
    },
  ];
}

/** The menu of `target` on a sequence diagram: its accessible name and its items. */
export function seqMenu(
  main: EditorView,
  anchor: number,
  target: TargetKey,
  model: SeqModel,
  acts: SeqMenuActs,
): { name: string; items: MenuItem[] } {
  switch (target.kind) {
    case "participant":
      return {
        name: "Participant actions",
        items: participantMenu(main, anchor, target.name, model, acts),
      };
    case "message":
      return {
        name: "Message actions",
        items: messageMenu(main, anchor, target, model, acts),
      };
    default:
      return {
        name: "Diagram actions",
        items: diagramMenu(main, anchor, acts),
      };
  }
}
