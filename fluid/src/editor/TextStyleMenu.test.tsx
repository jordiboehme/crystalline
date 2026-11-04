/**
 * The "Aa" text style menu: one radio group for what the caret's line is -
 * a title, a heading or body text. The menu is the control; the writes are
 * `setHeadingLevel`'s, and a Reflection's question comes from the bar.
 */

import { history, undo } from "@codemirror/commands";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test } from "vitest";

import { Tooltips } from "../components/primitives";
import { reflectionAdapter } from "./adapter";
import { EditorToolbar } from "./EditorToolbar";
import { baseExtensions, docText } from "./setup";

let view: EditorView | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
});

function mount(doc: string, anchor: number, reflection = false): EditorView {
  const adapter = reflection ? reflectionAdapter() : undefined;
  view = new EditorView({
    state: EditorState.create({
      doc,
      selection: EditorSelection.single(anchor),
      extensions: [
        EditorState.lineSeparator.of("\n"),
        ...baseExtensions(false, { history: false }),
        history(),
        ...(adapter ? adapter.extensions : []),
      ],
    }),
    parent: document.body,
  });
  render(
    adapter ? (
      <EditorToolbar view={view} adapter={adapter} />
    ) : (
      <EditorToolbar view={view} />
    ),
    { wrapper: Tooltips },
  );
  return view;
}

async function open(): Promise<HTMLElement> {
  await userEvent.click(screen.getByRole("button", { name: "Text style" }));
  return screen.findByRole("menu");
}

async function pick(name: string): Promise<void> {
  await userEvent.click(await screen.findByRole("menuitemradio", { name }));
}

describe("the text style menu", () => {
  test("the trigger is named Text style and shows Aa", () => {
    mount("x", 1);
    expect(
      screen.getByRole("button", { name: "Text style" }),
    ).toHaveTextContent("Aa");
  });

  test("lists Title, Heading 1, Heading 2, Heading 3 and Body in order", async () => {
    mount("x", 1);
    const menu = await open();
    expect(
      within(menu)
        .getAllByRole("menuitemradio")
        .map((item) => item.textContent),
    ).toEqual(["Title", "Heading 1", "Heading 2", "Heading 3", "Body"]);
  });

  test("Title on a plain line writes #", async () => {
    const v = mount("x", 1);
    await open();
    await pick("Title");
    expect(docText(v.state)).toBe("# x");
  });

  test("Heading 2 on a # line writes ###", async () => {
    const v = mount("# x", 3);
    await open();
    await pick("Heading 2");
    expect(docText(v.state)).toBe("### x");
  });

  test("Body takes the heading off", async () => {
    const v = mount("### x", 5);
    await open();
    await pick("Body");
    expect(docText(v.state)).toBe("x");
  });

  test("the current style is checked: ## is Heading 1", async () => {
    mount("## x", 4);
    const menu = await open();
    const checked = within(menu)
      .getAllByRole("menuitemradio")
      .filter((item) => item.getAttribute("aria-checked") === "true");
    expect(checked.map((item) => item.textContent)).toEqual(["Heading 1"]);
  });

  test("a plain line has Body checked", async () => {
    mount("x", 1);
    const menu = await open();
    expect(
      within(menu).getByRole("menuitemradio", { name: "Body" }),
    ).toHaveAttribute("aria-checked", "true");
  });

  test("focus returns to the editor once a style is picked", async () => {
    const v = mount("x", 1);
    await open();
    await pick("Title");
    await waitFor(() => {
      expect(v.contentDOM).toBe(document.activeElement);
    });
  });

  test("the keyboard opens, moves and closes the menu, and focus ends in the editor", async () => {
    const v = mount("x", 1);
    const user = userEvent.setup();
    screen.getByRole("button", { name: "Text style" }).focus();
    await user.keyboard("{Enter}");
    const menu = await screen.findByRole("menu");
    await user.keyboard("{ArrowDown}");
    expect(within(menu).getAllByRole("menuitemradio")).toContain(
      document.activeElement,
    );
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    await waitFor(() => {
      expect(document.activeElement).toBe(v.contentDOM);
    });
  });

  test("the trigger shows its name on hover", async () => {
    mount("x", 1);
    await userEvent.hover(screen.getByRole("button", { name: "Text style" }));
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Text style");
  });

  test("is off while the bar is off", () => {
    view = new EditorView({ state: EditorState.create({ doc: "x" }) });
    render(<EditorToolbar view={view} disabled />, { wrapper: Tooltips });
    expect(screen.getByRole("button", { name: "Text style" })).toBeDisabled();
  });
});

describe("the text style menu on a Reflection page", () => {
  const PAGE =
    "# P\n\n## A\n<!-- r:aaaa0001 -->\n\none\n\n## B\n<!-- r:bbbb0002 -->\n\ntwo\n";

  test("a section heading picked to Heading 2 asks, and joins the section once confirmed, in one undo", async () => {
    const v = mount(PAGE, PAGE.indexOf("## B") + 3, true);
    await open();
    await pick("Heading 2");
    const dialog = await screen.findByRole("alertdialog");
    expect(docText(v.state)).toBe(PAGE);
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Join sections" }),
    );
    expect(docText(v.state)).toBe(
      "# P\n\n## A\n<!-- r:aaaa0001 -->\n\none\n\n### B\n\ntwo\n",
    );
    undo(v);
    expect(docText(v.state)).toBe(PAGE);
  });

  test("Heading 1 on a non-section line writes ## and asks nothing", async () => {
    const v = mount(PAGE, PAGE.indexOf("one"), true);
    await open();
    await pick("Heading 1");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(docText(v.state)).toBe(PAGE.replace("\none\n", "\n## one\n"));
  });

  test("picking the style a section heading already has asks nothing", async () => {
    const v = mount(PAGE, PAGE.indexOf("## B") + 3, true);
    await open();
    await pick("Heading 1");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(docText(v.state)).toBe(PAGE);
  });
});
