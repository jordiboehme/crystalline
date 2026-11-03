/**
 * The live-preview decoration layer: a read-model over the buffer, never a
 * write-model. Syntax markers (heading #s, emphasis stars, backticks, link
 * brackets and targets, list, task and quote prefixes) and HTML comments,
 * which the reading page never draws, fold away wherever the hidden-mark
 * model says a line hides its marks (`marks/hiddenMarks.ts`): every line by
 * default, and with the setting on every line but the ones a selection
 * touches, which show exactly what is written. Nothing here rewrites the
 * document: a decoration is a view over the bytes, and the bytes come back out
 * of the buffer exactly as they went in.
 *
 * The list, task and quote marks are drawn by `marks/blocks.ts` (bullets,
 * numbers, task boxes, quote bars), so they are left to it here.
 *
 * The frontmatter block is left entirely alone - the parser misreads its
 * fences as thematic breaks, and the form panel is that region's structured
 * view.
 */

import { syntaxTree } from "@codemirror/language";
import type { Extension, Range } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import { Decoration, EditorView, ViewPlugin } from "@codemirror/view";

import { openWindow } from "../base";
import {
  answersElementEdits,
  elementEditAt,
  macMenuClick,
  pressElement,
  writable,
} from "./elementEdit";
import { linkAddress, linkAt, openableAddress } from "./elementVerbs";
import { frontmatterRegion } from "./frontmatterRegion";
import type { MarkKind } from "./marks/hiddenMarks";
import { marksOnLine } from "./marks/hiddenMarks";
import { markModeChanged, marksOnEditLine } from "./marks/mode";
import { parkingChanged } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";

/** The marks `marks/blocks.ts` draws in their place. */
const BLOCK_MARKS = new Set<MarkKind>(["list", "task", "quote"]);

/**
 * The hidden marks of the visible lines, from the model (`marks/hiddenMarks.ts`):
 * each one folds away as a plain replace, except the block marks (list, task
 * and quote), which `marks/blocks.ts` draws.
 */
function buildDecorations(view: EditorView): DecorationSet {
  const { state } = view;
  const doc = state.doc;
  const decorations: Range<Decoration>[] = [];
  // A line two visible ranges both reach is drawn once.
  let drawnTo = 0;
  for (const { from, to } of view.visibleRanges) {
    let number = Math.max(doc.lineAt(from).number, drawnTo + 1);
    const last = doc.lineAt(to).number;
    for (; number <= last; number += 1) {
      for (const mark of marksOnLine(state, number)) {
        if (BLOCK_MARKS.has(mark.kind)) {
          continue;
        }
        decorations.push(Decoration.replace({}).range(mark.from, mark.to));
      }
    }
    drawnTo = last;
  }
  return Decoration.set(decorations, true);
}

const hiddenMarkPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        // A selection reveals marks only with the setting on.
        (update.selectionSet && update.state.facet(marksOnEditLine)) ||
        update.viewportChanged ||
        parseAdvanced(update) ||
        parkingChanged(update) ||
        markModeChanged(update)
      ) {
        this.decorations = buildDecorations(update.view);
      }
    }
  },
  {
    decorations: (value) => value.decorations,
    // Hidden text is not a place a cursor may sit: arrowing through a folded
    // mark steps over it rather than into the middle of it.
    provide: (plugin) =>
      EditorView.atomicRanges.of(
        (view) => view.plugin(plugin)?.decorations ?? Decoration.none,
      ),
  },
);

/** A quiet tint over the frontmatter block, so the region reads as metadata. */
const frontmatterTint = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = this.tint(view);
    }

    update(update: ViewUpdate) {
      if (update.docChanged) {
        this.decorations = this.tint(update.view);
      }
    }

    tint(view: EditorView): DecorationSet {
      const region = frontmatterRegion(view.state.doc);
      if (!region) {
        return Decoration.none;
      }
      const lines: Range<Decoration>[] = [];
      const last = view.state.doc.lineAt(region.to).number;
      for (let number = 1; number <= last; number += 1) {
        const line = view.state.doc.line(number);
        lines.push(
          Decoration.line({ class: "cm-frontmatter" }).range(line.from),
        );
      }
      return Decoration.set(lines);
    }
  },
  { decorations: (value) => value.decorations },
);

const previewTheme = EditorView.baseTheme({
  ".cm-frontmatter": { color: "var(--color-slate-500)", fontSize: "0.85em" },
});

/** The words of every Markdown link in sight, marked as the place a press opens it. */
function buildLinkTexts(view: EditorView): DecorationSet {
  const { state } = view;
  const marks: Range<Decoration>[] = [];
  const seen = new Set<number>();
  for (const { from, to } of view.visibleRanges) {
    syntaxTree(state).iterate({
      from,
      to,
      enter: (node) => {
        if (node.name !== "Link" || seen.has(node.from)) {
          return;
        }
        seen.add(node.from);
        const link = linkAt(state, node.from);
        if (link?.from === node.from && link.text.from < link.text.to) {
          marks.push(LINK_TEXT.range(link.text.from, link.text.to));
        }
      },
    });
  }
  return Decoration.set(marks, true);
}

const LINK_TEXT = Decoration.mark({ class: "cm-link-text" });

/**
 * A press on a link's words. Cmd or Ctrl opens a web or mail address in a
 * new tab (never this one); a plain press asks for the "Link" popover. With
 * marks hidden on the line the press places no caret inside the link; with
 * the setting to show them the caret goes in as it always did, and the
 * popover opens too. True when the press is handled here, which keeps
 * CodeMirror from placing a caret for it.
 */
export function linkPressed(event: MouseEvent, view: EditorView): boolean {
  if (
    event.button !== 0 ||
    macMenuClick(event) ||
    !(event.target instanceof Element)
  ) {
    return false;
  }
  const el = event.target.closest(".cm-link-text");
  if (!el || !view.contentDOM.contains(el)) {
    return false;
  }
  const { state } = view;
  const pos = view.posAtDOM(el);
  if (!linkAt(state, pos)) {
    return false;
  }
  if (event.metaKey || event.ctrlKey) {
    const address = openableAddress(linkAddress(state, pos) ?? "");
    if (address !== null) {
      openWindow(address, "noopener,noreferrer");
    }
    event.preventDefault();
    return true;
  }
  if (!writable(state) || !answersElementEdits(view)) {
    return false;
  }
  const edit = elementEditAt(view, pos, el.getBoundingClientRect());
  if (!edit) {
    return false;
  }
  return pressElement(view, event, edit, pos);
}

const linkTexts = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildLinkTexts(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.viewportChanged ||
        parseAdvanced(update)
      ) {
        this.decorations = buildLinkTexts(update.view);
      }
    }
  },
  {
    decorations: (value) => value.decorations,
    eventHandlers: {
      mousedown: (event, view) => linkPressed(event, view),
    },
  },
);

/**
 * Link presses, for `hiddenMarkLayer()`: like the drawing, outside
 * `livePreview()`, so a table cell does not open popovers before it has the
 * layer.
 */
export const linkClicks: Extension = linkTexts;

/**
 * The hidden marks drawn, for `hiddenMarkLayer()`: outside `livePreview()`, so
 * the table cell editor, which reads the prose layers, does not get it yet.
 */
export const hiddenMarkDrawing: Extension = hiddenMarkPlugin;

/** The whole layer, handed to the caller's on/off compartment. */
export function livePreview(): Extension {
  return [frontmatterTint, previewTheme];
}
