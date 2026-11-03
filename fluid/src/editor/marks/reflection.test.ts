/**
 * A Reflection under the caret and delete rules over hidden marks: a
 * section heading's prefix is never dropped by Backspace, and the hidden id
 * lines keep every guard. Both modes, solo and in a room. (A comment never
 * being revealed in a Reflection is pinned in `hiddenMarks.test.ts`.)
 */

import { EditorSelection, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
} from "../../test/markKit";

afterEach(cleanupMarkViews);

const PAGE = "## Night\n<!-- r:7f3a9c2e -->\n\nText.\n";
const ID = "<!-- r:7f3a9c2e -->";

describe.each([
  { mode: "hidden" as const, room: false },
  { mode: "hidden" as const, room: true },
  { mode: "show" as const, room: false },
  { mode: "show" as const, room: true },
])("$mode mode, room: $room", ({ mode, room }) => {
  const view = () => markView(PAGE, { adapter: "reflection", mode, room });

  it("Backspace at a section heading's start changes nothing and says why", () => {
    const v = view();
    const announced: string[] = [];
    v.dispatch({
      effects: StateEffect.appendConfig.of(
        EditorView.updateListener.of((update) => {
          for (const tr of update.transactions) {
            for (const effect of tr.effects) {
              if (effect.is(EditorView.announce)) {
                announced.push(effect.value);
              }
            }
          }
        }),
      ),
    });
    setCaret(v, 3);
    keys(v, "Backspace");
    expect(bytes(v.state)).toBe(PAGE);
    expect(announced).toEqual(["Use Text style to change a section heading."]);
  });

  it("Backspace on the empty line below the id leaves the id line as it was", () => {
    const v = view();
    setCaret(v, 29);
    keys(v, "Backspace");
    // What the id guard does today: the blank line goes, the id stays whole.
    expect(bytes(v.state)).toBe("## Night\n<!-- r:7f3a9c2e -->\nText.\n");
    expect(v.state.doc.line(2).text).toBe(ID);
  });

  it("a selection from the heading into the text keeps the id line", () => {
    const v = view();
    v.dispatch({
      selection: EditorSelection.range(4, 31),
      userEvent: "select.pointer",
    });
    keys(v, "Backspace");
    expect(bytes(v.state)).toContain(`\n${ID}\n`);
    expect(caret(v)).toBeLessThanOrEqual(v.state.doc.line(1).to);
  });
});
