/**
 * The apply layer on the real surface, solo and in a room: where a compiled op
 * becomes one verified edit and one undo step. `apply.marks-hidden.test.ts` and
 * `apply.marks-show.test.ts` run this file again with the marks layer on; here,
 * with no mode set, the views are Raw.
 *
 * `verify` is stubbed wherever a test needs to act between compile and write,
 * or to stay inside the room's 500 ms capture window. The tests that name
 * `flowKind` as it is run real mermaid.
 */

import { syntaxTree } from "@codemirror/language";
import { EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, test, vi } from "vitest";

import { cellMarksMode } from "../../test/cellMarksMode";
import type { MarkViewOptions } from "../../test/markKit";
import {
  bytes,
  cleanupMarkViews,
  markView,
  setRaw,
  undoOnce,
  undoSteps,
} from "../../test/markKit";
import { parsedView } from "../../test/parse";
import { diagramAnchors } from "../diagramAnchors";
import { commandJudge } from "../marks/commandJudge";
import { hiddenMarkLayerOn, marksOnEditLine } from "../marks/mode";
import { baseExtensions } from "../setup";
import { flowKind, runDiagramOp } from "./apply";
import { applyEdits } from "./edits";
import { compileFlowOp, type FlowOp } from "./flowchart/ops";
import { parseFlowchart } from "./flowchart/parse";
import type { Verdict } from "./verdict";

afterEach(cleanupMarkViews);

const BODY = "flowchart TD\n    A[First step] --> B[Next step]";
const fenced = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;
const DOC = `Intro\n\n${fenced(BODY)}\n\nOutro\n`;
const ANCHOR = DOC.indexOf("```");
const RENAME: FlowOp = { op: "rename", id: "B", text: "Done" };
const RENAMED = DOC.replace("Next step", "Done");
const ADD: FlowOp = { op: "addChild", id: "A" };

const OK: Verdict = { ok: true, kind: "flowchart", domIds: new Map() };

/** `flowKind` with `verify` replaced; the stub also counts what it is asked. */
function withVerify(
  verify: (body: string, dark: boolean) => Promise<Verdict> | Verdict,
) {
  const asked: string[] = [];
  return {
    asked,
    kind: {
      ...flowKind,
      verify: (body: string, dark: boolean) => {
        asked.push(body);
        return Promise.resolve(verify(body, dark));
      },
    },
  };
}

/** The views of this run's mode: Raw off the twins, the marks layer on them. */
function modeOptions(): MarkViewOptions {
  const mode = cellMarksMode();
  return mode === "off" ? { raw: true } : { mode };
}

function viewOf(
  text: string,
  room: boolean,
  options: MarkViewOptions = {},
): EditorView {
  return markView(text, { ...modeOptions(), room, ...options });
}

/** Types at `pos` as a keystroke does. */
function typeAt(view: EditorView, pos: number, text: string): void {
  view.dispatch({
    changes: { from: pos, insert: text },
    userEvent: "input.type",
  });
}

const tracked = (view: EditorView) => view.state.field(diagramAnchors).size;

test("this run's views carry this run's marks layer", () => {
  const mode = cellMarksMode();
  const view = viewOf(DOC, false);
  expect(view.state.facet(hiddenMarkLayerOn)).toBe(mode !== "off");
  expect(view.state.facet(marksOnEditLine)).toBe(mode === "show");
});

describe.each([
  ["solo", false],
  ["in a room", true],
])("runDiagramOp %s", (_name, room) => {
  test("a rename on the skeleton applies, through real mermaid", async () => {
    const view = viewOf(DOC, room);
    await expect(runDiagramOp(view, ANCHOR, flowKind, RENAME)).resolves.toEqual(
      { applied: true },
    );
    expect(bytes(view.state)).toBe(RENAMED);
    expect(tracked(view)).toBe(0);
  });

  test("an add names what it made, typed", async () => {
    const view = viewOf(DOC, room);
    const outcome = await runDiagramOp(view, ANCHOR, flowKind, ADD);
    if (!outcome.applied) {
      throw new Error("refused");
    }
    // No cast: `flowKind` says what `created` holds.
    const id: string | undefined = outcome.created?.id;
    expect(id).toBe("C");
    expect(outcome.created).toEqual({ kind: "node", id: "C" });
    expect(bytes(view.state)).toContain(
      "--> B[Next step]\n    A --> C[New box]\n```",
    );
  });

  test("an anchor anywhere in the fence finds it", async () => {
    const view = viewOf(DOC, room);
    const inside = DOC.indexOf("Next step");
    const { kind } = withVerify(() => OK);
    await runDiagramOp(view, inside, kind, RENAME);
    expect(bytes(view.state)).toBe(RENAMED);
  });

  test("the op is one undo step, whatever was typed just before", async () => {
    const view = viewOf(DOC, room);
    const { kind } = withVerify(() => OK);
    const start = performance.now();
    typeAt(view, 0, "x");
    await runDiagramOp(view, ANCHOR + 1, kind, RENAME);
    expect(performance.now() - start).toBeLessThan(400);
    expect(bytes(view.state)).toBe(`x${RENAMED}`);
    undoOnce(view);
    expect(bytes(view.state)).toBe(`x${DOC}`);
    undoOnce(view);
    expect(bytes(view.state)).toBe(DOC);
  });

  test("typing right after the op, right beside its edit, is a step of its own", async () => {
    const view = viewOf(DOC, room);
    const { kind } = withVerify(() => OK);
    await runDiagramOp(view, ANCHOR, kind, RENAME);
    const end = RENAMED.indexOf("Done") + "Done".length;
    typeAt(view, end, "y");
    expect(bytes(view.state)).toBe(RENAMED.replace("Done", "Doney"));
    undoOnce(view);
    expect(bytes(view.state)).toBe(RENAMED);
    undoOnce(view);
    expect(bytes(view.state)).toBe(DOC);
  });

  test("tracking the anchor adds no undo step", async () => {
    const view = viewOf(DOC, room);
    const before = undoSteps(view);
    const { kind } = withVerify(() => ({ ok: false, reason: "no" }));
    await runDiagramOp(view, ANCHOR, kind, RENAME);
    expect(undoSteps(view)).toBe(before);
  });

  test("a body changed during verify is compiled again once and applied to", async () => {
    const view = viewOf(DOC, room);
    let first = true;
    const { kind, asked } = withVerify(() => {
      if (first) {
        first = false;
        view.dispatch({
          changes: { from: DOC.indexOf("First"), insert: "Very " },
        });
      }
      return OK;
    });
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: true,
    });
    expect(bytes(view.state)).toBe(
      DOC.replace("First", "Very First").replace("Next step", "Done"),
    );
    expect(asked).toEqual([
      BODY.replace("Next step", "Done"),
      BODY.replace("First", "Very First").replace("Next step", "Done"),
    ]);
    expect(tracked(view)).toBe(0);
  });

  test("a body changed twice is stale, and nothing of the op is written", async () => {
    const view = viewOf(DOC, room);
    let at = DOC.indexOf("First");
    const { kind, asked } = withVerify(() => {
      view.dispatch({ changes: { from: at, insert: "x" } });
      at += 1;
      return OK;
    });
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: false,
      refused: "stale",
    });
    expect(asked).toHaveLength(2);
    expect(bytes(view.state)).toBe(DOC.replace("First", "xxFirst"));
    expect(tracked(view)).toBe(0);
  });

  test("a verdict about a body that has moved on says nothing", async () => {
    const view = viewOf(DOC, room);
    let first = true;
    const { kind } = withVerify(() => {
      if (first) {
        first = false;
        view.dispatch({
          changes: { from: DOC.indexOf("First"), insert: "Very " },
        });
        return { ok: false, reason: "about the old body" };
      }
      return OK;
    });
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: true,
    });
  });

  test("a change before the fence moves the write, with no second compile", async () => {
    const view = viewOf(DOC, room);
    const { kind, asked } = withVerify(() => {
      view.dispatch({ changes: { from: 0, insert: "New first line\n\n" } });
      return OK;
    });
    await runDiagramOp(view, ANCHOR, kind, RENAME);
    expect(bytes(view.state)).toBe(`New first line\n\n${RENAMED}`);
    expect(asked).toHaveLength(1);
  });

  test("text put right before the fence, right at its body or right after it does not lose it", async () => {
    for (const [at, insert] of [
      [ANCHOR, "Line\n\n"],
      [DOC.indexOf("flowchart"), "%% note\n"],
      [DOC.lastIndexOf("```") + 3, "\n\nAfter"],
    ] as const) {
      const view = viewOf(DOC, room);
      let first = true;
      const { kind } = withVerify(() => {
        if (first) {
          first = false;
          view.dispatch({ changes: { from: at, insert } });
        }
        return OK;
      });
      await runDiagramOp(view, ANCHOR, kind, RENAME);
      expect(bytes(view.state)).toBe(
        (DOC.slice(0, at) + insert + DOC.slice(at)).replace(
          "Next step",
          "Done",
        ),
      );
    }
  });

  test("a fence deleted during verify is gone, and the next fence is not written to", async () => {
    // The same body twice: a mapped position landing on the next fence would
    // read as "unchanged" and write there.
    const one = fenced(BODY);
    const text = `${one}\n${one}\n`;
    const view = viewOf(text, room);
    const { kind } = withVerify(() => {
      view.dispatch({ changes: { from: 0, to: one.length + 1 } });
      return OK;
    });
    await expect(runDiagramOp(view, 0, kind, RENAME)).resolves.toEqual({
      applied: false,
      refused: "gone",
    });
    expect(bytes(view.state)).toBe(`${one}\n`);
    expect(tracked(view)).toBe(0);
  });

  test("a verdict of not ok gives invalid and no change", async () => {
    const view = viewOf(DOC, room);
    const { kind } = withVerify(() => ({ ok: false, reason: "no" }));
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: false,
      refused: "invalid",
    });
    expect(bytes(view.state)).toBe(DOC);
    expect(tracked(view)).toBe(0);
  });

  test("verify that could not run writes nothing and does not call the diagram broken", async () => {
    const view = viewOf(DOC, room);
    for (const verify of [
      () =>
        Promise.resolve<Verdict>({
          ok: false,
          reason: "mermaid could not be loaded",
          unavailable: true,
        }),
      () => Promise.reject<Verdict>(new Error("boom")),
    ]) {
      const { kind } = withVerify(verify);
      await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
        applied: false,
        refused: "unchecked",
      });
      expect(bytes(view.state)).toBe(DOC);
      expect(tracked(view)).toBe(0);
    }
  });

  test("every op is verified, and with the text it would write", async () => {
    const body = [
      "flowchart TD",
      "    A[First step] --> B[Next step]",
      "    B --> D",
    ].join("\n");
    const edge = { start: "A", end: "B", occurrence: 0 };
    const ops: FlowOp[] = [
      { op: "rename", id: "B", text: "Done" },
      { op: "shape", id: "B", shape: "rounded" },
      { op: "colour", id: "B", colour: "green" },
      { op: "direction", value: "LR" },
      { op: "addChild", id: "A" },
      { op: "addSibling", id: "B" },
      { op: "connect", from: "D", to: "A" },
      { op: "addBox" },
      { op: "deleteNode", id: "D" },
      { op: "deleteEdge", edge },
      { op: "edgeLabel", edge, text: "yes" },
      { op: "lineStyle", edge, line: "thick" },
      { op: "arrowhead", edge, head: "both" },
      { op: "reverse", edge },
    ];
    for (const op of ops) {
      const text = fenced(body);
      const view = viewOf(text, room);
      const { kind, asked } = withVerify(() => ({ ok: false, reason: "no" }));
      const compiled = compileFlowOp(parseFlowchart(body), op);
      if ("refused" in compiled) {
        throw new Error(`${op.op} refused ${compiled.refused}`);
      }
      await expect(runDiagramOp(view, 0, kind, op)).resolves.toEqual({
        applied: false,
        refused: "invalid",
      });
      expect(asked).toEqual([applyEdits(body, compiled.edits)]);
      expect(bytes(view.state)).toBe(text);
    }
  });

  test("real mermaid has the last word on a body the op's own check accepts", async () => {
    const body = 'flowchart TD\n  A@{ shape: foo, label: "x" }\n  B[y]';
    const op: FlowOp = { op: "rename", id: "B", text: "z" };
    expect("edits" in compileFlowOp(parseFlowchart(body), op)).toBe(true);
    const view = viewOf(fenced(body), room);
    await expect(runDiagramOp(view, 0, flowKind, op)).resolves.toEqual({
      applied: false,
      refused: "invalid",
    });
    expect(bytes(view.state)).toBe(fenced(body));
  });

  test("a rename to a direction text refuses (R7)", async () => {
    const view = viewOf(DOC, room);
    await expect(
      runDiagramOp(view, ANCHOR, flowKind, {
        op: "rename",
        id: "B",
        text: "go direction LR",
      }),
    ).resolves.toEqual({ applied: false, refused: "invalid" });
    expect(bytes(view.state)).toBe(DOC);
  });

  test("no fence at the anchor is gone", async () => {
    const view = viewOf(DOC, room);
    const { kind, asked } = withVerify(() => OK);
    await expect(runDiagramOp(view, 0, kind, RENAME)).resolves.toEqual({
      applied: false,
      refused: "gone",
    });
    expect(asked).toEqual([]);
  });

  test("a body the parser does not understand is unsupported", async () => {
    const text = fenced("flowchart TD\n  A(-x-)");
    const view = viewOf(text, room);
    const { kind, asked } = withVerify(() => OK);
    await expect(runDiagramOp(view, 0, kind, RENAME)).resolves.toEqual({
      applied: false,
      refused: "unsupported",
    });
    expect(asked).toEqual([]);
    expect(tracked(view)).toBe(0);
  });

  test("a compile refusal is returned as it is, with verify not asked", async () => {
    const view = viewOf(DOC, room);
    const { kind, asked } = withVerify(() => OK);
    await expect(
      runDiagramOp(view, ANCHOR, kind, { op: "rename", id: "Z", text: "t" }),
    ).resolves.toEqual({ applied: false, refused: "gone" });
    expect(asked).toEqual([]);
  });

  test("an op the source already says is a noop", async () => {
    const view = viewOf(DOC, room);
    const before = undoSteps(view);
    const { kind, asked } = withVerify(() => OK);
    await expect(
      runDiagramOp(view, ANCHOR, kind, {
        op: "rename",
        id: "B",
        text: "Next step",
      }),
    ).resolves.toEqual({ applied: false, noop: true });
    expect(asked).toEqual([]);
    expect(undoSteps(view)).toBe(before);
  });

  test("a compiler that throws refuses rather than rejects", async () => {
    const view = viewOf(DOC, room);
    const kind = {
      ...flowKind,
      compile: () => {
        throw new RangeError("overlap");
      },
    };
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: false,
      refused: "invalid",
    });
    expect(bytes(view.state)).toBe(DOC);
  });

  test("two ops at once both land, the second compiled on the first's result", async () => {
    const view = viewOf(DOC, room);
    const { kind } = withVerify(() => OK);
    const outcomes = await Promise.all([
      runDiagramOp(view, ANCHOR, kind, ADD),
      runDiagramOp(view, ANCHOR, kind, ADD),
    ]);
    expect(outcomes.map((o) => o.applied)).toEqual([true, true]);
    const text = bytes(view.state);
    expect(text).toContain("A --> C[New box]");
    expect(text).toContain("A --> D[New box]");
    expect(tracked(view)).toBe(0);
  });

  test("a page that stopped taking writes during verify is not written to", async () => {
    const view = viewOf(DOC, room);
    const { kind } = withVerify(() => {
      view.dispatch({
        effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
      });
      return OK;
    });
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: false,
      noop: true,
    });
    expect(bytes(view.state)).toBe(DOC);
  });

  test("a read-only page takes nothing, and verify is not asked", async () => {
    const view = viewOf(DOC, room);
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    const { kind, asked } = withVerify(() => OK);
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: false,
      noop: true,
    });
    expect(asked).toEqual([]);
  });

  test("a buffer swapped during verify loses the anchor", async () => {
    const view = viewOf(DOC, room);
    const { kind } = withVerify(() => {
      view.setState(
        EditorState.create({ doc: DOC, extensions: [diagramAnchors] }),
      );
      return OK;
    });
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: false,
      refused: "gone",
    });
    expect(bytes(view.state)).toBe(DOC);
  });

  test("a view destroyed during verify is gone, and nothing throws", async () => {
    const view = viewOf(DOC, room);
    const { kind } = withVerify(() => {
      view.destroy();
      return OK;
    });
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: false,
      refused: "gone",
    });
  });

  test("Raw turning on during verify does not lose the fence", async () => {
    const view = markView(DOC, { room, mode: "hidden" });
    const { kind } = withVerify(() => {
      setRaw(view, true);
      return OK;
    });
    await expect(runDiagramOp(view, ANCHOR, kind, RENAME)).resolves.toEqual({
      applied: true,
    });
    expect(bytes(view.state)).toBe(RENAMED);
  });

  test("the verdict gets the view's dark", async () => {
    const view = viewOf(DOC, room);
    const seen: boolean[] = [];
    const { kind } = withVerify((_body, dark) => {
      seen.push(dark);
      return OK;
    });
    await runDiagramOp(view, ANCHOR, kind, RENAME);
    expect(seen).toEqual([false]);
  });

  test("a parse that stopped short of the fence is carried on, not read as gone", async () => {
    const filler = "A paragraph of words that fills the page.\n\n".repeat(4000);
    const text = `${filler}${fenced(BODY)}\n`;
    const view = viewOf(text, room, { parsed: false });
    expect(syntaxTree(view.state).length).toBeLessThan(view.state.doc.length);
    const { kind } = withVerify(() => OK);
    await expect(
      runDiagramOp(view, filler.length, kind, RENAME),
    ).resolves.toEqual({ applied: true });
    expect(bytes(view.state)).toBe(
      `${filler}${fenced(BODY.replace("Next step", "Done"))}\n`,
    );
  });
});

describe("line breaks through a real view (R51)", () => {
  const crlf = DOC.replaceAll("\n", "\r\n");

  test("a CRLF document takes CRLF in the new line and keeps every other break", async () => {
    const view = viewOf(crlf, false, { lineSeparator: "\r\n" });
    await runDiagramOp(view, ANCHOR, flowKind, ADD);
    expect(bytes(view.state)).toBe(
      crlf.replace(
        "B[Next step]\r\n",
        "B[Next step]\r\n    A --> C[New box]\r\n",
      ),
    );
  });

  // In a CRLF buffer a lone `\n` is a character inside a line, and the markdown
  // parser reads a buffer line as one line. So the body can hold lone breaks,
  // and both fence lines must end in the buffer's own break.
  const MIXED = [
    "Intro\nmore\r\n\r\n```mermaid\r\n",
    "flowchart TD\n",
    "    A[First step] --> B[Next step]\r\n",
    "    B --> D\n",
    "    D --> E\r\n",
    "```\r\nOutro\nend\r\n",
  ];
  const MIXED_TEXT = MIXED.join("");
  const IN_BODY = MIXED_TEXT.indexOf("flowchart");

  test("a mixed document leaves each untouched line its own break", async () => {
    const view = viewOf(MIXED_TEXT, false, { lineSeparator: "\r\n" });
    await expect(
      runDiagramOp(view, IN_BODY, flowKind, ADD),
    ).resolves.toMatchObject({ applied: true });
    expect(bytes(view.state)).toBe(
      MIXED_TEXT.replace(
        "B[Next step]\r\n",
        "B[Next step]\r\n    A --> F[New box]\r\n",
      ),
    );
  });

  test("a mixed document's rename and delete touch only what they mean", async () => {
    const view = viewOf(MIXED_TEXT, false, { lineSeparator: "\r\n" });
    await runDiagramOp(view, IN_BODY, flowKind, RENAME);
    expect(bytes(view.state)).toBe(MIXED_TEXT.replace("Next step", "Done"));
    await runDiagramOp(view, IN_BODY, flowKind, {
      op: "deleteEdge",
      edge: { start: "B", end: "D", occurrence: 0 },
    });
    expect(bytes(view.state)).toBe(
      MIXED_TEXT.replace("Next step", "Done").replace("    B --> D\n", ""),
    );
  });

  test("a fence framed by lone breaks in a CRLF document is no fence, and is left alone", async () => {
    const text = `a\r\nb\n\`\`\`mermaid\n${BODY}\n\`\`\`\n`;
    const view = viewOf(text, false, { lineSeparator: "\r\n" });
    await expect(
      runDiagramOp(view, text.indexOf("```"), flowKind, RENAME),
    ).resolves.toEqual({ applied: false, refused: "gone" });
    expect(bytes(view.state)).toBe(text);
  });
});

describe("a waiting composition (D60)", () => {
  function judged(onJudge: (view: EditorView) => void): EditorView {
    const view = new EditorView({
      state: EditorState.create({
        doc: DOC,
        extensions: [
          ...baseExtensions(false),
          diagramAnchors,
          commandJudge.of(onJudge),
        ],
      }),
      parent: document.body,
    });
    return parsedView(view);
  }

  test("is judged before the op reads the text", async () => {
    const seen: string[] = [];
    const view = judged((v) => seen.push(bytes(v.state)));
    const { kind } = withVerify(() => OK);
    await runDiagramOp(view, ANCHOR, kind, RENAME);
    expect(seen.length).toBeGreaterThan(0);
    expect(bytes(view.state)).toBe(RENAMED);
    view.destroy();
  });

  test("whose judge changes the fence during verify is read before the write", async () => {
    let waiting = false;
    const view = judged((v) => {
      if (waiting) {
        waiting = false;
        v.dispatch({
          changes: { from: DOC.indexOf("First"), insert: "Very " },
        });
      }
    });
    let first = true;
    const verify = vi.fn(() => {
      waiting = first;
      first = false;
      return Promise.resolve(OK);
    });
    await expect(
      runDiagramOp(view, ANCHOR, { ...flowKind, verify }, RENAME),
    ).resolves.toEqual({ applied: true });
    expect(verify).toHaveBeenCalledTimes(2);
    expect(bytes(view.state)).toBe(
      DOC.replace("First", "Very First").replace("Next step", "Done"),
    );
    view.destroy();
  });
});
