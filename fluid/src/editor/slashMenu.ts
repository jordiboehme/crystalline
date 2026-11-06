/**
 * The `/` menu: the insert registry as a completion list, opened by a slash
 * at the start of an empty line and filtered as the person types on.
 *
 * Only a slash that starts a line opens it, so a path or a fraction in a
 * sentence never does, and a space right after the slash closes it: `/ ` is
 * text. It stays shut where no block can start at that line: inside an HTML
 * block or a comment that runs over lines, and on a line that lazily
 * continues a quote or callout (no `>`, still the quote's paragraph). Words may follow with spaces between them (`/code bl`); a query that
 * no longer matches anything closes the list and leaves what was typed.
 *
 * The filtering is our own rather than the completion's fuzzy match, which
 * reads only the label: an element is also found by the words it is known by
 * (`/mermaid`, `/todo`). The best match comes first: a label that starts with
 * the query, then a word of a label, then a keyword.
 *
 * Choosing an entry runs its registry action with the `/query` as the range
 * to replace, in one transaction, so one undo puts the typed text back.
 */

import type {
  Completion,
  CompletionResult,
  CompletionSection,
  CompletionSource,
} from "@codemirror/autocomplete";

import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";

import type { InsertAction } from "./inserts";
import { INSERT_ACTIONS } from "./inserts";
import { inCompletableProse } from "./prose";
import { PARSE_BUDGET, deadlineIn, parsedTo } from "./marks/parse";

/** A slash, then nothing or a word and more words: the whole line so far. */
const QUERY = /^\/(?:\w[\w ]*)?$/;

/** The blocks a line inside of is no place to start an element. */
const NO_BLOCK_START = new Set([
  "Blockquote",
  "HTMLBlock",
  "CommentBlock",
  "ProcessingInstructionBlock",
]);

/** Whether `pos` sits inside a block no element may start in. */
function insideClosedBlock(state: EditorState, pos: number): boolean {
  let inside = false;
  syntaxTree(state).iterate({
    from: pos,
    to: pos,
    enter: (node) => {
      if (NO_BLOCK_START.has(node.name)) {
        inside = true;
        return false;
      }
    },
  });
  return inside;
}

/** The sections the full list is drawn in. Diagram goes last: it is the longest. */
const SECTIONS: Record<InsertAction["group"], CompletionSection> = {
  Callout: { name: "Callout", rank: 0 },
  Blocks: { name: "Blocks", rank: 1 },
  Inline: { name: "Inline", rank: 2 },
  Diagram: { name: "Diagram", rank: 3 },
};

/**
 * How well `action` answers `query` (lower is better), or null when it does
 * not: the label starts with it, a word of the label does, or a keyword does.
 */
function rank(action: InsertAction, query: string): number | null {
  const label = action.label.toLowerCase();
  if (label.startsWith(query)) {
    return 0;
  }
  if (` ${label}`.includes(` ${query}`)) {
    return 1;
  }
  if (action.keywords.some((word) => word.startsWith(query))) {
    return 2;
  }
  return null;
}

function option(action: InsertAction, sectioned: boolean): Completion {
  return {
    label: action.label,
    ...(action.subgroup === undefined ? {} : { detail: "Diagram" }),
    type: "text",
    ...(sectioned ? { section: SECTIONS[action.group] } : {}),
    apply: (view, _completion, from, to) => {
      action.run(view, { from, to });
    },
  };
}

export const slashCompletions: CompletionSource = (
  context,
): CompletionResult | null => {
  const line = context.state.doc.lineAt(context.pos);
  const before = context.state.sliceDoc(line.from, context.pos);
  if (!QUERY.test(before)) {
    return null;
  }
  // Typing the `/` is itself an edit that can leave the tree short, and a short
  // tree holds no fence: a pick would write its element into one (D35). Only a
  // line that asks gets this parse; short of time, no menu rather than a wrong
  // one, and the next letter asks again.
  const state = parsedTo(
    context.state,
    line.to,
    deadlineIn(PARSE_BUDGET.typing),
  );
  if (
    !state ||
    !inCompletableProse(state, context.pos) ||
    insideClosedBlock(state, context.pos)
  ) {
    return null;
  }
  const query = before.slice(1).toLowerCase();
  const matches = INSERT_ACTIONS.flatMap((action, index) => {
    const score = rank(action, query);
    return score === null ? [] : [{ action, score, index }];
  }).sort((a, b) => a.score - b.score || a.index - b.index);
  if (matches.length === 0) {
    return null;
  }
  // The full list reads best in its sections; a filtered one best match first.
  const sectioned = query === "";
  return {
    from: line.from,
    to: context.pos,
    options: matches.map(({ action }) => option(action, sectioned)),
    filter: false,
  };
};
