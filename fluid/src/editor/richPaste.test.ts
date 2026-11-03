import { history, undo } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import type { Extension } from "@codemirror/state";
import { Compartment, EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import { reflectionAdapter } from "./adapter";
import { attachmentUploads } from "./attachments";
import { richPaste } from "./richPaste";
import { surfaceExtensions } from "./surface";

let view: EditorView | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
});

function mount(
  doc: string,
  extra: Extension[] = [],
  handler: Extension = richPaste(),
): EditorView {
  view = new EditorView({
    state: EditorState.create({
      doc,
      selection: EditorSelection.cursor(doc.length),
      extensions: [
        EditorState.lineSeparator.of("\n"),
        history(),
        markdown(),
        handler,
        ...extra,
      ],
    }),
    parent: document.body,
  });
  return view;
}

/** A paste event carrying the given clipboard flavours, the way a browser sends one. */
function paste(
  v: EditorView,
  data: { html?: string; plain?: string; files?: File[] },
): Event {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  const types = [
    ...(data.html === undefined ? [] : ["text/html"]),
    ...(data.plain === undefined ? [] : ["text/plain"]),
    ...(data.files?.length ? ["Files"] : []),
  ];
  Object.defineProperty(event, "clipboardData", {
    value: {
      getData: (type: string) =>
        type === "text/html"
          ? (data.html ?? "")
          : type === "text/plain"
            ? (data.plain ?? "")
            : "",
      types,
      files: data.files ?? [],
    },
  });
  v.contentDOM.dispatchEvent(event);
  return event;
}

function keydown(v: EditorView, init: KeyboardEventInit): void {
  v.contentDOM.dispatchEvent(
    new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
  );
}

const RICH = "<h2>Steps</h2><ul><li><b>Build</b> it</li></ul>";
const RICH_PLAIN = "Steps\nBuild it";

describe("richPaste", () => {
  it("inserts the converted Markdown of an HTML paste in one transaction", async () => {
    const v = mount("Intro\n\n");
    const event = paste(v, { html: RICH, plain: RICH_PLAIN });
    expect(event.defaultPrevented).toBe(true);
    await vi.waitFor(() =>
      expect(v.state.doc.toString()).toBe(
        "Intro\n\n## Steps\n\n- **Build** it",
      ),
    );
    undo(v);
    expect(v.state.doc.toString()).toBe("Intro\n\n");
  });

  it("converts at once once the converter has loaded", async () => {
    const v = mount("");
    paste(v, { html: RICH, plain: RICH_PLAIN });
    await vi.waitFor(() => expect(v.state.doc.length).toBeGreaterThan(0));
    v.dispatch({ changes: { from: 0, to: v.state.doc.length } });
    paste(v, { html: "<p><i>now</i></p>", plain: "now" });
    expect(v.state.doc.toString()).toBe("*now*");
  });

  it("leaves a paste with only plain text to the editor", () => {
    const v = mount("");
    paste(v, { plain: "a *b* c" });
    expect(v.state.doc.toString()).toBe("a *b* c");
  });

  it("pastes plain text after Mod-Shift-V", async () => {
    const v = mount("");
    keydown(v, { key: "V", code: "KeyV", shiftKey: true, ctrlKey: true });
    paste(v, { html: RICH, plain: RICH_PLAIN });
    expect(v.state.doc.toString()).toBe(RICH_PLAIN);
    // One shot: the next paste is rich again.
    v.dispatch({ changes: { from: 0, to: v.state.doc.length } });
    paste(v, { html: RICH, plain: RICH_PLAIN });
    await vi.waitFor(() =>
      expect(v.state.doc.toString()).toBe("## Steps\n\n- **Build** it"),
    );
  });

  it("forgets a Mod-Shift-V that brought no paste once another key is pressed", async () => {
    const v = mount("");
    keydown(v, { key: "V", code: "KeyV", shiftKey: true, metaKey: true });
    keydown(v, { key: "a", code: "KeyA" });
    paste(v, { html: RICH, plain: RICH_PLAIN });
    await vi.waitFor(() =>
      expect(v.state.doc.toString()).toBe("## Steps\n\n- **Build** it"),
    );
  });

  it("pastes plain text into a code fence", () => {
    const v = mount("```\n\n```\n");
    v.dispatch({ selection: { anchor: 4 } });
    paste(v, { html: RICH, plain: "let x = 1;" });
    expect(v.state.doc.toString()).toBe("```\nlet x = 1;\n```\n");
  });

  it("keeps a code editor's copy as its plain text, indentation and all", async () => {
    const v = mount("");
    const html =
      '<div style="white-space: pre;"><div><span style="color: #569cd6;">fn</span> main() {</div><div>    run();</div><div>}</div></div>';
    paste(v, { html, plain: "fn main() {\n    run();\n}" });
    await vi.waitFor(() =>
      expect(v.state.doc.toString()).toBe("fn main() {\n    run();\n}"),
    );
  });

  it("pastes the plain text when the HTML converts to nothing", async () => {
    const v = mount("");
    paste(v, { html: '<meta charset="utf-8">', plain: "just words" });
    await vi.waitFor(() => expect(v.state.doc.toString()).toBe("just words"));
  });

  it("leaves a read-only buffer alone", () => {
    const v = mount("x", [EditorState.readOnly.of(true)]);
    const event = paste(v, { html: RICH, plain: RICH_PLAIN });
    expect(event.defaultPrevented).toBe(true);
    expect(v.state.doc.toString()).toBe("x");
  });

  it("hands a paste that carries files to the uploads", () => {
    const onFiles = vi.fn();
    const v = mount("", [attachmentUploads(onFiles)]);
    const file = new File(["png"], "shot.png", { type: "image/png" });
    paste(v, { html: '<img src="x.png">', plain: "", files: [file] });
    expect(onFiles).toHaveBeenCalledWith([file]);
    expect(v.state.doc.toString()).toBe("");
  });

  it("converts a copy of text that brought a picture of itself along", async () => {
    const onFiles = vi.fn();
    const v = mount("", [attachmentUploads(onFiles)]);
    const picture = new File(["png"], "image.png", { type: "image/png" });
    paste(v, { html: RICH, plain: RICH_PLAIN, files: [picture] });
    await vi.waitFor(() =>
      expect(v.state.doc.toString()).toBe("## Steps\n\n- **Build** it"),
    );
    expect(onFiles).not.toHaveBeenCalled();
  });

  it("pastes plain text, not its picture, after Mod-Shift-V and into a fence", () => {
    const onFiles = vi.fn();
    const v = mount("```\n\n```\n", [attachmentUploads(onFiles)]);
    const picture = new File(["png"], "image.png", { type: "image/png" });
    keydown(v, { key: "V", code: "KeyV", shiftKey: true, metaKey: true });
    paste(v, { html: RICH, plain: "one", files: [picture] });
    v.dispatch({ selection: { anchor: 4 } });
    paste(v, { html: RICH, plain: "two", files: [picture] });
    expect(v.state.doc.toString()).toBe("```\ntwo\n```\none");
    expect(onFiles).not.toHaveBeenCalled();
  });

  it("keeps the HTML's text when a plain paste brings no plain text", () => {
    const onFiles = vi.fn();
    const v = mount("", [attachmentUploads(onFiles)]);
    const picture = new File(["png"], "image.png", { type: "image/png" });
    keydown(v, { key: "V", code: "KeyV", shiftKey: true, ctrlKey: true });
    paste(v, {
      html: "<p>Only <b>here</b><script>x()</script></p>",
      plain: "",
      files: [picture],
    });
    expect(v.state.doc.toString()).toBe("Only here");
    expect(onFiles).not.toHaveBeenCalled();
  });

  it("uploads a file that is no image even when text came with it", () => {
    const onFiles = vi.fn();
    const v = mount("", [attachmentUploads(onFiles)]);
    const deck = new File(["pdf"], "deck.pdf", { type: "application/pdf" });
    paste(v, { html: RICH, plain: RICH_PLAIN, files: [deck] });
    expect(onFiles).toHaveBeenCalledWith([deck]);
    expect(v.state.doc.toString()).toBe("");
  });

  it("strips a section id from a pasted code block in a Reflection buffer", async () => {
    const page = "## First\n<!-- r:0b1c2d3e -->\n\nBody.\n";
    const v = mount(page, reflectionAdapter().extensions);
    paste(v, {
      html: "<pre><code>a\n&lt;!-- r:aaaa1111 --&gt;\nb</code></pre>",
      plain: "a\n<!-- r:aaaa1111 -->\nb",
    });
    await vi.waitFor(() =>
      expect(v.state.doc.toString()).toBe(`${page}\`\`\`\na\nb\n\`\`\``),
    );
  });

  it("keeps a visible id-like line as text, never as an id", async () => {
    const page = "## First\n<!-- r:0b1c2d3e -->\n\nBody.\n";
    const v = mount(page, reflectionAdapter().extensions);
    paste(v, {
      html: "<h2>Pasted</h2><p>&lt;!-- r:aaaa1111 --&gt;</p>",
      plain: "Pasted\n<!-- r:aaaa1111 -->",
    });
    await vi.waitFor(() =>
      expect(v.state.doc.toString()).toBe(
        `${page}## Pasted\n\n\\<!-- r:aaaa1111 -->`,
      ),
    );
  });

  it("is part of the shared surface, beside the uploads, in a Reflection buffer", async () => {
    const onFiles = vi.fn();
    const page = "## First\n<!-- r:0b1c2d3e -->\n\nBody.\n";
    view = new EditorView({
      state: EditorState.create({
        doc: page,
        selection: EditorSelection.cursor(page.length),
        extensions: [
          EditorState.lineSeparator.of("\n"),
          ...surfaceExtensions({
            adapter: reflectionAdapter(),
            dark: false,
            history: true,
            screen: [],
            preview: new Compartment(),
            raw: false,
            images: [],
            uploads: attachmentUploads(onFiles),
            onTableContext: () => {},
            onImageContext: () => {},
            onElementContext: () => {},
            onFloatingContext: () => {},
          }),
        ],
      }),
      parent: document.body,
    });
    const v = view;
    const file = new File(["png"], "shot.png", { type: "image/png" });
    paste(v, { html: '<img src="x.png">', files: [file] });
    expect(onFiles).toHaveBeenCalledWith([file]);
    paste(v, {
      html: '<h2>Pasted</h2><p>&lt;!-- r:aaaa1111 --&gt;</p><div data-macro-name="tip"><p>Short.</p></div>',
      plain: "Pasted\n<!-- r:aaaa1111 -->\nShort.",
    });
    await vi.waitFor(() =>
      expect(v.state.doc.toString()).toBe(
        `${page}## Pasted\n\n\\<!-- r:aaaa1111 -->\n\n> [!TIP]\n> Short.`,
      ),
    );
    // Mod-Shift-V through the whole keymap stack still reaches the paste.
    keydown(v, { key: "V", code: "KeyV", shiftKey: true, ctrlKey: true });
    paste(v, { html: "<p><b>bold</b></p>", plain: "bold" });
    expect(v.state.doc.toString()).toBe(
      `${page}## Pasted\n\n\\<!-- r:aaaa1111 -->\n\n> [!TIP]\n> Short.bold`,
    );
  });
});

describe("richPaste when the converter cannot load", () => {
  afterEach(() => {
    vi.doUnmock("./htmlToMarkdown");
    vi.resetModules();
  });

  it("pastes the plain text, and loads the converter again on the next paste", async () => {
    let attempts = 0;
    vi.resetModules();
    vi.doMock("./htmlToMarkdown", async () => {
      attempts += 1;
      if (attempts === 1) {
        throw new Error("chunk failed to load");
      }
      return await vi.importActual<typeof import("./htmlToMarkdown")>(
        "./htmlToMarkdown",
      );
    });
    const fresh = await import("./richPaste");
    const v = mount("", [], fresh.richPaste());
    paste(v, { html: "<p><b>one</b></p>", plain: "one" });
    await vi.waitFor(() => expect(v.state.doc.toString()).toBe("one"));
    paste(v, { html: "<p><b>two</b></p>", plain: "two" });
    await vi.waitFor(() => expect(v.state.doc.toString()).toContain("**two**"));
    expect(attempts).toBe(2);
  });
});
