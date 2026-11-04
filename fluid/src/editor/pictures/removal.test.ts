/**
 * What a picture's delete or move takes away: the line it stands alone on,
 * with one break and one surplus blank line, or the bare span. Never an id
 * line, never a break that holds one.
 */

import type { Extension } from "@codemirror/state";
import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import { bytes } from "../../test/cellKit";
import { parsedState } from "../../test/parse";
import { hiddenIdLines } from "../hiddenIdLines";
import { baseExtensions, lineSeparatorFor } from "../setup";
import { picturesIn } from "./refs";
import { pictureRemoval } from "./removal";

function stateOf(doc: string, extra: Extension[] = []): EditorState {
  return parsedState(
    EditorState.create({
      doc,
      extensions: [
        ...lineSeparatorFor(doc),
        ...baseExtensions(false),
        ...extra,
      ],
    }),
  );
}

/** The document's bytes once its one picture is removed. */
function removed(doc: string, extra: Extension[] = []): string {
  const state = stateOf(doc, extra);
  const [ref] = picturesIn(state);
  if (ref === undefined) throw new Error("no picture");
  const range = pictureRemoval(state, ref);
  return bytes(state.update({ changes: { ...range, insert: "" } }).state);
}

describe("pictureRemoval", () => {
  it("takes a lone picture's line and one of the blank lines around it", () => {
    expect(removed("a\n\n![x](assets/p.png)\n\nb")).toBe("a\n\nb");
  });

  it("takes the line and its break when only one side is blank", () => {
    expect(removed("a\n![x](assets/p.png)\n\nb")).toBe("a\n\nb");
    expect(removed("a\n\n![x](assets/p.png)\nb")).toBe("a\n\nb");
  });

  it("takes only the span of a picture inside prose", () => {
    expect(removed("a ![x](assets/p.png) b")).toBe("a  b");
  });

  it("takes the break before a last line and keeps the blank line above", () => {
    expect(removed("a\n\n![x](assets/p.png)")).toBe("a\n");
  });

  it("takes the whole text of a page that is only the picture", () => {
    expect(removed("![x](assets/p.png)")).toBe("");
    expect(removed("  ![x](assets/p.png)  \n\nb")).toBe("\nb");
  });

  it("leaves a Reflection id line byte for byte", () => {
    const id = "<!-- r:7f3a9c2e -->";
    expect(
      removed(`## H\n${id}\n![x](assets/p.png)\n\nb`, [hiddenIdLines()]),
    ).toBe(`## H\n${id}\n\nb`);
    // On the last line the break before is the id line's own: the bare
    // line goes and that break stays.
    expect(removed(`## H\n${id}\n![x](assets/p.png)`, [hiddenIdLines()])).toBe(
      `## H\n${id}\n`,
    );
  });

  it("keeps the CRLF endings", () => {
    expect(removed("a\r\n\r\n![x](assets/p.png)\r\n\r\nb")).toBe("a\r\n\r\nb");
    expect(removed("a\r\n\r\n![x](assets/p.png)")).toBe("a\r\n");
    expect(
      removed("## H\r\n<!-- r:7f3a9c2e -->\r\n![x](assets/p.png)\r\n\r\nb", [
        hiddenIdLines(),
      ]),
    ).toBe("## H\r\n<!-- r:7f3a9c2e -->\r\n\r\nb");
  });
});
