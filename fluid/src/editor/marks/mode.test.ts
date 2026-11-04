import { StateEffect } from "@codemirror/state";
import { afterEach, describe, expect, it } from "vitest";

import {
  cleanupMarkViews,
  markView,
  setCaret,
  showsMarks,
} from "../../test/markKit";
import { mainSelectionParked } from "../parkedSelection";
import { marksHiddenOnLine } from "./mode";

afterEach(cleanupMarkViews);

describe("marksHiddenOnLine", () => {
  it("is true on the caret line in hidden mode", () => {
    const view = markView("a **b**", { mode: "hidden" });
    setCaret(view, 3);
    expect(showsMarks(view)).toBe(false);
    expect(marksHiddenOnLine(view.state, 1)).toBe(true);
  });

  it("is false on the caret line in show mode", () => {
    const view = markView("a **b**", { mode: "show" });
    setCaret(view, 3);
    expect(showsMarks(view)).toBe(true);
    expect(marksHiddenOnLine(view.state, 1)).toBe(false);
  });

  it("is true in show mode for a line the selection does not touch", () => {
    const view = markView("a **b**\nsecond", { mode: "show" });
    setCaret(view, 3);
    expect(marksHiddenOnLine(view.state, 1)).toBe(false);
    expect(marksHiddenOnLine(view.state, 2)).toBe(true);
  });

  it("follows the revealing ranges, not the selection, while main's selection is parked", () => {
    const view = markView("a **b**", { mode: "show" });
    setCaret(view, 3);
    expect(marksHiddenOnLine(view.state, 1)).toBe(false);
    // A cell is open: main's selection is stale and reveals nothing.
    view.dispatch({
      effects: StateEffect.appendConfig.of(mainSelectionParked.of(true)),
    });
    expect(view.state.selection.main.head).toBe(3);
    expect(marksHiddenOnLine(view.state, 1)).toBe(true);
  });

  it("is false in the source view, which has no hidden-mark layer", () => {
    const view = markView("a **b**", { mode: "hidden", raw: true });
    setCaret(view, 3);
    expect(marksHiddenOnLine(view.state, 1)).toBe(false);
  });
});
