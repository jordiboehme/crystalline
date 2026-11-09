// NO vi.mock("mermaid") here: the verdicts come from mermaid 12.2.0's real db.

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, test } from "vitest";

import { defined } from "../../test/assert";
import { parsedState } from "../../test/parse";
import { baseExtensions } from "../setup";
import { drawnDiagrams } from "./draws";
import { diagramExtension } from "./index";
import { requestVerdict } from "./verdict";

const FIXTURES = import.meta.glob<string>(
  [
    "../fixtures/diagrams/flowchart/f*.mmd",
    "../fixtures/diagrams/fallback/*.mmd",
    "../fixtures/diagrams/sequence/s01-*.mmd",
  ],
  { query: "?raw", import: "default", eager: true },
);

const bodies = Object.entries(FIXTURES).map(
  ([path, text]) =>
    [path.slice(path.lastIndexOf("/") + 1), text.replace(/\n$/, "")] as const,
);

let view: EditorView | null = null;

/** The caret sits after the document: a fence the caret is in is held (R52). */
function mount(doc: string): EditorView {
  const text = `${doc}\n\nend`;
  view = new EditorView({
    state: parsedState(
      EditorState.create({
        doc: text,
        selection: EditorSelection.cursor(text.length),
        extensions: [...baseExtensions(false), diagramExtension()],
      }),
    ),
    parent: document.body,
  });
  return view;
}

const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;

afterEach(() => {
  view?.destroy();
  view = null;
});

/** Waits until the verdict of `body` is in and the view had its turn. */
async function settled(body: string): Promise<void> {
  await requestVerdict(body, false);
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("the diagram field with real verdicts", () => {
  test.each(bodies.filter(([name]) => name.startsWith("x")))(
    "the fallback fixture %s is never drawn",
    async (_name, body) => {
      const v = mount(fence(body));
      await settled(body);
      expect(v.state.facet(drawnDiagrams)).toEqual([]);
      expect(v.dom.querySelector(".cm-diagram")).toBeNull();
    },
  );

  test.each(bodies.filter(([name]) => name.startsWith("y")))(
    "the sequence fallback fixture %s is never drawn",
    async (_name, body) => {
      const v = mount(fence(body));
      await settled(body);
      expect(v.state.facet(drawnDiagrams)).toEqual([]);
      expect(v.dom.querySelector(".cm-diagram")).toBeNull();
    },
  );

  test("the sequence starter is drawn once mermaid agrees", async () => {
    const body = defined(bodies.find(([name]) => name.startsWith("s01-")))[1];
    const v = mount(`intro\n\n${fence(body)}\n`);
    await settled(body);
    expect(v.state.facet(drawnDiagrams)).toEqual([7]);
    expect(v.dom.querySelector(".cm-diagram")).not.toBeNull();
  });

  test("a flowchart fixture is drawn once mermaid agrees", async () => {
    const body = defined(bodies.find(([name]) => name.startsWith("f01-")))[1];
    const v = mount(`intro\n\n${fence(body)}\n`);
    await settled(body);
    expect(v.state.facet(drawnDiagrams)).toEqual([7]);
    expect(v.dom.querySelector(".cm-diagram")).not.toBeNull();
  });

  test("a stateDiagram-v2 fence is never drawn", async () => {
    const v = mount(fence("stateDiagram-v2\n  [*] --> A"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
  });

  test("a flowchart inside a blockquote is never drawn", async () => {
    const body = "flowchart TD\n  A --> B";
    const v = mount(
      `> \`\`\`mermaid\n> ${body.replaceAll("\n", "\n> ")}\n> \`\`\`\n`,
    );
    await settled(body);
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
  });

  test("an unclosed flowchart fence is never drawn", async () => {
    const body = "flowchart TD\n  A --> B";
    const v = mount(`\`\`\`mermaid\n${body}`);
    await settled(body);
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
  });
});
