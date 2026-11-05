/**
 * Review Focus 4: a picture right under a Reflection section's hidden id line
 * goes from the keyboard and leaves the id line byte for byte where it was,
 * directly under its heading, still hidden.
 */

import { EditorSelection } from "@codemirror/state";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { hidesIdLines } from "../hiddenIdLines";
import { imagePreviews } from "../imagePreviews";
import {
  bytes,
  cleanupMarkViews,
  keys,
  markPeers,
  markView,
  setCaret,
} from "../../test/markKit";
import { prefetchPictureTools } from "./loader";

beforeAll(async () => {
  await prefetchPictureTools();
});

afterEach(cleanupMarkViews);

const ID = "<!-- r:abcd -->";
const PICTURE = "![x](assets/p.png)";

const CASES = [
  {
    name: "with text below",
    doc: `## H\n${ID}\n${PICTURE}\n\nText`,
    after: `## H\n${ID}\n\nText`,
  },
  {
    name: "where the page ends",
    doc: `## H\n${ID}\n${PICTURE}`,
    after: `## H\n${ID}\n`,
  },
] as const;

describe.each(["hidden", "show"] as const)(
  "a picture under an id line (%s mode)",
  (mode) => {
    it.each(CASES)("goes and leaves the id line, $name", ({ doc, after }) => {
      const view = markView(doc, {
        adapter: "reflection",
        mode,
        images: imagePreviews("eng"),
      });
      expect(hidesIdLines(view.state)).toBe(true);
      const from = doc.indexOf(PICTURE);
      setCaret(view, from + PICTURE.length);
      keys(view, "Backspace");
      expect(view.state.selection.main).toMatchObject({
        from,
        to: from + PICTURE.length,
      });
      keys(view, "Backspace");
      expect(bytes(view.state)).toBe(after);
      expect(view.state.doc.line(2).text).toBe(ID);
      expect(view.state.doc.line(1).text).toBe("## H");
      expect(hidesIdLines(view.state)).toBe(true);
    });

    it.each(CASES)("goes with Delete in a room, $name", ({ doc, after }) => {
      const { local, peer, sync } = markPeers(doc, {
        adapter: "reflection",
        mode,
        images: imagePreviews("eng"),
      });
      const from = doc.indexOf(PICTURE);
      local.dispatch({
        selection: EditorSelection.single(from, from + PICTURE.length),
      });
      keys(local, "Delete");
      expect(bytes(local.state)).toBe(after);
      sync();
      expect(bytes(peer.state)).toBe(after);
      expect(hidesIdLines(local.state)).toBe(true);
    });
  },
);
