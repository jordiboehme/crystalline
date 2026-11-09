/**
 * The apply layer with the real composition module on a surface view (D60, R71).
 * jsdom has no IME, so one is played the way `marks/composition.test.ts` plays
 * it: a stubbed `composing`, composed changes as `input.type.compose`, and
 * `compositionend`, which starts the 60 ms wait (fake timers). An ended
 * composition inside that wait is one still to be judged.
 */

import { EditorSelection } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
} from "../../test/markKit";
import { flowKind, runDiagramOp } from "./apply";
import type { FlowOp } from "./flowchart/ops";
import type { Verdict } from "./verdict";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  cleanupMarkViews();
  vi.useRealTimers();
});

const OK: Verdict = { ok: true, kind: "flowchart", domIds: new Map() };
const KIND = { ...flowKind, verify: () => Promise.resolve(OK) };
const RENAME: FlowOp = { op: "rename", id: "B", text: "Done" };

const fence = (name: string) =>
  `\`\`\`mermaid\nflowchart TD\n    A[${name}] --> B[Next]\n\`\`\``;
const ONE = fence("One");
const TWO = fence("Two");
const renamed = (text: string) => text.replace("B[Next]", "B[Done]");

function ime(view: EditorView) {
  const state = { on: false };
  for (const name of ["composing", "compositionStarted"]) {
    Object.defineProperty(view, name, {
      configurable: true,
      get: () => state.on,
    });
  }
  let first = true;
  return {
    start() {
      first = true;
      state.on = true;
    },
    change(from: number, to: number, insert: string) {
      view.dispatch({
        changes: { from, to, insert },
        selection: EditorSelection.cursor(from + insert.length),
        userEvent: first ? "input.type.compose.start" : "input.type.compose",
      });
      first = false;
    },
    /** Ended, and with `settle` false still waiting to be judged. */
    end(settle: boolean) {
      state.on = false;
      view.contentDOM.dispatchEvent(
        new Event("compositionend", { bubbles: true }),
      );
      if (settle) vi.advanceTimersByTime(60);
    },
  };
}

/** Bold pressed at the end of `ab`, `cd` composed: `abcd`, waiting to be `ab**cd**`. */
function composeBold(view: EditorView) {
  setCaret(view, 2);
  keys(view, "Mod-b");
  const keyboard = ime(view);
  keyboard.start();
  keyboard.change(2, 2, "cd");
  return keyboard;
}

const CASES = (["hidden", "show"] as const).flatMap((mode) =>
  [false, true].map((room) => ({ mode, room })),
);

describe.each(CASES)("$mode mode, room $room", ({ mode, room }) => {
  test("a correction that adds text above: the op lands on the fence asked for", async () => {
    const view = markView(`ab\n\n${ONE}\n${TWO}\n`, { mode, room });
    composeBold(view).end(false);
    const anchor = bytes(view.state).lastIndexOf("```mermaid");

    await expect(runDiagramOp(view, anchor, KIND, RENAME)).resolves.toEqual({
      applied: true,
    });
    expect(bytes(view.state)).toBe(`ab**cd**\n\n${ONE}\n${renamed(TWO)}\n`);
  });

  test("with one fence, a correction above is no false gone", async () => {
    const view = markView(`ab\n\n${ONE}\n`, { mode, room });
    composeBold(view).end(false);
    const anchor = bytes(view.state).indexOf("```");

    await expect(runDiagramOp(view, anchor + 1, KIND, RENAME)).resolves.toEqual(
      { applied: true },
    );
    expect(bytes(view.state)).toBe(`ab**cd**\n\n${renamed(ONE)}\n`);
  });

  test("a correction that takes text out: the op stays off the next fence", async () => {
    const view = markView(`**a** **b**\n\n${ONE}\n${TWO}\n`, { mode, room });
    const keyboard = ime(view);
    keyboard.start();
    keyboard.change(5, 6, "");
    keyboard.end(false);
    // The closing marks of fence One, as the text stands: four positions on
    // from here is fence Two, and the correction takes out four.
    const anchor = bytes(view.state).indexOf(TWO) - 2;

    await expect(runDiagramOp(view, anchor, KIND, RENAME)).resolves.toEqual({
      applied: true,
    });
    expect(bytes(view.state)).toBe(`**ab**\n\n${renamed(ONE)}\n${TWO}\n`);
  });

  test("a running composition refuses stale, and the person's Bold survives", async () => {
    const view = markView(`ab\n\n${ONE}\n`, { mode, room });
    const keyboard = composeBold(view);
    const anchor = bytes(view.state).indexOf("```");
    const verify = vi.fn(() => Promise.resolve(OK));

    await expect(
      runDiagramOp(view, anchor, { ...flowKind, verify }, RENAME),
    ).resolves.toEqual({ applied: false, refused: "stale" });
    expect(verify).not.toHaveBeenCalled();

    keyboard.end(true);
    expect(bytes(view.state)).toBe(`ab**cd**\n\n${ONE}\n`);
  });

  test("a composition that starts during verify refuses stale, and the Bold survives", async () => {
    const view = markView(`ab\n\n${ONE}\n`, { mode, room });
    let keyboard: ReturnType<typeof ime> | null = null;
    const kind = {
      ...flowKind,
      verify: () => {
        keyboard = composeBold(view);
        return Promise.resolve(OK);
      },
    };

    await expect(
      runDiagramOp(view, ONE.indexOf("flowchart") + 4, kind, RENAME),
    ).resolves.toEqual({ applied: false, refused: "stale" });

    (keyboard as ReturnType<typeof ime> | null)?.end(true);
    expect(bytes(view.state)).toBe(`ab**cd**\n\n${ONE}\n`);
  });
});
