/**
 * Crystalline's own line conventions, made visible while staying plain text:
 * the bracket category of an observation, the rel type of a relation, and the
 * trailing tags, each marked so a writer can see the line landed as what they
 * meant. Completion feeds on the domain's vocabulary - what is already in
 * use, never an enforced set.
 */

import type { CompletionSource } from "@codemirror/autocomplete";
import type { Extension, Range } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import { Decoration, EditorView, ViewPlugin } from "@codemirror/view";

import type { Vocabulary } from "../api/vocabulary";
import { frontmatterRegion } from "./frontmatterRegion";

export const OBSERVATION_LINE = /^-\s+\[([^\]\s][^\]]*)\]\s/;
export const RELATION_LINE = /^-\s+([a-z][\w-]*)\s+\[\[/;
const TRAILING_TAG = /#[\w-]+/g;

function buildMarks(view: EditorView): DecorationSet {
  const marks: Range<Decoration>[] = [];
  const doc = view.state.doc;
  const fmEnd = frontmatterRegion(doc)?.to ?? -1;
  for (const { from, to } of view.visibleRanges) {
    let position = from;
    while (position <= to) {
      const line = doc.lineAt(position);
      if (line.from > fmEnd) {
        const observation = OBSERVATION_LINE.exec(line.text);
        if (observation) {
          const category = observation[1] ?? "";
          const start = line.from + line.text.indexOf("[");
          marks.push(
            Decoration.mark({ class: "cm-obs-category" }).range(
              start,
              start + category.length + 2,
            ),
          );
          for (const tag of line.text.matchAll(TRAILING_TAG)) {
            marks.push(
              Decoration.mark({ class: "cm-line-tag" }).range(
                line.from + tag.index,
                line.from + tag.index + tag[0].length,
              ),
            );
          }
        } else {
          const relation = RELATION_LINE.exec(line.text);
          if (relation) {
            const relType = relation[1] ?? "";
            const start = line.from + line.text.indexOf(relType);
            marks.push(
              Decoration.mark({ class: "cm-rel-type" }).range(
                start,
                start + relType.length,
              ),
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
      if (update.docChanged || update.viewportChanged) {
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
    color: "var(--color-sky-700)",
  },
  ".cm-line-tag": { color: "var(--color-slate-500)" },
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
