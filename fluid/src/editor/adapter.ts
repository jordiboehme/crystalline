/**
 * What one kind of document adds to the shared editor surface.
 *
 * Both editors mount the same surface (`surface.ts`): the format bar, its
 * commands, the live preview, the fence and image previews. What differs is
 * here, behind one interface. An engram has observation and relation lines,
 * wikilink chips and completions, the vocabulary, and a frontmatter block the
 * Details rail edits. A Reflection page has hidden section id lines, which no
 * command may part from their headings, and a heading change that makes a
 * section vanish into the one above is asked about first.
 */

import type { CompletionSource } from "@codemirror/autocomplete";
import type { EditorState, Extension } from "@codemirror/state";
import type { QueryClient } from "@tanstack/react-query";

import type { Vocabulary } from "../api/vocabulary";
import { crystallineCompletions, crystallineLines } from "./crystallineLines";
import { footnotes } from "./footnotes";
import { frontmatterFold } from "./frontmatterFold";
import { hiddenIdLines, isIdLineAt } from "./hiddenIdLines";
import { wikilinkChips, wikilinkCompletions } from "./wikilinkChips";

export interface DocumentAdapter {
  kind: "engram" | "reflection";
  /**
   * Behaviour only this kind of document has, on in every mode (the
   * Reflection's hidden id lines and their guards).
   */
  extensions: Extension[];
  /**
   * Read-model layers only this kind draws (the engram's chips, its
   * observation and relation lines, its frontmatter fold). They sit inside
   * the preview compartment, so the Raw toggle turns them off with the rest
   * of the preview.
   */
  previewLayers: Extension[];
  /**
   * Whether this kind links engrams with `[[...]]`, which decides whether
   * the format bar offers Wiki link. A Reflection page renders the brackets
   * as plain text.
   */
  wikilinks: boolean;
  /** The completions this kind offers while typing, in every mode. */
  completions: CompletionSource[];
  /**
   * Asked before a heading change on line `line` (1-based) that leaves it at
   * `nextLevel` (0 is no heading). Answers the sentence to confirm, or null
   * to go ahead.
   */
  confirmHeadingChange(
    state: EditorState,
    line: number,
    nextLevel: number,
  ): string | null;
}

/**
 * What a Reflection page says before a section heading stops being one.
 *
 * The section's provenance record goes with it on the next save, so the
 * agent rebuilds the sources. Nothing here promises that threads or sources
 * move with the text.
 */
export const JOIN_SECTIONS =
  "This joins the section with the one above. The agent will see the change and rebuild its sources.";

export function engramAdapter(opts: {
  /** The engram's own domain, which decides what a completion prefixes. */
  domain: string;
  /** The cache the completion's title lookup goes through. */
  client: QueryClient;
  /**
   * The domain's vocabulary, read fresh on every completion rather than
   * closed over: the fetch behind it lands after the buffer mounts.
   */
  vocab: () => Vocabulary | null;
}): DocumentAdapter {
  return {
    kind: "engram",
    wikilinks: true,
    extensions: [],
    previewLayers: [
      wikilinkChips(),
      crystallineLines(),
      footnotes(),
      // The Details rail is the metadata surface, so the block itself is
      // hidden from the text here rather than being shown twice; Raw shows
      // it. The MANIFEST editor deliberately does not get this: it has no
      // form, so its frontmatter is only ever visible in the buffer.
      frontmatterFold(),
    ],
    completions: [
      wikilinkCompletions(opts.domain, opts.client),
      crystallineCompletions(opts.vocab),
    ],
    confirmHeadingChange: () => null,
  };
}

export function reflectionAdapter(): DocumentAdapter {
  return {
    kind: "reflection",
    wikilinks: false,
    extensions: [hiddenIdLines()],
    previewLayers: [footnotes()],
    // No wikilink completions: a Reflection page links no engrams.
    completions: [],
    confirmHeadingChange: (state, line, nextLevel) =>
      // A section is a `##` with its id line below it. Made `###`, a title
      // or plain text, it joins the section above, and its id goes.
      isIdLineAt(state, line + 1) && nextLevel !== 2 ? JOIN_SECTIONS : null,
  };
}
