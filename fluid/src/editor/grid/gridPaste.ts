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
import type { SpanChange } from "../tableModel";
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
  setOpenCell,
} from "./openCell";
import { gridTableAt, tableIn } from "./structure";

/** The rows and cells of a clipboard block; null when it is one value. */
export function blockOf(plain: string): string[][] | null {
  if (!/[\t\r\n]/.test(plain)) return null;
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

function absolute(from: number, changes: SpanChange[]): ChangeSpec[] {
  return changes.map((change) => ({
    from: from + change.from,
    ...(change.to === undefined ? {} : { to: from + change.to }),
    ...(change.insert === undefined ? {} : { insert: change.insert }),
  }));
}

/** Inserts `text` at the cell view's selection, as typed. */
function typed(view: EditorView, text: string): void {
  if (!view.dom.isConnected) return;
  const flat = text.replace(/\s*(?:\r\n|\r|\n)[\s]*/g, " ");
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
    if (!changes) return false;
    apply(absolute(from, changes));
  }
  const dataRows = cell.row + rows.length;
  while (model().lines.length - 1 < dataRows) {
    const m = model();
    const changes = addRowBelow(m, m.lines.length - 1, lb);
    if (!changes) return false;
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
    values.forEach((value, c) => {
      const target = grown.rows[cell.row + r]?.[cell.column + c];
      if (!target) return;
      const closing = doc.sliceString(target.content.to, target.content.to + 1);
      fills.push({
        from: target.content.from,
        to: target.content.to,
        insert: stored(value, closing === "|"),
      });
    });
  });
  apply(fills);

  const final = tableIn(doc, from, to);
  const home = final.rows[cell.row]?.[cell.column];
  if (!home) return false;
  const reshaped =
    final.model.columns !== table.model.columns ||
    final.rows.length !== table.rows.length;
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
      const rows = blockOf(plain);
      if (rows === null || (rows.length === 1 && rows[0]?.length === 1)) {
        typed(view, rows?.[0]?.[0] ?? plain);
        return true;
      }
      if (!fill(main, rows)) typed(view, plain);
      return true;
    },
  }),
);
