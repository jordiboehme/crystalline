/**
 * A move whose insert Part 2's insert judge places elsewhere (P8).
 *
 * A landing is always a line end below a whole block, where the judge finds
 * nothing to move, so no real page reaches this path: the judge is stood in
 * for here, and the move must write what it answers and select the picture
 * where that put it.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  markView,
  undoOnce,
} from "../../test/markKit";
import { parsedState } from "../../test/parse";
import { judgeInsert } from "../marks/insertCheck";
import { dropLanding } from "./landing";
import { selectedPicture } from "./refs";
import { movePicture } from "./verbs";

vi.mock("../marks/insertCheck", async (original) => {
  const real = await original<typeof import("../marks/insertCheck")>();
  return { ...real, judgeInsert: vi.fn(real.judgeInsert) };
});

afterEach(() => {
  cleanupMarkViews();
  vi.mocked(judgeInsert).mockClear();
});

describe("a move the insert judge relocates", () => {
  it("writes the judged place and selects the picture there", () => {
    const doc = "![x](assets/p.png)\n\nmid\n\nlater\n";
    const view = markView(doc);
    const state = parsedState(view.state);
    const landing = dropLanding(state, doc.indexOf("mid"));
    if (!landing) throw new Error("no landing");
    // The judge sends the block to the page's end instead.
    vi.mocked(judgeInsert).mockImplementationOnce((begin, _deleted, insert) => {
      const at = begin.doc.length;
      return {
        changes: begin.changes({ from: at, insert: insert.text }),
        from: at,
        end: at + insert.text.length,
      };
    });
    expect(movePicture(view, 0, landing)).toBe(true);
    expect(vi.mocked(judgeInsert)).toHaveBeenCalledTimes(1);
    expect(bytes(view.state)).toBe("mid\n\nlater\n\n\n![x](assets/p.png)\n");
    const selected = selectedPicture(parsedState(view.state));
    expect(selected?.token).toBe("assets/p.png");
    expect(selected?.from).toBe("mid\n\nlater\n\n\n".length);
    undoOnce(view);
    expect(bytes(view.state)).toBe(doc);
  });

  it("asks the judge with the text as the document counts it, in CRLF", () => {
    const doc = "![x](assets/p.png)\r\n\r\nmid\r\n\r\nlater\r\n";
    const view = markView(doc, { lineSeparator: "\r\n" });
    const state = parsedState(view.state);
    const landing = dropLanding(state, state.doc.toString().indexOf("mid"));
    if (!landing) throw new Error("no landing");
    expect(movePicture(view, 0, landing)).toBe(true);
    const asked = vi.mocked(judgeInsert).mock.calls[0]?.[2];
    expect(asked?.text).toBe("\n\n![x](assets/p.png)\n");
    expect(bytes(view.state)).toBe(
      "mid\r\n\r\n![x](assets/p.png)\r\n\r\n\r\nlater\r\n",
    );
  });
});
