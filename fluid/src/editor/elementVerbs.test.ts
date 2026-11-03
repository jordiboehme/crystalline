/**
 * The element verbs: what a popover commit writes. Each one returns the
 * changes relative to the document and never dispatches, so every claim here
 * is on the changed spans and the bytes they give.
 */

import type { ChangeSpec, EditorState } from "@codemirror/state";
import { ChangeSet } from "@codemirror/state";
import { afterEach, describe, expect, it } from "vitest";

import { bytes, cleanupMarkViews, markView } from "../test/markKit";
import {
  linkAddress,
  linkAt,
  linkChanges,
  newLinkChanges,
  unlinkChanges,
  unwikilinkChanges,
  wikilinkAt,
  wikilinkChanges,
  writtenAddress,
} from "./elementVerbs";

afterEach(cleanupMarkViews);

function stateOf(
  text: string,
  adapter: "engram" | "reflection" = "engram",
): EditorState {
  return markView(text, { adapter }).state;
}

function apply(state: EditorState, changes: ChangeSpec[] | null): string {
  if (changes === null) {
    throw new Error("the verb refused");
  }
  return bytes(state.update({ changes }).state);
}

/** The spans a change set replaces, in the start document's positions. */
function spans(
  state: EditorState,
  changes: ChangeSpec[] | null,
): [number, number][] {
  if (changes === null) {
    throw new Error("the verb refused");
  }
  const set = ChangeSet.of(changes, state.doc.length);
  const out: [number, number][] = [];
  set.iterChangedRanges((fromA, toA) => {
    out.push([fromA, toA]);
  });
  return out;
}

describe("linkChanges", () => {
  it("replaces only the address span", () => {
    const state = stateOf("[t](https://a.b)");
    const changes = linkChanges(state, 1, { address: "https://c.d" });
    expect(spans(state, changes)).toEqual([[4, 15]]);
    expect(apply(state, changes)).toBe("[t](https://c.d)");
  });

  it("keeps the title", () => {
    const state = stateOf('[t](https://a.b "T")');
    expect(
      apply(state, linkChanges(state, 1, { address: "https://c.d" })),
    ).toBe('[t](https://c.d "T")');
  });

  it("fills an empty address", () => {
    const state = stateOf("a [t]() b");
    expect(apply(state, linkChanges(state, 3, { address: "https://x" }))).toBe(
      "a [t](https://x) b",
    );
  });

  it("writes an address with a space inside angle brackets", () => {
    const state = stateOf("[t](u)");
    expect(apply(state, linkChanges(state, 1, { address: "my file.md" }))).toBe(
      "[t](<my file.md>)",
    );
  });

  it("writes nothing when the address did not change", () => {
    const state = stateOf("[t](u)");
    expect(linkChanges(state, 1, { address: "u" })).toEqual([]);
  });

  it("refuses a reference link, whose definition is shared", () => {
    const state = stateOf("[t][r]\n\n[r]: u");
    expect(linkChanges(state, 1, { address: "v" })).toBeNull();
  });

  it("refuses a stale request whose link is gone", () => {
    const state = stateOf("a t b");
    expect(linkChanges(state, 2, { address: "v" })).toBeNull();
  });

  it("edits a crystalline address in a Reflection like any other", () => {
    const text = "See [the note](crystalline://eng/a) here.\n";
    const state = stateOf(text, "reflection");
    const at = text.indexOf("the note");
    const changes = linkChanges(state, at, {
      address: "crystalline://eng/b",
    });
    const start = text.indexOf("crystalline://");
    expect(spans(state, changes)).toEqual([
      [start, start + "crystalline://eng/a".length],
    ]);
    expect(apply(state, changes)).toBe(
      "See [the note](crystalline://eng/b) here.\n",
    );
  });
});

describe("newLinkChanges", () => {
  it("wraps a selection", () => {
    const state = stateOf("go there");
    expect(
      apply(
        state,
        newLinkChanges(
          state,
          { from: 3, to: 8 },
          { text: "", address: "https://x" },
        ),
      ),
    ).toBe("go [there](https://x)");
  });

  it("uses the address as the text at a caret with no text", () => {
    const state = stateOf("");
    expect(
      apply(
        state,
        newLinkChanges(
          state,
          { from: 0, to: 0 },
          { text: "", address: "https://x" },
        ),
      ),
    ).toBe("[https://x](https://x)");
  });

  it("writes the typed text at a caret", () => {
    const state = stateOf("a  b");
    expect(
      apply(
        state,
        newLinkChanges(state, { from: 2, to: 2 }, { text: "x", address: "u" }),
      ),
    ).toBe("a [x](u) b");
  });

  it("escapes brackets in the typed text", () => {
    const state = stateOf("");
    expect(
      apply(
        state,
        newLinkChanges(
          state,
          { from: 0, to: 0 },
          { text: "a]b", address: "u" },
        ),
      ),
    ).toBe("[a\\]b](u)");
  });

  it("refuses an empty address", () => {
    const state = stateOf("go there");
    expect(
      newLinkChanges(state, { from: 3, to: 8 }, { text: "", address: " " }),
    ).toBeNull();
  });

  it("refuses a selection across lines", () => {
    const state = stateOf("a\nb");
    expect(
      newLinkChanges(state, { from: 0, to: 3 }, { text: "", address: "u" }),
    ).toBeNull();
  });

  it("refuses inside code", () => {
    const state = stateOf("a `code` b");
    expect(
      newLinkChanges(state, { from: 3, to: 7 }, { text: "", address: "u" }),
    ).toBeNull();
  });

  it("refuses inside the frontmatter", () => {
    const state = stateOf("---\ntitle: x\n---\nbody");
    expect(
      newLinkChanges(state, { from: 4, to: 9 }, { text: "", address: "u" }),
    ).toBeNull();
  });

  it("refuses on a Reflection id line and inside a comment", () => {
    const text = "## Night\n<!-- r:7f3a9c2e -->\n\nbody <!-- c --> x\n";
    const state = stateOf(text, "reflection");
    const idLine = text.indexOf("<!-- r:");
    expect(
      newLinkChanges(
        state,
        { from: idLine + 5, to: idLine + 7 },
        { text: "", address: "u" },
      ),
    ).toBeNull();
    const comment = text.indexOf("<!-- c");
    expect(
      newLinkChanges(
        state,
        { from: comment + 5, to: comment + 6 },
        { text: "", address: "u" },
      ),
    ).toBeNull();
  });
});

describe("unlinkChanges", () => {
  it("deletes the opening and the closing link marks only", () => {
    const state = stateOf("a [t](u) b");
    const changes = unlinkChanges(state, 3);
    expect(spans(state, changes)).toEqual([
      [2, 3],
      [4, 8],
    ]);
    expect(apply(state, changes)).toBe("a t b");
  });

  it("keeps the definition of a reference link", () => {
    const state = stateOf("[t][r]\n\n[r]: u");
    expect(apply(state, unlinkChanges(state, 1))).toBe("t\n\n[r]: u");
  });

  it("refuses where there is no link", () => {
    expect(unlinkChanges(stateOf("a t b"), 2)).toBeNull();
  });
});

describe("wikilinkChanges", () => {
  it("replaces the target", () => {
    const state = stateOf("[[Old]]");
    const changes = wikilinkChanges(state, 3, { target: "New" });
    expect(spans(state, changes)).toEqual([[2, 5]]);
    expect(apply(state, changes)).toBe("[[New]]");
  });

  it("keeps an alias", () => {
    const state = stateOf("[[Old|words]]");
    expect(apply(state, wikilinkChanges(state, 3, { target: "New" }))).toBe(
      "[[New|words]]",
    );
  });

  it("refuses an empty target or one with brackets or a bar", () => {
    const state = stateOf("[[Old]]");
    expect(wikilinkChanges(state, 3, { target: " " })).toBeNull();
    expect(wikilinkChanges(state, 3, { target: "a]]b" })).toBeNull();
    expect(wikilinkChanges(state, 3, { target: "a|b" })).toBeNull();
  });

  it("refuses inside code", () => {
    const state = stateOf("`[[Old]]`");
    expect(wikilinkAt(state, 3)).toBeNull();
    expect(wikilinkChanges(state, 3, { target: "New" })).toBeNull();
  });
});

describe("unwikilinkChanges", () => {
  it("keeps the alias words", () => {
    const state = stateOf("[[Old|words]]");
    expect(apply(state, unwikilinkChanges(state, 3))).toBe("words");
  });

  it("keeps the target without an alias", () => {
    const state = stateOf("a [[Old]] b");
    expect(apply(state, unwikilinkChanges(state, 4))).toBe("a Old b");
  });
});

describe("the readers", () => {
  it("reads an inline link", () => {
    const state = stateOf("a [t](u) b");
    expect(linkAt(state, 3)).toMatchObject({
      from: 2,
      to: 8,
      text: { from: 3, to: 4 },
      address: { from: 6, to: 7 },
      reference: false,
    });
    expect(linkAddress(state, 3)).toBe("u");
  });

  it("reads a reference link and its definition's address", () => {
    const state = stateOf("[t][r]\n\n[r]: https://x");
    expect(linkAt(state, 1)).toMatchObject({
      text: { from: 1, to: 2 },
      address: null,
      reference: true,
    });
    expect(linkAddress(state, 1)).toBe("https://x");
  });

  it("reads no footnote, image or wikilink as a link", () => {
    expect(linkAt(stateOf("a[^1]\n\n[^1]: n"), 2)).toBeNull();
    expect(linkAt(stateOf("![t](u)"), 2)).toBeNull();
    expect(linkAt(stateOf("[[Old]]"), 3)).toBeNull();
  });

  it("reads an address written in angle brackets without them", () => {
    const state = stateOf("[t](<my file.md>)");
    expect(linkAddress(state, 1)).toBe("my file.md");
  });

  it("writes an address in angle brackets only when it needs them", () => {
    expect(writtenAddress("https://x")).toBe("https://x");
    expect(writtenAddress("a b")).toBe("<a b>");
    expect(writtenAddress("a)b")).toBe("<a)b>");
    expect(writtenAddress("a>b c")).toBeNull();
  });
});

describe("a new link and the marks around it", () => {
  it("refuses a selection that holds one mark of a pair", () => {
    const state = stateOf("a **bold** c");
    expect(
      newLinkChanges(state, { from: 0, to: 6 }, { text: "", address: "u" }),
    ).toBeNull();
  });

  it("wraps a whole bold word, marks and all", () => {
    const state = stateOf("a **bold** c");
    expect(
      apply(
        state,
        newLinkChanges(state, { from: 2, to: 10 }, { text: "", address: "u" }),
      ),
    ).toBe("a [**bold**](u) c");
  });

  it("refuses inside an existing link", () => {
    const state = stateOf("a [t](u) c");
    expect(
      newLinkChanges(state, { from: 3, to: 4 }, { text: "", address: "v" }),
    ).toBeNull();
  });
});
