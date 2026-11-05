/**
 * Moving a picture by dragging it in a room (Review Focus 3): two peers,
 * each a surface view with the picture layer (`markPeers`), whose writes
 * reach each other only on `sync()`. Peer A drags; peer B types or deletes
 * meanwhile.
 */

import type { TransactionSpec } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { FakeTransfer, fireDrag } from "../../test/dragKit";
import {
  bytes,
  cleanupMarkViews,
  markPeers,
  undoOnce,
} from "../../test/markKit";
import { imagePreviews } from "../imagePreviews";
import { PICTURE_CHANGED } from "./dragMove";
import { prefetchPictureTools } from "./loader";

beforeAll(async () => {
  await prefetchPictureTools();
});

afterEach(() => {
  cleanupMarkViews();
  vi.restoreAllMocks();
});

const AT = { x: 40, y: 80 };
const P = "![x](assets/p.png#right)";
const PAGE = `Intro ${P} words.\n\nSecond.\n\nThird.\n\nEnd.\n`;

/** Peer B types `text` at `at`, as a person does. */
function typeAt(view: EditorView, at: number, text: string): void {
  view.dispatch({
    changes: { from: at, insert: text },
    selection: { anchor: at + text.length },
    userEvent: "input.type",
  });
}

/** The announcements `view` makes from now on. */
function heard(view: EditorView): string[] {
  const said: string[] = [];
  const own = view.dispatch.bind(view);
  view.dispatch = ((...specs: TransactionSpec[]) => {
    const tr = view.state.update(...specs);
    for (const effect of tr.effects) {
      if (effect.is(EditorView.announce)) said.push(effect.value);
    }
    own(tr);
  }) as EditorView["dispatch"];
  return said;
}

/** How often `needle` stands in `text`. */
function count(text: string, needle: string): number {
  return text.split(needle).length - 1;
}

describe.each(["hidden", "show"] as const)(
  "a picture dragged in a room (%s mode)",
  (mode) => {
    function peers() {
      return markPeers(PAGE, { mode, images: imagePreviews("eng") });
    }

    /** A's drag of its picture, dropped with the pointer at `pos`. */
    function startDrag(view: EditorView): {
      transfer: FakeTransfer;
      box: Element;
    } {
      const box = view.contentDOM.querySelector(".cm-picture");
      if (!box) throw new Error("no drawn picture");
      const transfer = new FakeTransfer();
      fireDrag(box, "dragstart", transfer);
      return { transfer, box };
    }

    function finishDrag(
      view: EditorView,
      drag: { transfer: FakeTransfer; box: Element },
      pos: number,
    ): MouseEvent {
      vi.spyOn(view, "posAtCoords").mockReturnValue(pos);
      fireDrag(view.contentDOM, "dragover", drag.transfer, AT);
      const drop = fireDrag(view.contentDOM, "drop", drag.transfer, AT);
      fireDrag(drag.box, "dragend", drag.transfer);
      return drop;
    }

    it("keeps what the other peer typed in both paragraphs", () => {
      const { local, peer, sync } = peers();
      const drag = startDrag(local);
      // B types where the picture leaves and where it lands, unseen by A.
      typeAt(peer, PAGE.indexOf(" words"), "B1");
      typeAt(peer, bytes(peer.state).indexOf("Third.") + "Third".length, "B2");
      finishDrag(local, drag, PAGE.indexOf("Third"));
      expect(bytes(local.state)).toBe(
        `Intro  words.\n\nSecond.\n\nThird.\n\n${P}\n\n\nEnd.\n`,
      );
      sync();
      const text = bytes(local.state);
      expect(bytes(peer.state)).toBe(text);
      expect(text).toContain("B1");
      expect(text).toContain("ThirdB2.");
      expect(count(text, P)).toBe(1);
      expect(text).toBe(
        `Intro B1 words.\n\nSecond.\n\nThirdB2.\n\n${P}\n\n\nEnd.\n`,
      );
    });

    it("undoes only the move with A's undo", () => {
      const { local, peer, sync } = peers();
      const drag = startDrag(local);
      typeAt(peer, PAGE.indexOf(" words"), "B1");
      typeAt(peer, bytes(peer.state).indexOf("Third.") + "Third".length, "B2");
      finishDrag(local, drag, PAGE.indexOf("Third"));
      sync();
      undoOnce(local);
      sync();
      const expected = `Intro ${P}B1 words.\n\nSecond.\n\nThirdB2.\n\nEnd.\n`;
      expect(bytes(local.state)).toBe(expected);
      expect(bytes(peer.state)).toBe(expected);
    });

    it("still moves the picture when the other peer typed above it meanwhile (P35)", () => {
      const { local, peer, sync } = peers();
      const drag = startDrag(local);
      typeAt(peer, 0, "B0 ");
      sync();
      const said = heard(local);
      finishDrag(local, drag, bytes(local.state).indexOf("Third"));
      const expected = `B0 Intro  words.\n\nSecond.\n\nThird.\n\n${P}\n\n\nEnd.\n`;
      expect(bytes(local.state)).toBe(expected);
      expect(said).toEqual([]);
      sync();
      expect(bytes(peer.state)).toBe(expected);
    });

    it("refuses the move and says why when the other peer changed the picture (P35)", () => {
      const { local, peer, sync } = peers();
      const drag = startDrag(local);
      // B renames the picture: its alt "x" becomes "xy".
      typeAt(peer, PAGE.indexOf(P) + 3, "y");
      sync();
      const before = bytes(local.state);
      const said = heard(local);
      const drop = finishDrag(local, drag, before.indexOf("Third"));
      expect(drop.defaultPrevented).toBe(true);
      expect(bytes(local.state)).toBe(before);
      expect(said).toEqual([PICTURE_CHANGED]);
    });

    it("writes nothing when the other peer deleted the picture meanwhile", () => {
      const { local, peer, sync } = peers();
      const drag = startDrag(local);
      const from = PAGE.indexOf(P);
      peer.dispatch({
        changes: { from, to: from + P.length },
        userEvent: "delete.selection",
      });
      sync();
      const before = bytes(local.state);
      expect(before).toBe("Intro  words.\n\nSecond.\n\nThird.\n\nEnd.\n");
      const drop = finishDrag(local, drag, PAGE.indexOf("Third") - P.length);
      expect(drop.defaultPrevented).toBe(true);
      expect(bytes(local.state)).toBe(before);
      sync();
      expect(bytes(peer.state)).toBe(before);
    });
  },
);
