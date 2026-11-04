/**
 * The editors' text at the reading size: the same 16px and line height the
 * reading page sets prose in, with no inset of its own, so the text, the
 * images and the toolbar line up on one left edge in the reading column.
 *
 * Separate from `editorTheme` because the MANIFEST editor shares that one
 * and keeps its own face. `Prec.high` for the reason `RAW_MONO` gives: two
 * themes setting one property are decided by mount order, and this one is
 * added after the base set.
 */

import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { Prec } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";

/**
 * Headings at the reading page's sizes (spec 3.3): the title 30/38, a section
 * 21/28, a subsection 17/26, against the 16px body.
 */
const readingHeadings = HighlightStyle.define([
  {
    tag: tags.heading1,
    fontSize: "1.875em",
    fontWeight: "650",
    letterSpacing: "-0.02em",
    lineHeight: "1.27",
  },
  { tag: tags.heading2, fontSize: "1.3125em", fontWeight: "620" },
  { tag: tags.heading3, fontSize: "1.0625em", fontWeight: "600" },
]);

export const readingTheme: Extension = Prec.high([
  syntaxHighlighting(readingHeadings),
  EditorView.theme({
    "&": { fontSize: "1rem" },
    ".cm-scroller": { lineHeight: "1.72" },
    ".cm-content": { padding: "0.5rem 0 3rem" },
    ".cm-line": { padding: "0" },
    // A callout's lines inset like the page's callout: its words in from the
    // rule, its head and foot padded, the right corners rounded.
    ".cm-line.callout": { padding: "0 0.75rem 0 1rem" },
    ".cm-line.callout.cm-callout-first": {
      paddingTop: "0.5rem",
      borderTopRightRadius: "0.375rem",
    },
    ".cm-line.callout.cm-callout-last": {
      paddingBottom: "0.5rem",
      borderBottomRightRadius: "0.375rem",
    },
    // A picture under its line stands on the text's own left edge, as on
    // the reading page; one in place of its reference sits in the line.
    ".cm-image-preview.cm-picture-block": { padding: "0.5rem 0" },
    // A frontmatter block shown as text (the Reflection editor) reads as the
    // page's settings: small, mono and plain, even where the markdown parser
    // takes a `key:` line over the closing fence for a heading.
    ".cm-line.cm-frontmatter": {
      fontFamily: "var(--font-mono, ui-monospace, monospace)",
      fontSize: "0.8125rem",
    },
    ".cm-line.cm-frontmatter *": {
      fontSize: "inherit",
      fontWeight: "inherit",
    },
  }),
]);
