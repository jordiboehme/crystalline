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
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test } from "vitest";

import { ATTACHMENT_ACCEPT } from "../api/files";
import { Tooltips } from "../components/primitives";
import { parsedState } from "../test/parse";
import { JOIN_SECTIONS, reflectionAdapter } from "./adapter";
import { AttachButton } from "./AttachButton";
import { EditorToolbar } from "./EditorToolbar";
import { frontmatterFold } from "./frontmatterFold";
import { baseExtensions, docText, lineSeparatorFor } from "./setup";
import { MERMAID_SKELETON } from "./toolbar";
import type { Format } from "./marks/formatting";
import {
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  typeText,
} from "../test/markKit";

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

/** Open Insert > Diagram, the diagram menu. */
async function openDiagrams(): Promise<HTMLElement> {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Insert" }));
  await user.click(await screen.findByRole("menuitem", { name: "Diagram" }));
  return screen.findByRole("menu", { name: "Diagram" });
}

/** Open the diagram menu and pick one of its starters. */
async function pickDiagram(label: string): Promise<void> {
  const user = userEvent.setup();
  const diagrams = await openDiagrams();
  await user.click(within(diagrams).getByRole("menuitem", { name: label }));
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

  test("one Insert button stands where the diagram and code buttons were", () => {
    mount("prose\n", 5);
    const names = screen
      .getAllByRole("button")
      .map((button) => button.getAttribute("aria-label") ?? button.textContent);
    expect(names).not.toContain("Insert diagram");
    expect(names).not.toContain("Insert code block");
    // Right after the table picker, the other insert verb that asks first.
    expect(names.indexOf("Insert")).toBe(names.indexOf("Insert table") + 1);
  });

  test("Insert > Code block inserts a bare fence", async () => {
    const v = mount("prose\n", 5);
    await press("Insert");
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Code block" }),
    );
    // A blank line, the bare fence, its body line and the close - dropped
    // below the cursor's line, which keeps its own trailing break.
    expect(docText(v.state)).toBe("prose\n\n```\n\n```\n\n");
  });
});

describe("the diagram menu", () => {
  test("offers sixteen starters under three group labels", async () => {
    mount("prose\n", 5);
    const diagrams = await openDiagrams();
    // Sixteen by count rather than by reading the starters module back at
    // itself: a menu that quietly lost an entry would agree with the module
    // and disagree with the decision that named all sixteen.
    expect(within(diagrams).getAllByRole("menuitem")).toHaveLength(16);
    for (const label of ["Everyday", "Planning and product", "Technical"]) {
      expect(within(diagrams).getByText(label)).not.toBeNull();
    }
    // The labels are headings, not choices: only the sixteen are selectable.
    expect(
      within(diagrams).getByRole("menuitem", { name: "User journey" }),
    ).not.toBeNull();
    expect(
      within(diagrams).getByRole("menuitem", { name: "Radar" }),
    ).not.toBeNull();
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

  test("the keyboard route to a flowchart lands the old diagram button's text", async () => {
    // Decision 16, end to end: Enter opens Insert on Callout, Down reaches
    // Diagram, Right opens it on its first starter, and Enter takes the
    // flowchart - the same bytes the old one-shape button inserted.
    const v = mount("prose\n", 5);
    const user = userEvent.setup();
    screen.getByRole("button", { name: "Insert" }).focus();
    await user.keyboard("{Enter}");
    await screen.findByRole("menuitem", { name: "Callout" });
    await user.keyboard("{ArrowDown}{ArrowRight}");
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
    const diagrams = await openDiagrams();
    for (const label of ["Everyday", "Planning and product", "Technical"]) {
      expect(
        within(diagrams).getByRole("group", { name: label }),
      ).not.toBeNull();
    }
  });

  test("Escape closes the menu and gives the buffer back", async () => {
    // The other half of what the close handler promises. The select path is
    // pinned below; a refactor that moved `view.focus()` into the item handler
    // would keep that one green and strand the caret on the trigger here.
    const v = mount("prose\n", 5);
    const user = userEvent.setup();
    await openDiagrams();
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

  test("a drawn grid hands the verbs to its own menu (Raw takes them back)", () => {
    const { rerender } = render(
      <EditorToolbar view={null} tableActive tableDrawn />,
      { wrapper: Tooltips },
    );
    for (const name of SEGMENT) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
    rerender(<EditorToolbar view={null} tableActive tableDrawn raw />);
    for (const name of SEGMENT) {
      expect(screen.getByRole("button", { name })).not.toBeNull();
    }
  });

  test("a table the grid does not draw keeps the verbs", () => {
    render(<EditorToolbar view={null} tableActive tableDrawn={false} />, {
      wrapper: Tooltips,
    });
    for (const name of SEGMENT) {
      expect(screen.getByRole("button", { name })).not.toBeNull();
    }
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

  test("the attach button is the same control on its own", async () => {
    const picked: File[][] = [];
    const { container } = render(
      <AttachButton
        onAttach={(files) => {
          picked.push(files);
        }}
      />,
      { wrapper: Tooltips },
    );
    const input =
      container.querySelector<HTMLInputElement>('input[type="file"]');
    expect(input?.accept).toBe(ATTACHMENT_ACCEPT);
    expect(input?.multiple).toBe(true);
    const opened: string[] = [];
    // Once: nothing opens a real dialog in jsdom, and the upload below clicks
    // the input too.
    input?.addEventListener(
      "click",
      (event) => {
        event.preventDefault();
        opened.push("picker");
      },
      { once: true },
    );

    const button = screen.getByRole("button", { name: "Attach a file" });
    expect(button).toBeEnabled();
    await userEvent.click(button);
    const file = new File(["x"], "shot.png", { type: "image/png" });
    await userEvent.upload(input as HTMLInputElement, file);

    expect(opened).toEqual(["picker"]);
    expect(picked).toEqual([[file]]);
    expect(input?.value).toBe("");
  });

  test("the attach button on its own can be turned off", () => {
    render(<AttachButton onAttach={() => undefined} disabled />, {
      wrapper: Tooltips,
    });
    expect(
      screen.getByRole("button", { name: "Attach a file" }),
    ).toBeDisabled();
  });
});

/**
 * The image-format menu is context too: it is drawn only where the caret is
 * actually on an attachment image, and every row of it edits the target of
 * that one reference. What the verbs do is `imageVerbs.test.ts`'s job; what is
 * under test here is that each label reaches the verb it promises.
 */
describe("the image-format menu", () => {
  const IMAGE_DOC = "Before\n\n![Shot](assets/2026/08/shot.png)\n";
  const ON_IMAGE = IMAGE_DOC.indexOf("![Shot]") + 2;

  /** A buffer with the caret on that image, and the bar above it. */
  function mountOnImage(doc = IMAGE_DOC, at = ON_IMAGE): EditorView {
    view = new EditorView({
      state: parsedState(
        EditorState.create({
          doc,
          selection: EditorSelection.single(at),
          extensions: [...lineSeparatorFor(doc), ...baseExtensions(false)],
        }),
      ),
      parent: document.body,
    });
    bar = render(<EditorToolbar view={view} imageActive />, {
      wrapper: Tooltips,
    });
    return view;
  }

  /** Open the menu and pick one of its rows. */
  async function pickFormat(label: string): Promise<void> {
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Image format" }));
    await user.click(await screen.findByRole("menuitem", { name: label }));
  }

  test("is there only while the caret is on an image", () => {
    const { rerender } = render(<EditorToolbar view={null} />, {
      wrapper: Tooltips,
    });
    expect(screen.queryByRole("button", { name: "Image format" })).toBeNull();
    rerender(<EditorToolbar view={null} imageActive />);
    expect(screen.getByRole("button", { name: "Image format" })).not.toBeNull();
  });

  test("every row wears a glyph of its own, none of them borrowed", async () => {
    const user = userEvent.setup();
    mountOnImage();
    const glyphOf = (element: HTMLElement) =>
      element.querySelector("svg")?.getAttribute("class");
    // Read before opening: an open Radix menu hides the rest of the page from
    // the accessibility tree, trigger included.
    const trigger = glyphOf(
      screen.getByRole("button", { name: "Image format" }),
    );
    await user.click(screen.getByRole("button", { name: "Image format" }));
    const rows = [
      "Centered",
      "Full width",
      "Float left",
      "Float right",
      "Width 25%",
      "Width 50%",
      "Width 75%",
    ].map((name) => glyphOf(screen.getByRole("menuitem", { name })));
    const glyphs = [trigger, ...rows];
    for (const drawn of glyphs) {
      expect(drawn).toBeTruthy();
      expect(drawn).not.toContain("lucide");
      expect(drawn).toContain("crystalline-icon-image");
    }
    expect(new Set(glyphs).size).toBe(8);
  });

  const ROWS: [string, string][] = [
    ["Full width", "![Shot](assets/2026/08/shot.png#full)"],
    ["Float left", "![Shot](assets/2026/08/shot.png#left)"],
    ["Float right", "![Shot](assets/2026/08/shot.png#right)"],
    ["Width 25%", "![Shot](assets/2026/08/shot.png#w=25%)"],
    ["Width 50%", "![Shot](assets/2026/08/shot.png#w=50%)"],
    ["Width 75%", "![Shot](assets/2026/08/shot.png#w=75%)"],
  ];
  for (const [label, written] of ROWS) {
    test(`${label} rewrites the target under the caret`, async () => {
      const v = mountOnImage();
      await pickFormat(label);
      expect(docText(v.state)).toContain(written);
    });
  }

  test("Centered puts the reference back to the bare path an upload wrote", async () => {
    const doc = "![Shot](assets/2026/08/shot.png#left,w=25%)\n";
    const v = mountOnImage(doc, 2);
    await pickFormat("Centered");
    expect(docText(v.state)).toBe("![Shot](assets/2026/08/shot.png)\n");
  });

  test("a width composes with the placement rather than replacing it", async () => {
    const v = mountOnImage();
    await pickFormat("Float right");
    await pickFormat("Width 50%");
    expect(docText(v.state)).toContain(
      "![Shot](assets/2026/08/shot.png#right,w=50%)",
    );
  });

  test("the caret goes back to the buffer when the menu closes", async () => {
    const v = mountOnImage();
    await pickFormat("Float left");
    await waitFor(() => {
      expect(v.hasFocus).toBe(true);
    });
  });
});

describe("the callout and status segments", () => {
  const COLOURS = ["Grey", "Blue", "Green", "Yellow", "Red", "Purple"];
  const CALLOUT_DOC = "> [!NOTE]\n> body\n";
  const STATUS_DOC = "Build :status[Done]{green} today\n";

  /** A buffer with the caret at `at`, and the bar drawn for its element. */
  function mountOn(
    doc: string,
    at: number,
    props: Partial<Parameters<typeof EditorToolbar>[0]>,
  ): EditorView {
    view = new EditorView({
      state: parsedState(
        EditorState.create({
          doc,
          selection: EditorSelection.single(at),
          extensions: [...lineSeparatorFor(doc), ...baseExtensions(false)],
        }),
      ),
      parent: document.body,
    });
    bar = render(<EditorToolbar view={view} {...props} />, {
      wrapper: Tooltips,
    });
    return view;
  }

  test("neither is there outside a callout or a label", () => {
    render(<EditorToolbar view={null} />, { wrapper: Tooltips });
    expect(screen.queryByRole("button", { name: /callout kind/i })).toBeNull();
    expect(screen.queryByRole("button", { name: "Starts open" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Starts closed" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Does not fold" })).toBeNull();
    for (const name of COLOURS) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
  });

  test("in a callout, the kind and the fold appear, and no colours", () => {
    render(
      <EditorToolbar
        view={null}
        calloutActive={{ callout: "warning", fold: "closed" }}
      />,
      { wrapper: Tooltips },
    );
    const kind = screen.getByRole("button", { name: /callout kind/i });
    // The visible word is part of the name.
    expect(kind.textContent).toContain("Warning");
    expect(
      screen
        .getByRole("button", { name: "Starts closed" })
        .getAttribute("aria-pressed"),
    ).toBe("true");
    expect(
      screen
        .getByRole("button", { name: "Starts open" })
        .getAttribute("aria-pressed"),
    ).toBe("false");
    for (const name of COLOURS) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
    // An addition: the format buttons stay where they were.
    expect(screen.getByRole("button", { name: "Bold" })).not.toBeNull();
  });

  test("in a status label, six colours named by their word appear", () => {
    render(<EditorToolbar view={null} statusActive="green" />, {
      wrapper: Tooltips,
    });
    for (const name of COLOURS) {
      const button = screen.getByRole("button", { name });
      expect(button.getAttribute("aria-pressed")).toBe(
        name === "Green" ? "true" : "false",
      );
    }
    expect(screen.queryByRole("button", { name: /callout kind/i })).toBeNull();
  });

  test("the kind menu rewrites the kind and hands the caret back", async () => {
    const user = userEvent.setup();
    const v = mountOn(CALLOUT_DOC, CALLOUT_DOC.indexOf("body"), {
      calloutActive: { callout: "note", fold: null },
    });
    await user.click(screen.getByRole("button", { name: /callout kind/i }));
    // Named by its trigger, as Radix names a menu.
    const menu = await screen.findByRole("menu", { name: /callout kind/i });
    expect(
      within(menu)
        .getByRole("menuitemradio", { name: "Note" })
        .getAttribute("aria-checked"),
    ).toBe("true");
    await user.click(within(menu).getByRole("menuitemradio", { name: "Tip" }));
    expect(docText(v.state)).toBe("> [!TIP]\n> body\n");
    await waitFor(() => {
      expect(v.hasFocus).toBe(true);
    });
  });

  test("the fold buttons write the sign", async () => {
    const v = mountOn(CALLOUT_DOC, CALLOUT_DOC.indexOf("body"), {
      calloutActive: { callout: "note", fold: null },
    });
    await press("Starts closed");
    expect(docText(v.state)).toBe("> [!NOTE]-\n> body\n");
  });

  test("Does not fold takes the sign out, and a details callout does not offer it", async () => {
    const doc = "> [!NOTE]-\n> body\n";
    const v = mountOn(doc, doc.indexOf("body"), {
      calloutActive: { callout: "note", fold: "closed" },
    });
    await press("Does not fold");
    expect(docText(v.state)).toBe("> [!NOTE]\n> body\n");
    bar?.rerender(
      <EditorToolbar
        view={null}
        calloutActive={{ callout: "details", fold: null }}
      />,
    );
    expect(screen.queryByRole("button", { name: "Does not fold" })).toBeNull();
    expect(
      screen
        .getByRole("button", { name: "Starts closed" })
        .getAttribute("aria-pressed"),
    ).toBe("true");
  });

  test("a colour button rewrites the label's colour", async () => {
    const v = mountOn(STATUS_DOC, STATUS_DOC.indexOf("Done"), {
      statusActive: "green",
    });
    await press("Purple");
    expect(docText(v.state)).toBe("Build :status[Done]{purple} today\n");
  });
});

describe("a heading change the document asks about first", () => {
  const PAGE =
    "# P\n\n## A\n<!-- r:aaaa0001 -->\n\none\n\n## B\n<!-- r:bbbb0002 -->\n\ntwo\n";

  /** A reflection buffer with the caret on `## B` and the bar above it. */
  function mountPage(): EditorView {
    const adapter = reflectionAdapter();
    const at = PAGE.indexOf("## B") + 3;
    view = new EditorView({
      state: EditorState.create({
        doc: PAGE,
        selection: EditorSelection.single(at),
        extensions: [
          EditorState.lineSeparator.of("\n"),
          ...baseExtensions(false, { history: false }),
          ...adapter.extensions,
        ],
      }),
      parent: document.body,
    });
    render(<EditorToolbar view={view} adapter={adapter} />, {
      wrapper: Tooltips,
    });
    return view;
  }

  async function pickHeading(name: string): Promise<void> {
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Text style" }));
    await user.click(await screen.findByRole("menuitemradio", { name }));
  }

  test("says what joining does and changes nothing until it is confirmed", async () => {
    const v = mountPage();
    await pickHeading("Heading 2");
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent(JOIN_SECTIONS);
    expect(docText(v.state)).toBe(PAGE);
    await waitFor(() => {
      expect(dialog).toContainElement(document.activeElement as HTMLElement);
    });
    await userEvent.click(
      screen.getByRole("button", { name: "Join sections" }),
    );
    expect(docText(v.state)).toBe(
      "# P\n\n## A\n<!-- r:aaaa0001 -->\n\none\n\n### B\n\ntwo\n",
    );
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await waitFor(() => {
      expect(v.hasFocus).toBe(true);
    });
  });

  test("Cancel leaves the text as it was and the caret in the buffer", async () => {
    const v = mountPage();
    await pickHeading("Body");
    await screen.findByRole("alertdialog");
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(docText(v.state)).toBe(PAGE);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    await waitFor(() => {
      expect(v.hasFocus).toBe(true);
    });
  });

  test("a heading change that splits no section goes ahead without a question", async () => {
    const v = mountPage();
    act(() => {
      v.dispatch({ selection: { anchor: PAGE.indexOf("one") } });
    });
    await pickHeading("Heading 1");
    expect(docText(v.state)).toContain("\n## one\n");
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});

describe("the pressed format buttons", () => {
  afterEach(cleanupMarkViews);

  /** A hidden-mode buffer with the bar drawing what the buffer reports. */
  function pressedBar(doc: string): EditorView {
    let formats: ReadonlySet<Format> = new Set();
    let redraw: ((formats: ReadonlySet<Format>) => void) | null = null;
    const v = markView(doc, {
      onFormatState: (next) => {
        formats = next as ReadonlySet<Format>;
        redraw?.(formats);
      },
    });
    const ui = (now: ReadonlySet<Format>) => (
      <EditorToolbar view={v} formats={now} />
    );
    const result = render(ui(formats), { wrapper: Tooltips });
    redraw = (now) => {
      act(() => {
        result.rerender(ui(now));
      });
    };
    return v;
  }

  const pressed = (name: string) =>
    screen.getByRole("button", { name }).getAttribute("aria-pressed");

  test("Bold is pressed with the caret in bold, not outside it, and after Mod-b", () => {
    const v = pressedBar("a **bold** c");
    act(() => {
      setCaret(v, 5);
    });
    expect(pressed("Bold")).toBe("true");
    expect(pressed("Italic")).toBe("false");
    act(() => {
      setCaret(v, 1);
    });
    expect(pressed("Bold")).toBe("false");
    act(() => {
      keys(v, "Mod-b");
    });
    expect(pressed("Bold")).toBe("true");
  });

  test("with no formats told (the MANIFEST), the buttons are not toggles", () => {
    mount("a **b** c", 5);
    expect(
      screen.getByRole("button", { name: "Bold" }).hasAttribute("aria-pressed"),
    ).toBe(false);
  });

  test("Bold with no selection writes nothing until a letter is typed", async () => {
    const v = pressedBar("ab");
    act(() => {
      setCaret(v, 2);
    });
    await press("Bold");
    expect(docText(v.state)).toBe("ab");
    expect(pressed("Bold")).toBe("true");
    act(() => {
      typeText(v, "c");
    });
    expect(docText(v.state)).toBe("ab**c**");
  });

  test("Bold on a selection formats the visible characters", async () => {
    const v = pressedBar("ab **cd** ef");
    act(() => {
      v.dispatch({ selection: EditorSelection.range(1, 6) });
    });
    await press("Bold");
    expect(docText(v.state)).toBe("a**b cd** ef");
  });
});
