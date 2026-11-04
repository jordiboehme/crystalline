/**
 * The editor grammar knows the dialect's markers: `[!KIND]` at the head of a
 * quote and the `:status[` and `]{...}` around a label are marks, painted in
 * the same token class as a quote's `>`, and neither is read as a link.
 */

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import { parsedState } from "../test/parse";
import { baseExtensions } from "./setup";

let view: EditorView | null = null;

afterEach(() => {
  view?.destroy();
  view = null;
});

const DOC =
  "# Head\n\n> [!NOTE]+ Title\n> body\n\nA :status[Done]{green} label\n";

function mount(doc = DOC): EditorView {
  view = new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        selection: EditorSelection.cursor(0),
        extensions: baseExtensions(false),
      }),
    ),
    parent: document.body,
  });
  return view;
}

/** The class of the innermost element holding exactly `text`. */
function classOf(v: EditorView, text: string): string | null {
  const spans = [...v.contentDOM.querySelectorAll("span")];
  return spans.find((span) => span.textContent === text)?.className ?? null;
}

describe("the dialect markers", () => {
  it("are parsed as marks, not links", () => {
    const v = mount();
    const names: string[] = [];
    syntaxTree(parsedState(v.state)).iterate({
      enter: (node) => {
        names.push(node.name);
      },
    });
    expect(names).toContain("CalloutMark");
    expect(names).toContain("StatusLabel");
    expect(names.filter((name) => name === "StatusMark")).toHaveLength(2);
    expect(names).not.toContain("Link");
  });

  it("wear the token class a quote mark wears", () => {
    const v = mount();
    // A quote mark: a heading mark also wears the heading's own size.
    const mark = classOf(v, ">");
    expect(mark).toBeTruthy();
    expect(classOf(v, "[!NOTE]+")).toBe(mark);
    expect(classOf(v, ":status[")).toBe(mark);
    expect(classOf(v, "]{green}")).toBe(mark);
  });

  /** The node names of `doc`'s tree, in order. */
  function namesOf(doc: string): string[] {
    const v = mount(doc);
    const names: string[] = [];
    syntaxTree(parsedState(v.state)).iterate({
      enter: (node) => {
        names.push(node.name);
      },
    });
    v.destroy();
    view = null;
    return names;
  }

  it("take a callout marker only in a quote", () => {
    expect(namesOf("> [!NOTE]\n> body\n")).toContain("CalloutMark");
    // A quote whose first line is blank still opens with it, as on the page.
    expect(namesOf(">\n> [!NOTE]\n> body\n")).toContain("CalloutMark");
    for (const doc of [
      "[!NOTE] at the start of a paragraph\n",
      "# [!NOTE] in a heading\n",
      "- [!NOTE] in a list item\n",
      "| [!NOTE] | b |\n| --- | --- |\n| 1 | 2 |\n",
    ]) {
      expect(namesOf(doc)).not.toContain("CalloutMark");
    }
    // A later paragraph of a quote is marked too, which is cosmetic: the
    // grammar asks only the public parent type, and everything that acts on
    // a callout asks for the quote's first paragraph (`calloutOf`).
    expect(namesOf("> words first\n>\n> [!NOTE] later\n")).toContain(
      "CalloutMark",
    );
    // A link stays a link.
    const link = namesOf("> [!note](https://example.com)\n");
    expect(link).not.toContain("CalloutMark");
    expect(link).toContain("Link");
  });

  it("read double-dollar math on one line, never in code", () => {
    const names = namesOf("Area $$x^2$$ here\n\n$$y$$\n");
    expect(names.filter((name) => name === "InlineMath")).toHaveLength(2);
    expect(namesOf("Run `$$x$$` here\n")).not.toContain("InlineMath");
    expect(namesOf("Costs $5 and $6\n")).not.toContain("InlineMath");
    expect(namesOf("Open $$x\nclose$$\n")).not.toContain("InlineMath");
    expect(namesOf("$$\na\n$$\n")).not.toContain("InlineMath");
  });
});
