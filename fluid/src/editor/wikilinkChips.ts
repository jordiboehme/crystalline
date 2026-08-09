/**
 * `[[Target]]` as an atom: drawn as a reference chip in the same three states
 * every other surface draws a reference in, and handed back as plain text the
 * moment a selection touches it. The resolver arrives through a facet because
 * it is built from two requests that land after the editor mounts.
 *
 * Nothing here rewrites the document. A chip is a decoration over the bytes
 * that are already there, and the brackets come back the instant the cursor
 * reaches them, so what an author edits is always the literal text.
 *
 * The completion feeds on title search across every registered domain - the
 * palette's own lookup - and inserts the `domain:Title` prefixed form when the
 * target lives outside the engram being edited. Its ranges come from the
 * completion's own offsets, which are positions in the document rather than
 * indexes into the file's string, and the insertion is an ordinary
 * transaction.
 */

import type { CompletionSource } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import type { Extension, Range } from "@codemirror/state";
import { Facet } from "@codemirror/state";
import type { QueryClient } from "@tanstack/react-query";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";

import { NO_SEARCH, fetchSearch, titleMatchesKey } from "../api/search";
import type { ReferenceState, WikilinkResolver } from "../wikilinks";
import { WIKILINK, parseWikiTarget, referenceState } from "../wikilinks";
import { frontmatterRegion } from "./frontmatterRegion";

/**
 * The one resolver this editor session answers reference questions with.
 *
 * A facet rather than a constructor argument: the answer depends on two
 * requests that land after the buffer is already on screen, and a compartment
 * holding this facet lets them reach the chips without rebuilding the state.
 */
export const wikilinkResolverFacet = Facet.define<
  WikilinkResolver,
  WikilinkResolver
>({
  combine: (values) => values[0] ?? (() => null),
});

/** One `[[Target]]`, drawn as the reference it is. */
class ChipWidget extends WidgetType {
  // Assigned in the body rather than declared as constructor parameters: the
  // build erases types, it does not run a TypeScript transform, and parameter
  // properties are syntax that would need one.
  readonly label: string;
  readonly state: ReferenceState;
  readonly inner: string;

  constructor(label: string, state: ReferenceState, inner: string) {
    super();
    this.label = label;
    this.state = state;
    this.inner = inner;
  }

  override eq(other: ChipWidget): boolean {
    return (
      other.label === this.label &&
      other.state === this.state &&
      other.inner === this.inner
    );
  }

  override toDOM(): HTMLElement {
    const chip = document.createElement("span");
    chip.className = `cm-wikilink cm-wikilink-${this.state}`;
    chip.textContent = this.label;
    // What is actually written, for a reader wondering which domain a
    // prefixed target names.
    chip.title = `[[${this.inner}]]`;
    return chip;
  }
}

/**
 * The subtrees a `[[...]]` is content rather than a reference in: code of
 * either kind, where the brackets are what somebody wrote ABOUT a wikilink.
 * The same call the reader surface makes, where `MarkdownBody` skips its
 * `code` and `pre` elements, so a snippet reads as a snippet on both.
 */
const CODE_CONTEXTS = new Set([
  "InlineCode",
  "FencedCode",
  "CodeBlock",
  "CodeText",
]);

/**
 * Where the parser says code sits inside `from`..`to`.
 *
 * Collected in one pass over the outer tree rather than resolved per match:
 * a fenced block with a language mounts a whole nested tree, and the block's
 * own node is the thing that has to be seen whether or not that inner parse
 * has landed yet.
 */
function codeRanges(
  view: EditorView,
  from: number,
  to: number,
): { from: number; to: number }[] {
  const ranges: { from: number; to: number }[] = [];
  syntaxTree(view.state).iterate({
    from,
    to,
    enter: (node) => {
      if (CODE_CONTEXTS.has(node.name)) {
        ranges.push({ from: node.from, to: node.to });
      }
    },
  });
  return ranges;
}

function buildChips(view: EditorView): DecorationSet {
  const resolve = view.state.facet(wikilinkResolverFacet);
  const doc = view.state.doc;
  // The frontmatter block ends here; -1 when there is none, which no match
  // start can fall at or below. Brackets in metadata are values, not prose.
  const fmEnd = frontmatterRegion(doc)?.to ?? -1;
  const chips: Range<Decoration>[] = [];
  for (const { from, to } of view.visibleRanges) {
    const text = doc.sliceString(from, to);
    const code = codeRanges(view, from, to);
    for (const match of text.matchAll(WIKILINK)) {
      const start = from + match.index;
      const end = start + match[0].length;
      if (start <= fmEnd) {
        continue;
      }
      // Inside code it is text about a wikilink rather than one.
      if (code.some((range) => start >= range.from && start < range.to)) {
        continue;
      }
      // An atom a selection touches is text being edited, not a chip.
      const touched = view.state.selection.ranges.some(
        (range) => range.from <= end && range.to >= start,
      );
      if (touched) {
        continue;
      }
      // WIKILINK's one capture group is not optional, so it is always present
      // in a match; the fallback only documents that to the checker.
      const inner = match[1] ?? "";
      const target = parseWikiTarget(inner);
      chips.push(
        Decoration.replace({
          widget: new ChipWidget(
            // The target text alone: the brackets were the source's way of
            // marking a reference, and the chip marks it now.
            target.target,
            // Bracket text carries no parsed reference of its own, so the
            // payload has nothing to say about it beyond the resolver.
            referenceState(resolve(inner), null),
            inner,
          ),
        }).range(start, end),
      );
    }
  }
  return Decoration.set(chips.sort((a, b) => a.from - b.from || a.to - b.to));
}

const chipsPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildChips(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.selectionSet ||
        update.viewportChanged ||
        // The resolver lands after the buffer does; the chips it was drawn
        // without have to be redrawn when it arrives.
        update.startState.facet(wikilinkResolverFacet) !==
          update.state.facet(wikilinkResolverFacet)
      ) {
        this.decorations = buildChips(update.view);
      }
    }
  },
  {
    decorations: (value) => value.decorations,
    // A chip is one thing: arrowing across it steps over the whole reference
    // rather than into the middle of the hidden brackets.
    provide: (plugin) =>
      EditorView.atomicRanges.of(
        (view) => view.plugin(plugin)?.decorations ?? Decoration.none,
      ),
  },
);

const chipTheme = EditorView.baseTheme({
  ".cm-wikilink": {
    borderRadius: "0.25rem",
    padding: "0 0.25rem",
  },
  "&light .cm-wikilink": { backgroundColor: "var(--color-slate-100)" },
  "&dark .cm-wikilink": { backgroundColor: "var(--color-slate-800)" },
  "&light .cm-wikilink-resolved": { color: "var(--color-sky-700)" },
  "&dark .cm-wikilink-resolved": { color: "var(--color-sky-300)" },
  // Neither a link nor a broken one yet: drawn as the prose it is until the
  // graph says where it lands.
  ".cm-wikilink-pending": { color: "inherit" },
  ".cm-wikilink-unresolved": {
    textDecoration: "underline dotted",
    opacity: "0.7",
  },
});

/** The chip layer; belongs inside the caller's preview compartment. */
export function wikilinkChips(): Extension {
  return [chipsPlugin, chipTheme];
}

/** How many title matches the completion offers. A list read at a glance. */
const COMPLETION_HITS = 10;

/** The text between an opening `[[` and the cursor, with nothing closing it. */
const OPEN_BRACKETS = /\[\[[^\][]*$/;

/**
 * How long an answer to one term is reused before it is asked again, in ms.
 *
 * The lookup is the palette's, under the palette's key, so a term either of
 * them has already asked about is free for the other within this window.
 */
const TITLE_MATCH_FRESH_MS = 30_000;

/**
 * The `[[` completion source, fed by title search across the domains.
 *
 * `currentDomain` is the engram's own: a hit from anywhere else is inserted
 * prefixed, because a bare title only resolves within the engram's domain.
 *
 * No `validFor` is returned, deliberately. A pattern that matched any
 * bracket-free text would tell CodeMirror the first answer covers every later
 * keystroke, and the list would keep offering the matches for `Be` while
 * somebody typed `Beta Note` - the source would never be asked again. Without
 * one, every keystroke re-asks, and three things keep that from becoming a
 * request storm: CodeMirror waits out `activateOnTypingDelay` before starting
 * a query, it never runs a second query for a source while one is still in
 * flight (further keystrokes ride along on the running one and it restarts
 * once, afterwards, if the answer no longer fits), and the fetch itself goes
 * through the query cache, where an in-flight term is shared rather than
 * refetched and a term asked again inside the freshness window is not asked
 * at all.
 */
export function wikilinkCompletions(
  currentDomain: string,
  client: QueryClient,
): CompletionSource {
  return async (context) => {
    const match = context.matchBefore(OPEN_BRACKETS);
    if (!match) {
      return null;
    }
    // Past the two brackets: what is replaced is the target text, never the
    // opening pair the author typed.
    const from = match.from + 2;
    const term = context.state.sliceDoc(from, context.pos);
    if (term === "") {
      // Opened on the bare `[[`: nothing to look up yet, but an answer rather
      // than nothing, so the session stays open for the first typed
      // character to ask.
      return { from, options: [] };
    }
    const page = await client.fetchQuery({
      // The palette's own key: the two lookups are the same question, and one
      // having asked it already spares the other the request.
      queryKey: titleMatchesKey(term),
      queryFn: () => fetchSearch({ ...NO_SEARCH, q: term, mode: "title" }, 1),
      staleTime: TITLE_MATCH_FRESH_MS,
    });
    return {
      from,
      options: page.hits.slice(0, COMPLETION_HITS).map((hit) => {
        const foreign = hit.domain !== currentDomain;
        return {
          label: hit.title,
          type: "text",
          // Named only where it is news: every other hit lives right here.
          ...(foreign ? { detail: hit.domain } : {}),
          apply: (
            view: EditorView,
            _completion: unknown,
            applyFrom: number,
            applyTo: number,
          ) => {
            const name = foreign ? `${hit.domain}:${hit.title}` : hit.title;
            // Swallow a closing pair the author already typed rather than
            // leaving `]]]]` behind.
            const closed =
              view.state.sliceDoc(applyTo, applyTo + 2) === "]]"
                ? applyTo + 2
                : applyTo;
            const insert = `${name}]]`;
            view.dispatch({
              changes: { from: applyFrom, to: closed, insert },
              selection: { anchor: applyFrom + insert.length },
            });
          },
        };
      }),
    };
  };
}
