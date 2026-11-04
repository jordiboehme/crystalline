/**
 * The drop line (Review Focus 2): wherever it is drawn is exactly where the
 * dropped file or the moved picture lands.
 *
 * The pointer is a stubbed `posAtCoords` and each line block a stubbed
 * `lineBlockAt` whose bottom names the position it was asked about, so the
 * line's top says which landing it was drawn for. Each fixture writes out
 * by hand where the block it lands below ends and the document after a file
 * dropped at the same point, so a landing that is wrong in the same way for
 * the line and the drop still fails: the reference's first character is two
 * line breaks past the end the line was drawn at, every time.
 */

import { EditorState, StateEffect } from "@codemirror/state";
import type { BlockInfo, EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import type { AssetHome } from "../../api/files";
import { FakeTransfer, fireDrag } from "../../test/dragKit";
import type { MarkViewOptions } from "../../test/markKit";
import { bytes, cleanupMarkViews, markView } from "../../test/markKit";
import { attachmentUploads, uploadAttachments } from "../attachments";
import { prefetchGrid } from "../grid/loader";
import { imagePreviews } from "../imagePreviews";
import { prefetchPictureTools } from "./loader";

beforeAll(async () => {
  await Promise.all([prefetchPictureTools(), prefetchGrid()]);
});

afterEach(() => {
  cleanupMarkViews();
  vi.restoreAllMocks();
});

const AT = { x: 40, y: 80 };
const LINK = "![shot.png](../assets/2026/10/shot.png)";
const P = "![x](assets/p.png)";

const home: AssetHome = {
  key: ["fake"],
  list: () => Promise.resolve([]),
  upload: (path) =>
    Promise.resolve({ path, mime: "image/png", size: 8, sha256: "a" }),
  remove: () => Promise.resolve(),
  url: (path) => `/fake/${path}`,
  resolve: () => null,
  refused: () => null,
  linkTo: (path) => `../${path}`,
};

/** How far below the document's top a block's bottom sits. */
const BASE = 1000;

interface Built {
  view: EditorView;
  /** The uploads the drops started. */
  uploads: Promise<number>[];
  /** The positions `lineBlockAt` was asked about. */
  asked: number[];
}

/** A view with the picture layer, the grid and the upload handlers, pointed at `pos`. */
function build(doc: string, pos: number, options: MarkViewOptions = {}): Built {
  const uploads: Promise<number>[] = [];
  const asked: number[] = [];
  let built: EditorView | null = null;
  const view = markView(doc, {
    images: imagePreviews("eng"),
    uploads: attachmentUploads((files) => {
      if (!built) return;
      uploads.push(
        uploadAttachments({
          home,
          files,
          view: built,
          onError: () => undefined,
          now: new Date(2026, 9, 15),
        }),
      );
    }),
    ...options,
  });
  built = view;
  vi.spyOn(view, "posAtCoords").mockReturnValue(pos);
  vi.spyOn(view, "lineBlockAt").mockImplementation((at: number) => {
    asked.push(at);
    return {
      from: at,
      to: at,
      top: BASE + at - 20,
      height: 20,
      bottom: BASE + at,
    } as unknown as BlockInfo;
  });
  return { view, uploads, asked };
}

/** The drawn drop line of `view`, or null. */
function dropLineOf(view: EditorView): HTMLElement | null {
  return view.scrollDOM.querySelector<HTMLElement>(".cm-drop-line");
}

/** Where a line drawn at a block bottom of `bottom` sits in the scroller. */
function topFor(view: EditorView, bottom: number): string {
  const offset =
    view.documentTop -
    view.scrollDOM.getBoundingClientRect().top +
    view.scrollDOM.scrollTop;
  return `${String(offset + bottom)}px`;
}

/** A file dragged over the view, as Chromium sends it: the type, no files. */
function fileOver(view: EditorView): MouseEvent {
  return fireDrag(
    view.contentDOM,
    "dragover",
    new FakeTransfer({ namesFiles: true }),
    AT,
  );
}

/** Each fixture: the text, where the pointer is, and what the line and the drop say. */
const FIXTURES: {
  name: string;
  doc: string;
  pointer: (doc: string) => number;
  /** The end of the block the file lands below, written out by hand. */
  end: (doc: string) => number;
  after: string;
  options?: MarkViewOptions;
}[] = [
  {
    name: "a plain line",
    doc: "one\n\ntwo\n\nthree\n",
    pointer: (doc) => doc.indexOf("two") + 1,
    end: () => 8,
    after: `one\n\ntwo\n\n${LINK}\n\n\nthree\n`,
  },
  {
    name: "a Reflection heading with its hidden id line",
    doc: "## H\n<!-- r:7f3a9c2e -->\n\nBody.\n",
    pointer: () => 1,
    end: () => 24,
    after: `## H\n<!-- r:7f3a9c2e -->\n\n${LINK}\n\n\nBody.\n`,
    options: { adapter: "reflection" },
  },
  {
    name: "a drawn table",
    doc: "intro\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nend\n",
    pointer: (doc) => doc.indexOf("1 |"),
    end: () => 40,
    after: `intro\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n${LINK}\n\n\nend\n`,
  },
  {
    name: "a fence",
    doc: "a\n\n```\ncode\n\nmore\n```\n\nb\n",
    pointer: (doc) => doc.indexOf("code"),
    end: () => 21,
    after: `a\n\n\`\`\`\ncode\n\nmore\n\`\`\`\n\n${LINK}\n\n\nb\n`,
  },
  {
    name: "the folded frontmatter",
    doc: "---\ntitle: T\n---\nBody line\nmore\n\nend\n",
    pointer: () => 5,
    end: () => 31,
    after: `---\ntitle: T\n---\nBody line\nmore\n\n${LINK}\n\n\nend\n`,
  },
  {
    name: "the last line",
    doc: "one\n\nlast",
    pointer: (doc) => doc.length,
    end: () => 9,
    after: `one\n\nlast\n\n${LINK}\n`,
  },
];

describe.each(FIXTURES)(
  "the drop line over $name",
  ({ doc, pointer, end, after, options }) => {
    it("is drawn where the dropped file then lands", async () => {
      const { view, uploads, asked } = build(doc, pointer(doc), options);
      const over = fileOver(view);
      expect(over.defaultPrevented).toBe(true);
      const line = dropLineOf(view);
      expect(line).not.toBeNull();
      expect(line?.getAttribute("aria-hidden")).toBe("true");
      expect(asked.at(-1)).toBe(end(doc));
      expect(line?.style.top).toBe(topFor(view, BASE + end(doc)));

      const drop = fireDrag(
        view.contentDOM,
        "drop",
        new FakeTransfer({ files: [new File(["x"], "shot.png")] }),
        AT,
      );
      expect(drop.defaultPrevented).toBe(true);
      expect(dropLineOf(view)).toBeNull();
      expect(await Promise.all(uploads)).toEqual([1]);
      expect(bytes(view.state)).toBe(after);
      // Below the block the line was drawn at, after one blank line.
      expect(bytes(view.state).indexOf(LINK)).toBe(end(doc) + 2);
    });

    it("is drawn where a picture dragged there then lands", () => {
      const source = `${doc}${doc.endsWith("\n") ? "" : "\n"}\n${P}\n`;
      const { view, asked } = build(source, pointer(doc), options);
      const box = view.contentDOM.querySelector(".cm-picture");
      if (!box) throw new Error("no drawn picture");
      const transfer = new FakeTransfer();
      fireDrag(box, "dragstart", transfer);
      fireDrag(view.contentDOM, "dragover", transfer, AT);
      expect(asked.at(-1)).toBe(end(doc));
      expect(dropLineOf(view)?.style.top).toBe(topFor(view, BASE + end(doc)));
      fireDrag(view.contentDOM, "drop", transfer, AT);
      expect(dropLineOf(view)).toBeNull();
      const text = bytes(view.state);
      expect(text.indexOf(P)).toBe(end(doc) + 2);
      expect(text.split(P)).toHaveLength(2);
    });
  },
);

describe("the drop line", () => {
  const DOC = "one\n\ntwo\n";

  it("is not drawn for a text drag", () => {
    const { view } = build(DOC, 1);
    const transfer = new FakeTransfer();
    transfer.setData("text/plain", "words");
    fireDrag(view.contentDOM, "dragover", transfer, AT);
    expect(dropLineOf(view)).toBeNull();
  });

  it("goes when the drag leaves the editor, and stays over its own parts", () => {
    const { view } = build(DOC, 1);
    fileOver(view);
    const inner = view.contentDOM.querySelector(".cm-line");
    fireDrag(view.contentDOM, "dragleave", null, AT, inner);
    expect(dropLineOf(view)).not.toBeNull();
    fireDrag(view.contentDOM, "dragleave", null, AT, document.body);
    expect(dropLineOf(view)).toBeNull();
    fileOver(view);
    fireDrag(view.contentDOM, "dragleave", null, AT, null);
    expect(dropLineOf(view)).toBeNull();
  });

  it("goes when the drag ends and when a drop is taken by another handler", () => {
    const { view } = build(DOC, 1);
    fileOver(view);
    fireDrag(view.contentDOM, "dragend", null);
    expect(dropLineOf(view)).toBeNull();
    fileOver(view);
    // A drop the upload handlers do not take (no files) still ends the drag.
    fireDrag(view.contentDOM, "drop", new FakeTransfer(), AT);
    expect(dropLineOf(view)).toBeNull();
  });

  it("is drawn once however often the dragover comes", () => {
    const { view } = build(DOC, 1);
    fileOver(view);
    fileOver(view);
    expect(view.scrollDOM.querySelectorAll(".cm-drop-line")).toHaveLength(1);
  });

  it("is not drawn for a picture dragged from another view", () => {
    const one = build(`${P}\n`, 0).view;
    const { view } = build(DOC, 1);
    const box = one.contentDOM.querySelector(".cm-picture");
    if (!box) throw new Error("no drawn picture");
    const transfer = new FakeTransfer();
    fireDrag(box, "dragstart", transfer);
    fireDrag(view.contentDOM, "dragover", transfer, AT);
    expect(dropLineOf(view)).toBeNull();
    fireDrag(box, "dragend", transfer);
  });

  it("is not drawn on a page that takes no writes", () => {
    const { view } = build(DOC, 1);
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    fileOver(view);
    expect(dropLineOf(view)).toBeNull();
  });
});
