/**
 * `[[Target]]` inside the buffer: drawn as a chip in the same three states
 * every other surface draws a reference in, handed back as the text it really
 * is the moment a selection touches it, and completed from title search across
 * the registered domains.
 *
 * Every assertion is on rendered DOM or on the document a completion leaves
 * behind, because both claims are about what an author sees and types. The
 * fidelity test is the counterweight: a chip is a read-model, so the buffer
 * under it must come back out byte-identical however far the cursor roams.
 */

import { CompletionContext } from "@codemirror/autocomplete";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { EngramRow } from "../api/engrams";
import { singlePage } from "../api/engrams";
import { fetchSearch } from "../api/search";
import type { WikilinkResolution } from "../wikilinks";
import { baseExtensions, docText, lineSeparatorFor } from "./setup";
import {
  wikilinkChips,
  wikilinkCompletions,
  wikilinkResolverFacet,
} from "./wikilinkChips";

vi.mock("../api/search", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/search")>();
  return { ...actual, fetchSearch: vi.fn() };
});

const searchMock = vi.mocked(fetchSearch);

const DOC = "---\nt: x\n---\n\nSee [[Beta Note]] and [[ghost]].\n";

/** One search hit, with only the fields a completion reads spelled out. */
function hit(domain: string, permalink: string, title: string): EngramRow {
  return {
    domain,
    permalink,
    title,
    type: null,
    status: null,
    tags: [],
    kind: null,
    line: null,
    snippet: null,
  };
}

function editor(
  resolve: (inner: string) => WikilinkResolution | null,
  doc = DOC,
): EditorView {
  return new EditorView({
    // A named separator on every state this file builds, tests included: a
    // buffer that names none rewrites a CRLF document to LF on read-back.
    state: EditorState.create({
      doc,
      selection: EditorSelection.cursor(0),
      extensions: [
        ...lineSeparatorFor(doc),
        ...baseExtensions(false),
        wikilinkResolverFacet.of(resolve),
        wikilinkChips(),
      ],
    }),
    parent: document.body,
  });
}

/** A state a completion source is asked about, cursor at the end of `doc`. */
function stateOf(doc: string): EditorState {
  return EditorState.create({
    doc,
    selection: EditorSelection.cursor(doc.length),
    extensions: lineSeparatorFor(doc),
  });
}

beforeEach(() => {
  searchMock.mockReset();
});

describe("wikilink chips", () => {
  it("draws the tri-state chips off the resolver", () => {
    const view = editor((inner) =>
      inner === "Beta Note"
        ? { kind: "resolved", href: "/d/eng/e/beta", label: "Beta Note" }
        : { kind: "unresolved" },
    );
    expect(view.dom.querySelector(".cm-wikilink-resolved")?.textContent).toBe(
      "Beta Note",
    );
    expect(view.dom.querySelector(".cm-wikilink-unresolved")?.textContent).toBe(
      "ghost",
    );
    view.destroy();
  });

  it("draws a target nobody has placed yet as pending rather than broken", () => {
    const view = editor(() => null);
    expect(view.dom.querySelector(".cm-wikilink-pending")?.textContent).toBe(
      "Beta Note",
    );
    expect(view.dom.querySelector(".cm-wikilink-unresolved")).toBeNull();
    view.destroy();
  });

  it("labels a cross-domain target with its target text and keeps the source in the tooltip", () => {
    const view = editor(
      () => ({ kind: "unresolved" }),
      "See [[ops:Runbook]].\n",
    );
    const chip = view.dom.querySelector(".cm-wikilink");
    expect(chip?.textContent).toBe("Runbook");
    expect(chip?.getAttribute("title")).toBe("[[ops:Runbook]]");
    view.destroy();
  });

  it("hands the atom back as text when the cursor enters it", () => {
    const view = editor(() => ({ kind: "unresolved" }));
    view.dispatch({
      selection: EditorSelection.cursor(DOC.indexOf("Beta") + 1),
    });
    expect(view.contentDOM.textContent).toContain("[[Beta Note]]");
    view.destroy();
  });

  it("leaves brackets inside the frontmatter block alone", () => {
    const view = editor(() => ({ kind: "unresolved" }), "---\nt: [[x]]\n---\n");
    expect(view.dom.querySelector(".cm-wikilink")).toBeNull();
    view.destroy();
  });

  it("decorates without touching the document, line endings included", () => {
    for (const doc of [DOC, DOC.replace(/\n/g, "\r\n")]) {
      const view = editor(() => ({ kind: "unresolved" }), doc);
      // `docText`, never `doc.toString()`: the read-back is the file's bytes.
      expect(docText(view.state)).toBe(doc);
      for (let at = 0; at <= view.state.doc.length; at += 1) {
        view.dispatch({ selection: EditorSelection.cursor(at) });
      }
      expect(docText(view.state)).toBe(doc);
      view.destroy();
    }
  });
});

describe("the [[ completion", () => {
  it("offers title matches, domain-prefixed when the hit lives elsewhere", async () => {
    searchMock.mockResolvedValueOnce(
      singlePage([
        hit("eng", "beta", "Beta Note"),
        hit("ops", "runbook", "Runbook"),
      ]),
    );
    const doc = "See [[Bet";
    const state = stateOf(doc);
    const result = await wikilinkCompletions("eng")(
      new CompletionContext(state, doc.length, false),
    );
    expect(result).not.toBeNull();
    // Title search across every registered domain: the palette's own lookup.
    expect(searchMock).toHaveBeenCalledWith(
      expect.objectContaining({ q: "Bet", mode: "title", domains: [] }),
      1,
    );
    const labels = result?.options.map((option) => option.label);
    expect(labels).toEqual(["Beta Note", "Runbook"]);
    // The domain is named only where it is news.
    expect(result?.options[0]?.detail).toBeUndefined();
    expect(result?.options[1]?.detail).toBe("ops");

    // Applying the cross-domain hit inserts the prefixed form and closes.
    const view = new EditorView({ state, parent: document.body });
    const runbook = result?.options[1];
    expect(typeof runbook?.apply).toBe("function");
    (
      runbook?.apply as (
        v: EditorView,
        c: unknown,
        f: number,
        t: number,
      ) => void
    )(view, runbook, result?.from ?? 0, doc.length);
    expect(docText(view.state)).toBe("See [[ops:Runbook]]");
    view.destroy();
  });

  it("applies a same-domain hit bare and swallows a closing pair already typed", async () => {
    searchMock.mockResolvedValueOnce(
      singlePage([hit("eng", "beta", "Beta Note")]),
    );
    const doc = "See [[Bet]] here";
    const at = doc.indexOf("]]");
    const state = stateOf(doc);
    const result = await wikilinkCompletions("eng")(
      new CompletionContext(state, at, false),
    );
    const beta = result?.options[0];
    const view = new EditorView({ state, parent: document.body });
    (beta?.apply as (v: EditorView, c: unknown, f: number, t: number) => void)(
      view,
      beta,
      result?.from ?? 0,
      at,
    );
    expect(docText(view.state)).toBe("See [[Beta Note]] here");
    // The cursor lands past the closing pair, ready for the next word.
    expect(view.state.selection.main.head).toBe("See [[Beta Note]]".length);
    view.destroy();
  });

  it("asks for nothing on a bare [[ and stays quiet outside one", async () => {
    const opened = await wikilinkCompletions("eng")(
      new CompletionContext(stateOf("See [["), 6, false),
    );
    expect(opened?.options).toEqual([]);
    expect(searchMock).not.toHaveBeenCalled();

    for (const doc of ["See Bet", "See [[done]] and more"]) {
      expect(
        await wikilinkCompletions("eng")(
          new CompletionContext(stateOf(doc), doc.length, false),
        ),
      ).toBeNull();
    }
    expect(searchMock).not.toHaveBeenCalled();
  });
});
