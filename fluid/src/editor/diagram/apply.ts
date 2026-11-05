/**
 * Where a compiled diagram op becomes a real edit: verified by mermaid, written
 * as one undo step, and safe with other people in the room.
 *
 * Verifying is asynchronous, so the text can change between compile and write.
 * The op therefore tracks its fence (`diagramAnchors`), and after the verdict
 * reads the fence again: an unchanged body is written in the same tick as that
 * read, so nothing can come between them; a changed one is compiled again once,
 * and a second change is `stale`. A verdict about a body that has moved on says
 * nothing, so the read comes before the verdict is looked at.
 *
 * Every op is verified, without exception. A result can pass the op's own check
 * and still be read differently by mermaid, and nothing is written unverified:
 * a check that could not run (mermaid failing to load) gives `unchecked`, which
 * neither writes nor calls the diagram broken.
 *
 * Lazy, as the parser and the verdict are. It runs `dispatch`, so it is never
 * called inside an update.
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import { EditorView } from "@codemirror/view";

import { diagramAnchors, trackAnchor, untrackAnchor } from "../diagramAnchors";
import { judgeBeforeCommand } from "../marks/commandJudge";
import { dispatchIsolated } from "../roomUndo";
import {
  type OpResult,
  type RefusalCode,
  type TextEdit,
  applyEdits,
  toChanges,
} from "./edits";
import { type MermaidFence, fenceAt } from "./fence";
import type { FlowCreated, FlowModel } from "./flowchart/model";
import { type FlowOp, compileFlowOp } from "./flowchart/ops";
import { parseFlowchart } from "./flowchart/parse";
import { type Verdict, requestVerdict } from "./verdict";

/**
 * One kind of diagram, as the apply layer drives it. `M` is its model, `O` its
 * ops and `Created` what its adding ops say they made. `dark` is the view's, for
 * the verdict's mermaid config.
 */
export interface DiagramKindOps<M, O, Created = unknown> {
  parse(body: string): M & { understood: boolean };
  compile(model: M, op: O): OpResult<Created>;
  verify(body: string, dark: boolean): Promise<Verdict>;
}

export type ApplyOutcome<Created = unknown> =
  | { applied: true; created?: Created }
  | { applied: false; refused: RefusalCode }
  | { applied: false; noop: true };

export const flowKind: DiagramKindOps<FlowModel, FlowOp, FlowCreated> = {
  parse: parseFlowchart,
  compile: compileFlowOp,
  verify: requestVerdict,
};

/** How long one read may wait for the parse. Far above a real edit's reparse. */
const PARSE_MS = 500;

let lastTicket = 0;

const refusal = (refused: RefusalCode) =>
  ({ applied: false, refused }) as const;
const NOOP = { applied: false, noop: true } as const;

/** Whether the page takes a write: an ended Reflection does not. */
function writable(view: EditorView): boolean {
  const { state } = view;
  return state.facet(EditorView.editable) && !state.readOnly;
}

/**
 * The fence at `pos`, on a tree that reaches it: an edit reparses on a budget
 * of 20 ms, so under load the tree can stop short of a fence that is there.
 */
function readFence(view: EditorView, pos: number): MermaidFence | null {
  const { state } = view;
  if (syntaxTree(state).length < state.doc.length) {
    ensureSyntaxTree(state, state.doc.length, PARSE_MS);
    view.dispatch({});
  }
  return fenceAt(view.state, pos);
}

/** The compiler's answer for `body`; a compiler that throws has made no edit. */
function compiled<M, O, Created>(
  kind: DiagramKindOps<M, O, Created>,
  body: string,
  op: O,
): OpResult<Created> {
  try {
    const model = kind.parse(body);
    return model.understood
      ? kind.compile(model, op)
      : { refused: "unsupported" };
  } catch {
    return { refused: "invalid" };
  }
}

/** The result of `edits`, or null for edits that do not fit the body. */
function resultOf(body: string, edits: readonly TextEdit[]): string | null {
  try {
    return applyEdits(body, edits);
  } catch {
    return null;
  }
}

/** The verdict, with a `verify` that threw read as one that could not run. */
async function verdictOn<M, O, Created>(
  kind: DiagramKindOps<M, O, Created>,
  body: string,
  dark: boolean,
): Promise<Verdict> {
  try {
    return await kind.verify(body, dark);
  } catch (error) {
    return { ok: false, reason: String(error), unavailable: true };
  }
}

/** The fence a ticket tracks now, or null when its place is lost. */
function trackedFence(view: EditorView, ticket: number): MermaidFence | null {
  if (!view.dom.isConnected) {
    return null;
  }
  const pos = view.state.field(diagramAnchors, false)?.get(ticket);
  return pos === undefined ? null : readFence(view, pos);
}

/**
 * Runs `op` on the mermaid fence at `anchor`. It does not reject. A page that
 * takes no writes, or stopped taking them while the op waited, gives `noop`:
 * no page offers an op there, and every other writer does nothing on one.
 */
export async function runDiagramOp<M, O, Created>(
  view: EditorView,
  anchor: number,
  kind: DiagramKindOps<M, O, Created>,
  op: O,
): Promise<ApplyOutcome<Created>> {
  if (!writable(view)) {
    return NOOP;
  }
  judgeBeforeCommand(view);
  let fence = readFence(view, anchor);
  if (fence === null) {
    return refusal("gone");
  }
  let ticket: number | null = null;
  try {
    for (let pass = 0; pass < 2; pass++) {
      const result = compiled(kind, fence.body, op);
      if ("refused" in result) {
        return refusal(result.refused);
      }
      if (result.edits.length === 0) {
        return NOOP;
      }
      const next = resultOf(fence.body, result.edits);
      if (next === null) {
        return refusal("invalid");
      }
      if (ticket === null) {
        // Inside the body: an insert right before the fence or right at the
        // body stays clear of it, and a deletion of the fence runs across it.
        ticket = ++lastTicket;
        view.dispatch({
          effects: trackAnchor.of({ ticket, pos: fence.bodyFrom }),
        });
      }
      const verdict = await verdictOn(
        kind,
        next,
        view.state.facet(EditorView.darkTheme),
      );
      judgeBeforeCommand(view);
      const now = trackedFence(view, ticket);
      if (now === null) {
        return refusal("gone");
      }
      if (now.body !== fence.body) {
        fence = now;
        continue;
      }
      if (!verdict.ok) {
        return refusal(verdict.unavailable ? "unchecked" : "invalid");
      }
      if (!writable(view)) {
        return NOOP;
      }
      dispatchIsolated(view, {
        changes: toChanges(now.bodyFrom, result.edits, view.state.lineBreak),
        userEvent: "input.diagram",
      });
      return result.created === undefined
        ? { applied: true }
        : { applied: true, created: result.created };
    }
    return refusal("stale");
  } finally {
    if (
      ticket !== null &&
      view.dom.isConnected &&
      view.state.field(diagramAnchors, false)?.has(ticket)
    ) {
      view.dispatch({ effects: untrackAnchor.of(ticket) });
    }
  }
}
