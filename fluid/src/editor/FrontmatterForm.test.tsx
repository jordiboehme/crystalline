/**
 * The form is a view over the buffer, and these tests hold it to that: a real
 * EditorView, real dispatches, and assertions on the document that comes out.
 *
 * Two properties get their own tests because they are the ones a plausible
 * implementation gets wrong. A hand edit in the text has to reach the fields,
 * since the buffer is the only copy of a value. And an edit computed on the
 * buffer's own string has to be translated through the document's line API
 * before it is dispatched, because a CRLF buffer counts each break as one
 * position while the string counts it as two.
 */

import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";

import type { Vocabulary } from "../api/vocabulary";
import CmEditor from "./CmEditor";
import { FrontmatterForm } from "./FrontmatterForm";
import { lineSeparatorFor } from "./setup";

const DOC =
  "---\ntitle: Alpha\nstatus: stable\ntags:\n  - eng\nvalid_from: 2026-01-01\n---\n\nBody.\n";

/** Every view a test parented on the body, torn down between tests. */
const views: EditorView[] = [];

afterEach(() => {
  while (views.length > 0) {
    views.pop()?.destroy();
  }
});

function mounted(vocabulary: Vocabulary | null = null) {
  const view = new EditorView({
    state: EditorState.create({
      doc: DOC,
      extensions: lineSeparatorFor(DOC),
    }),
    parent: document.body,
  });
  views.push(view);
  render(<FrontmatterForm doc={DOC} view={view} vocabulary={vocabulary} />);
  return view;
}

/** The view behind the one buffer a `Live` test rendered. */
function liveView(): EditorView {
  const content = document.querySelector(".cm-content");
  expect(content).not.toBeNull();
  const view = EditorView.findFromDOM(content as HTMLElement);
  expect(view).not.toBeNull();
  return view!;
}

/**
 * The wiring the editor screen has: one buffer, and a form fed by whatever
 * that buffer currently says. Nothing here holds a second copy of a value.
 */
function Live({ content }: { content: string }) {
  const [doc, setDoc] = useState(content);
  const [view, setView] = useState<EditorView | null>(null);
  return (
    <>
      <CmEditor
        initialDoc={content}
        extensions={lineSeparatorFor(content)}
        ariaLabel="Engram source"
        onReady={setView}
        onDocChanged={setDoc}
      />
      <FrontmatterForm doc={doc} view={view} vocabulary={null} />
    </>
  );
}

describe("the frontmatter form", () => {
  it("shows the fields the block carries, an absent bound as its own state", () => {
    mounted();
    expect(screen.getByLabelText("Status")).toHaveValue("stable");
    expect(screen.getByLabelText("Valid from")).toHaveValue("2026-01-01");
    // No date is not a blank field: it is the answer, and it says so.
    expect(screen.queryByLabelText("Valid to")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Valid to: Forever" }),
    ).toBeInTheDocument();
  });

  it("draws the unbounded state as a box of its own, dashed rather than filled", () => {
    mounted();
    // The state was legible only as a word floating in the rail. It has the
    // field's box now, dashed: an absent bound is an answer, and a solid rule
    // would draw it as the empty field it is not. Same `h-8` as the picker
    // that replaces it, so pressing it moves nothing.
    const ghost = screen.getByRole("button", { name: "Valid to: Forever" });
    expect(ghost.className).toContain("border-dashed");
    expect(ghost.className).toContain("h-8");
  });

  it("an edit dispatches a single-line change into the buffer", async () => {
    const view = mounted();
    const status = screen.getByLabelText("Status");
    await userEvent.clear(status);
    await userEvent.type(status, "draft");
    // Blur commits (change event).
    await userEvent.tab();
    expect(view.state.doc.toString()).toContain("status: draft");
    expect(view.state.doc.toString()).toContain("  - eng");
  });

  it("clearing a date removes the key rather than writing a sentinel", async () => {
    const view = mounted();
    await userEvent.click(
      screen.getByRole("button", { name: "Clear to always" }),
    );
    expect(view.state.doc.toString()).not.toContain("valid_from");
  });

  it("round-trips the upper bound between forever and a date, both ways, twice", async () => {
    render(<Live content={DOC} />);
    await screen.findByLabelText("Engram source");
    const view = liveView();

    // Forever is what an absent upper bound says, and it is one click from a
    // picker that already has the keyboard.
    await userEvent.click(
      screen.getByRole("button", { name: "Valid to: Forever" }),
    );
    const picker = screen.getByLabelText("Valid to");
    expect(picker).toHaveFocus();
    // Opening the picker is not an answer: nothing is written until a date is.
    expect(view.state.sliceDoc()).toBe(DOC);
    // And the way back is open before any date is picked, so the swap is
    // never a one-way door.
    expect(
      screen.getByRole("button", { name: "Clear to forever" }),
    ).toBeInTheDocument();

    fireEvent.change(picker, { target: { value: "2027-02-03" } });
    expect(view.state.sliceDoc()).toContain("valid_to: 2027-02-03");
    // A picked date keeps the way back visible rather than closing it.
    await userEvent.click(
      await screen.findByRole("button", { name: "Clear to forever" }),
    );
    expect(view.state.sliceDoc()).toBe(DOC);
    expect(screen.queryByLabelText("Valid to")).toBeNull();

    // Twice: the state the clear returned to is the state that swaps again.
    await userEvent.click(
      screen.getByRole("button", { name: "Valid to: Forever" }),
    );
    expect(screen.getByLabelText("Valid to")).toHaveFocus();
  });

  it("round-trips the lower bound between a date and always", async () => {
    render(<Live content={DOC} />);
    await screen.findByLabelText("Engram source");
    const view = liveView();

    await userEvent.click(
      screen.getByRole("button", { name: "Clear to always" }),
    );
    expect(view.state.sliceDoc()).not.toContain("valid_from");
    expect(screen.queryByLabelText("Valid from")).toBeNull();

    await userEvent.click(
      screen.getByRole("button", { name: "Valid from: Always" }),
    );
    const picker = screen.getByLabelText("Valid from");
    expect(picker).toHaveFocus();
    fireEvent.change(picker, { target: { value: "2026-01-01" } });
    expect(view.state.sliceDoc()).toContain("valid_from: 2026-01-01");
  });

  it("keyboard-editing a date leaves its line where it is, never removing it", async () => {
    render(<Live content={DOC} />);
    await screen.findByLabelText("Engram source");
    const view = liveView();
    const lineOf = (text: string, key: string) =>
      text.split("\n").findIndex((line) => line.startsWith(key));
    const before = lineOf(view.state.sliceDoc(), "valid_from:");

    // A date control reports every partly entered date as the empty string.
    // That is somebody typing, not somebody asking for the bound to go.
    fireEvent.change(screen.getByLabelText("Valid from"), {
      target: { value: "" },
    });
    expect(view.state.sliceDoc()).toContain("valid_from: 2026-01-01");

    fireEvent.change(screen.getByLabelText("Valid from"), {
      target: { value: "2027-02-03" },
    });
    const after = view.state.sliceDoc();
    // One value, on the line it was already on: no removal, no relocation to
    // the bottom of the block, and no other byte touched.
    expect(after).toBe(DOC.replace("2026-01-01", "2027-02-03"));
    expect(lineOf(after, "valid_from:")).toBe(before);
  });

  it("setting an absent bound adds its line before the closing fence", async () => {
    render(<Live content={DOC} />);
    await screen.findByLabelText("Engram source");
    const view = liveView();
    await userEvent.click(
      screen.getByRole("button", { name: "Valid to: Forever" }),
    );
    fireEvent.change(screen.getByLabelText("Valid to"), {
      target: { value: "2027-02-03" },
    });
    expect(view.state.sliceDoc()).toBe(
      DOC.replace("---\n\nBody.", "valid_to: 2027-02-03\n---\n\nBody."),
    );
    // And the clear control, the one removal path, takes it away again.
    await userEvent.click(
      await screen.findByRole("button", { name: "Clear to forever" }),
    );
    expect(view.state.sliceDoc()).toBe(DOC);
  });

  it("adds and removes tags as a block list", async () => {
    render(<Live content={DOC} />);
    await screen.findByLabelText("Engram source");
    const view = liveView();
    await userEvent.type(screen.getByLabelText("Add tag"), "editor{Enter}");
    expect(view.state.sliceDoc()).toContain("tags:\n  - eng\n  - editor\n");
    // The chip for the new tag is there because the buffer says so, not
    // because the form remembered adding it.
    await userEvent.click(
      await screen.findByRole("button", { name: "Remove tag eng" }),
    );
    expect(view.state.sliceDoc()).toContain("tags:\n  - editor\n");
    expect(view.state.sliceDoc()).not.toContain("- eng\n");
  });

  it("offers the domain's own tags as suggestions", () => {
    mounted({
      tags: [
        { name: "eng", engrams: 4 },
        { name: "deep", engrams: 2 },
      ],
      categories: [],
      relationTypes: [],
    });
    const suggestions = screen
      .getByLabelText("Add tag")
      .getAttribute("list") as string;
    const options = document
      .getElementById(suggestions)
      ?.querySelectorAll("option");
    expect([...(options ?? [])].map((option) => option.value)).toEqual([
      "eng",
      "deep",
    ]);
  });

  it("offers recommended statuses and types without demanding them", async () => {
    const view = mounted();

    // Opened on a status it already holds, the field still shows the whole
    // vocabulary - and says what each word is for, which is the point.
    await userEvent.click(screen.getByLabelText("Status"));
    const statuses = screen
      .getAllByRole("option")
      .map((option) => option.dataset.value);
    expect(statuses).toContain("deprecated");
    expect(
      screen.getByRole("option", { name: /superseded/ }),
    ).toHaveTextContent("replaced by a newer engram");

    // A value nobody recommends is written exactly as typed.
    const status = screen.getByLabelText("Status");
    await userEvent.clear(status);
    await userEvent.type(status, "brewing");
    await userEvent.tab();
    expect(view.state.doc.toString()).toContain("status: brewing");
  });

  it("writes the value that was picked out of the suggestions", async () => {
    const view = mounted();
    await userEvent.click(screen.getByLabelText("Type"));
    expect(screen.getByRole("option", { name: /runbook/ })).toHaveTextContent(
      "the steps to take when something happens",
    );
    await userEvent.click(screen.getByRole("option", { name: /^guide/ }));

    expect(view.state.doc.toString()).toContain("type: guide");
    expect(screen.getByLabelText("Type")).toHaveValue("guide");
  });

  it("follows a hand edit in the text rather than holding its own copy", async () => {
    render(<Live content={DOC} />);
    await screen.findByLabelText("Engram source");
    const view = liveView();
    expect(screen.getByLabelText("Status")).toHaveValue("stable");

    const at = DOC.indexOf("stable");
    view.dispatch({
      changes: { from: at, to: at + "stable".length, insert: "legacy" },
    });
    await waitFor(() => {
      expect(screen.getByLabelText("Status")).toHaveValue("legacy");
    });
  });

  it("writes into a CRLF buffer on the line the field names", async () => {
    const crlf = DOC.replace(/\n/g, "\r\n");
    render(<Live content={crlf} />);
    await screen.findByLabelText("Engram source");
    await waitFor(() => {
      expect(screen.getByLabelText("Valid from")).toHaveValue("2026-01-01");
    });
    await userEvent.click(
      screen.getByRole("button", { name: "Clear to always" }),
    );
    // The whole file, its own endings intact, minus exactly one line.
    expect(liveView().state.sliceDoc()).toBe(
      crlf.replace("valid_from: 2026-01-01\r\n", ""),
    );
  });

  it("says so when the document has no frontmatter block", () => {
    render(<FrontmatterForm doc={"# Alpha\n"} view={null} vocabulary={null} />);
    expect(screen.getByText(/no frontmatter block/i)).toBeInTheDocument();
    expect(screen.queryByLabelText("Status")).not.toBeInTheDocument();
  });
});
