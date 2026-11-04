/**
 * The picture model: every inline image of a document as a classified span.
 *
 * Every state here is parsed to the end (`parsedState`): the classification
 * reads the syntax tree, and a short tree would answer for less of the text.
 */

import type { Extension } from "@codemirror/state";
import { EditorSelection, EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import type { AssetResolver } from "../../api/files";
import { bytes } from "../../test/cellKit";
import { parsedState } from "../../test/parse";
import { imageSpansIn } from "../imageFormat";
import { mathRegions } from "../mathBlocks";
import { baseExtensions, lineSeparatorFor } from "../setup";
import type { PictureRef } from "./refs";
import { pictureAssets, pictureAt, picturesIn, selectedPicture } from "./refs";

function stateOf(doc: string, extra: Extension[] = []): EditorState {
  return parsedState(
    EditorState.create({
      doc,
      extensions: [
        ...lineSeparatorFor(doc),
        ...baseExtensions(false),
        ...extra,
      ],
    }),
  );
}

function slice(state: EditorState, from: number, to: number): string {
  return state.doc.sliceString(from, to);
}

function only(state: EditorState): PictureRef {
  const refs = picturesIn(state);
  expect(refs).toHaveLength(1);
  const [ref] = refs;
  if (ref === undefined) throw new Error("no picture");
  return ref;
}

describe("imageSpansIn", () => {
  it("finds every inline image span, attachment or not, with its target", () => {
    const text =
      "a ![x](https://e.org/a.png) [l](assets/b.png) ![y](assets/c.pdf)";
    const spans = imageSpansIn(text);
    expect(spans.map((s) => text.slice(s.from, s.to))).toEqual([
      "![x](https://e.org/a.png)",
      "![y](assets/c.pdf)",
    ]);
    expect(spans.map((s) => s.token)).toEqual([
      "https://e.org/a.png",
      "assets/c.pdf",
    ]);
    expect(spans.map((s) => text.slice(s.targetFrom, s.targetTo))).toEqual([
      "https://e.org/a.png",
      "assets/c.pdf",
    ]);
  });

  it("skips fenced code", () => {
    expect(imageSpansIn("```\n![x](assets/a.png)\n```\n")).toEqual([]);
  });
});

describe("picturesIn", () => {
  it("reads an attachment with its alt, target, format and written path", () => {
    const state = stateOf("a ![Shot](assets/s.png#right,w=50%) b");
    const ref = only(state);
    expect(ref.kind).toBe("attachment");
    expect(slice(state, ref.from, ref.to)).toBe(
      "![Shot](assets/s.png#right,w=50%)",
    );
    expect(slice(state, ref.altFrom, ref.altTo)).toBe("Shot");
    expect(slice(state, ref.targetFrom, ref.targetTo)).toBe(
      "assets/s.png#right,w=50%",
    );
    expect(ref.token).toBe("assets/s.png#right,w=50%");
    expect(ref.format).toEqual({ align: "right", width: "50%" });
    expect(ref.written).toBe("assets/s.png");
    // The default resolver links an attachment by its stored path.
    expect(ref.src).toBe("assets/s.png");
    expect(ref.reason).toBeNull();
    expect(ref.inLink).toBe(false);
    expect(ref.inCell).toBe(false);
  });

  it("draws an external picture from its own address", () => {
    const ref = only(stateOf("![x](https://e.org/a.png)"));
    expect(ref.kind).toBe("external");
    expect(ref.src).toBe("https://e.org/a.png");
    expect(ref.format).toBeNull();
    expect(ref.written).toBe("");
    expect(only(stateOf("![x](HTTP://e.org/a.png)")).kind).toBe("external");
  });

  it("calls a non-image attachment and a path outside assets unavailable", () => {
    for (const doc of [
      "![x](assets/a.pdf)",
      "![x](notes/a.png)",
      "![x](ftp://e.org/a.png)",
      "![x]()",
    ]) {
      const ref = only(stateOf(doc));
      expect(ref.kind, doc).toBe("unavailable");
      expect(ref.src, doc).toBeNull();
      expect(ref.reason, doc).toBe("This image is not available.");
      expect(ref.format, doc).toBeNull();
    }
  });

  it("refuses what the resolver refuses, with its reason", () => {
    const resolver: AssetResolver = {
      resolve: () => "assets/a.png",
      url: (path) => `/files/${path}`,
      refused: (target) =>
        target.startsWith("crystalline://")
          ? "A Reflection cannot show an engram's file."
          : null,
    };
    const ref = only(
      stateOf("![x](crystalline://d/assets/a.png)", [
        pictureAssets.of(resolver),
      ]),
    );
    expect(ref.kind).toBe("refused");
    expect(ref.reason).toBe("A Reflection cannot show an engram's file.");
    expect(ref.src).toBeNull();
  });

  it("asks the last resolver provided", () => {
    const reflection: AssetResolver = {
      resolve: (target) =>
        target.startsWith("../assets/") ? target.slice(3) : null,
      url: (path) => `/reflection/${path}`,
      refused: () => null,
    };
    const ref = only(
      stateOf("![x](../assets/a.png)", [
        pictureAssets.of({
          resolve: () => null,
          url: (p) => p,
          refused: () => null,
        }),
        pictureAssets.of(reflection),
      ]),
    );
    expect(ref.kind).toBe("attachment");
    expect(ref.src).toBe("/reflection/assets/a.png");
    expect(ref.written).toBe("../assets/a.png");
  });

  it("leaves a reference-style image as text", () => {
    expect(picturesIn(stateOf("![a][r]\n\n[r]: assets/a.png\n"))).toEqual([]);
  });

  it("knows a picture inside a link", () => {
    const ref = only(stateOf("[![x](assets/a.png)](https://e.org)"));
    expect(ref.inLink).toBe(true);
    expect(ref.inCell).toBe(false);
    expect(ref.from).toBe(1);
  });

  it("knows a picture inside a table cell", () => {
    const ref = only(
      stateOf("| a | b |\n| --- | --- |\n| ![x](assets/a.png) | c |\n"),
    );
    expect(ref.inCell).toBe(true);
    expect(ref.kind).toBe("attachment");
  });

  it("reads brackets in the alt and parentheses in the target as written", () => {
    const state = stateOf('![a [b] c](assets/a(1).png "Q3")');
    const ref = only(state);
    expect(slice(state, ref.altFrom, ref.altTo)).toBe("a [b] c");
    expect(slice(state, ref.targetFrom, ref.targetTo)).toBe("assets/a(1).png");
    expect(ref.kind).toBe("attachment");
  });

  it("finds nothing in a fence or a math block", () => {
    expect(picturesIn(stateOf("```\n![x](assets/a.png)\n```\n"))).toEqual([]);
    const math = stateOf("text\n\n$$\n![x](assets/a.png)\n$$\n\nmore\n");
    // The tree reads the formula line as a paragraph with an image in it;
    // the math region is what keeps it out.
    expect(mathRegions(math).length).toBeGreaterThan(0);
    expect(picturesIn(math)).toEqual([]);
  });

  it("answers the same positions for a CRLF document and keeps its bytes", () => {
    const lf = "one\n\ntwo ![x](assets/a.png)\n\n![y](https://e.org/b.png)\n";
    const crlf = lf.replace(/\n/g, "\r\n");
    const lfRefs = picturesIn(stateOf(lf));
    const crlfState = stateOf(crlf);
    const crlfRefs = picturesIn(crlfState);
    expect(crlfRefs).toHaveLength(2);
    expect(crlfRefs).toEqual(lfRefs);
    expect(bytes(crlfState)).toBe(crlf);
  });

  it("reads only the asked range", () => {
    const doc = "![a](assets/a.png)\n\n![b](assets/b.png)\n";
    const state = stateOf(doc);
    const second = doc.indexOf("![b]");
    expect(picturesIn(state, second, doc.length).map((r) => r.from)).toEqual([
      second,
    ]);
  });
});

describe("pictureAt and selectedPicture", () => {
  const doc = "go ![x](assets/a.png) on";
  const from = doc.indexOf("![");
  const to = doc.indexOf(")") + 1;

  it("finds the picture from either end and from inside", () => {
    const state = stateOf(doc);
    for (const pos of [from, from + 3, to]) {
      expect(pictureAt(state, pos)?.from, String(pos)).toBe(from);
    }
    expect(pictureAt(state, 1)).toBeNull();
  });

  it("finds the picture on the right of a junction when the left one is text", () => {
    const junction = "![a][r]![x](assets/a.png)\n\n[r]: assets/b.png\n";
    const state = stateOf(junction);
    const at = junction.indexOf("![x]");
    expect(pictureAt(state, at)?.from).toBe(at);
  });

  it("is the picture only when the selection is exactly its span", () => {
    const at = (anchor: number, head: number) =>
      selectedPicture(
        stateOf(doc).update({ selection: EditorSelection.single(anchor, head) })
          .state,
      );
    expect(at(from, to)?.to).toBe(to);
    expect(at(to, from)?.from).toBe(from);
    expect(at(from, from)).toBeNull();
    expect(at(from, to - 1)).toBeNull();
    expect(at(from + 1, to)).toBeNull();
  });
});
