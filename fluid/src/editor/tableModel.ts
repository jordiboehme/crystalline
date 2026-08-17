/**
 * A markdown table, read as structure and edited as the smallest possible set
 * of changes.
 *
 * The module is deliberately free of CodeMirror: it takes the TEXT of a table
 * span and returns span-relative changes, so it can be reasoned about and
 * tested without a document, a view or a DOM. The one thing it cannot know on
 * its own is the document's line separator, so that is passed in - the
 * dispatch layer hands it `state.lineBreak`, which keeps the "never a literal
 * newline" rule true without dragging the editor into this file.
 *
 * THE CALLING CONVENTION, because a CRLF document makes it load-bearing.
 * CodeMirror counts a line break as ONE document position however many
 * characters the separator has, so a span's offsets equal document offsets
 * only when the span is read with a single-character join:
 *
 *   const span = state.doc.sliceString(from, to);   // NEVER state.sliceDoc
 *   const changes = addRowBelow(model, row, state.lineBreak);
 *   // then map: { from: from + change.from, to: from + (change.to ?? ...) }
 *
 * `state.sliceDoc` re-joins with `state.lineBreak`, so on a CRLF document its
 * string is LONGER than the range it came from and every offset past the first
 * break is inflated - every verb then edits the wrong place. The `separator`
 * parameter is insertion TEXT only, never a length: nothing here derives a
 * position from it, so `"\r\n"` costing two characters and one position is
 * never a contradiction. The parse still tolerates a stray CR (it reads as
 * trailing whitespace) so a raw file slice does not explode, but the spans it
 * hands back are only document offsets under the convention above.
 *
 * WHY MINIMAL CHANGES. Every structural verb here could be written as "render
 * the whole table again and replace the span". It is not, because the buffer
 * is shared: under Yjs a replacement of the whole span deletes every character
 * a collaborator is currently typing inside the table and re-inserts new ones,
 * so their concurrent edit is stranded on deleted text and their cursor jumps.
 * Per-line insertions at computed offsets interleave cleanly instead - a
 * remote edit inside a cell survives an "add column" that happened at the same
 * moment, because the two touch disjoint positions. So: add-column emits ONE
 * insertion per line, add-row ONE insertion, delete-column one deletion per
 * line, alignment one replacement of a single delimiter cell, and `prettify`
 * is the sole verb that rewrites lines wholesale - explicitly, by user
 * request, and even then only the lines whose text actually changes.
 *
 * WHAT IT TOLERATES (GFM, not a stricter grammar): cells are split on
 * UNESCAPED pipes only, so a `\|` stays cell content; rows without leading or
 * trailing pipes parse and are edited at their real cell boundaries rather
 * than being normalized; a ragged row (fewer cells than the header) is
 * tolerated and a verb that targets a column it lacks extends it as part of
 * the same single insertion. `parseTable` returns null when the second line is
 * not delimiter-shaped, and the verbs return null when they refuse - null is
 * the whole refusal channel, so the caller has exactly one thing to check.
 */

export type Align = "left" | "center" | "right" | "none";

/** One cell's span, in offsets LOCAL to its line. */
export interface TableCell {
  raw: string;
  from: number;
  to: number;
}

export interface TableLine {
  /** Offset of the line start within the span text. */
  start: number;
  /** The raw line, without its separator. */
  text: string;
  indent: string;
  leadingPipe: boolean;
  trailingPipe: boolean;
  cells: TableCell[];
}

export interface TableModel {
  /** 0 = header, 1 = delimiter, 2.. = data rows. */
  lines: TableLine[];
  /** Header cell count - the table's column count. */
  columns: number;
  /** Per column, read off the delimiter row. */
  aligns: Align[];
}

/** A change in offsets relative to the parsed span. */
export interface SpanChange {
  from: number;
  to?: number;
  insert?: string;
}

/** A delimiter cell: optional colons around a run of at least one dash. */
const DELIMITER_CELL = /^\s*:?-+:?\s*$/;

/** GFM's own floor for a rule cell, and so the floor for a prettified column. */
const MIN_DASHES = 3;

/** The placeholder a new header cell carries, matching the insert skeleton. */
const NEW_HEADER_CELL = "Column";

/**
 * Positions of every pipe not escaped by a preceding backslash. One scan, no
 * backtracking. Knowingly simpler than GFM in one spelling: a literal escaped
 * backslash before a real boundary (`\\|`) reads as content here, where GFM
 * would split. The one-character look-back is what keeps the scan linear.
 */
function unescapedPipes(text: string): number[] {
  const found: number[] = [];
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "|" && text[i - 1] !== "\\") found.push(i);
  }
  return found;
}

function leadingWhitespace(text: string): string {
  let i = 0;
  while (i < text.length && (text[i] === " " || text[i] === "\t")) i += 1;
  return text.slice(0, i);
}

/** Index just past the last non-whitespace character (a CR included). */
function trimmedEnd(text: string, floor: number): number {
  let end = text.length;
  while (end > floor && /\s/.test(text[end - 1] ?? "")) end -= 1;
  return end;
}

function parseLine(text: string, start: number): TableLine {
  const indent = leadingWhitespace(text);
  const end = trimmedEnd(text, indent.length);
  const pipes = unescapedPipes(text);
  const leadingPipe = pipes.length > 0 && pipes[0] === indent.length;
  const trailingPipe =
    pipes.length > (leadingPipe ? 1 : 0) && pipes[pipes.length - 1] === end - 1;
  const inner = pipes.slice(
    leadingPipe ? 1 : 0,
    trailingPipe ? pipes.length - 1 : pipes.length,
  );

  const cells: TableCell[] = [];
  let from = leadingPipe ? (pipes[0] ?? 0) + 1 : indent.length;
  for (const pipe of inner) {
    cells.push({ raw: text.slice(from, pipe), from, to: pipe });
    from = pipe + 1;
  }
  const last = trailingPipe ? (pipes[pipes.length - 1] ?? end) : end;
  cells.push({ raw: text.slice(from, last), from, to: Math.max(from, last) });

  return { start, text, indent, leadingPipe, trailingPipe, cells };
}

function alignOf(cell: TableCell | undefined): Align {
  const raw = cell?.raw.trim() ?? "";
  const opens = raw.startsWith(":");
  const closes = raw.endsWith(":") && raw.length > 1;
  if (opens && closes) return "center";
  if (opens) return "left";
  if (closes) return "right";
  return "none";
}

/**
 * Parse a table span, whose offsets are document offsets under the calling
 * convention at the top of this file. Lines are split on the line feed and a
 * carriage return is treated as trailing whitespace, so a raw CRLF slice
 * parses rather than exploding - but only an LF-joined span (`doc.sliceString`)
 * gives back spans a transaction can use.
 *
 * The two empty-cell guards below are defensive rather than reachable:
 * `parseLine` always pushes a final cell, so a line has at least one. They
 * stay because the refusal they express (a table needs a header and a rule)
 * should be readable at the place it is decided.
 */
export function parseTable(span: string): TableModel | null {
  const lines: TableLine[] = [];
  let start = 0;
  for (const raw of span.split("\n")) {
    lines.push(parseLine(raw, start));
    start += raw.length + 1;
  }
  // A span that ends on its line break has an empty last line; that is the
  // break, not a row.
  while (
    lines.length > 2 &&
    (lines[lines.length - 1]?.text.trim() ?? "") === ""
  ) {
    lines.pop();
  }

  const header = lines[0];
  const delimiter = lines[1];
  if (!header || !delimiter) return null;
  if (header.cells.length === 0) return null;
  if (delimiter.cells.length === 0) return null;
  if (!delimiter.cells.every((cell) => DELIMITER_CELL.test(cell.raw)))
    return null;

  const aligns: Align[] = [];
  for (let column = 0; column < header.cells.length; column += 1) {
    aligns.push(alignOf(delimiter.cells[column]));
  }
  return { lines, columns: header.cells.length, aligns };
}

/**
 * The column a line-local offset sits in, clamped into the line's range: an
 * offset on a boundary pipe belongs to the cell that ends there, and an offset
 * past the last cell belongs to the last cell.
 */
export function columnAt(line: TableLine, offset: number): number {
  for (let i = 0; i < line.cells.length; i += 1) {
    const cell = line.cells[i];
    if (cell && offset <= cell.to) return i;
  }
  return Math.max(0, line.cells.length - 1);
}

/** A padded field between two pipes: `| text `, and `|  ` when text is empty. */
function field(text: string): string {
  return `| ${text} `;
}

/** How many of a rule cell's characters are colons rather than dashes. */
function colons(align: Align): number {
  if (align === "center") return 2;
  return align === "none" ? 0 : 1;
}

/** A rule cell drawn with a given dash run, never shorter than GFM's floor. */
function ruleWithDashes(align: Align, dashes: number): string {
  const bar = "-".repeat(Math.max(MIN_DASHES, dashes));
  if (align === "center") return `:${bar}:`;
  if (align === "left") return `:${bar}`;
  if (align === "right") return `${bar}:`;
  return bar;
}

/** A rule cell stretched to a total width, colons included. */
function ruleText(align: Align, width: number): string {
  return ruleWithDashes(align, width - colons(align));
}

/** The narrowest a rule cell of this alignment can be drawn. */
function minRuleWidth(align: Align): number {
  return MIN_DASHES + colons(align);
}

/**
 * Insert a new row below `row`, clamped so rows 0 and 1 both land BELOW the
 * delimiter: a data row between the header and the rule is not a GFM table.
 *
 * The emission is exactly one insertion at the end of the target line, of
 * `separator + indent + "|" + "  |".repeat(columns)`. The caret goes into the
 * new row's first cell, and that position is stated relative to the NEW LINE
 * rather than to the separator's length: the inserted break costs exactly one
 * document position whatever the separator spells, so after the dispatch the
 * new row is the line at `change.from + 1` and its first cell's interior sits
 * `indent.length + 2` past that line's start - `lineAt(pos).from + indent + 2`,
 * which is a spelling a CRLF document cannot break.
 *
 * `change.from + 1` is SPAN-relative, like everything this module emits, so it
 * is a document position only inside a span that starts at 0. A dispatch layer
 * maps it first, exactly as it maps the change itself: the document spelling
 * is `lineAt(node.from + change.from + 1).from + indent + 2`.
 */
export function addRowBelow(
  model: TableModel,
  row: number,
  separator: string,
): SpanChange[] | null {
  if (row < 0 || row >= model.lines.length) return null;
  const target = model.lines[Math.max(row, 1)];
  if (!target) return null;
  const cells = "  |".repeat(model.columns);
  return [
    {
      from: target.start + target.text.length,
      insert: `${separator}${target.indent}|${cells}`,
    },
  ];
}

/**
 * Add a column after `column`: one insertion per line, at the boundary that
 * closes the target cell. A row too short to have that column is extended with
 * empty cells inside the same insertion, so every row grows by the same number
 * of columns. A row without a trailing pipe that gains a cell at its END gains
 * a closing pipe with it - the minimum needed for the new cell to exist.
 */
export function addColumnAfter(
  model: TableModel,
  column: number,
): SpanChange[] | null {
  if (column < 0 || column >= model.columns) return null;

  const changes: SpanChange[] = [];
  for (let index = 0; index < model.lines.length; index += 1) {
    const line = model.lines[index];
    if (!line) continue;
    const target = Math.min(column, line.cells.length - 1);
    const anchor = line.cells[target];
    if (!anchor) continue;

    // A delimiter row must stay delimiter-shaped even where it is extended.
    const filler = index === 1 ? "-".repeat(MIN_DASHES) : "";
    const added = index === 0 ? NEW_HEADER_CELL : filler;
    const fields: string[] = [];
    for (let missing = target; missing < column; missing += 1)
      fields.push(field(filler));
    fields.push(field(added));

    const atEnd = target === line.cells.length - 1 && !line.trailingPipe;
    changes.push({
      from: line.start + anchor.to,
      insert: atEnd ? ` ${fields.join("")}|` : fields.join(""),
    });
  }
  return changes;
}

/**
 * Delete a data row: the line plus exactly one break. The header and the
 * delimiter are refused - a GFM table needs both.
 *
 * Both ends of the deleted range come from the MODEL's own line spans - the
 * start of the following line, or the end of the preceding one for the last
 * row. Deriving the preceding break from `_separator.length` instead would be
 * a length where the rest of the module has text, and it would eat a character
 * of the previous line on a CRLF document, so `_separator` is unused here.
 */
export function deleteRow(
  model: TableModel,
  row: number,
  _separator: string,
): SpanChange[] | null {
  if (row < 2 || row >= model.lines.length) return null;
  const line = model.lines[row];
  if (!line) return null;

  const next = model.lines[row + 1];
  if (next) return [{ from: line.start, to: next.start }];
  const previous = model.lines[row - 1];
  if (!previous) return null;
  return [
    {
      from: previous.start + previous.text.length,
      to: line.start + line.text.length,
    },
  ];
}

/**
 * Delete a column: per line, the cell's span plus ONE adjoining pipe - the one
 * that follows it, or the one that precedes it when the cell is the last on
 * its line. A row too short to have the column is left untouched. The last
 * column of a table is refused; a table needs one.
 *
 * The whole verb is also refused when any one line would be left blank by it,
 * which a line without edge pipes that is ragged down to the target column is:
 * its only cell IS the column, so the deletion takes the line down to nothing.
 * A blank line ENDS a table in GFM, so that emission would not narrow this
 * table, it would split it in two and leave the rows below as prose. Refusing
 * is the only outcome that keeps the document parseable, and it costs a rare
 * shape one click that does nothing rather than a repair nobody asked for.
 */
export function deleteColumn(
  model: TableModel,
  column: number,
): SpanChange[] | null {
  if (model.columns <= 1) return null;
  if (column < 0 || column >= model.columns) return null;

  const changes: SpanChange[] = [];
  for (const line of model.lines) {
    const cell = line.cells[column];
    if (!cell) continue;
    let from = cell.from;
    let to = cell.to;
    if (column < line.cells.length - 1) {
      to = cell.to + 1;
    } else if (cell.from > line.indent.length) {
      from = cell.from - 1;
    }
    if (`${line.text.slice(0, from)}${line.text.slice(to)}`.trim() === "")
      return null;
    changes.push({ from: line.start + from, to: line.start + to });
  }
  return changes;
}

/**
 * Set a column's alignment by replacing ONE delimiter cell with the canonical
 * colon form. The dash run keeps its width, so an already-prettified table
 * stays aligned; re-padding the data column is `prettify`'s job and the two
 * verbs compose.
 *
 * It always emits its one change, even when the cell already carries that
 * alignment: the contract the caller checks stays two-state (null refuses,
 * anything else is dispatched) rather than growing an "empty but not refused"
 * third case.
 */
export function setAlignment(
  model: TableModel,
  column: number,
  align: Align,
): SpanChange[] | null {
  if (column < 0 || column >= model.columns) return null;
  const delimiter = model.lines[1];
  const cell = delimiter?.cells[column];
  if (!cell) return null;

  const dashes = (cell.raw.match(/-/g) ?? []).length;
  return [
    {
      from: delimiter.start + cell.from,
      to: delimiter.start + cell.to,
      insert: ` ${ruleWithDashes(align, dashes)} `,
    },
  ];
}

function pad(text: string, width: number, align: Align): string {
  const room = Math.max(0, width - text.length);
  if (align === "right") return " ".repeat(room) + text;
  if (align === "center") {
    const left = Math.floor(room / 2);
    return " ".repeat(left) + text + " ".repeat(room - left);
  }
  return text + " ".repeat(room);
}

/**
 * Re-pad the table so its pipes line up in a monospaced reader: every cell
 * padded to its column's widest trimmed content, single-space margins,
 * canonical leading and trailing pipes, and the rule row's colons stretched
 * with its dashes. Cell TEXT is verbatim - prettify moves padding, never a
 * character of content. It can still change what a row MEANS in one case: a
 * row carrying more cells than the header has columns widens the whole table
 * to fit them, so a cell the renderer was ignoring becomes a real column. The
 * alternative - clamping to the header's count - would drop that cell's text,
 * which is the worse of the two. The header row pads left whatever the
 * column's alignment is, and data cells follow the alignment.
 *
 * This is the one verb that rewrites lines, and it emits a change only for a
 * line whose text actually changes, so an already-canonical table emits
 * nothing at all. `_separator` is unused: the rewrite is per line, so no line
 * break is ever produced, and the parameter is kept for symmetry with the
 * verbs that do insert lines.
 */
export function prettify(model: TableModel, _separator: string): SpanChange[] {
  let count = model.columns;
  for (const line of model.lines) count = Math.max(count, line.cells.length);

  const aligns: Align[] = [];
  const widths: number[] = [];
  for (let column = 0; column < count; column += 1) {
    const align = model.aligns[column] ?? "none";
    let width = minRuleWidth(align);
    for (let index = 0; index < model.lines.length; index += 1) {
      if (index === 1) continue;
      const raw = model.lines[index]?.cells[column]?.raw.trim() ?? "";
      width = Math.max(width, raw.length);
    }
    aligns.push(align);
    widths.push(width);
  }

  const changes: SpanChange[] = [];
  for (let index = 0; index < model.lines.length; index += 1) {
    const line = model.lines[index];
    if (!line) continue;
    const fields: string[] = [];
    for (let column = 0; column < count; column += 1) {
      const align = aligns[column] ?? "none";
      const width = widths[column] ?? MIN_DASHES;
      if (index === 1) {
        fields.push(ruleText(align, width));
        continue;
      }
      const raw = line.cells[column]?.raw.trim() ?? "";
      fields.push(pad(raw, width, index === 0 ? "left" : align));
    }
    const text = `${line.indent}| ${fields.join(" | ")} |`;
    if (text !== line.text) {
      changes.push({
        from: line.start,
        to: line.start + line.text.length,
        insert: text,
      });
    }
  }
  return changes;
}
