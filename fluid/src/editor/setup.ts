/**
 * The baseline every editor surface shares: markdown parsing with GFM and
 * per-language fenced-code highlighting, history, the standard keymaps,
 * in-document search, wrapping, and a theme per scheme.
 *
 * Nothing in here ever rewrites the document: the parser is a read-model for
 * decorations and highlighting, and the buffer stays the literal file text.
 */

import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { search, searchKeymap } from "@codemirror/search";
import type { Extension } from "@codemirror/state";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { tags } from "@lezer/highlight";

/**
 * Round-trip the document's own line endings. CodeMirror splits an incoming
 * string on `/\r\n?|\n/` unless the state names a separator, and rebuilds it
 * from whatever separator the state carries, so a buffer that names none
 * silently rewrites a CRLF file to LF on save - the exact diff noise this
 * editor exists to never make.
 *
 * A separator is always named, never only for the CRLF case. The default
 * split also swallows a lone `\r`, which is a byte an LF document is entitled
 * to contain, and pinning "\n" leaves it inside the line it was written on.
 * Between the two branches, every byte sequence survives the round trip: the
 * separator claims the endings the document actually uses and anything else
 * is line content.
 */
export function lineSeparatorFor(content: string): Extension[] {
  return [EditorState.lineSeparator.of(separatorOf(content))];
}

/**
 * The one spelling of the separator-detection rule. `lineSeparatorFor` and
 * every buffer swap decide with this; a second inline `includes("\r\n")` is
 * the drift this export exists to prevent.
 */
export function separatorOf(content: string): "\r\n" | "\n" {
  return content.includes("\r\n") ? "\r\n" : "\n";
}

/**
 * The buffer as bytes, and the only sanctioned way to read a document back
 * out for saving.
 *
 * `state.doc.toString()` is not it: `Text` joins its lines with "\n"
 * unconditionally and knows nothing about the separator the state was built
 * with, so it hands back an LF rewrite of a CRLF file without complaint.
 * `sliceDoc` joins with `state.lineBreak`, which is what `lineSeparatorFor`
 * set.
 */
export function docText(state: EditorState): string {
  return state.sliceDoc();
}

/**
 * Build the state a view opens with, or is later reset to wholesale via
 * `view.setState` rather than a dispatch - a full document swap across a
 * changed line separator, say, where a dispatch would split the incoming
 * text with the separator the state already has rather than the one the
 * text actually uses.
 *
 * The accessible name and the doc-changed subscription live here, not beside
 * the view: `setState` replaces the whole config, so anything a caller wants
 * to survive the swap has to travel inside the state it swaps in.
 */
export function buildEditorState(
  doc: string,
  extensions: Extension[],
  ariaLabel: string,
  onDocChanged?: (doc: string) => void,
): EditorState {
  return EditorState.create({
    doc,
    extensions: [
      ...extensions,
      // The name goes on the element that is actually the text box.
      // CodeMirror gives its content `role="textbox"` and the editing
      // focus, while the host below is a plain div, and an aria-label on
      // an element with no role names nothing.
      EditorView.contentAttributes.of({ "aria-label": ariaLabel }),
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          // `docText` rather than `doc.toString()`: what a subscriber
          // gets is the file's own bytes, endings included.
          onDocChanged?.(docText(update.state));
        }
      }),
    ],
  });
}

/**
 * Replace the whole buffer with `content`, preserving whichever line ending
 * `content` actually uses - shared by "take the server version", "restore
 * draft" and the external swaps a session hands in, all of which bring in
 * text that was never typed into this session's own state.
 *
 * A plain `view.dispatch` splits an inserted string using the STATE's
 * existing line separator (`ChangeSet.of` reads
 * `state.facet(EditorState.lineSeparator)`), not the one the string itself
 * uses, so a swap across CRLF and LF rebuilds the state fresh with
 * `content`'s own separator via `view.setState`, in place on the same view
 * rather than a full remount, so everything around the buffer survives.
 * `setState` does not run the transaction pipeline, so the rebuilt state's
 * own doc-changed subscription never fires for the swap itself and
 * `onDocChanged` is called directly afterward.
 *
 * `extensionsFor` and `ariaLabel` travel in because `setState` replaces the
 * whole configuration: every layer has to be rebuilt into the new state as it
 * stands right now - the decoration compartment at whatever the toggle is on,
 * the resolver at whatever the graph has answered - or a separator-changing
 * swap would silently drop them. It takes the content because a rebuilt
 * state's line separator comes from the text being swapped in, not from the
 * one being replaced.
 */
export function replaceBuffer(
  view: EditorView,
  content: string,
  extensionsFor: (content: string) => Extension[],
  ariaLabel: string,
  onDocChanged: (doc: string) => void,
): void {
  const mounted = view.state.facet(EditorState.lineSeparator) ?? "\n";
  if (separatorOf(content) === mounted) {
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: content },
    });
    return;
  }
  view.setState(
    buildEditorState(content, extensionsFor(content), ariaLabel, onDocChanged),
  );
  onDocChanged(content);
}

/** The one highlight style; colors lean on the app's slate/sky palette. */
const editorHighlight = HighlightStyle.define([
  { tag: tags.heading1, fontSize: "1.4em", fontWeight: "600" },
  { tag: tags.heading2, fontSize: "1.2em", fontWeight: "600" },
  { tag: tags.heading3, fontSize: "1.1em", fontWeight: "600" },
  { tag: [tags.heading4, tags.heading5, tags.heading6], fontWeight: "600" },
  { tag: tags.strong, fontWeight: "600" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.strikethrough, textDecoration: "line-through" },
  {
    tag: tags.monospace,
    fontFamily: "var(--font-mono, ui-monospace, monospace)",
  },
  {
    tag: tags.link,
    color: "var(--color-sky-700)",
    textDecoration: "underline",
  },
  { tag: tags.url, color: "var(--color-sky-700)" },
  { tag: tags.comment, color: "var(--color-slate-500)" },
  { tag: tags.keyword, color: "var(--color-sky-700)" },
  { tag: tags.string, color: "var(--color-emerald-700)" },
  { tag: tags.number, color: "var(--color-amber-700)" },
  { tag: tags.meta, color: "var(--color-slate-500)" },
  { tag: tags.processingInstruction, color: "var(--color-slate-400)" },
]);

/** The chrome around the text, once per scheme. */
function editorTheme(dark: boolean): Extension {
  return EditorView.theme(
    {
      "&": { fontSize: "0.9375rem" },
      ".cm-content": {
        fontFamily: "inherit",
        padding: "0.75rem 0",
        caretColor: dark ? "#e2e8f0" : "#0f172a",
      },
      ".cm-line": { padding: "0 0.75rem" },
      "&.cm-focused": { outline: "none" },
      ".cm-cursor": { borderLeftColor: dark ? "#e2e8f0" : "#0f172a" },
      ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
        backgroundColor: dark
          ? "rgba(56, 130, 246, 0.30)"
          : "rgba(56, 130, 246, 0.20)",
      },
      ".cm-panels": {
        backgroundColor: dark ? "#0f172a" : "#f8fafc",
        color: "inherit",
      },
    },
    { dark },
  );
}

/** Everything an editor surface starts from. */
export function baseExtensions(dark: boolean): Extension[] {
  return [
    history(),
    search({ top: true }),
    keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap]),
    EditorView.lineWrapping,
    markdown({ base: markdownLanguage, codeLanguages: languages }),
    syntaxHighlighting(editorHighlight),
    editorTheme(dark),
  ];
}
