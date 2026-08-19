/**
 * The bar as a component: that every label reaches the verb it promises, and
 * that the context segment appears and disappears without taking the caret
 * with it.
 *
 * What the verbs DO is `toolbar.test.ts`'s and `tableVerbs.test.ts`'s job, on
 * plain views with no React in sight. What is under test here is the wiring
 * between the two - an accessible name, a click, and the buffer edit that
 * follows - which is exactly the seam a renamed constant or a copied `onClick`
 * breaks without any command test noticing.
 */

import { undo } from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { RenderResult } from "@testing-library/react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test } from "vitest";

import { ATTACHMENT_ACCEPT } from "../api/files";
import { Tooltips } from "../components/primitives";
import { parsedState } from "../test/parse";
import { EditorToolbar } from "./EditorToolbar";
import { frontmatterFold } from "./frontmatterFold";
import { baseExtensions, docText, lineSeparatorFor } from "./setup";
import { MERMAID_SKELETON } from "./toolbar";

const TABLE_DOC = "Before\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nAfter\n";
const IN_TABLE = TABLE_DOC.indexOf("| 1") + 2;

/** The frontmatter a preview-mode buffer hides behind its chip. */
const YAML = "---\ntitle: T\nstatus: draft\n---\n";
/** That block, folded, with a caret at 0 - where a freshly opened engram puts it. */
const FOLDED_DOC = `${YAML}\nBody line\n`;

/** The segment's six controls, by the names that are the contract. */
const SEGMENT = [
  "Add row below",
  "Add column after",
  "Delete row",
  "Delete column",
  "Align column",
  "Prettify table",
];

let view: EditorView | null = null;
/** The bar `mount` put up, so a test can redraw that ONE rather than add another. */
// Only the redraw is kept, rather than the whole render result: a `render`
// call carrying a `wrapper` infers its query set as the bare `Queries`
// interface, which is not the concrete `RenderResult` this held before, and
// the one method this helper actually uses is the same either way.
let bar: Pick<RenderResult, "rerender"> | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
  bar = null;
});

/** A live buffer with the bar above it, exactly as a screen mounts them. */
function mount(
  doc: string,
  anchor: number,
  head = anchor,
  tableActive = false,
  /** Extras the buffer is built with - the update listener a count rides on. */
  extensions: Extension[] = [],
): EditorView {
  view = new EditorView({
    // `parsedState` for the same reason the comment below gives: the whole
    // table segment reaches through `tableContextAt` into the syntax tree, and
    // a new state's first parse is cut off after 20ms of wall clock. Nothing
    // here was seen failing, but neither was `tableVerbs.test.ts` in sixteen
    // loaded runs, and it is the same read of the same tree.
    state: parsedState(
      EditorState.create({
        doc,
        selection: EditorSelection.single(anchor, head),
        // The markdown language, because table detection reads the syntax tree.
        extensions: [
          ...lineSeparatorFor(doc),
          ...baseExtensions(false),
          ...extensions,
        ],
      }),
    ),
    parent: document.body,
  });
  bar = render(<EditorToolbar view={view} tableActive={tableActive} />, {
    wrapper: Tooltips,
  });
  return view;
}

/** The same bar again with the segment flipped - what a screen's state does. */
function flip(tableActive: boolean): void {
  bar?.rerender(<EditorToolbar view={view} tableActive={tableActive} />);
}

function press(name: string): Promise<void> {
  return userEvent.click(screen.getByRole("button", { name }));
}

/** Open the align menu and pick one of its items. */
async function align(label: string): Promise<void> {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Align column" }));
  await user.click(await screen.findByRole("menuitem", { name: label }));
}

/** Open the diagram menu and pick one of its starters. */
async function pickDiagram(label: string): Promise<void> {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Insert diagram" }));
  await user.click(await screen.findByRole("menuitem", { name: label }));
}

describe("the format buttons", () => {
  test("Strikethrough wraps the selection", async () => {
    const v = mount("hello world", 0, 5);
    await press("Strikethrough");
    expect(docText(v.state)).toBe("~~hello~~ world");
  });

  test("Numbered list numbers the cursor's line", async () => {
    const v = mount("one\ntwo\n", 0);
    await press("Numbered list");
    expect(docText(v.state)).toBe("1. one\ntwo\n");
  });

  test("Blockquote quotes the cursor's line", async () => {
    const v = mount("one\ntwo\n", 0);
    await press("Blockquote");
    expect(docText(v.state)).toBe("> one\ntwo\n");
  });

  test("Link inserts a link with its text selected", async () => {
    const v = mount("", 0);
    await press("Link");
    expect(docText(v.state)).toBe("[text](url)");
    const { from, to } = v.state.selection.main;
    expect(v.state.sliceDoc(from, to)).toBe("text");
  });

  test("Insert table opens the size picker and inserts from it", async () => {
    // The pin for the one event-path question this bar raises: it cancels its
    // own mousedown to keep the selection in the buffer, and a Popover trigger
    // opens on CLICK, which a cancelled mousedown never suppresses. If that
    // were wrong the button would look dead.
    const v = mount("prose\n", 5);
    await press("Insert table");
    expect(screen.getByRole("group", { name: "Table size" })).not.toBeNull();
    await press("2 columns by 2 rows");
    expect(docText(v.state)).toBe(
      "prose\n\n| Column | Column |\n| --- | --- |\n|  |  |\n\n",
    );
  });

  test("Insert code block inserts a bare fence", async () => {
    const v = mount("prose\n", 5);
    await press("Insert code block");
    // A blank line, the bare fence, its body line and the close - dropped
    // below the cursor's line, which keeps its own trailing break.
    expect(docText(v.state)).toBe("prose\n\n```\n\n```\n\n");
  });
});

describe("the diagram menu", () => {
  test("offers sixteen starters under three group labels", async () => {
    mount("prose\n", 5);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Insert diagram" }));
    // Sixteen by count rather than by reading the starters module back at
    // itself: a menu that quietly lost an entry would agree with the module
    // and disagree with the decision that named all sixteen.
    expect(await screen.findAllByRole("menuitem")).toHaveLength(16);
    for (const label of ["Everyday", "Planning and product", "Technical"]) {
      expect(screen.getByText(label)).not.toBeNull();
    }
    // The labels are headings, not choices: only the sixteen are selectable.
    expect(
      screen.getByRole("menuitem", { name: "User journey" }),
    ).not.toBeNull();
    expect(screen.getByRole("menuitem", { name: "Radar" })).not.toBeNull();
  });

  test("User journey inserts its starter with the title selected", async () => {
    const v = mount("prose\n", 5);
    await pickDiagram("User journey");
    const lines = docText(v.state).split("\n");
    const fence = lines.indexOf("```mermaid");
    expect(fence).toBeGreaterThan(-1);
    expect(lines[fence + 1]).toBe("journey");
    // The caret arrives on the first word worth replacing, which is the whole
    // point of the picker over the old one-shape button.
    const { from, to } = v.state.selection.main;
    expect(v.state.sliceDoc(from, to)).toBe("First visit");
  });

  test("Enter Enter reproduces the old diagram button exactly", async () => {
    // Decision 16, end to end: opening the menu from the keyboard highlights
    // the first item, so the shortest route costs one extra keypress and
    // lands the same bytes the button used to insert on its own.
    const v = mount("prose\n", 5);
    const user = userEvent.setup();
    screen.getByRole("button", { name: "Insert diagram" }).focus();
    await user.keyboard("{Enter}");
    await screen.findByRole("menuitem", { name: "Flowchart" });
    await user.keyboard("{Enter}");
    expect(docText(v.state)).toBe(
      `prose\n\n${MERMAID_SKELETON.join("\n")}\n\n`,
    );
    // Only the caret differs from the old insert, and deliberately so.
    const { from, to } = v.state.selection.main;
    expect(v.state.sliceDoc(from, to)).toBe("First step");
  });

  test("each group carries its heading as its accessible name", async () => {
    // The three groups exist for a screen reader as much as for an eye: a
    // `role="group"` that names nothing announces sixteen flat items, which is
    // the list the grouping was introduced to break up.
    mount("prose\n", 5);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Insert diagram" }));
    for (const label of ["Everyday", "Planning and product", "Technical"]) {
      expect(await screen.findByRole("group", { name: label })).not.toBeNull();
    }
  });

  test("Escape closes the menu and gives the buffer back", async () => {
    // The other half of what the close handler promises. The select path is
    // pinned below; a refactor that moved `view.focus()` into the item handler
    // would keep that one green and strand the caret on the trigger here.
    const v = mount("prose\n", 5);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Insert diagram" }));
    await screen.findByRole("menuitem", { name: "Flowchart" });
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menuitem")).toBeNull();
    });
    expect(v.hasFocus).toBe(true);
    expect(docText(v.state)).toBe("prose\n");
  });

  test("one undo takes a whole starter back out", async () => {
    // One transaction is one undo step: the verb tags itself as input, so the
    // fence never joins the typing around it and never comes out in pieces.
    const v = mount("prose\n", 5);
    await pickDiagram("Sequence");
    expect(docText(v.state)).toContain("sequenceDiagram");
    undo(v);
    expect(docText(v.state)).toBe("prose\n");
  });

  test("the fold guard fires through the menu's own select path", async () => {
    // This menu is the only insert verb whose guard is reached from an
    // `onSelect` rather than an `onClick`, and both of the guard's answers
    // matter here: a caret the buffer parked inside the hidden block is moved
    // to the body, and a selection somebody made on purpose is refused.
    const v = mount(FOLDED_DOC, 0, 0, false, [frontmatterFold()]);
    await pickDiagram("Flowchart");
    expect(docText(v.state)).toBe(
      `${YAML}${MERMAID_SKELETON.join("\n")}\n\nBody line\n`,
    );
    const { from, to } = v.state.selection.main;
    expect(v.state.sliceDoc(from, to)).toBe("First step");
  });

  test("a selection over the folded block refuses the starter", async () => {
    const v = mount(FOLDED_DOC, 0, FOLDED_DOC.length, false, [
      frontmatterFold(),
    ]);
    await pickDiagram("Flowchart");
    expect(docText(v.state)).toBe(FOLDED_DOC);
  });

  test("a starter lands in one transaction and gives the buffer back", async () => {
    let edits = 0;
    const v = mount("prose\n", 5, 5, false, [
      EditorView.updateListener.of((update) => {
        if (update.docChanged) edits += 1;
      }),
    ]);
    await pickDiagram("Sequence");
    expect(docText(v.state)).toContain("sequenceDiagram");
    expect(edits).toBe(1);
    // The menu hands focus back to its trigger on close unless it is told
    // otherwise; an author who just picked a diagram wants the buffer.
    expect(v.hasFocus).toBe(true);
  });
});

describe("the table segment", () => {
  test("its controls are there only while the caret is in a table", () => {
    const { rerender } = render(<EditorToolbar view={null} />, {
      wrapper: Tooltips,
    });
    for (const name of SEGMENT) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
    rerender(<EditorToolbar view={null} tableActive />);
    for (const name of SEGMENT) {
      expect(screen.getByRole("button", { name })).not.toBeNull();
    }
    // And the format buttons never move: the segment is an addition.
    expect(screen.getByRole("button", { name: "Bold" })).not.toBeNull();
  });

  test("all four table-shape verbs wear a glyph of their own", () => {
    // Taste, pinned because it is invisible to every other assertion here:
    // these four differ in two dimensions at once - what they do and which
    // axis they do it to - and the bar now says both. The shared trash can
    // that used to stand in for both deletes said only the first, so a glyph
    // that drifts back to it would break nothing and lose the axis.
    render(<EditorToolbar view={null} tableActive />, { wrapper: Tooltips });
    const glyph = (name: string) =>
      screen
        .getByRole("button", { name })
        .querySelector("svg")
        ?.getAttribute("class");
    const names = [
      "Add row below",
      "Add column after",
      "Delete row",
      "Delete column",
    ];
    const glyphs = names.map(glyph);
    for (const drawn of glyphs) {
      expect(drawn).toBeTruthy();
    }
    expect(new Set(glyphs).size).toBe(4);
    // And no trash can anywhere in the segment: the axis is the half that
    // used to go missing, and it is the half these two now carry.
    for (const drawn of glyphs) {
      expect(drawn).not.toContain("trash");
    }
  });

  /*
   * Every one of the six is pinned to the verb its label promises, and each
   * assertion names a result no OTHER control in the segment produces: a
   * rewiring that swapped two of them - "Delete column" running the row
   * delete, say - would otherwise ship with every gate green.
   */

  test("Add row below edits the buffer", async () => {
    const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
    await press("Add row below");
    expect(docText(v.state)).toBe(
      "Before\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n|  |  |\n\nAfter\n",
    );
  });

  test("Add column after edits the buffer", async () => {
    const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
    await press("Add column after");
    expect(docText(v.state)).toContain("| a | Column | b |");
  });

  test("Delete row takes the caret's row and only that", async () => {
    const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
    await press("Delete row");
    expect(docText(v.state)).toBe(
      "Before\n\n| a | b |\n| --- | --- |\n\nAfter\n",
    );
  });

  test("Delete column takes the caret's column and only that", async () => {
    const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
    await press("Delete column");
    expect(docText(v.state)).toBe("Before\n\n| b |\n| --- |\n| 2 |\n\nAfter\n");
  });

  test("Prettify table pads the whole table", async () => {
    const doc = "| name | n |\n| --- | --- |\n| longer | 2 |\n";
    const v = mount(doc, doc.indexOf("longer"), doc.indexOf("longer"), true);
    await press("Prettify table");
    // Three, not one, for the narrow column: see `tableVerbs.test.ts`, where
    // the rule-width floor this follows from is spelled out.
    expect(docText(v.state)).toContain("| name   | n   |");
  });

  test("the align menu says which alignment each row is", async () => {
    // Taste again, and the reason all four glyphs are hand drawn: the verb
    // acts on a COLUMN, so every mark in this menu is framed as one, and the
    // trigger no longer impersonates its own centre row. Four drawings, four
    // different things said, none of them borrowed from the icon set - a row
    // that drifted back to a bare text-alignment mark would lose the column.
    const user = userEvent.setup();
    mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
    const glyphOf = (element: HTMLElement) =>
      element.querySelector("svg")?.getAttribute("class");
    // Read before opening: an open Radix menu hides the rest of the page from
    // the accessibility tree, trigger included, so afterwards there is no
    // button by that name to ask.
    const trigger = glyphOf(
      screen.getByRole("button", { name: "Align column" }),
    );
    await user.click(screen.getByRole("button", { name: "Align column" }));
    const rows = ["Align left", "Align center", "Align right"].map((name) =>
      glyphOf(screen.getByRole("menuitem", { name })),
    );
    const glyphs = [trigger, ...rows];
    for (const drawn of glyphs) {
      expect(drawn).toBeTruthy();
      // The set's own marks carry its stem; these carry ours, so a swap back
      // to a borrowed glyph reads here rather than in a screenshot.
      expect(drawn).not.toContain("lucide");
      expect(drawn).toContain("crystalline-icon-align-column");
    }
    expect(new Set(glyphs).size).toBe(4);
  });

  /*
   * The align menu is the one place a user-visible label is bound to a typed
   * enum value, so all three mappings are pinned separately: an array that was
   * reordered rather than relabelled would still put the right colons in the
   * wrong place, and only a per-alignment assertion catches that.
   */
  const RULES: [string, string][] = [
    ["Align left", "| :--- | --- |"],
    ["Align center", "| :---: | --- |"],
    ["Align right", "| ---: | --- |"],
  ];
  for (const [label, rule] of RULES) {
    test(`${label} rewrites the caret's rule cell`, async () => {
      const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
      await align(label);
      // The caret's column is the FIRST, so the second rule cell must stay
      // exactly as it was - alignment touches one delimiter cell, no more.
      expect(docText(v.state)).toContain(rule);
    });
  }

  test("appearing moves no focus", () => {
    const v = mount(TABLE_DOC, IN_TABLE);
    v.focus();
    const focused = document.activeElement;
    // The SAME bar redrawn with the segment on, which is what a screen's
    // state does - rendering a second bar would prove nothing about the
    // appearance path.
    flip(true);
    expect(
      screen.getByRole("button", { name: "Add column after" }),
    ).not.toBeNull();
    expect(document.activeElement).toBe(focused);
  });
});

/**
 * The attach verb is drawn only where files can actually go: a surface that
 * hands the bar no handler - the MANIFEST editor - gets no button rather than
 * a button that refuses.
 */
describe("the attach verb", () => {
  /** A live buffer, the way every other test here builds one. */
  function buffer(): EditorView {
    view = new EditorView({
      state: EditorState.create({
        doc: "prose\n",
        selection: EditorSelection.single(5),
        extensions: [...lineSeparatorFor("prose\n"), ...baseExtensions(false)],
      }),
      parent: document.body,
    });
    return view;
  }

  /** The bar over that buffer, with an upload handler on it. */
  function mountWithAttach(onAttach: (files: File[]) => void): RenderResult {
    return render(<EditorToolbar view={buffer()} onAttach={onAttach} />, {
      wrapper: Tooltips,
    });
  }

  test("is absent on a surface that takes no attachments", () => {
    render(<EditorToolbar view={buffer()} />, { wrapper: Tooltips });
    expect(screen.queryByRole("button", { name: "Attach a file" })).toBeNull();
  });

  test("opens a picker filtered to the allowlist and hands the files over", async () => {
    const picked: File[][] = [];
    const { container } = mountWithAttach((files) => {
      picked.push(files);
    });

    const input =
      container.querySelector<HTMLInputElement>('input[type="file"]');
    expect(input).not.toBeNull();
    expect(input?.accept).toBe(ATTACHMENT_ACCEPT);
    expect(input?.accept).toContain(".png");
    expect(input?.accept).not.toContain(".md");
    expect(input?.multiple).toBe(true);

    const file = new File(["x"], "shot.png", { type: "image/png" });
    await userEvent.upload(input as HTMLInputElement, file);

    expect(picked).toEqual([[file]]);
    // Cleared, so picking the same file again still fires a change.
    expect(input?.value).toBe("");
  });

  test("the button reaches the picker", async () => {
    const opened: string[] = [];
    const { container } = mountWithAttach(() => undefined);
    const input =
      container.querySelector<HTMLInputElement>('input[type="file"]');
    input?.addEventListener("click", (event) => {
      // Nothing opens a real dialog in jsdom; what is under test is that the
      // button's press reaches this element at all.
      event.preventDefault();
      opened.push("picker");
    });

    await userEvent.click(
      screen.getByRole("button", { name: "Attach a file" }),
    );

    expect(opened).toEqual(["picker"]);
  });
});
