/**
 * Crystalline's own line conventions, made visible while staying plain text:
 * the bracket category of an observation, the rel type of a relation, and the
 * trailing tags, each marked so a writer can see the line landed as what they
 * meant. Completion feeds on the domain's vocabulary - what is already in
 * use, never an enforced set.
 */

import type { CompletionSource } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import type { Extension, Range } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import { Decoration, EditorView, ViewPlugin } from "@codemirror/view";

import type { Vocabulary } from "../api/vocabulary";
import { frontmatterRegion } from "./frontmatterRegion";
import {
  marksHiddenOnLine,
  markModeChanged,
  marksOnEditLine,
} from "./marks/mode";
import { parkingChanged } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";
import { CODE_CONTEXTS, inCompletableProse } from "./prose";
import {
  joinBullet,
  placeTags,
  relationType,
  splitCategory,
} from "./relationLines";

/**
 * The bracket span `[from, to)` of an observation line's category, in line
 * offsets, or null: `split_category` (parse.rs, `splitCategory` here) on the
 * content after the literal "- " `top_level_bullet` strips. Not "-\s+": a
 * tab after the dash is no bullet to the server's parser, and a nested or
 * indented bullet is no observation. A task box (`[ ]`, `[x]`, `[X]`,
 * `[~]`) and a footnote reference (`- [^1] text`) are never a category; a
 * category may start with a space and may end the line, as on the server.
 */
export function observationCategory(
  text: string,
): { from: number; to: number } | null {
  if (!text.startsWith("- ")) {
    return null;
  }
  const span = splitCategory(text.slice(2));
  return span ? { from: span.from + 2, to: span.to + 2 } : null;
}

const TRAILING_TAG = /#[\w-]+/g;

/** `is_hashtag` (parse.rs): `#` and then one or more of A-Z a-z 0-9 `_` `-` `/`. */
const HASHTAG = /^#[A-Za-z0-9_/-]+$/;

/**
 * The tags of an observation line as `split_observation` (parse.rs) reads
 * them: from the text after the category, a trailing parenthesized group is
 * the context and is set aside, then whitespace-separated hashtag tokens are
 * taken from the end until one is not a tag. Offsets are in the line.
 */
function serverTags(
  text: string,
  bodyFrom: number,
): { from: number; to: number }[] {
  let end = text.length;
  while (end > bodyFrom && /\s/.test(text[end - 1] ?? "")) {
    end -= 1;
  }
  if (text[end - 1] === ")") {
    const open = text.lastIndexOf("(", end - 1);
    if (open >= bodyFrom) {
      end = open;
    }
  }
  const found: { from: number; to: number }[] = [];
  for (;;) {
    while (end > bodyFrom && /\s/.test(text[end - 1] ?? "")) {
      end -= 1;
    }
    let start = end;
    while (start > bodyFrom && !/\s/.test(text[start - 1] ?? "")) {
      start -= 1;
    }
    if (start >= end || !HASHTAG.test(text.slice(start, end))) {
      break;
    }
    found.unshift({ from: start, to: end });
    end = start;
  }
  return found;
}

/**
 * Where a bullet is code rather than an observation or relation line.
 * Mirrors `wikilinkChips.ts`'s `codeRanges`: the server's `scan_body` skips
 * a `body_lines` entry whose `in_fence` is set (parse.rs), and a line
 * inside a fence is exactly what a `CODE_CONTEXTS` node marks here. One
 * pass over the outer tree rather than resolved per line, since a fenced
 * block with a language mounts a nested tree and the block's own node is
 * what has to be seen whether or not that inner parse has landed.
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

function buildMarks(view: EditorView): DecorationSet {
  const marks: Range<Decoration>[] = [];
  const doc = view.state.doc;
  const fmEnd = frontmatterRegion(doc)?.to ?? -1;
  for (const { from, to } of view.visibleRanges) {
    const code = codeRanges(view, from, to);
    let position = from;
    while (position <= to) {
      const line = doc.lineAt(position);
      const inCode = code.some(
        (range) => line.from >= range.from && line.from < range.to,
      );
      if (line.from > fmEnd && !inCode) {
        // Hidden marks draw chips on the words; a line whose marks show (the
        // caret line in show mode, Raw) keeps the bracketed marking.
        const chips = marksHiddenOnLine(view.state, line.number);
        // The n-th line after this one, unless it is code.
        const lineAfter = (offset: number): string | undefined => {
          const number = line.number + offset;
          if (number > doc.lines) {
            return undefined;
          }
          const next = doc.line(number);
          return code.some(
            (range) => next.from >= range.from && next.from < range.to,
          )
            ? undefined
            : next.text;
        };
        const observation = observationCategory(line.text);
        if (observation) {
          const start = line.from + observation.from;
          const end = line.from + observation.to;
          marks.push(
            chips
              ? Decoration.mark({ class: "cm-category-chip" }).range(
                  start + 1,
                  end - 1,
                )
              : Decoration.mark({ class: "cm-obs-category" }).range(start, end),
          );
          if (chips) {
            // The server reads tags from the joined bullet, so a wrapped
            // observation's chips sit on whichever line holds them.
            const bullet = joinBullet(lineAfter);
            const found = bullet
              ? placeTags(
                  bullet,
                  serverTags(bullet.content, observation.to - 2),
                )
              : [];
            for (const tag of found) {
              const holder = doc.line(line.number + tag.offset);
              marks.push(
                Decoration.mark({ class: "cm-tag-chip" }).range(
                  holder.from + tag.from,
                  holder.from + tag.to,
                ),
              );
            }
          } else {
            for (const tag of line.text.matchAll(TRAILING_TAG)) {
              marks.push(
                Decoration.mark({ class: "cm-line-tag" }).range(
                  line.from + tag.index,
                  line.from + tag.index + tag[0].length,
                ),
              );
            }
          }
        } else {
          const span = relationType(lineAfter);
          if (span) {
            marks.push(
              Decoration.mark({
                class: chips ? "cm-relation-chip" : "cm-rel-type",
              }).range(line.from + span.from, line.from + span.to),
            );
          }
        }
      }
      position = line.to + 1;
    }
  }
  return Decoration.set(marks.sort((a, b) => a.from - b.from || a.to - b.to));
}

const linesPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildMarks(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.viewportChanged ||
        parseAdvanced(update) ||
        // The chips replace the bracketed marking where marks are hidden, so
        // a selection that reveals a line (show mode), a flip of the setting
        // or of the layer, and a parked selection draw it again.
        (update.selectionSet && update.state.facet(marksOnEditLine)) ||
        markModeChanged(update) ||
        parkingChanged(update)
      ) {
        this.decorations = buildMarks(update.view);
      }
    }
  },
  { decorations: (value) => value.decorations },
);

const linesTheme = EditorView.baseTheme({
  ".cm-obs-category": {
    fontFamily: "var(--font-mono, ui-monospace, monospace)",
    fontSize: "0.85em",
    color: "var(--color-slate-600)",
    backgroundColor: "var(--color-slate-100)",
    borderRadius: "0.25rem",
  },
  ".cm-rel-type": {
    fontFamily: "var(--font-mono, ui-monospace, monospace)",
    fontSize: "0.85em",
    color: "var(--link)",
  },
  ".cm-line-tag": { color: "var(--color-slate-500)" },
  ".cm-category-chip": {
    fontFamily: "var(--font-mono, ui-monospace, monospace)",
    fontSize: "0.85em",
    color: "var(--color-slate-600)",
    backgroundColor: "var(--color-slate-100)",
    borderRadius: "0.25rem",
    padding: "0 0.3em",
  },
  ".cm-relation-chip": {
    fontFamily: "var(--font-mono, ui-monospace, monospace)",
    fontSize: "0.85em",
    color: "var(--link)",
    backgroundColor: "var(--color-slate-100)",
    borderRadius: "0.25rem",
    padding: "0 0.3em",
  },
  ".cm-tag-chip": {
    color: "var(--color-slate-500)",
    backgroundColor: "var(--color-slate-100)",
    borderRadius: "0.25rem",
  },
});

/** The recognition layer; belongs inside the caller's preview compartment. */
export function crystallineLines(): Extension {
  return [linesPlugin, linesTheme];
}

/** Rank a vocabulary list into options, commonest first (already sorted). */
function options(names: { name: string }[], type: string) {
  return names.map((entry) => ({ label: entry.name, type }));
}

/**
 * Completion for the three vocabularies a line can ask about: a category
 * after `- [`, a relation type after `- `, and a tag after `#`. The getter is
 * read per completion, so the vocabulary can arrive late - the extension set
 * is snapshotted once at mount, and a closure over the fetch's own answer
 * would go on returning `null` forever once it landed.
 */
export function crystallineCompletions(
  vocab: () => Vocabulary | null,
): CompletionSource {
  return (context) => {
    if (!inCompletableProse(context.state, context.pos)) {
      return null;
    }
    const words = vocab();
    if (!words) {
      return null;
    }
    const line = context.state.doc.lineAt(context.pos);
    const before = context.state.sliceDoc(line.from, context.pos);

    const category = /^-\s+\[([\w-]*)$/.exec(before);
    if (category) {
      const name = category[1] ?? "";
      return {
        from: context.pos - name.length,
        options: options(words.categories, "keyword"),
        validFor: /^[\w-]*$/,
      };
    }
    const relation = /^-\s+([a-z][\w-]*)?$/.exec(before);
    if (relation) {
      const name = relation[1] ?? "";
      return {
        from: context.pos - name.length,
        options: options(words.relationTypes, "keyword"),
        validFor: /^[\w-]*$/,
      };
    }
    const tag = /#([\w-]*)$/.exec(before);
    if (tag) {
      const name = tag[1] ?? "";
      return {
        from: context.pos - name.length,
        options: options(words.tags, "constant"),
        validFor: /^[\w-]*$/,
      };
    }
    return null;
  };
}
