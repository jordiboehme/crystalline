/**
 * Pasting into the open cell.
 *
 * `cellPaste` runs first in the cell view. One value (no tab, no line break)
 * goes in as typed, through the cell's own filter, which escapes the pipes. A
 * block (a spreadsheet copies TSV; an HTML table from a page arrives as its
 * plain text too) fills cells from the open cell rightward and downward,
 * replacing each target cell's content, and adds the columns and rows it
 * needs. The whole paste is ONE main transaction: the table verbs' changes
 * and the content replacements are worked out one after the other on a
 * scratch copy of the document and composed into one change set, so it is one
 * undo step solo and in a room. HTML with no plain text is converted by
 * `htmlToMarkdown` (loaded on first use) with line breaks turned into spaces.
 * Files are refused quietly: images in cells are a later part.
 */

import type { ChangeSpec, Extension, Text } from "@codemirror/state";
import { ChangeSet, Prec } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { transferFiles } from "../attachments";
import { isOwnCopy } from "../clipboardHtml";
import type { SpanChange, TableLine } from "../tableModel";
import { addColumnAfter, addRowBelow, parseTable } from "../tableModel";
import { noteUndoLanding } from "./cellEditor";
import { closeUndoGroup } from "./gridKeys";
import { hostOpenCell } from "./gridWidget";
import {
  cellMain,
  gridWritable,
  isDestroyed,
  mountedCell,
  openCellField,
  openCellRange,
  parseThrough,
  setOpenCell,
} from "./openCell";
import { gridTableAt, tableIn } from "./structure";

/**
 * Whether `html` holds a table with some row of more than one cell. A table
 * of one column, which a page also uses to lay out plain text, does not make
 * a paste a block.
 */
function htmlGrid(html: string): boolean {
  if (!/<table[\s>]/i.test(html)) return false;
  return html
    .split(/<tr[\s>]/i)
    .slice(1)
    .some((row) => {
      const end = row.search(/<\/tr\s*>/i);
      const cells = (end === -1 ? row : row.slice(0, end)).match(
        /<t[dh][\s>]/gi,
      );
      return (cells?.length ?? 0) > 1;
    });
}

/**
 * The rows and cells of a clipboard block; null when it is one value. A
 * block has a tab, or comes with an HTML table of two or more columns
 * beside its plain text (finding 8): two lines of prose are one value, typed with the line break
 * as a space, and never overwrite the cell below.
 */
export function blockOf(plain: string, html = ""): string[][] | null {
  if (!/[\t\r\n]/.test(plain)) return null;
  // Lines with no tab are prose with line breaks, one value, unless the
  // clipboard also holds an HTML table with a row of two or more cells.
  if (!plain.includes("\t") && !htmlGrid(html)) return null;
  const lines = plain.replace(/\r\n?/g, "\n").split("\n");
  if (lines.length > 1 && lines.at(-1) === "") lines.pop();
  const rows = lines.map((line) => line.split("\t").map((cell) => cell.trim()));
  return rows;
}

/** A cell's text as the cell stores it: pipes escaped, even run at the end. */
function stored(text: string, closing: boolean): string {
  let out = "";
  let run = 0;
  for (const char of text) {
    if (char === "|") {
      out += run % 2 === 0 ? "\\|" : "\\\\|";
      run = 1;
      continue;
    }
    out += char;
    run = char === "\\" ? run + 1 : 0;
  }
  if (closing && run % 2 === 1) out += "\\";
  return out;
}

/**
 * Whether a cell's text can open a line without a leading pipe: it must start
 * with a letter or a digit, and not as a list number (`1.` or `1)` followed
 * by a blank starts a list). Anything else (`-`, `#`, `>`, `<`, a fence) could
 * start another block and end the table, so such a row keeps its pipe.
 */
function opensBare(text: string): boolean {
  return /^[\p{L}\p{N}]/u.test(text) && !/^\d{1,9}[.)](\s|$)/.test(text);
}

/**
 * R23: the edge pipes a pasted row no longer needs. The verbs that grow the
 * table add a row or a column with empty cells, and an empty edge cell needs
 * its edge pipe; once the paste has filled it, a row that the table spells
 * without that pipe (`want`) gets its own spelling back. A row the paste did
 * not reach keeps every byte, and so does an edge cell left empty.
 */
function spare(from: number, line: TableLine, want: TableLine): ChangeSpec[] {
  const trims: ChangeSpec[] = [];
  const first = line.cells[0];
  const last = line.cells.at(-1);
  if (!first || !last) return trims;
  if (line.leadingPipe && !want.leadingPipe && opensBare(first.raw.trim())) {
    const blank = first.raw.length - first.raw.trimStart().length;
    trims.push({
      from: from + line.start + line.indent.length,
      to: from + line.start + first.from + blank,
    });
  }
  const text = last.raw.trim();
  if (
    line.trailingPipe &&
    !want.trailingPipe &&
    text !== "" &&
    !text.endsWith("\\") &&
    (line.cells.length > 1 || trims.length === 0)
  ) {
    trims.push({
      from: from + line.start + last.from + last.raw.trimEnd().length,
      to: from + line.start + last.to + 1,
    });
  }
  return trims;
}

function absolute(from: number, changes: SpanChange[]): ChangeSpec[] {
  return changes.map((change) => ({
    from: from + change.from,
    ...(change.to === undefined ? {} : { to: from + change.to }),
    ...(change.insert === undefined ? {} : { insert: change.insert }),
  }));
}

/**
 * Inserts `text` at the cell view's selection, as typed: the line breaks of
 * the ends dropped (a copied paragraph ends in one), the others turned into
 * a space.
 */
function typed(view: EditorView, text: string): void {
  if (!view.dom.isConnected) return;
  const flat = text
    .replace(/^[\s]*(?:\r\n|\r|\n)\s*|\s*(?:\r\n|\r|\n)[\s]*$/g, "")
    .replace(/\s*(?:\r\n|\r|\n)[\s]*/g, " ");
  view.dispatch(view.state.replaceSelection(flat), {
    userEvent: "input.paste",
    scrollIntoView: true,
  });
}

/** Fills the block into the table; false when nothing could be done. */
function fill(main: EditorView, rows: string[][]): boolean {
  const open = main.state.field(openCellField, false);
  const cell = openCellRange(main.state);
  if (!open || !cell) return false;
  const table = gridTableAt(main.state, open.head);
  if (!table) return false;
  const { state } = main;
  /**
   * Works the paste out on a scratch copy of the document. Null, or a throw
   * (a grown table that stops parsing), means it cannot be done as a block,
   * and the caller types the paste as one value instead (finding 9).
   */
  const plan = () => {
    const lb = state.lineBreak;
    const wide = Math.max(...rows.map((row) => row.length));

    let doc: Text = state.doc;
    let total = ChangeSet.empty(doc.length);
    let to = table.to;
    const from = table.from;
    const apply = (specs: ChangeSpec[]) => {
      const set = ChangeSet.of(specs, doc.length, lb);
      doc = set.apply(doc);
      total = total.compose(set);
      to = set.mapPos(to, 1);
    };
    const model = () => {
      const parsed = parseTable(doc.sliceString(from, to));
      if (!parsed) throw new Error("the pasted-into table stopped parsing");
      return parsed;
    };

    const columns = cell.column + wide;
    while (model().columns < columns) {
      const m = model();
      const changes = addColumnAfter(m, m.columns - 1);
      if (!changes) return null;
      apply(absolute(from, changes));
    }
    const dataRows = cell.row + rows.length;
    while (model().lines.length - 1 < dataRows) {
      const m = model();
      const changes = addRowBelow(m, m.lines.length - 1, lb);
      if (!changes) return null;
      apply(absolute(from, changes));
    }

    // A ragged row shorter than the paste reaches grows in the same change set,
    // in its own spelling, so no pasted value is dropped.
    const extend: ChangeSpec[] = [];
    const shaped = model();
    rows.forEach((values, r) => {
      const index = cell.row + r === 0 ? 0 : cell.row + r + 1;
      const line = shaped.lines[index];
      const last = line?.cells.at(-1);
      if (!line || !last) return;
      const missing = cell.column + values.length - line.cells.length;
      if (missing <= 0) return;
      if (line.trailingPipe) {
        extend.push({
          from: from + line.start + last.to + 1,
          insert: "  |".repeat(missing),
        });
      } else {
        extend.push({
          from: from + line.start + last.to,
          insert: ` |${"  |".repeat(missing)}`,
        });
      }
    });
    if (extend.length > 0) apply(extend);

    const grown = tableIn(doc, from, to);
    const fills: ChangeSpec[] = [];
    rows.forEach((values, r) => {
      const index = cell.row + r === 0 ? 0 : cell.row + r + 1;
      const line = grown.model.lines[index];
      values.forEach((value, c) => {
        const target = grown.rows[cell.row + r]?.[cell.column + c];
        if (!target || !line) return;
        const closing = doc.sliceString(
          target.content.to,
          target.content.to + 1,
        );
        // An empty value at an edge without a pipe would take the cell with it
        // (` | 2` is a leading pipe, `1 | ` a trailing one): the one pipe it
        // needs to exist goes in its place (R23).
        const column = cell.column + c;
        const bare =
          (column === 0 && !line.leadingPipe) ||
          (column === line.cells.length - 1 && !line.trailingPipe);
        fills.push({
          from: target.content.from,
          to: target.content.to,
          insert: value === "" && bare ? "|" : stored(value, closing === "|"),
        });
      });
    });
    apply(fills);

    // The rows the paste reached get the table's own spelling back where a
    // grown row or column added an edge pipe they no longer need. A new row
    // copies the spelling of the last row the table had.
    const filled = model();
    const trims: ChangeSpec[] = [];
    const lastOld = table.model.lines.at(-1);
    rows.forEach((_values, r) => {
      const index = cell.row + r === 0 ? 0 : cell.row + r + 1;
      const line = filled.lines[index];
      const want = table.model.lines[index] ?? lastOld;
      if (line && want) trims.push(...spare(from, line, want));
    });
    if (trims.length > 0) apply(trims);

    const final = tableIn(doc, from, to);
    const home = final.rows[cell.row]?.[cell.column];
    if (!home) return null;
    const reshaped =
      final.model.columns !== table.model.columns ||
      final.rows.length !== table.rows.length;
    return { total, home, reshaped, to };
  };
  let planned: ReturnType<typeof plan>;
  try {
    planned = plan();
  } catch {
    return false;
  }
  if (!planned) return false;
  const { total, home, reshaped, to } = planned;
  const before = state.doc;
  closeUndoGroup(main, "before");
  main.dispatch({
    changes: total,
    effects: setOpenCell.of({
      anchor: home.content.to,
      head: home.content.to,
      focused: true,
    }),
    userEvent: "input.paste",
  });
  closeUndoGroup(main, "after");
  parseThrough(main, to);
  if (reshaped) {
    const left = { anchor: open.anchor, head: open.head, focused: true };
    noteUndoLanding(main, {
      before,
      after: main.state.doc,
      restore: () => {
        main.dispatch({ effects: setOpenCell.of(left) });
        hostOpenCell(main, "now");
      },
    });
    hostOpenCell(main, "now");
  }
  return true;
}

type Converter = typeof import("../htmlToMarkdown");
let loading: Promise<Converter> | null = null;

function loadConverter(): Promise<Converter> {
  loading ??= import("../htmlToMarkdown").catch((error: unknown) => {
    loading = null;
    throw error;
  });
  return loading;
}

/** The cell view's paste handler, ahead of everything else in the cell. */
export const cellPaste: Extension = Prec.highest(
  EditorView.domEventHandlers({
    paste(event, view) {
      const main = view.state.facet(cellMain);
      const data = event.clipboardData;
      if (!main || isDestroyed(main) || data === null) return false;
      event.preventDefault();
      if (!gridWritable(main.state)) return true;
      const plain = data.getData("text/plain");
      if (plain === "") {
        if (transferFiles(data).length > 0) return true;
        const html = data.getData("text/html");
        if (html.trim() === "") return true;
        loadConverter().then(
          (converter) => {
            if (
              isDestroyed(main) ||
              !gridWritable(main.state) ||
              mountedCell(main)?.view !== view
            )
              return;
            typed(view, converter.htmlToMarkdown(html));
          },
          () => undefined,
        );
        return true;
      }
      // Our own copy is read by its plain text alone, as a copy before it
      // carried HTML was.
      const clipHtml = data.getData("text/html");
      const rows = blockOf(plain, isOwnCopy(clipHtml) ? "" : clipHtml);
      if (rows === null || (rows.length === 1 && rows[0]?.length === 1)) {
        typed(view, rows?.[0]?.[0] ?? plain);
        return true;
      }
      if (!fill(main, rows)) typed(view, plain);
      return true;
    },
  }),
);
