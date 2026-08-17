/**
 * The frontmatter fold, as the things a reader of it needs to be true: the
 * block is one chip, the caret walks past it rather than into it, the chip's
 * own effect brings the yaml back, and an edit dispatched INTO the folded
 * region (which is what the frontmatter form does all day) leaves the summary
 * current rather than stale.
 */

import {
  cursorCharLeft,
  deleteCharBackward,
  history,
  undo,
} from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, test } from "vitest";

import { frontmatterFold, unfoldEffect } from "./frontmatterFold";
import { baseExtensions } from "./setup";

const DOC =
  "---\ntype: guide\nstatus: current\ntags:\n- protocol\n---\n\n# Body\n";

function mount(doc: string, extensions: Extension[] = []): EditorView {
  return new EditorView({
    state: EditorState.create({
      doc,
      extensions: [frontmatterFold(), ...extensions],
    }),
    parent: document.body,
  });
}

/**
 * The ancestor scope of every emitted chip rule: one per scheme, because a
 * base theme rewrites both the bare selector and its `&dark` variant to a
 * class the view puts on its own root.
 */
function chipScopes(): string[] {
  return Array.from(document.styleSheets)
    .flatMap((sheet) => Array.from(sheet.cssRules))
    .filter((rule): rule is CSSStyleRule => "selectorText" in rule)
    .map((rule) => rule.selectorText)
    .filter((selector) => selector.includes("cm-frontmatter-chip"))
    .map((selector) => selector.replace(/\s*\.cm-frontmatter-chip.*$/, ""));
}

describe("frontmatterFold", () => {
  test("the block is replaced by a summary chip", () => {
    const view = mount(DOC);
    const chip = view.dom.querySelector(".cm-frontmatter-chip");
    expect(chip?.textContent).toContain("guide");
    expect(chip?.textContent).toContain("current");
    // The yaml lines are not in the visible DOM while folded.
    expect(view.contentDOM.textContent).not.toContain("status: current");
    view.destroy();
  });

  test("the chip counts a tag list written flush against the margin", () => {
    // The shape this fixture already has, now asserted: block items at zero
    // indentation are a tag list, and a summary that counted none of them
    // told the reader the block held fewer tags than it does.
    const view = mount(DOC.replace("- protocol\n", "- protocol\n- smoke\n"));
    expect(
      view.dom.querySelector(".cm-frontmatter-chip")?.textContent,
    ).toContain("2 tags");
    view.destroy();
  });

  test("cursor motion steps over the fold rather than into it", () => {
    const view = mount(DOC);
    // Put the caret just after the fold, then walk left. The first two steps
    // are ordinary text motion - body start to blank line to the region's
    // trailing edge - and prove nothing about atomicity: a fold without
    // `atomicRanges` sits at the same 52. The THIRD step is the one that
    // discriminates. Atomic, it jumps the whole hidden block and lands at 0;
    // non-atomic, it walks to 51, one character inside yaml nobody can see.
    view.dispatch({ selection: { anchor: DOC.indexOf("# Body") } });
    cursorCharLeft(view);
    cursorCharLeft(view);
    expect(view.state.selection.main.head).toBe(
      DOC.lastIndexOf("---") + "---".length,
    );
    cursorCharLeft(view);
    expect(view.state.selection.main.head).toBe(0);
    view.destroy();
  });

  test("the unfold effect brings the yaml back", () => {
    const view = mount(DOC);
    view.dispatch({ effects: unfoldEffect.of(true) });
    expect(view.contentDOM.textContent).toContain("status: current");
    view.destroy();
  });

  test("the chip is a button, and activating it hands focus to the text", () => {
    // A real button, so Enter and Space reach it the way a click does; and
    // since activating it REMOVES it from the DOM, focus has to be put
    // somewhere deliberate or a keyboard user is dumped back on the body.
    const view = mount(DOC);
    const chip = view.dom.querySelector<HTMLButtonElement>(
      "button.cm-frontmatter-chip",
    );
    chip?.click();
    expect(view.contentDOM.textContent).toContain("status: current");
    expect(view.dom.querySelector(".cm-frontmatter-chip")).toBeNull();
    expect(document.activeElement).toBe(view.contentDOM);
    // The caret is on the first line of the yaml that just appeared.
    expect(view.state.doc.lineAt(view.state.selection.main.head).number).toBe(
      2,
    );
    view.destroy();
  });

  test("a document with no frontmatter shows no chip", () => {
    const view = mount("# Just a body\n");
    expect(view.dom.querySelector(".cm-frontmatter-chip")).toBeNull();
    view.destroy();
  });

  test("a form edit into the folded region refreshes the summary", () => {
    const view = mount(DOC);
    // `sliceDoc` rather than `doc.toString()`: the sanctioned read, the one
    // `docText` itself makes.
    const at = view.state.sliceDoc().indexOf("current");
    view.dispatch({
      changes: { from: at, to: at + "current".length, insert: "draft" },
    });
    expect(
      view.dom.querySelector(".cm-frontmatter-chip")?.textContent,
    ).toContain("draft");
    view.destroy();
  });

  test("a hand edit that removes the closing fence unfolds rather than hides", () => {
    // Nothing may stay behind a chip once there is no block to summarize:
    // the region goes null, the recompute hands back an empty set and every
    // line is on screen again - including the ones that were folded.
    const view = mount(DOC);
    const at = view.state.sliceDoc().lastIndexOf("---");
    view.dispatch({ changes: { from: at, to: at + 3, insert: "" } });
    expect(view.dom.querySelector(".cm-frontmatter-chip")).toBeNull();
    expect(view.contentDOM.textContent).toContain("status: current");
    view.destroy();
  });

  test("backspace at the fold's edge takes the whole block, and undo restores it", () => {
    // The sharp end of atomicity, pinned rather than discovered later: with
    // the caret at the region's trailing edge - exactly where the left-arrow
    // walk above parks it - one Backspace deletes all six hidden lines,
    // because the delete command pulls its target out of the atomic range to
    // the range's start. This is CodeMirror's own folded-range behavior and
    // it is accepted (see the module doc); what must not change silently is
    // that it stays ONE undo away.
    const view = mount(DOC, [history()]);
    view.dispatch({
      selection: { anchor: DOC.lastIndexOf("---") + "---".length },
    });
    deleteCharBackward(view);
    expect(view.state.sliceDoc()).toBe("\n\n# Body\n");
    // Loudly, rather than quietly: the chip is gone and no yaml took its
    // place, so nothing about the screen suggests the block is still there.
    expect(view.dom.querySelector(".cm-frontmatter-chip")).toBeNull();
    undo(view);
    expect(view.state.sliceDoc()).toBe(DOC);
    // Restored as text, not as a chip: the unfolded state is terminal, so
    // what comes back is the block itself, in full view.
    expect(view.contentDOM.textContent).toContain("status: current");
    view.destroy();
  });

  test("the dark chip rule is scoped to a class the dark editor wears", () => {
    // The trap this pins: a base theme's `&dark` block is rewritten to the
    // view's dark scope class, and it applies only if the editor root
    // actually carries that class. Asserted through the emitted stylesheet
    // and the root's own classes, because jsdom resolves neither `var()` nor
    // cascade order through `getComputedStyle`.
    const dark = mount(DOC, baseExtensions(true));
    const light = mount(DOC, baseExtensions(false));
    const scopes = chipScopes();
    // One rule per scheme: the plain one and the `&dark` one.
    expect(scopes).toHaveLength(2);
    // The dark root wears both scopes (base plus dark, and the dark rule is
    // emitted second, so it wins at equal specificity); the light root wears
    // only the base one.
    expect(scopes.filter((scope) => dark.dom.matches(scope))).toHaveLength(2);
    expect(scopes.filter((scope) => light.dom.matches(scope))).toHaveLength(1);
    dark.destroy();
    light.destroy();
  });
});
