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

import { ensureSyntaxTree, language, syntaxTree } from "@codemirror/language";
import type { Parser } from "@lezer/common";
import type { EditorState, Line } from "@codemirror/state";
import { EditorSelection } from "@codemirror/state";
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

/** A ticket of `diagramAnchors` no op holds, for a place held across several ops. */
export function nextAnchorTicket(): number {
  lastTicket += 1;
  return lastTicket;
}

/**
 * An op worked out again on each read of the fence, for one that names a part by
 * something the text can move, as an arrow's occurrence (R94).
 */
export type OpResolver<M, O> = (
  model: M,
  bodyFrom: number,
) => O | { refused: RefusalCode };

const refusal = (refused: RefusalCode) =>
  ({ applied: false, refused }) as const;
const NOOP = { applied: false, noop: true } as const;

/**
 * Whether an IME is composing. The judge stands back then, and a write would
 * end the composition's log unjudged, so the person would lose their Bold.
 */
function composingNow(view: EditorView): boolean {
  return view.composing || view.compositionStarted;
}

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
  fence: MermaidFence,
  op: O | OpResolver<M, O>,
): OpResult<Created> {
  try {
    const model = kind.parse(fence.body);
    if (!model.understood) {
      return { refused: "unsupported" };
    }
    const resolved =
      typeof op === "function"
        ? (op as OpResolver<M, O>)(model, fence.bodyFrom)
        : op;
    if (
      typeof resolved === "object" &&
      resolved !== null &&
      "refused" in resolved
    ) {
      return resolved;
    }
    return kind.compile(model, resolved);
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
 * Removes the whole fence at `anchor` as one undo step: its lines and the line
 * break after them when a plain line follows. No next key lands on a mark: with
 * no such line the fence's line stays, empty, and takes the caret.
 * Same refusals as an op, and synchronous, since nothing is left to verify.
 */
export function removeFence(view: EditorView, anchor: number): ApplyOutcome {
  if (!writable(view)) {
    return NOOP;
  }
  if (composingNow(view)) {
    return refusal("stale");
  }
  const ticket = ++lastTicket;
  view.dispatch({ effects: trackAnchor.of({ ticket, pos: anchor }) });
  try {
    judgeBeforeCommand(view);
    const fence = trackedFence(view, ticket);
    if (fence === null) {
      return refusal("gone");
    }
    if (!fence.topLevel) {
      return refusal("unsupported");
    }
    const { state } = view;
    const { doc } = state;
    const first = doc.lineAt(fence.from);
    const last = doc.lineAt(fence.to);
    const below = last.number < doc.lines ? doc.line(last.number + 1) : null;
    const from = first.from;
    // A plain line below takes the caret. Any other block below starts with a mark
    // a letter would break, so the fence's line stays, empty, and what is typed
    // next has a line of its own (R83).
    const above = first.number > 1 ? doc.line(first.number - 1) : null;
    const to =
      below === null || !plainLine(state, below, above) ? last.to : below.from;
    dispatchIsolated(view, {
      changes: { from, to },
      selection: EditorSelection.cursor(from),
      userEvent: "delete.diagram",
    });
    if (view.state.doc === doc) {
      return refusal("invalid");
    }
    return { applied: true };
  } finally {
    if (
      view.dom.isConnected &&
      view.state.field(diagramAnchors, false)?.has(ticket)
    ) {
      view.dispatch({ effects: untrackAnchor.of(ticket) });
    }
  }
}

/**
 * Whether a letter at the start of `line` is only a letter: blank, or a paragraph
 * that changes nothing above when it moves up under it. `===` or `---|---` under a
 * line of text would make that line a heading or a table, and `foo` under a list
 * item, a quote or a table's row would become part of it, with no byte changed. So
 * the parser is asked rather than a list of such lines kept: the two lines together
 * must read as the line above did, plus a paragraph, or as one paragraph.
 */
function plainLine(
  state: EditorState,
  line: Line,
  above: Line | null,
): boolean {
  if (line.text.trim() === "") {
    return true;
  }
  if (
    /^\s*\$\$/.test(line.text) ||
    syntaxTree(state).topNode.childAfter(line.from)?.name !== "Paragraph"
  ) {
    return false;
  }
  if (above === null) {
    return true;
  }
  const parser = state.facet(language)?.parser;
  if (parser === undefined) {
    return true;
  }
  // The whole block above: a table's last row alone reads as a paragraph.
  const block = syntaxTree(state).topNode.childAfter(above.from);
  const text =
    block !== null && block.from <= above.from
      ? state.doc.sliceString(block.from, above.to)
      : above.text;
  const alone = blocksOf(parser, text);
  const joined = blocksOf(parser, `${text}\n${line.text}`);
  const want = alone.at(-1) === "Paragraph" ? alone : [...alone, "Paragraph"];
  return joined.join() === want.join();
}

/** The names of the top-level blocks `text` reads as, alone. */
function blocksOf(parser: Parser, text: string): string[] {
  const names: string[] = [];
  for (
    let node = parser.parse(text).topNode.firstChild;
    node !== null;
    node = node.nextSibling
  ) {
    names.push(node.name);
  }
  return names;
}

/**
 * Runs `op` on the mermaid fence at `anchor`, a position in the text as it
 * stands when this is called. It does not reject. A page that takes no writes,
 * or stopped taking them while the op waited, gives `noop`: no page offers an op
 * there, and every other writer does nothing on one. A running composition gives
 * `stale`. A fence that is not top level gives `unsupported`: its body carries
 * prefixes, and no page offers it.
 */
export async function runDiagramOp<M, O, Created>(
  view: EditorView,
  anchor: number,
  kind: DiagramKindOps<M, O, Created>,
  op: O | OpResolver<M, O>,
): Promise<ApplyOutcome<Created>> {
  if (!writable(view)) {
    return NOOP;
  }
  if (composingNow(view)) {
    return refusal("stale");
  }
  // From before the judge: its correction moves what the caller pointed at.
  const ticket = ++lastTicket;
  view.dispatch({ effects: trackAnchor.of({ ticket, pos: anchor }) });
  try {
    judgeBeforeCommand(view);
    let fence = trackedFence(view, ticket);
    if (fence === null) {
      return refusal("gone");
    }
    if (!fence.topLevel) {
      return refusal("unsupported");
    }
    // Inside the body: an insert right before the fence or right at the body
    // stays clear of it, and a deletion of the fence runs across it.
    view.dispatch({
      effects: trackAnchor.of({ ticket, pos: fence.bodyFrom }),
    });
    for (let pass = 0; pass < 2; pass++) {
      const result = compiled(kind, fence, op);
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
      const verdict = await verdictOn(
        kind,
        next,
        view.state.facet(EditorView.darkTheme),
      );
      if (!writable(view)) {
        return NOOP;
      }
      if (composingNow(view)) {
        return refusal("stale");
      }
      judgeBeforeCommand(view);
      const now = trackedFence(view, ticket);
      if (now === null) {
        return refusal("gone");
      }
      if (!now.topLevel) {
        return refusal("unsupported");
      }
      if (now.body !== fence.body) {
        fence = now;
        continue;
      }
      if (!verdict.ok) {
        return refusal(verdict.unavailable ? "unchecked" : "invalid");
      }
      const written = view.state.doc;
      dispatchIsolated(view, {
        changes: toChanges(
          now.bodyFrom,
          result.edits,
          view.state.lineBreak,
          now.body,
        ),
        userEvent: "input.diagram",
      });
      if (view.state.doc === written) {
        return refusal("invalid");
      }
      return result.created === undefined
        ? { applied: true }
        : { applied: true, created: result.created };
    }
    return refusal("stale");
  } finally {
    if (
      view.dom.isConnected &&
      view.state.field(diagramAnchors, false)?.has(ticket)
    ) {
      view.dispatch({ effects: untrackAnchor.of(ticket) });
    }
  }
}
