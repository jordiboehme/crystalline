/**
 * The element verbs: what a popover commit writes. Each one returns the
 * changes relative to the document and never dispatches, so every claim here
 * is on the changed spans and the bytes they give.
 */

import { syntaxTree } from "@codemirror/language";
import type { ChangeSpec, EditorState } from "@codemirror/state";
import { ChangeSet } from "@codemirror/state";
import { afterEach, describe, expect, it, vi } from "vitest";

import { bytes, cleanupMarkViews, markView } from "../test/markKit";
import { calloutFoldChanges, calloutKindChanges } from "./elementContext";
import {
  formulaRefusal,
  linkAddress,
  linkAt,
  mathAt,
  mathChanges,
  removeMathChanges,
  statusAt,
  statusChanges,
  statusWords,
  taskAt,
  linkChanges,
  linkWords,
  newLinkChanges,
  referenceDecoder,
  unlinkChanges,
  unwikilinkChanges,
  wikilinkAt,
  wikilinkChanges,
  writtenAddress,
  writtenLabel,
} from "./elementVerbs";
import { parsedState } from "../test/parse";

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
    expect(linkAt(parsedState(state), 3)).toMatchObject({
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
    expect(linkAt(parsedState(state), 1)).toMatchObject({
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
    // The parser reads no escaped angle bracket inside angle brackets.
    expect(writtenAddress("a>b c")).toBeNull();
    expect(writtenAddress("a)b")).toBe("a\\)b");
    expect(writtenAddress("a\nb")).toBeNull();
  });
});

describe("what is written reads back as typed", () => {
  const ADDRESSES = [
    "https://x/a\\_b",
    "C:\\dir\\file",
    "my file.md",
    "a(b)",
    "a)b",
    "a<b>c",
    "<a>",
    "a (b) c",
    "x&amp;y",
    "^1",
    "[x]",
  ];

  it.each(ADDRESSES)("an address %j", (address) => {
    const state = stateOf("[t](u)");
    const next = state.update({
      changes: linkChanges(state, 1, { address }) ?? [],
    }).state;
    expect(linkAddress(next, 1)).toBe(address);
  });

  const TEXTS = [
    "a\\b",
    "^1",
    "a[b]c",
    "a(b) c",
    "<x>",
    "*em* and _em_",
    "`code`",
    "~~s~~",
    "x&amp;y",
    "[[Old]]",
  ];

  it.each(TEXTS)("a new link's text %j", (text) => {
    const state = stateOf("");
    const next = state.update({
      changes:
        newLinkChanges(state, { from: 0, to: 0 }, { text, address: "u" }) ?? [],
    }).state;
    expect(linkAt(parsedState(next), 1)).not.toBeNull();
    expect(linkWords(next, 1)).toBe(text);
    expect(linkAddress(next, 1)).toBe("u");
  });

  it("a new link's text that is its address", () => {
    const state = stateOf("");
    const next = state.update({
      changes:
        newLinkChanges(
          state,
          { from: 0, to: 0 },
          { text: "", address: "https://x/a_b" },
        ) ?? [],
    }).state;
    expect(linkWords(next, 1)).toBe("https://x/a_b");
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

describe("an address with a character reference", () => {
  it("reads as the renderer decodes it", () => {
    const state = stateOf("[t](a?x=1&amp;y=2)");
    expect(linkAddress(state, 1)).toBe("a?x=1&y=2");
    expect(linkAddress(stateOf("[t](a&#38;b&#x26;c)"), 1)).toBe("a&b&c");
    // No reference without its semicolon, and no unknown name.
    expect(linkAddress(stateOf("[t](a&amp-b&nosuch;c)"), 1)).toBe(
      "a&amp-b&nosuch;c",
    );
  });

  it("decodes in an inert document, never in the live page", () => {
    // The textarea lives in a document of its own, never the live page.
    expect(referenceDecoder().ownerDocument).not.toBe(document);
    expect(referenceDecoder().isConnected).toBe(false);
    const create = vi.spyOn(document, "createElement");
    const state = stateOf(
      "[t](&lt;img&#32;src=x&#32;onerror=alert(1)&gt;&lt;script&gt;x&lt;/script&gt;)",
    );
    create.mockClear();
    expect(linkAddress(state, 1)).toBe(
      "<img src=x onerror=alert(1)><script>x</script>",
    );
    expect(create).not.toHaveBeenCalled();
    // The editor's own widget buffers are images; none of the address's.
    expect(
      document.querySelector("img[onerror], img[src='x'], script"),
    ).toBeNull();
  });

  it("keeps an escaped ampersand as written", () => {
    expect(linkAddress(stateOf("[t](a\\&amp;b)"), 1)).toBe("a&amp;b");
  });

  it.each(["a?x=1&y=3", "x&amp;y", "a&#38;b", "a\\&amp;b", "&lt;"])(
    "writes %j so it reads back the same",
    (address) => {
      const state = stateOf("[t](a?x=1&amp;y=2)");
      const next = state.update({
        changes: linkChanges(state, 1, { address }) ?? [],
      }).state;
      expect(linkAddress(next, 1)).toBe(address);
    },
  );
});

describe("two elements side by side", () => {
  it("reads the wikilink that starts at a shared position", () => {
    const state = stateOf("[[A]][[B]]");
    expect(wikilinkAt(state, 5)).toMatchObject({ from: 5, to: 10 });
    expect(apply(state, wikilinkChanges(state, 5, { target: "C" }))).toBe(
      "[[A]][[C]]",
    );
  });

  it("reads the link that starts at a shared position", () => {
    const state = stateOf("[a](b)[t](u)");
    expect(linkAt(parsedState(state), 6)).toMatchObject({ from: 6, to: 12 });
  });
});

describe("statusChanges", () => {
  it("replaces only the words when only they changed", () => {
    const state = stateOf(":status[Done]{green}");
    const changes = statusChanges(state, 0, { words: "Shipped" });
    expect(spans(state, changes)).toEqual([[8, 12]]);
    expect(apply(state, changes)).toBe(":status[Shipped]{green}");
  });

  it("replaces only the colour word when only it changed", () => {
    const state = stateOf(":status[Done]{green}");
    const changes = statusChanges(state, 0, { colour: "blue" });
    expect(spans(state, changes)).toEqual([[14, 19]]);
    expect(apply(state, changes)).toBe(":status[Done]{blue}");
  });

  it("writes both in one change list", () => {
    const state = stateOf(":status[Done]{green}");
    expect(
      apply(
        state,
        statusChanges(state, 0, { words: "Shipped", colour: "blue" }),
      ),
    ).toBe(":status[Shipped]{blue}");
  });

  it("adds braces to a label that has none", () => {
    const state = stateOf("a :status[Done] b");
    expect(apply(state, statusChanges(state, 4, { colour: "red" }))).toBe(
      "a :status[Done]{red} b",
    );
  });

  it("writes nothing for a field that says what the document says", () => {
    const state = stateOf(":status[Done]{green}");
    expect(statusChanges(state, 0, { words: "Done", colour: "green" })).toEqual(
      [],
    );
  });

  it.each(["", "   "])("refuses the words %j", (words) => {
    const state = stateOf(":status[Done]{green}");
    expect(statusChanges(state, 0, { words })).toBeNull();
  });

  it.each([
    "a]b",
    "a[b",
    "a\\",
    "a\\]",
    "\\*",
    "a\\[b",
    "see [t](u)",
    "[x",
    "x]]",
    "]",
    "a\\\\b",
  ])("writes the words %j so they read back the same", (words) => {
    const state = stateOf("a :status[Done]{green} tail");
    const next = state.update({
      changes: statusChanges(state, 2, { words }) ?? [],
    }).state;
    expect(statusWords(next, 2)).toBe(words);
    const label = statusAt(parsedState(next), 2);
    expect(label?.from).toBe(2);
    expect(bytes(next).slice(label?.to ?? 0)).toBe(" tail");
    expect(label?.colour).toBe("green");
  });

  it("keeps a source escape it does not own as written", () => {
    const state = stateOf(":status[a \\* b]");
    expect(statusWords(state, 0)).toBe("a \\* b");
  });

  it("reads no label inside code, the frontmatter or an id line", () => {
    expect(statusAt(stateOf("`:status[a]`"), 2)).toBeNull();
    expect(statusAt(stateOf("---\nt: :status[a]\n---\nb"), 8)).toBeNull();
  });
});

describe("mathChanges", () => {
  it("replaces only the formula of inline math", () => {
    const state = stateOf("$`x^2`$");
    const changes = mathChanges(state, 0, { formula: "y" });
    expect(spans(state, changes)).toEqual([[2, 5]]);
    expect(apply(state, changes)).toBe("$`y`$");
  });

  it("replaces only the formula of double-dollar math on one line", () => {
    const state = stateOf("a $$x$$ b");
    expect(apply(state, mathChanges(state, 3, { formula: "y + 1" }))).toBe(
      "a $$y + 1$$ b",
    );
  });

  it("replaces the inner lines of a dollar block", () => {
    const state = stateOf("$$\na\n$$");
    expect(mathAt(parsedState(state), 0)).toMatchObject({
      block: true,
      formula: "a",
    });
    expect(apply(state, mathChanges(state, 0, { formula: "b" }))).toBe(
      "$$\nb\n$$",
    );
  });

  it("keeps a math fence's lines byte for byte", () => {
    const state = stateOf("```math  \na\n```  \nafter");
    expect(apply(state, mathChanges(state, 0, { formula: "b\nc" }))).toBe(
      "```math  \nb\nc\n```  \nafter",
    );
  });

  it("gives every new line of a block in a quote its quote marks", () => {
    const state = stateOf("> $$\n> a\n> $$");
    expect(apply(state, mathChanges(state, 2, { formula: "b\nc" }))).toBe(
      "> $$\n> b\n> c\n> $$",
    );
  });

  it("fills a block with no inner lines", () => {
    const state = stateOf("```math\n```");
    expect(apply(state, mathChanges(state, 0, { formula: "x" }))).toBe(
      "```math\nx\n```",
    );
  });

  it("writes nothing for an unchanged formula and refuses an empty one", () => {
    const state = stateOf("$`x`$");
    expect(mathChanges(state, 0, { formula: "x" })).toEqual([]);
    expect(mathChanges(state, 0, { formula: " " })).toBeNull();
  });

  it.each([
    ["$$\nx\n$$", "a\n$$\nb", "dollar-line"],
    ["```math\nx\n```", "a\n```\nb", "fence-line"],
  ])(
    "refuses a formula that holds its own delimiter (%j, %j)",
    (text, formula, why) => {
      const state = stateOf(text);
      const math = mathAt(parsedState(state), 0);
      expect(math).not.toBeNull();
      expect(formulaRefusal(math!.form, formula)).toBe(why);
      expect(mathChanges(state, 0, { formula })).toBeNull();
    },
  );

  it.each([
    ["$`x`$", "a$b \\$ c"],
    ["$$x$$", "a$b"],
    ["$$\nx\n$$", "a\n$\n \\$$ b"],
    ["```math\nx\n```", "a\n``\nb ```"],
    ["> $$\n> x\n> $$", "a\n\n> b"],
  ])(
    "writes into %j the formula %j so it reads back the same",
    (text, formula) => {
      const state = stateOf(text);
      const next = state.update({
        changes: mathChanges(state, 0, { formula }) ?? [],
      }).state;
      expect(mathAt(parsedState(next), 0)?.formula).toBe(formula);
    },
  );

  it("removes the whole element in one change", () => {
    const state = stateOf("a $`x`$ b");
    const changes = removeMathChanges(state, 3);
    expect(changes).toHaveLength(1);
    expect(apply(state, changes)).toBe("a  b");
  });

  it("reads no formula in the frontmatter or in code", () => {
    expect(mathAt(stateOf("`$$x$$`"), 3)).toBeNull();
    expect(mathAt(stateOf("```\n$$\nx\n$$\n```"), 5)).toBeNull();
  });
});

describe("the task and callout readers", () => {
  it("reads the box of the task line", () => {
    const state = stateOf("- [ ] t");
    expect(taskAt(parsedState(state), 6)).toEqual({ from: 2, to: 5 });
    expect(taskAt(stateOf("- t"), 2)).toBeNull();
  });

  it("writes a callout kind and fold sign as one character run each", () => {
    const state = stateOf("> [!NOTE]+ T\n> b");
    expect(apply(state, calloutKindChanges(state, 2, "warning"))).toBe(
      "> [!WARNING]+ T\n> b",
    );
    expect(apply(state, calloutFoldChanges(state, 2, "closed"))).toBe(
      "> [!NOTE]- T\n> b",
    );
    expect(apply(state, calloutFoldChanges(state, 2, null))).toBe(
      "> [!NOTE] T\n> b",
    );
    expect(
      calloutFoldChanges(stateOf("> [!DETAILS]\n> b"), 2, null),
    ).toBeNull();
  });
});

describe("two status labels and formulas side by side", () => {
  it("reads the label that starts at a shared position", () => {
    const state = stateOf(":status[a]:status[b]");
    expect(statusAt(parsedState(state), 10)).toMatchObject({
      from: 10,
      to: 20,
    });
  });

  it("reads the formula that starts at a shared position", () => {
    const state = stateOf("$`a`$$`b`$");
    expect(mathAt(parsedState(state), 5)).toMatchObject({ from: 5, to: 10 });
  });
});

describe("the formula verbs at the edges", () => {
  it.each([
    [
      "a block in a callout's body",
      "> [!NOTE] T\n> $$\n> x\n> $$\n> b",
      14,
      "> [!NOTE] T\n> b",
    ],
    [
      "a block that ends a callout",
      "> [!NOTE] T\n> $$\n> x\n> $$",
      14,
      "> [!NOTE] T",
    ],
    ["a block on its own", "a\n\n$$\nx\n$$\n\nb", 4, "a\n\n\nb"],
  ])("removes %s with its line break", (_name, text, at, after) => {
    const state = stateOf(text);
    const next = state.update({
      changes: removeMathChanges(state, at) ?? [],
    }).state;
    expect(bytes(next)).toBe(after);
  });

  it("keeps a callout one callout after its formula is removed", () => {
    const state = stateOf("> [!NOTE] T\n> $$\n> x\n> $$\n> b");
    const next = state.update({
      changes: removeMathChanges(state, 14) ?? [],
    }).state;
    expect(calloutFoldChanges(next, bytes(next).indexOf("b"), "open")).toEqual([
      { from: 9, to: 9, insert: "+" },
    ]);
  });

  it.each([
    ["a dollar block", "a\n\n$$\nx\n$$\n"],
    ["a math fence", "a\n\n```math\nx\n```\n"],
  ])("refuses a ## line in %s on a Reflection page", (_name, text) => {
    const state = stateOf(text, "reflection");
    const at = text.indexOf("x");
    const math = mathAt(parsedState(state), at);
    expect(math).not.toBeNull();
    expect(formulaRefusal(math!.form, "y\n## z", true)).toBe("heading-line");
    expect(mathChanges(state, at, { formula: "y\n## z" })).toBeNull();
    // An engram is not read section by section.
    expect(
      mathChanges(stateOf(text), at, { formula: "y\n## z" }),
    ).not.toBeNull();
  });

  it("writes a ## line into a quoted block on a Reflection page", () => {
    const text = "a\n\n> $$\n> x\n> $$\n";
    const state = stateOf(text, "reflection");
    const at = text.indexOf("x");
    const next = state.update({
      changes: mathChanges(state, at, { formula: "y\n## z" }) ?? [],
    }).state;
    expect(bytes(next)).toBe("a\n\n> $$\n> y\n> ## z\n> $$\n");
    expect(mathAt(parsedState(next), at)?.formula).toBe("y\n## z");
  });
});

describe("no reader reaches an id line, a comment or the frontmatter", () => {
  it("reads nothing from a Reflection id line", () => {
    const text = "## A :status[x] $`y`$\n<!-- r:0a1b -->\n- [ ] t\n";
    const state = stateOf(text, "reflection");
    expect(
      statusAt(parsedState(state), text.indexOf(":status") + 1),
    ).not.toBeNull();
    const id = text.indexOf("<!--");
    for (let at = id; at <= id + 15; at += 1) {
      expect(statusAt(parsedState(state), at)).toBeNull();
      expect(mathAt(parsedState(state), at)).toBeNull();
      expect(taskAt(parsedState(state), at)).toBeNull();
      expect(statusChanges(state, at, { words: "z" })).toBeNull();
      expect(mathChanges(state, at, { formula: "z" })).toBeNull();
    }
  });

  it("reads no formula inside an HTML comment", () => {
    expect(mathAt(stateOf("<!--\n$$\nx\n$$\n-->"), 8)).toBeNull();
    expect(mathAt(stateOf("<!--\n$$\nx\n$$\n-->\n"), 5)).toBeNull();
  });

  it("reads no formula or label in the frontmatter", () => {
    const text = "---\na: $$x$$ :status[b]\n---\nc";
    const state = stateOf(text);
    expect(mathAt(parsedState(state), 8)).toBeNull();
    expect(statusAt(parsedState(state), 15)).toBeNull();
  });
});

describe("fix round 1: formulas the page reads the same", () => {
  /** Writes `formula` into the formula at `at` and reads it back. */
  function written(text: string, at: number, formula: string) {
    const state = stateOf(text);
    const changes = mathChanges(state, at, { formula });
    expect(changes).not.toBeNull();
    const next = state.update({ changes: changes ?? [] }).state;
    return {
      next,
      text: bytes(next),
      read: mathAt(parsedState(next), at)?.formula,
    };
  }

  it.each([
    ["a $$ block in a list item", "- a\n  $$\n  x\n  $$", "  "],
    [
      "a $$ block in a quote in a list item",
      "- a\n  > $$\n  > x\n  > $$",
      "  > ",
    ],
  ])("keeps every new line of %s inside it", (_name, text, prefix) => {
    const at = text.indexOf("x");
    expect(mathAt(stateOf(text), at)?.formula).toBe("x");
    const { next, text: after, read } = written(text, at, "x\ny");
    expect(after).toBe(text.replace(`${prefix}x`, `${prefix}x\n${prefix}y`));
    expect(read).toBe("x\ny");
    // The new line is still the list item's.
    const y = after.indexOf("y");
    const node = syntaxTree(parsedState(next)).resolveInner(y, 1);
    let inItem = false;
    for (let n: typeof node | null = node; n; n = n.parent) {
      if (n.name === "ListItem") inItem = true;
    }
    expect(inItem).toBe(true);
  });

  it.each([
    ["$`x`$", " x\\ "],
    ["$`x`$", " a "],
    ["$$x$$", " x\\ "],
    ["$$x$$", " a "],
  ])(
    "pads %j so the formula %j reads back with its spaces",
    (text, formula) => {
      const { text: after, read } = written(text, 0, formula);
      expect(read).toBe(formula);
      // One space more on each side, which the page takes away again.
      expect(after).toContain(` ${formula} `);
    },
  );

  it("reads a padded formula as the page does", () => {
    expect(mathAt(stateOf("$` a `$"), 0)?.formula).toBe("a");
    expect(mathAt(stateOf("$$ a $$"), 0)?.formula).toBe("a");
    expect(mathAt(stateOf("$` a`$"), 0)?.formula).toBe(" a");
  });

  it("opens a double-backtick formula and keeps its backticks", () => {
    const state = stateOf("$``a`b``$");
    expect(mathAt(parsedState(state), 0)).toMatchObject({
      form: "code",
      formula: "a`b",
    });
    expect(written("$``a`b``$", 0, "c").text).toBe("$``c``$");
  });

  it.each([
    ["$`x`$", "a`b", "$``a`b``$"],
    ["$`x`$", "`a", "$`` `a ``$"],
    ["$``x``$", "a``b", "$```a``b```$"],
    ["$$x$$", "a$$b", "$$$a$$b$$$"],
    ["$$x$$", "$a", "$$ $a $$"],
  ])(
    "writes into %j the formula %j with the delimiters it needs",
    (text, formula, after) => {
      const result = written(text, 0, formula);
      expect(result.text).toBe(after);
      expect(result.read).toBe(formula);
    },
  );

  it("reads no formula after an escaped dollar sign, and one after an escaped backslash", () => {
    expect(mathAt(stateOf("a \\$`x`$"), 4)).toBeNull();
    expect(mathAt(stateOf("a \\\\$`x`$"), 6)?.formula).toBe("x");
  });
});

describe("fix round 1: label words from the source and back", () => {
  it.each([
    "a\\\\*b",
    "a\\\\\\\\",
    "a\\\\[b]c",
    "a\\*",
    "\\\\",
    "see [t](u)",
    "a\\]",
    "x \\\\\\* y",
  ])("keeps the source words %j through an untouched field", (source) => {
    const text = `:status[${source}]{red}`;
    const state = stateOf(text);
    expect(statusAt(parsedState(state), 0)?.to).toBe(text.length);
    const field = statusWords(state, 0);
    // Written as if typed: what comes back is the source as it was.
    expect(writtenLabel(field ?? "")).toBe(source);
  });
});

describe("a math fence in a list item (I6)", () => {
  it.each([
    ["after a blank line", "- item\n\n  ```math\n  x^2\n  ```"],
    ["right under the item", "- item\n  ```math\n  x^2\n  ```"],
  ])("reads its formula and keeps its indent, %s", (_name, text) => {
    const state = stateOf(text);
    const at = text.indexOf("x^2");
    expect(mathAt(parsedState(state), at)).toMatchObject({
      block: true,
      formula: "x^2",
    });
    expect(apply(state, mathChanges(state, at, { formula: "a\nb" }))).toBe(
      text.replace("  x^2", "  a\n  b"),
    );
  });
});
