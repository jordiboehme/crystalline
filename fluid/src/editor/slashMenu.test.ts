/**
 * The `/` menu: typed at the start of an empty line, it offers the insert
 * registry at the caret, filtered as the person types, and Enter runs the
 * chosen entry in place of the `/query`.
 *
 * Driven through the shared editor surface, the way both editors mount it,
 * and through real keys (`runScopeHandlers`), so the completion keymap and
 * its timing are the ones a person meets.
 */

import { completionStatus, currentCompletions } from "@codemirror/autocomplete";
import { undo } from "@codemirror/commands";
import { Compartment, EditorSelection, EditorState } from "@codemirror/state";
import { EditorView, runScopeHandlers } from "@codemirror/view";
import { QueryClient } from "@tanstack/react-query";
import { waitFor } from "@testing-library/react";
import { afterEach, describe, expect, test } from "vitest";

import type { DocumentAdapter } from "./adapter";
import { engramAdapter, reflectionAdapter } from "./adapter";
import { surfaceExtensions } from "./surface";

let view: EditorView | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
});

const ENGRAM = (): DocumentAdapter =>
  engramAdapter({
    domain: "eng",
    client: new QueryClient(),
    vocab: () => null,
  });

function open(
  doc: string,
  pos: number,
  adapter: DocumentAdapter = ENGRAM(),
): EditorView {
  view = new EditorView({
    state: EditorState.create({
      doc,
      selection: EditorSelection.cursor(pos),
      extensions: [
        EditorState.lineSeparator.of("\n"),
        ...surfaceExtensions({
          adapter,
          dark: false,
          history: true,
          screen: [],
          preview: new Compartment(),
          raw: true,
          images: [],
          uploads: [],
          onTableContext: () => {},
          onImageContext: () => {},
          onElementContext: () => {},
          onFloatingContext: () => {},
        }),
      ],
    }),
    parent: document.body,
  });
  return view;
}

/** Type `text` at the caret, one character at a time, as keys would. */
function type(v: EditorView, text: string): void {
  for (const char of text) {
    const at = v.state.selection.main.head;
    v.dispatch({
      changes: { from: at, insert: char },
      selection: EditorSelection.cursor(at + 1),
      userEvent: "input.type",
    });
  }
}

function press(v: EditorView, key: string): boolean {
  return runScopeHandlers(
    v,
    new KeyboardEvent("keydown", { key, bubbles: true }),
    "editor",
  );
}

function labels(v: EditorView): string[] {
  return currentCompletions(v.state).map((option) => option.label);
}

async function opened(v: EditorView): Promise<void> {
  await waitFor(() => {
    expect(completionStatus(v.state)).toBe("active");
    expect(labels(v).length).toBeGreaterThan(0);
  });
}

/**
 * Enter, once the open list takes keys: it ignores them for its first 75ms
 * (`interactionDelay`), so a key pressed as it appears is not taken by
 * mistake, and Enter would be a new line until then.
 */
async function pick(v: EditorView): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 120));
  expect(press(v, "Enter")).toBe(true);
}

/** Give a popup that would open the time it takes to open. */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 250));
}

function selected(v: EditorView): string {
  const { from, to } = v.state.selection.main;
  return v.state.sliceDoc(from, to);
}

describe("the slash menu", () => {
  test("draws the bare list in Callout, Blocks, Inline, Diagram order", async () => {
    const v = open("Intro.\n\n", 8);
    type(v, "/");
    await opened(v);
    const names = currentCompletions(v.state).map((entry) => {
      const { section } = entry;
      return typeof section === "string" ? section : section?.name;
    });
    expect([...new Set(names)]).toEqual([
      "Callout",
      "Blocks",
      "Inline",
      "Diagram",
    ]);
    // Every Diagram option comes after every Inline option.
    expect(names.lastIndexOf("Inline")).toBeLessThan(names.indexOf("Diagram"));
  });

  test("offers the whole list on a bare slash", async () => {
    const v = open("Intro.\n\n", 8);
    type(v, "/");
    await opened(v);
    expect(labels(v)).toContain("Warning");
    expect(labels(v)).toContain("Flowchart");
    expect(labels(v)).toContain("Task list");
  });

  test("/war puts Warning first and Enter applies it in place of /war", async () => {
    const v = open("Intro.\n\n", 8);
    type(v, "/war");
    await opened(v);
    expect(labels(v)[0]).toBe("Warning");
    await pick(v);
    expect(v.state.doc.toString()).toBe("Intro.\n\n> [!WARNING]\n> Text\n");
    expect(selected(v)).toBe("Text");
    expect(completionStatus(v.state)).toBeNull();
  });

  test("one undo puts the typed /war back", async () => {
    const v = open("Intro.\n\n", 8);
    type(v, "/war");
    await opened(v);
    await pick(v);
    undo(v);
    expect(v.state.doc.toString()).toBe("Intro.\n\n/war");
  });

  test("filters by the words an element is known by", async () => {
    const v = open("", 0);
    type(v, "/mermaid");
    await opened(v);
    expect(labels(v)).toContain("Flowchart");
    expect(labels(v)).not.toContain("Warning");
  });

  test("a slash in the middle of text opens nothing", async () => {
    const v = open("Some text ", 10);
    type(v, "/war");
    await settle();
    expect(completionStatus(v.state)).toBeNull();
  });

  test("Escape closes it and leaves the text", async () => {
    const v = open("", 0);
    type(v, "/war");
    await opened(v);
    expect(press(v, "Escape")).toBe(true);
    expect(completionStatus(v.state)).toBeNull();
    expect(v.state.doc.toString()).toBe("/war");
  });

  test("a space after the slash closes it and leaves the text", async () => {
    const v = open("", 0);
    type(v, "/");
    await opened(v);
    type(v, " ");
    await settle();
    expect(completionStatus(v.state)).toBeNull();
    expect(v.state.doc.toString()).toBe("/ ");
  });

  test("a space that ends every match closes it and leaves the text", async () => {
    const v = open("", 0);
    type(v, "/war");
    await opened(v);
    type(v, " ");
    await settle();
    expect(currentCompletions(v.state)).toHaveLength(0);
    expect(v.state.doc.toString()).toBe("/war ");
  });

  test("stays quiet inside a code fence", async () => {
    const v = open("```\n\n```\n", 4);
    type(v, "/usr");
    await settle();
    expect(completionStatus(v.state)).toBeNull();
  });

  test("stays quiet inside a code fence in the Reflection editor", async () => {
    const doc = "## One\n<!-- r:aaaa1111 -->\n\n```\n\n```\n";
    const v = open(doc, doc.indexOf("```\n") + 4, reflectionAdapter());
    type(v, "/usr");
    await settle();
    expect(completionStatus(v.state)).toBeNull();
  });

  test("stays quiet inside a comment that runs over lines", async () => {
    const v = open("<!--\n\n-->\n", "<!--\n".length);
    type(v, "/note");
    await settle();
    expect(completionStatus(v.state)).toBeNull();
  });

  test("stays quiet on a line that lazily continues a callout", async () => {
    const doc = "> [!NOTE]\n> Read this\n";
    const v = open(doc, doc.length - 1);
    type(v, "\n/code");
    await settle();
    expect(completionStatus(v.state)).toBeNull();
  });

  test("works in the Reflection editor too, below a section", async () => {
    const v = open(
      "## One\n<!-- r:aaaa1111 -->\n\n",
      "## One\n<!-- r:aaaa1111 -->\n\n".length,
      reflectionAdapter(),
    );
    type(v, "/code");
    await opened(v);
    expect(labels(v)[0]).toBe("Code block");
    await pick(v);
    expect(v.state.doc.toString()).toBe(
      "## One\n<!-- r:aaaa1111 -->\n\n```\n\n```\n",
    );
  });
});
