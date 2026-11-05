/**
 * The hidden-mark model: which source ranges of a line are Markdown marks a
 * reader never sees (`**`, a heading's `## `, a link's `](...)`, a comment).
 *
 * Pure and state only: read off the syntax tree of the one line asked about,
 * with no view and no viewport, so the drawing, the caret rules and the
 * formatting commands all ask the same question and get the same answer.
 * Positions are document positions, never indexes into the file's string.
 *
 * A line whose marks show (`marksHiddenOnLine` false: Raw, or show mode on a
 * touched line) has none, except an HTML comment in a Reflection, which is
 * never shown there. The frontmatter block, code and math hold no marks: a
 * `$$` block too, and an unclosed `$$` to where the page ends it (D41).
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Line, Text } from "@codemirror/state";
import type { SyntaxNode, SyntaxNodeRef, Tree } from "@lezer/common";
import { IterMode } from "@lezer/common";

import { htmlCommentRanges } from "../../markdown/htmlComments";
import { isIdLine } from "../../reflectionText";
import { observationCategory } from "../crystallineLines";
import { frontmatterRegion } from "../frontmatterRegion";
import { inMathRegion, mathRegions } from "../mathBlocks";
import { hidesIdLines, pageHidesComments } from "../hiddenIdLines";
import { drawnAsPicture, marksHiddenOnLine } from "./mode";

export type MarkKind =
  | "strong"
  | "em"
  | "strike"
  | "code"
  | "link"
  | "heading"
  | "list"
  | "task"
  | "quote"
  | "comment"
  | "category";

export interface Mark {
  from: number;
  to: number;
  kind: MarkKind;
  side: "open" | "close" | "prefix" | "whole";
  /** The index of the paired mark in the same line's array, if it is there. */
  partner: number | null;
}

/**
 * An image with no alt text, linked or not (`![](src)`, `[![](src)](href)`):
 * nothing of it is text, so it is no pair of marks but one atom, drawn as one
 * piece that a caret steps over and a key removes whole (D40).
 */
export interface MarkAtom {
  from: number;
  to: number;
}

/** Touching marks taken together: what a caret steps over in one go. */
export interface MarkRun {
  from: number;
  to: number;
  marks: Mark[];
}

/**
 * A test hook: how many syntax nodes `walkTree` has stood on, in all, the
 * ones it stepped over included, plus every line the mark scan looks at for
 * a node and every step of its line search (D29). A cost test reads it to
 * count work instead of timing it.
 */
export const treeWalkWork = { steps: 0 };

/**
 * `tree.iterate({ from, to, enter, leave })`, step for step (named nodes
 * only), counting every node the cursor stands on in `treeWalkWork`.
 */
export function walkTree(
  tree: Tree,
  from: number,
  to: number,
  enter: (node: SyntaxNodeRef) => boolean | undefined,
  leave?: (node: SyntaxNodeRef) => void,
): void {
  const cursor = tree.cursor(IterMode.IncludeAnonymous);
  for (;;) {
    treeWalkWork.steps += 1;
    let entered = false;
    if (
      cursor.from <= to &&
      cursor.to >= from &&
      (cursor.type.isAnonymous || enter(cursor) !== false)
    ) {
      if (cursor.firstChild()) continue;
      entered = true;
    }
    for (;;) {
      if (entered && leave && !cursor.type.isAnonymous) leave(cursor);
      if (cursor.nextSibling()) break;
      if (!cursor.parent()) return;
      entered = true;
    }
  }
}

/** The subtrees that hold no marks at all. */
const NO_MARKS = new Set([
  "FencedCode",
  "CodeBlock",
  "CodeText",
  "InlineMath",
  "StatusLabel",
  "LinkReference",
]);

/** The inline pairs whose first and last child are the marks. */
const PAIRS = new Map<string, MarkKind>([
  ["Emphasis", "em"],
  ["StrongEmphasis", "strong"],
  ["Strikethrough", "strike"],
  ["InlineCode", "code"],
]);

/** The not-applicable box after a list mark, which the parser reads as text. */
const NOT_APPLICABLE = /^\[~\](?=[ \t]|$)/;

/** A link label as CommonMark matches it to a definition. */
export function labelKey(label: string): string {
  return label.trim().replace(/\s+/g, " ").toLowerCase();
}

/** Every link definition's label with its address span, once per parse tree. */
const definitionCache = new WeakMap<
  Tree,
  Map<string, { from: number; to: number } | null>
>();

function definitions(
  state: EditorState,
): Map<string, { from: number; to: number } | null> {
  const tree = syntaxTree(state);
  const cached = definitionCache.get(tree);
  if (cached) {
    return cached;
  }
  const labels = new Map<string, { from: number; to: number } | null>();
  tree.iterate({
    enter: (node) => {
      if (node.name === "LinkReference") {
        const label = node.node.getChild("LinkLabel");
        if (label) {
          const key = labelKey(
            state.doc.sliceString(label.from + 1, label.to - 1),
          );
          // The first definition of a label is the one CommonMark uses.
          if (!labels.has(key)) {
            const url = node.node.getChild("URL");
            labels.set(key, url ? { from: url.from, to: url.to } : null);
          }
        }
        return false;
      }
      // Definitions are blocks: nothing inline or in code holds one.
      if (
        node.name === "Paragraph" ||
        node.name.startsWith("ATXHeading") ||
        node.name.startsWith("SetextHeading") ||
        NO_MARKS.has(node.name) ||
        node.name === "Table"
      ) {
        return false;
      }
    },
  });
  definitionCache.set(tree, labels);
  return labels;
}

/**
 * The address span of the definition `label` names: undefined when no
 * definition has that label, null when it has one without an address.
 */
export function linkDefinition(
  state: EditorState,
  label: string,
): { from: number; to: number } | null | undefined {
  return definitions(state).get(labelKey(label));
}

/** Where the frontmatter block ends, once per document; -1 without one. */
const frontmatterCache = new WeakMap<Text, number>();

function frontmatterEnd(doc: Text): number {
  let end = frontmatterCache.get(doc);
  if (end === undefined) {
    end = frontmatterRegion(doc)?.to ?? -1;
    frontmatterCache.set(doc, end);
  }
  return end;
}

/** A mark before its partner index is known: partners are linked by object. */
interface Draft {
  from: number;
  to: number;
  kind: MarkKind;
  side: Mark["side"];
  pair: Draft | null;
}

function isSpace(char: string): boolean {
  return char === " " || char === "\t";
}

/** One line of a scan: what the walk found on it so far. */
interface LineScan {
  at: Line;
  /** Whether the line's marks are read; else only its comments. */
  hidden: boolean;
  /**
   * Whether the line is `$$` math the page draws (D41): formula source, so
   * only its quote marks are read.
   */
  math: boolean;
  drafts: Draft[];
  /** The images with no alt text on the line. */
  atoms: MarkAtom[];
  /** Whether a code block covers the line's start. */
  inCode: boolean;
  /**
   * Where an observation's category or a task box already took a `[`, so
   * the parser's link over the same brackets is not read twice.
   */
  taken: Set<number>;
  /** The category's bracket span in line offsets (M1), or null. */
  category: { from: number; to: number } | null;
  /** The depth of a node whose inside this line does not read, or -1. */
  skip: number;
  /** The walk step that last found the node in hand on this line. */
  seen: number;
}

/** What one walk over a range of lines found. */
export interface MarkScan {
  /** The marks of each line, as `marksOnLine` gives them: first line first. */
  lines: Mark[][];
  /** The atoms of each line (images with no alt text), first line first. */
  atoms: MarkAtom[][];
  /**
   * The closing child of each pair whose marks are its node's first and last
   * child (an inline pair, a link), by the start of its first child: how an
   * opening mark finds its partner on another line, as `partnerAcrossLines`
   * does from the tree.
   */
  closers: Map<number, { from: number; to: number }>;
}

function addTo(
  scan: LineScan,
  from: number,
  to: number,
  kind: MarkKind,
  side: Mark["side"],
): Draft | null {
  if (from < scan.at.from || to > scan.at.to || from >= to) {
    return null;
  }
  const draft: Draft = { from, to, kind, side, pair: null };
  scan.drafts.push(draft);
  return draft;
}

function pair(open: Draft | null, close: Draft | null): void {
  if (open && close) {
    open.pair = close;
    close.pair = open;
  }
}

/**
 * The first of `lines` (sorted, touching) from `index` on whose end is at or
 * after `pos`. The walk asks with positions that never go back, so a cursor
 * moved forward finds every line in one pass over them in all.
 */
function reaching(lines: readonly Line[], index: number, pos: number): number {
  let at = index;
  while (at < lines.length && (lines[at]?.to ?? 0) < pos) {
    treeWalkWork.steps += 1;
    at += 1;
  }
  return at;
}

/**
 * The marks of line `line` (1-based), sorted by position. Empty when the
 * line's marks show, apart from a Reflection's comments. With `always`, the
 * marks the line has whether they show or not (Raw, show mode on the caret's
 * line, a table cell): what the format commands read a line's formats from.
 */
export function marksOnLine(
  state: EditorState,
  line: number,
  options: { always?: boolean } = {},
): Mark[] {
  return scanMarks(state, line, line, options).lines[0] ?? [];
}

/**
 * The atoms of line `line` (1-based): its images with no alt text, where its
 * marks are hidden. A line with no `![]` has none, without a walk.
 */
export function atomsOnLine(state: EditorState, line: number): MarkAtom[] {
  if (!state.doc.line(line).text.includes("![]")) {
    return [];
  }
  return scanMarks(state, line, line).atoms[0] ?? [];
}

/**
 * `always`: the marks a line has whether they show or not. `pictures`: on
 * the lines whose marks are hidden, a drawn picture's marks too, which the
 * drawing and the caret never read (the picture is one atom there) but a
 * deletion that cuts into the picture is judged by (P2).
 */
export interface ScanOptions {
  always?: boolean;
  pictures?: boolean;
}

/**
 * The marks of lines `first` to `last` (1-based), each line exactly as
 * `marksOnLine` gives it, read in one walk over the syntax tree. Asking line
 * by line walks a paragraph's nodes from its start once per line, which a
 * long hard-wrapped paragraph pays for quadratically; this walks them once.
 */
export function scanMarks(
  state: EditorState,
  first: number,
  last: number,
  options: ScanOptions = {},
): MarkScan {
  const doc = state.doc;
  const fence = frontmatterEnd(doc);
  // A Reflection never shows a comment, in either mode, in a table cell too.
  const comments = hidesIdLines(state) || state.facet(pageHidesComments);
  // `$$` math the page draws is formula source, never marks (D41).
  const math = mathRegions(state);
  const lines: Line[] = [];
  const scans: (LineScan | null)[] = [];
  for (let number = first; number <= last; number++) {
    const at = doc.line(number);
    lines.push(at);
    const hidden =
      at.from > fence &&
      (options.always === true || marksHiddenOnLine(state, number));
    const inMath = math.length > 0 && inMathRegion(math, at.from);
    scans.push(
      at.from <= fence || (!hidden && !comments)
        ? null
        : {
            at,
            hidden,
            math: inMath,
            drafts: [],
            atoms: [],
            inCode: false,
            taken: new Set(),
            category: hidden && !inMath ? observationCategory(at.text) : null,
            skip: -1,
            seen: -1,
          },
    );
  }
  const closers = new Map<number, { from: number; to: number }>();
  const live = scans.filter((scan) => scan !== null);
  const head = live[0];
  const tail = live.at(-1);
  if (head && tail) {
    walkMarks(
      state,
      lines,
      scans,
      closers,
      head.at.from,
      tail.at.to,
      options.always === true || options.pictures === true,
    );
  }
  return {
    lines: scans.map((scan) => (scan ? finished(scan) : [])),
    atoms: scans.map((scan) => scan?.atoms ?? []),
    closers,
  };
}

function walkMarks(
  state: EditorState,
  lines: readonly Line[],
  scans: readonly (LineScan | null)[],
  closers: Map<number, { from: number; to: number }>,
  from: number,
  to: number,
  withPictures: boolean,
): void {
  const doc = state.doc;
  const char = (pos: number) => doc.sliceString(pos, pos + 1);
  // A picture the picture layer draws in place is its span, so the drawing,
  // the caret and the keys read no marks in it (P5). The marks a line has
  // whether they show or not (`always`: the delete check, the format
  // commands) and the marks a deletion keeps (`pictures`) still include its
  // pair, so a range that cuts into it is judged as before.
  const picture = (node: SyntaxNode) =>
    !withPictures && drawnAsPicture(state, node);
  // A `>` with the one optional space after it.
  const quote = (scan: LineScan, from: number, to: number) => {
    addTo(scan, from, isSpace(char(to)) ? to + 1 : to, "quote", "prefix");
  };
  let defined: Map<string, unknown> | null = null;
  const isDefined = (label: string) => {
    defined ??= definitions(state);
    return defined.has(labelKey(label));
  };
  // The first line the node in hand reaches: nodes come in the order they
  // start, so it only moves forward.
  let head = 0;
  // The line holding `pos`, if it was found on the node in hand. Asked
  // with growing positions inside one node, from its first line on.
  let step = 0;
  let near = 0;
  const found = (pos: number): LineScan | null => {
    near = reaching(lines, near, pos);
    const scan = scans[near];
    return scan && scan.seen === step && scan.at.from <= pos ? scan : null;
  };

  /** A link's opening mark, and where its closing mark starts and ends. */
  const linkPlan = (node: SyntaxNode) => {
    const open = node.firstChild;
    if (!open || open.name !== "LinkMark") {
      return null;
    }
    const image = node.name === "Image";
    let close: SyntaxNode | null = open.nextSibling;
    while (close && !(close.name === "LinkMark" && char(close.from) === "]")) {
      close = close.nextSibling;
    }
    if (!close) {
      return null;
    }
    const text = doc.sliceString(open.to, close.from);
    // A footnote reference is Task 10's, never a link.
    if (!image && text.startsWith("^")) {
      return null;
    }
    const after = close.nextSibling;
    // `[[Target]]`: the wikilink chip's, never a link. Only a bare bracket
    // link: an image inside a link (`[![alt](src)](href)`) and a link inside
    // literal brackets (`[[t](u)]`) are what the page reads them as (D39).
    if (
      !image &&
      after === null &&
      char(node.from - 1) === "[" &&
      char(node.to) === "]"
    ) {
      return null;
    }
    let end = close.to;
    if (after?.name === "LinkMark" && char(after.from) === "(") {
      end = node.to;
    } else {
      // A reference: full (`][ref]`), collapsed (`][]`) or shortcut (`]`).
      let label = text;
      if (after?.name === "LinkLabel") {
        end = after.to;
        const written = doc.sliceString(after.from + 1, after.to - 1);
        if (written.trim() !== "") {
          label = written;
        }
      }
      if (!isDefined(label)) {
        return null;
      }
    }
    return { open, close: close.from, end };
  };

  /**
   * Where node `node` ends when it is an image with no alt text, or a link
   * whose words are only such an image: the end of its atom, else null.
   */
  const atomEnd = (
    node: SyntaxNode,
    plan: { open: SyntaxNode; close: number; end: number },
  ): number | null => {
    if (node.name === "Image") {
      return plan.open.to === plan.close ? plan.end : null;
    }
    const image = node.getChild("Image");
    if (
      !image ||
      image.from !== plan.open.to ||
      image.to !== plan.close ||
      // A drawn picture is no atom: the link's own marks hide around it.
      picture(image)
    ) {
      return null;
    }
    const inner = linkPlan(image);
    return inner && inner.open.to === inner.close && inner.end === image.to
      ? plan.end
      : null;
  };

  /** Whether a pair node's last child is its first child's partner. */
  const closer = (node: SyntaxNode) => {
    const open = node.firstChild;
    const close = node.lastChild;
    if (open && close && close.from !== open.from && close.name === open.name) {
      closers.set(open.from, { from: close.from, to: close.to });
    }
  };

  /**
   * The marks node `ref` gives the lines in `hidden` (all of whose marks are
   * read). True when those lines do not read its inside.
   */
  const marksOf = (ref: SyntaxNodeRef, hidden: readonly LineScan[]) => {
    const name = ref.name;
    if (NO_MARKS.has(name)) {
      if (name === "FencedCode" || name === "CodeBlock") {
        for (const scan of hidden) {
          if (ref.from <= scan.at.from && ref.to >= scan.at.from) {
            scan.inCode = true;
          }
        }
        // In a quote or a callout the parser hangs each code line's `>`
        // under the block: those are the quote's marks, not code.
        for (const mark of ref.node.getChildren("QuoteMark")) {
          const scan = found(mark.from);
          if (scan?.hidden) quote(scan, mark.from, mark.to);
        }
      }
      return true;
    }
    const node = ref.node;
    const pairKind = PAIRS.get(name);
    if (pairKind !== undefined) {
      // `$`...`$` is inline math, drawn by `elementPreviews.ts`.
      const math =
        name === "InlineCode" &&
        char(ref.from - 1) === "$" &&
        doc.sliceString(ref.from - 2, ref.from - 1) !== "\\" &&
        char(ref.to) === "$";
      const first = node.firstChild;
      const last = node.lastChild;
      if (!math && first && last && first.from !== last.from) {
        for (const scan of hidden) {
          pair(
            addTo(scan, first.from, first.to, pairKind, "open"),
            addTo(scan, last.from, last.to, pairKind, "close"),
          );
        }
        closer(node);
      }
      // Inside code nothing is a mark; inside emphasis, more may be.
      return name === "InlineCode";
    }
    if (name === "Link" || name === "Image") {
      if (picture(node)) {
        return true;
      }
      const plan = linkPlan(node);
      const atom = plan ? atomEnd(node, plan) : null;
      if (atom !== null) {
        for (const scan of hidden) {
          if (node.from >= scan.at.from && atom <= scan.at.to) {
            scan.atoms.push({ from: node.from, to: atom });
          }
        }
        return true;
      }
      if (plan) {
        for (const scan of hidden) {
          if (!scan.taken.has(plan.open.from)) {
            pair(
              addTo(scan, plan.open.from, plan.open.to, "link", "open"),
              addTo(scan, plan.close, plan.end, "link", "close"),
            );
          }
        }
        closer(node);
      }
      return false;
    }
    if (name === "Autolink") {
      const first = node.firstChild;
      const last = node.lastChild;
      if (first?.name === "LinkMark" && last?.name === "LinkMark") {
        for (const scan of hidden) {
          pair(
            addTo(scan, first.from, first.to, "link", "open"),
            addTo(scan, last.from, last.to, "link", "close"),
          );
        }
        closer(node);
      }
      return true;
    }
    if (name.startsWith("ATXHeading")) {
      const open = node.firstChild;
      const close = node.lastChild;
      for (const scan of hidden) {
        // Only with the space after it: a `#` run alone is a heading being
        // typed.
        const prefix =
          open?.name === "HeaderMark" && isSpace(char(open.to))
            ? addTo(scan, open.from, open.to + 1, "heading", "prefix")
            : null;
        if (
          prefix &&
          close &&
          close.from !== prefix.from &&
          close.name === "HeaderMark"
        ) {
          pair(prefix, addTo(scan, close.from, close.to, "heading", "close"));
        }
      }
      return false;
    }
    if (name === "ListMark") {
      if (isSpace(char(ref.to))) {
        const box = ref.to + 1;
        const notApplicable = NOT_APPLICABLE.test(
          doc.sliceString(box, box + 4),
        );
        for (const scan of hidden) {
          addTo(scan, ref.from, ref.to + 1, "list", "prefix");
          if (notApplicable) {
            scan.taken.add(box);
            addTo(
              scan,
              box,
              isSpace(char(box + 3)) ? box + 4 : box + 3,
              "task",
              "prefix",
            );
          }
        }
      }
      return false;
    }
    if (name === "TaskMarker") {
      for (const scan of hidden) {
        scan.taken.add(ref.from);
        addTo(
          scan,
          ref.from,
          isSpace(char(ref.to)) ? ref.to + 1 : ref.to,
          "task",
          "prefix",
        );
      }
      return false;
    }
    if (name === "QuoteMark") {
      for (const scan of hidden) {
        quote(scan, ref.from, ref.to);
      }
      return false;
    }
    return false;
  };

  // The depth of the node in hand: a line that does not read a node's
  // inside skips every node below it, until the walk is back at its depth.
  let depth = 0;
  const hidden: LineScan[] = [];
  walkTree(
    syntaxTree(state),
    from,
    to,
    (ref) => {
      step += 1;
      const at = depth;
      hidden.length = 0;
      let any = false;
      // A line that reads only comments reads every inside.
      let inside = false;
      head = reaching(lines, head, ref.from);
      near = head;
      for (
        let index = head;
        index < lines.length && (lines[index]?.from ?? 0) <= ref.to;
        index++
      ) {
        treeWalkWork.steps += 1;
        const scan = scans[index];
        if (!scan) continue;
        if (scan.skip !== -1) {
          if (at > scan.skip) continue;
          scan.skip = -1;
        }
        scan.seen = step;
        any = true;
        if (scan.hidden && (!scan.math || ref.name === "QuoteMark")) {
          hidden.push(scan);
        } else {
          inside = true;
        }
      }
      if (!any) {
        return false;
      }
      if (ref.name === "Comment" || ref.name === "CommentBlock") {
        const text = doc.sliceString(ref.from, ref.to);
        for (const comment of htmlCommentRanges(text)) {
          const start = ref.from + comment.from;
          const end = ref.from + comment.to;
          near = reaching(lines, near, start);
          for (
            let index = near;
            index < lines.length && (lines[index]?.from ?? 0) <= end;
            index++
          ) {
            treeWalkWork.steps += 1;
            const scan = scans[index];
            if (!scan || scan.seen !== step || isIdLine(scan.at.text)) continue;
            addTo(
              scan,
              Math.max(start, scan.at.from),
              Math.min(end, scan.at.to),
              "comment",
              "whole",
            );
          }
        }
        return false;
      }
      if (hidden.length > 0) {
        if (marksOf(ref, hidden)) {
          for (const scan of hidden) scan.skip = at;
        } else {
          inside = true;
        }
      }
      if (!inside) {
        return false;
      }
      depth += 1;
      return undefined;
    },
    () => {
      depth -= 1;
    },
  );
}

/** A line's drafts as its sorted marks, its category read last. */
function finished(scan: LineScan): Mark[] {
  const { at, drafts, taken, category } = scan;
  if (category && !scan.inCode) {
    const open = at.from + category.from;
    const close = at.from + category.to - 1;
    taken.add(open);
    // The link the parser read over these brackets, if any, is dropped.
    for (let index = drafts.length - 1; index >= 0; index -= 1) {
      const draft = drafts[index];
      if (draft?.kind === "link" && draft.from === open) {
        drafts.splice(index, 1);
        if (draft.pair) {
          drafts.splice(drafts.indexOf(draft.pair), 1);
        }
      }
    }
    pair(
      addTo(scan, open, open + 1, "category", "open"),
      addTo(scan, close, close + 1, "category", "close"),
    );
  }
  drafts.sort((a, b) => a.from - b.from || a.to - b.to);
  return drafts.map((draft) => ({
    from: draft.from,
    to: draft.to,
    kind: draft.kind,
    side: draft.side,
    partner: draft.pair ? drafts.indexOf(draft.pair) : null,
  }));
}

/** The marks of line `line` grouped into runs of touching marks. */
export function runsOnLine(state: EditorState, line: number): MarkRun[] {
  return runsOf(marksOnLine(state, line));
}

/** Sorted marks grouped into runs of touching marks. */
export function runsOf(marks: readonly Mark[]): MarkRun[] {
  const runs: MarkRun[] = [];
  for (const mark of marks) {
    const last = runs.at(-1);
    if (last && last.to >= mark.from) {
      last.to = Math.max(last.to, mark.to);
      last.marks.push(mark);
    } else {
      runs.push({ from: mark.from, to: mark.to, marks: [mark] });
    }
  }
  return runs;
}

/** The run with `from <= pos <= to` on `pos`'s line, or null. */
export function runAt(state: EditorState, pos: number): MarkRun | null {
  const line = state.doc.lineAt(pos).number;
  return (
    runsOnLine(state, line).find((run) => run.from <= pos && pos <= run.to) ??
    null
  );
}
