/**
 * Preview widgets below the two block kinds that earn them: a mermaid fence
 * renders its diagram, a pipe table renders a readable grid. Both draw AFTER
 * their source, never in place of it, so every assertion checks the widget's
 * own DOM while confirming the buffer itself is untouched.
 */

import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import mermaid from "mermaid";
import { afterEach, describe, expect, it, vi } from "vitest";

import { describeMermaidError, fencePreviews } from "./fencePreviews";
import { baseExtensions } from "./setup";

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(() => Promise.resolve({ svg: "<svg data-diagram></svg>" })),
  },
}));

/**
 * A queued rejection may not outlive the test that queued it.
 *
 * `mockRejectedValueOnce` stays in the queue when the render it was written
 * for never happens - a widget whose DOM CodeMirror reuses, an assertion that
 * fails before the second render - and the next test then watches a diagram
 * break for no reason it can see. Restoring the mock's own resolving
 * implementation after each test keeps a failure inside the test that caused
 * it; the shape is captured here because `RenderResult` has more required
 * fields than this suite cares to spell.
 */
const renderOk = vi.mocked(mermaid.render).getMockImplementation();

afterEach(() => {
  vi.mocked(mermaid.render).mockReset();
  if (renderOk) {
    vi.mocked(mermaid.render).mockImplementation(renderOk);
  }
});

function editor(doc: string): EditorView {
  return new EditorView({
    doc,
    selection: EditorSelection.cursor(0),
    extensions: [...baseExtensions(false), fencePreviews(false)],
    parent: document.body,
  });
}

/**
 * What mermaid 11.16.1 actually throws, captured verbatim.
 *
 * Every string below came out of the installed renderer, parsing the half-typed
 * diagram named beside it under this app's own configuration; nothing here is
 * written by hand. That matters more than it sounds: mermaid's jison grammars
 * (flowchart, sequence, class, state, ER, journey, mindmap, C4, quadrant,
 * timeline, xy - most of what the picker offers) fail with FOUR lines, where
 * only the last one is a complaint and the middle two are the author's own
 * source echoed back under a caret ruler. A hand-written two-line fixture makes
 * every "pick the message" rule look right, including the rule that ships the
 * echo.
 */
const MESSAGES = {
  /** `flowchart TD` + `  A[Step` - unterminated at the end of the fence. */
  flowchartAtEnd:
    "Parse error on line 3:\n...owchart TD  A[Step\n---------------------^\nExpecting 'SQE', 'DOUBLECIRCLEEND', 'PE', '-)', 'STADIUMEND', 'SUBROUTINEEND', 'PIPE', 'CYLINDEREND', 'DIAMOND_STOP', 'TAGEND', 'TRAPEND', 'INVTRAPEND', 'UNICODE_TEXT', 'TEXT', 'TAGSTART', got '1'",
  /** A four-line `graph TD` whose last line is a trailing arrow. */
  graphAtEnd:
    "Parse error on line 5:\n...B  B --> C  C -->\n--------------------^\nExpecting 'AMP', 'COLON', 'PIPE', 'TESTSTR', 'DOWN', 'DEFAULT', 'NUM', 'COMMA', 'NODE_STRING', 'BRKT', 'MINUS', 'MULT', 'UNICODE_TEXT', got 'EOF'",
  /** `C[[[` on the third line of a four-line flowchart - a real line 3. */
  flowchartMidBody:
    "Parse error on line 3:\n...t TD  A --> B  C[[[  D --> E\n---------------------^\nExpecting 'TAGEND', 'STR', 'MD_STR', 'UNICODE_TEXT', 'TEXT', 'TAGSTART', got 'SQS'",
  /** A class diagram with a stray `@@@`: jison's OTHER spelling, with a period. */
  classLexical:
    "Lexical error on line 3. Unrecognized text.\n...agram  class Foo  @@@\n---------------------^",
  /** pie, via langium: one line, a column, and a newline inside the backticks. */
  pieLangium:
    "Parsing failed:  Parse error on line 2, column 11: Expecting token of type 'NUMBER_PIE' but found `\n`.",
  /** architecture, via langium: a lexer lead with a second error trailing it. */
  architectureLangium:
    "Parsing failed: Lexer error on line 4, column 9: unexpected character: ->(<- at offset: 80, skipped 1 characters. Parse error on line 4, column 8: Expecting token of type 'ARCH_TITLE' but found `>`.",
  /** radar, via langium, when it cannot even say which line it choked on. */
  radarNoLine:
    "Parsing failed:  Parse error on line ?, column ?: Expecting token of type 'NUMBER' but found ``.",
  /**
   * radar with an unclosed `curve c{`: langium's OTHER expectation shape, a
   * numbered wall of alternatives with the token it found on the last line.
   */
  radarAlternatives:
    "Parsing failed:  Parse error on line 3, column 11: Expecting: one of these possible Token sequences:\n  1. [NUMBER]\n  2. [NEWLINE, NUMBER]\n  3. [NEWLINE, NEWLINE, NEWLINE]\n  4. [NEWLINE, NEWLINE, NUMBER]\n  5. [ID]\n  6. [NEWLINE, ID]\n  7. [NEWLINE, NEWLINE, NEWLINE]\n  8. [NEWLINE, NEWLINE, ID]\nbut found: '\n'",
  /** Any text mermaid cannot type-detect - the whole fence body rides along. */
  unknownWithLineInside:
    "No diagram type detected matching given configuration for text: notadiagram see line 42 here",
  /** The same family, where the body the author typed IS a parser message. */
  unknownWithLeadInside:
    "No diagram type detected matching given configuration for text: notadiagram Parse error on line 5: retry",
  /** The same error for an empty fence, which is what a fresh ```mermaid is. */
  unknownEmpty:
    "No diagram type detected matching given configuration for text: ",
  /** A sequence diagram's hand-written complaint: no line number, all meaning. */
  sequenceHuman: "Trying to inactivate an inactive participant (Alice)",
  /** A mindmap's second root, whose label is the author's own astral text. */
  mindmapAstral: `There can be only one root. No parent could be found for ("x${"\u{1F600}".repeat(200)}")`,
} as const;

describe("fence previews", () => {
  it("renders a mermaid fence's diagram below the fence", async () => {
    const view = editor("```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    // The source is still the buffer, untouched.
    expect(view.state.doc.toString()).toContain("graph TD; A-->B;");
    view.destroy();
  });

  it("suppresses mermaid's own error rendering", async () => {
    // This preview redraws on every keystroke, so it renders half-typed
    // diagrams constantly and most of those fail. Mermaid's default is to
    // append its error graphic to `document.body`, outside the editor and
    // outside anything that ever tears it down: without this flag the bombs
    // pile up under the page for the whole session.
    const view = editor("```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(mermaid.initialize).toHaveBeenCalled();
    });
    expect(vi.mocked(mermaid.initialize).mock.calls.at(-1)?.[0]).toMatchObject({
      suppressErrorRendering: true,
    });
    view.destroy();
  });

  it("previews in the same palette the reading view draws in", async () => {
    // One configuration serves both surfaces: a diagram that changed color
    // between the editor and the page would read as two different diagrams.
    const view = editor("```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(mermaid.initialize).toHaveBeenCalled();
    });
    const config = vi.mocked(mermaid.initialize).mock.calls.at(-1)?.[0];
    expect(config).toMatchObject({ theme: "base" });
    expect(config?.themeVariables).toMatchObject({
      primaryColor: "#ccfbf1",
      primaryBorderColor: "#0f766e",
      noteBkgColor: "#f1f5f9",
      noteTextColor: "#0f172a",
      titleColor: "#0f172a",
    });
    view.destroy();
  });

  it("renders a table preview below the pipe syntax", () => {
    const view = editor("| a | b |\n|---|---|\n| 1 | 2 |\n");
    const table = view.dom.querySelector(".cm-table-preview table");
    expect(table).not.toBeNull();
    expect(table?.querySelectorAll("th").length).toBe(2);
    expect(table?.querySelectorAll("td").length).toBe(2);
    expect(table?.textContent).toContain("1");
    view.destroy();
  });

  it("shows nothing for a fence of another language", () => {
    const view = editor("```rust\nfn main() {}\n```\n");
    expect(view.dom.querySelector(".cm-mermaid-preview")).toBeNull();
    view.destroy();
  });

  it("a diagram that will not parse says why, quietly", async () => {
    vi.mocked(mermaid.render).mockRejectedValueOnce(
      new Error(MESSAGES.flowchartAtEnd),
    );
    const view = editor("```mermaid\nflowchart TD\n  A[Step\n```\n");
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-error")).not.toBeNull();
    });
    const caption = view.dom.querySelector(".cm-mermaid-error");
    // The parser's complaint, at the line of the buffer it complains about:
    // `  A[Step` is document line 3, and mermaid's own "line 3" counts inside
    // the two-line fence body, which has no line 3.
    expect(caption?.textContent).toContain("Line 3:");
    expect(caption?.textContent).toContain("Expecting 'SQE'");
    // Never the author's own source read back to them: mermaid puts the echo
    // of `...owchart TD  A[Step` between its lead-in and its complaint, and
    // that text is already on screen one line above the caption.
    expect(caption?.textContent).not.toContain("owchart TD");
    // Text, no markup, and nothing that announces itself: this fires on most
    // keystrokes while a diagram is being typed.
    expect(caption?.childElementCount).toBe(0);
    expect(caption?.getAttribute("role")).toBeNull();
    expect(caption?.getAttribute("aria-live")).toBeNull();
    // The buffer is untouched.
    expect(view.state.doc.toString()).toContain("A[Step");
    view.destroy();
  });

  it("counts the caption's line the way the findings panel does", async () => {
    // One meaning of "line N" per screen. `FindingsPanel` renders "Go to line
    // N" for a DOCUMENT line a few centimetres away from this caption, so a
    // fence-relative number here would be a second, silent meaning: this fence
    // body starts at document line 6, and mermaid's "line 3" for its two lines
    // clamps to the second one.
    vi.mocked(mermaid.render).mockRejectedValueOnce(
      new Error(MESSAGES.flowchartAtEnd),
    );
    const view = editor(
      "# Title\n\nintro\n\n```mermaid\nflowchart TD\n  A[Step\n```\n",
    );
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-error")).not.toBeNull();
    });
    expect(view.dom.querySelector(".cm-mermaid-error")?.textContent).toContain(
      "Line 7:",
    );
    view.destroy();
  });

  it("re-reads the line when the fence is pushed down the document", async () => {
    // The widget carries the fence's own first line, so that line is part of
    // what makes two widgets equal. Without it CodeMirror reuses the DOM of a
    // widget whose source has not changed - correct for the diagram, wrong for
    // the number, which would keep naming the line the fence used to sit on.
    // The correction is made IN the caption rather than by drawing again: one
    // rejection is queued, and one is all mermaid is asked for.
    vi.mocked(mermaid.render).mockRejectedValueOnce(
      new Error(MESSAGES.flowchartAtEnd),
    );
    const view = editor(
      "# Title\n\nintro\n\n```mermaid\nflowchart TD\n  A[Step\n```\n",
    );
    await vi.waitFor(() => {
      expect(
        view.dom.querySelector(".cm-mermaid-error")?.textContent,
      ).toContain("Line 7:");
    });
    const caption = view.dom.querySelector(".cm-mermaid-error");
    view.dispatch({ changes: { from: 0, insert: "one more line\n" } });
    await vi.waitFor(() => {
      expect(
        view.dom.querySelector(".cm-mermaid-error")?.textContent,
      ).toContain("Line 8:");
    });
    // The same caption element, rewritten: nothing was torn down to say a
    // different number.
    expect(view.dom.querySelector(".cm-mermaid-error")).toBe(caption);
    expect(vi.mocked(mermaid.render)).toHaveBeenCalledTimes(1);
    view.destroy();
  });

  it("keeps the diagram mounted when the fence only changes line", async () => {
    // Enter typed anywhere above a diagram moves its fence, which changes the
    // line the caption would name and so makes the new widget unequal to the
    // old one. Equality is only the FIRST question CodeMirror asks: the second
    // is whether the new widget can take over the mounted DOM, and a widget
    // whose source and theme are unchanged can - so the SVG stays on screen
    // instead of blinking out for a tick on every Enter.
    const view = editor("intro\n\n```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    const svg = view.dom.querySelector(".cm-mermaid-preview svg");
    // A mark on the node itself: the assertion is about THIS element surviving,
    // not about some svg being present again a tick later.
    svg?.setAttribute("data-generation", "first");
    view.dispatch({ changes: { from: 0, insert: "one more line\n" } });
    // Synchronously after the edit, which is where the flash was.
    expect(view.dom.querySelector(".cm-mermaid-preview svg")).toBe(svg);
    expect(
      view.dom.querySelector(
        ".cm-mermaid-preview svg[data-generation='first']",
      ),
    ).not.toBeNull();
    expect(vi.mocked(mermaid.render)).toHaveBeenCalledTimes(1);
    view.destroy();
  });

  it("captions a late failure at the line the fence reached", async () => {
    // A render is a promise, so a fence can move while its own render is in
    // flight: the box changes hands, and the rejection then arrives holding a
    // widget that was retired one edit ago. The caption has to say where the
    // fence IS - which is why the box, not the closure, carries the line.
    let fail: ((cause: unknown) => void) | undefined;
    vi.mocked(mermaid.render).mockImplementationOnce(
      () =>
        new Promise<never>((_resolve, reject) => {
          fail = reject;
        }),
    );
    const view = editor(
      "# Title\n\nintro\n\n```mermaid\nflowchart TD\n  A[Step\n```\n",
    );
    // The render has started - the module import and the call are both
    // promises - and has not answered yet.
    await vi.waitFor(() => {
      expect(fail).not.toBeUndefined();
    });
    view.dispatch({ changes: { from: 0, insert: "one more line\n" } });
    fail?.(new Error(MESSAGES.flowchartAtEnd));
    await vi.waitFor(() => {
      expect(
        view.dom.querySelector(".cm-mermaid-error")?.textContent,
      ).toContain("Line 8:");
    });
    view.destroy();
  });

  it("draws again when the fence's own text changes", async () => {
    // The other half of the same rule: taking over a box is for a diagram that
    // is the same diagram. Edit the body and the preview has to be rendered
    // again, however cheap keeping the old one would be.
    const view = editor("```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    const svg = view.dom.querySelector(".cm-mermaid-preview svg");
    svg?.setAttribute("data-generation", "first");
    // The end of the body line: "```mermaid\n" is 11 characters and
    // "graph TD; A-->B;" is 16 more.
    view.dispatch({ changes: { from: 27, insert: " C-->D;" } });
    expect(view.state.doc.toString()).toContain("A-->B; C-->D;");
    await vi.waitFor(() => {
      expect(vi.mocked(mermaid.render)).toHaveBeenCalledTimes(2);
    });
    expect(
      view.dom.querySelector(
        ".cm-mermaid-preview svg[data-generation='first']",
      ),
    ).toBeNull();
    view.destroy();
  });

  it("keeps the author's own angle brackets as text", async () => {
    // The shape is langium's, captured; the token inside the backticks is
    // whatever the author typed, so markup reaches the caption on the ordinary
    // path and has to stay a string.
    vi.mocked(mermaid.render).mockRejectedValueOnce(
      new Error(
        "Parsing failed:  Parse error on line 1, column 3: Expecting token of type 'NUMBER_PIE' but found `<b>x</b>`.",
      ),
    );
    const view = editor('```mermaid\npie "<b>x</b>" :\n```\n');
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-error")).not.toBeNull();
    });
    const caption = view.dom.querySelector(".cm-mermaid-error");
    expect(caption?.textContent).toContain("`<b>x</b>`");
    expect(caption?.childElementCount).toBe(0);
    view.destroy();
  });

  it("a good render never shows the caption", async () => {
    const view = editor("```mermaid\ngraph TD; A-->B;\n```\n");
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    expect(view.dom.querySelector(".cm-mermaid-error")).toBeNull();
    view.destroy();
  });

  it("does not preview a table-shaped line that is fence content", () => {
    // The syntax tree, not a line-shaped regex, decides what a table is: a
    // pipe row inside a fence is code the author wrote about a table, not
    // one the parser mounts a `Table` node for.
    const view = editor("```text\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n");
    expect(view.dom.querySelector(".cm-table-preview")).toBeNull();
    view.destroy();
  });
});

describe("describeMermaidError", () => {
  it("keeps the complaint and drops the echoed source above it", () => {
    // The fence body is `flowchart TD` + `  A[Step` at document lines 2 and 3.
    // Mermaid's four-line message carries its own lead-in, the author's source,
    // a caret ruler and, last, the only line it wrote to explain the failure.
    expect(
      describeMermaidError(new Error(MESSAGES.flowchartAtEnd), {
        firstLine: 2,
        lineCount: 2,
      }),
    ).toBe(
      "Line 3: Expecting 'SQE', 'DOUBLECIRCLEEND', 'PE', '-)', 'STADIUMEND', 'SUBROUTINEEND', 'PIPE', 'CYLINDEREND', 'DIAMOND_STOP', 'TAGEND', 'TRAPEND' ... got '1'",
    );
  });

  it("elides the middle of a token wall rather than its ends", () => {
    // Both ends of `Expecting <a wall of token names>, got 'X'` carry meaning
    // and the tail carries most of it, so a plain tail cut would throw away
    // the half a person actually reads.
    const caption = describeMermaidError(new Error(MESSAGES.flowchartAtEnd), {
      firstLine: 2,
      lineCount: 2,
    });
    expect(caption.startsWith("Line 3: Expecting 'SQE',")).toBe(true);
    expect(caption.endsWith(" ... got '1'")).toBe(true);
    expect(Array.from(caption).length).toBeLessThanOrEqual(160);
  });

  it("maps mermaid's line inside the fence to a line of the document", () => {
    // A fence body of four lines starting at document line 5, and a genuine
    // error on its third line: 5 + 3 - 1 = 7.
    expect(
      describeMermaidError(new Error(MESSAGES.flowchartMidBody), {
        firstLine: 5,
        lineCount: 4,
      }),
    ).toBe(
      "Line 7: Expecting 'TAGEND', 'STR', 'MD_STR', 'UNICODE_TEXT', 'TEXT', 'TAGSTART', got 'SQS'",
    );
  });

  it("clamps a line the fence does not have to its last one", () => {
    // The state a diagram is in on nearly every keystroke: the construct is
    // unterminated, so the parser reaches the end of the text and reports one
    // line past it. Here a four-line body starting at document line 2 gets
    // "line 5" - document line 6, which is the closing fence or worse.
    expect(
      describeMermaidError(new Error(MESSAGES.graphAtEnd), {
        firstLine: 2,
        lineCount: 4,
      }),
    ).toBe(
      "Line 5: Expecting 'AMP', 'COLON', 'PIPE', 'TESTSTR', 'DOWN', 'DEFAULT', 'NUM', 'COMMA', 'NODE_STRING', 'BRKT', 'MINUS', 'MULT', 'UNICODE_TEXT', got 'EOF'",
    );
  });

  it("reads jison's other spelling, the one with a period", () => {
    // Not `Lexical error on line 3:` - mermaid writes a period and puts the
    // informative words on the lead-in line itself, with the echo below.
    expect(
      describeMermaidError(new Error(MESSAGES.classLexical), {
        firstLine: 10,
        lineCount: 3,
      }),
    ).toBe("Line 12: Unrecognized text.");
  });

  it("starts a langium caption at the complaint, not at a comma", () => {
    // These messages name a column after the line, and the found token can be
    // a newline, which is why the caption is built from a whitespace-collapsed
    // message rather than a raw first line.
    expect(
      describeMermaidError(new Error(MESSAGES.pieLangium), {
        firstLine: 4,
        lineCount: 2,
      }),
    ).toBe("Line 5: Expecting token of type 'NUMBER_PIE' but found ` `.");
  });

  it("takes the line from a lexer lead as readily as a parse lead", () => {
    expect(
      describeMermaidError(new Error(MESSAGES.architectureLangium), {
        firstLine: 2,
        lineCount: 4,
      }),
    ).toBe(
      "Line 5: unexpected character: ->(<- at offset: 80, skipped 1 characters. Parse error on line 4, column 8: Expecting token of type 'ARCH_TITLE' but found `>`.",
    );
  });

  it("says the complaint without a number when mermaid has none", () => {
    // langium answers `line ?, column ?` when it cannot place the failure; the
    // sentence is still worth reading, so it ships without a line prefix.
    expect(
      describeMermaidError(new Error(MESSAGES.radarNoLine), {
        firstLine: 2,
        lineCount: 3,
      }),
    ).toBe("Expecting token of type 'NUMBER' but found ``.");
  });

  it("lets mermaid's hand-written sentences through", () => {
    // The messages that carry a line number are jison's machine text; the ones
    // without a line are the sentences a maintainer wrote for a person. The
    // old rule threw exactly those away.
    expect(
      describeMermaidError(new Error(MESSAGES.sequenceHuman), {
        firstLine: 2,
        lineCount: 3,
      }),
    ).toBe("Trying to inactivate an inactive participant (Alice)");
  });

  it("never lets the author's own text donate a line number", () => {
    // `UnknownDiagramError` embeds the whole fence body, so an unanchored
    // search for "line N" reads a number out of the author's prose - and then
    // asserts it as the failing line.
    expect(
      describeMermaidError(new Error(MESSAGES.unknownWithLineInside), {
        firstLine: 2,
        lineCount: 3,
      }),
    ).toBe("This diagram does not render yet.");
  });

  it("never lets an author-typed lead donate one either", () => {
    // The same family, one step meaner: the fence body IS a parser message, so
    // the lazy lead-in pattern - which has to be lazy, to eat langium's
    // `Parsing failed:` - matches the author's own words inside the quoted
    // body. The undetectable-diagram family is therefore recognized on the
    // whole message, before anything is stripped off the front of it.
    expect(
      describeMermaidError(new Error(MESSAGES.unknownWithLeadInside), {
        firstLine: 8,
        lineCount: 1,
      }),
    ).toBe("This diagram does not render yet.");
  });

  it("keeps the token langium found at the end of its alternatives wall", () => {
    // langium's second expectation shape: `Expecting:` with a colon and a
    // numbered list of token sequences, closing with `but found:` instead of
    // jison's `, got`. The wall is long enough to need the cap, and the token
    // it choked on is the half a person reads - so the middle gives way here
    // too, rather than the tail being cut off.
    const caption = describeMermaidError(
      new Error(MESSAGES.radarAlternatives),
      {
        firstLine: 2,
        lineCount: 3,
      },
    );
    expect(caption.startsWith("Line 4: Expecting: one of these")).toBe(true);
    expect(caption.endsWith(" ... but found: ' '")).toBe(true);
    expect(Array.from(caption).length).toBeLessThanOrEqual(160);
  });

  it("answers an empty fence with the same calm sentence", () => {
    // What a fresh ```mermaid fence throws until the first parseable word, so
    // this sentence is the one a person sees most often.
    expect(
      describeMermaidError(new Error(MESSAGES.unknownEmpty), {
        firstLine: 2,
        lineCount: 1,
      }),
    ).toBe("This diagram does not render yet.");
  });

  it("uses the same sentence when the lead-in is all mermaid said", () => {
    // Constructed rather than captured: no message in the corpus stops after
    // its lead-in, and the caption still may not end in a bare colon.
    expect(
      describeMermaidError(new Error("Parse error on line 2:"), {
        firstLine: 3,
        lineCount: 5,
      }),
    ).toBe("Line 4: This diagram does not render yet.");
  });

  it("caps at 160 characters without splitting one in half", () => {
    // The author's own label reaches the caption through mermaid's message,
    // emoji included, and a cap counted in UTF-16 units cuts an astral
    // character down the middle, leaving a lone surrogate that draws as the
    // replacement glyph.
    const caption = describeMermaidError(new Error(MESSAGES.mindmapAstral), {
      firstLine: 2,
      lineCount: 3,
    });
    expect(Array.from(caption)).toHaveLength(160);
    expect(caption.endsWith("...")).toBe(true);
    expect(caption).not.toMatch(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])/);
  });

  it("survives a cause that is not an Error", () => {
    // A failed dynamic import or a thrown string reaches the same `.catch`.
    const fence = { firstLine: 2, lineCount: 4 };
    expect(describeMermaidError(undefined, fence)).toBe(
      "This diagram does not render yet.",
    );
    expect(describeMermaidError({ trouble: true }, fence)).toBe(
      "This diagram does not render yet.",
    );
    expect(describeMermaidError(MESSAGES.flowchartMidBody, fence)).toBe(
      "Line 4: Expecting 'TAGEND', 'STR', 'MD_STR', 'UNICODE_TEXT', 'TEXT', 'TAGSTART', got 'SQS'",
    );
  });
});
